use super::provider::{TimedText, TimingPrecision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WhisperSegmentTicks {
    pub text: String,
    pub start: i64,
    pub end: i64,
}

pub(crate) fn timing_or_unavailable<E>(
    result: std::result::Result<Option<Vec<TimedText>>, E>,
) -> (Vec<TimedText>, TimingPrecision) {
    match result {
        Ok(Some(segments)) if !segments.is_empty() => (segments, TimingPrecision::Exact),
        Ok(None) | Ok(Some(_)) | Err(_) => (Vec::new(), TimingPrecision::Unavailable),
    }
}

/// Convert whisper.cpp's centisecond segment offsets to milliseconds only when
/// all ranges are valid and the timed segment text exactly accounts for the
/// canonical transcript (ignoring whitespace differences between segments).
pub(crate) fn verify_whisper_timed_segments(
    canonical_text: &str,
    segments: &[WhisperSegmentTicks],
    audio_duration_ms: i64,
) -> Option<Vec<TimedText>> {
    if audio_duration_ms <= 0 || segments.is_empty() || canonical_text.trim().is_empty() {
        return None;
    }

    let normalize_transcript = |text: &str| {
        let mut normalized = String::with_capacity(text.len());
        let mut previous_non_space = None;
        let mut pending_space = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                pending_space = true;
                continue;
            }
            if pending_space
                && !(previous_non_space.is_some_and(is_cjk) && is_cjk(ch))
                && !normalized.is_empty()
            {
                normalized.push(' ');
            }
            normalized.push(ch);
            previous_non_space = Some(ch);
            pending_space = false;
        }
        normalized
    };
    let mut previous_end = 0;
    let mut timed = Vec::with_capacity(segments.len());
    let mut timed_text = Vec::with_capacity(segments.len());

    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty()
            || segment.start < previous_end
            || segment.start < 0
            || segment.end <= segment.start
        {
            return None;
        }

        let start_ms = segment.start.checked_mul(10)?;
        let end_ms = segment.end.checked_mul(10)?;
        if end_ms > audio_duration_ms {
            return None;
        }

        timed_text.push(text);
        timed.push(TimedText {
            text: text.to_string(),
            start_ms,
            end_ms,
        });
        previous_end = segment.end;
    }

    let combined = timed_text.join(" ");
    (normalize_transcript(&combined) == normalize_transcript(canonical_text)).then_some(timed)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2EFF
            | 0x2F00..=0x2FDF
            | 0x3000..=0x303F
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x3130..=0x318F
            | 0x31A0..=0x31BF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::{timing_or_unavailable, verify_whisper_timed_segments, WhisperSegmentTicks};

    fn segment(text: &str, start: i64, end: i64) -> WhisperSegmentTicks {
        WhisperSegmentTicks {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn centiseconds_map_to_milliseconds_when_text_matches() {
        let timed = verify_whisper_timed_segments(
            "你好世界最后一句",
            &[
                segment(" 你好", 0, 35),
                segment("世界", 35, 72),
                segment("最后一句", 72, 110),
            ],
            1200,
        )
        .expect("valid model timestamps and complete text should be retained");

        assert_eq!(timed.len(), 3);
        assert_eq!(timed[0].start_ms, 0);
        assert_eq!(timed[0].end_ms, 350);
        assert_eq!(timed[2].start_ms, 720);
        assert_eq!(timed[2].end_ms, 1100);
        assert_eq!(
            timed.iter().map(|s| s.text.as_str()).collect::<String>(),
            "你好世界最后一句"
        );
    }

    #[test]
    fn incomplete_tail_text_rejects_all_exact_ranges() {
        let result = verify_whisper_timed_segments(
            "会议结论和最后行动项",
            &[segment("会议结论和", 0, 80), segment("最后", 80, 110)],
            1500,
        );

        assert!(
            result.is_none(),
            "partial timed text must not hide a canonical tail"
        );
    }

    #[test]
    fn failed_timestamp_decode_keeps_canonical_transcript_without_timing() {
        let (timed, precision) = timing_or_unavailable::<&str>(Err("timestamp pass failed"));

        assert!(timed.is_empty());
        assert_eq!(precision, super::TimingPrecision::Unavailable);
    }

    #[test]
    fn english_word_boundaries_must_match_but_cjk_segment_spaces_may_differ() {
        assert!(
            verify_whisper_timed_segments("New York", &[segment("NewYork", 0, 50)], 1000,)
                .is_none()
        );
        assert!(verify_whisper_timed_segments(
            "你好世界",
            &[segment("你好", 0, 35), segment("世界", 35, 70)],
            1000,
        )
        .is_some());
    }

    #[test]
    fn invalid_or_out_of_audio_ranges_reject_the_complete_timing_set() {
        assert!(verify_whisper_timed_segments(
            "甲乙",
            &[segment("甲", 0, 80), segment("乙", 70, 120)],
            2000,
        )
        .is_none());
        assert!(verify_whisper_timed_segments("甲", &[segment("甲", -1, 2)], 100).is_none());
        assert!(verify_whisper_timed_segments("甲", &[segment("甲", 0, 11)], 100).is_none());
        assert!(verify_whisper_timed_segments("甲", &[segment("甲", 0, 1)], 9).is_none());
        assert!(verify_whisper_timed_segments("甲", &[], 100).is_none());
    }
}
