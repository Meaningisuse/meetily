//! Maps VAD-packed transcription samples back to the original 16 kHz source clock.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeMapSpan {
    pub packed_start_sample: u64,
    pub source_start_sample: u64,
    pub sample_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeMapError {
    EmptyOrReversedRange,
    InvalidSpan,
    PackedOverlap,
    UnmappedPackedRange,
}

/// Map a half-open interval in packed audio to one or more half-open source spans.
/// Discontinuous source regions remain separate; callers must not bridge the gap.
pub fn map_span(
    packed_start_sample: u64,
    packed_end_sample: u64,
    spans: &[TimeMapSpan],
) -> Result<Vec<SourceSpan>, TimeMapError> {
    if packed_end_sample <= packed_start_sample {
        return Err(TimeMapError::EmptyOrReversedRange);
    }
    let mut ordered = spans.to_vec();
    ordered.sort_by_key(|span| span.packed_start_sample);
    let mut previous_end = None;
    for span in &ordered {
        if span.sample_count == 0 {
            return Err(TimeMapError::InvalidSpan);
        }
        let end = span
            .packed_start_sample
            .checked_add(span.sample_count)
            .ok_or(TimeMapError::InvalidSpan)?;
        span.source_start_sample
            .checked_add(span.sample_count)
            .ok_or(TimeMapError::InvalidSpan)?;
        if previous_end.is_some_and(|value| span.packed_start_sample < value) {
            return Err(TimeMapError::PackedOverlap);
        }
        previous_end = Some(end);
    }

    let mut mapped = Vec::<SourceSpan>::new();
    let mut covered_until = packed_start_sample;
    for span in &ordered {
        let span_end = span.packed_start_sample + span.sample_count;
        let start = packed_start_sample.max(span.packed_start_sample);
        let end = packed_end_sample.min(span_end);
        if start >= end {
            continue;
        }
        if start != covered_until {
            return Err(TimeMapError::UnmappedPackedRange);
        }
        let source_start = span.source_start_sample + (start - span.packed_start_sample);
        let source_end = source_start + (end - start);
        if let Some(previous) = mapped.last_mut() {
            if previous.end_sample == source_start {
                previous.end_sample = source_end;
            } else {
                mapped.push(SourceSpan {
                    start_sample: source_start,
                    end_sample: source_end,
                });
            }
        } else {
            mapped.push(SourceSpan {
                start_sample: source_start,
                end_sample: source_end,
            });
        }
        covered_until = end;
        if covered_until == packed_end_sample {
            break;
        }
    }
    if covered_until != packed_end_sample {
        return Err(TimeMapError::UnmappedPackedRange);
    }
    Ok(mapped)
}

/// Convert an absolute sample index between rates with one rational operation.
/// Avoid repeated per-chunk rounding, which accumulates drift on long recordings.
pub fn resampled_sample_position(
    source_sample: u64,
    source_rate: u32,
    target_rate: u32,
) -> Result<u64, TimeMapError> {
    if source_rate == 0 || target_rate == 0 {
        return Err(TimeMapError::InvalidSpan);
    }
    let scaled = u128::from(source_sample) * u128::from(target_rate);
    let result = scaled / u128::from(source_rate);
    u64::try_from(result).map_err(|_| TimeMapError::InvalidSpan)
}

#[cfg(test)]
mod tests {
    use super::{map_span, resampled_sample_position, SourceSpan, TimeMapError, TimeMapSpan};

    #[test]
    fn vad_gap_maps_to_original_clock_without_faking_continuity() {
        let spans = [
            TimeMapSpan {
                packed_start_sample: 0,
                source_start_sample: 0,
                sample_count: 16_000,
            },
            TimeMapSpan {
                packed_start_sample: 16_000,
                source_start_sample: 160_000,
                sample_count: 16_000,
            },
        ];
        assert_eq!(
            map_span(16_000, 32_000, &spans).unwrap(),
            vec![SourceSpan {
                start_sample: 160_000,
                end_sample: 176_000,
            }]
        );
        assert_eq!(
            map_span(15_000, 17_000, &spans).unwrap(),
            vec![
                SourceSpan {
                    start_sample: 15_000,
                    end_sample: 16_000,
                },
                SourceSpan {
                    start_sample: 160_000,
                    end_sample: 161_000,
                },
            ]
        );
    }

    #[test]
    fn resampling_does_not_accumulate_drift_over_two_hours() {
        for source_rate in [44_100, 48_000] {
            let source_samples = u64::from(source_rate) * 60 * 120;
            let target_samples =
                resampled_sample_position(source_samples, source_rate, 16_000).unwrap();
            let restored_source_samples =
                resampled_sample_position(target_samples, 16_000, source_rate).unwrap();
            let drift_seconds =
                source_samples.abs_diff(restored_source_samples) as f64 / f64::from(source_rate);
            assert!(
                drift_seconds <= 1.0 / 16_000.0,
                "rate={source_rate}, drift={drift_seconds}"
            );
        }
    }

    #[test]
    fn unmapped_ranges_and_overlapping_packed_spans_are_rejected() {
        assert_eq!(
            map_span(
                0,
                20,
                &[
                    TimeMapSpan {
                        packed_start_sample: 0,
                        source_start_sample: 100,
                        sample_count: 10,
                    },
                    TimeMapSpan {
                        packed_start_sample: 11,
                        source_start_sample: 111,
                        sample_count: 9,
                    },
                ],
            ),
            Err(TimeMapError::UnmappedPackedRange)
        );
        assert_eq!(
            map_span(
                0,
                12,
                &[
                    TimeMapSpan {
                        packed_start_sample: 0,
                        source_start_sample: 0,
                        sample_count: 10,
                    },
                    TimeMapSpan {
                        packed_start_sample: 9,
                        source_start_sample: 20,
                        sample_count: 3,
                    },
                ],
            ),
            Err(TimeMapError::PackedOverlap)
        );
    }
}
