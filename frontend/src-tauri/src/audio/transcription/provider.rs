// audio/transcription/provider.rs
//
// Defines the unified TranscriptionProvider trait and common types for all
// transcription engines (Whisper, Parakeet, future providers).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ============================================================================
// TRANSCRIPTION PROVIDER TRAIT & ERROR TYPES
// ============================================================================

/// Granular error types for transcription operations
#[derive(Debug, Clone)]
pub enum TranscriptionError {
    ModelNotLoaded,
    AudioTooShort { samples: usize, minimum: usize },
    EngineFailed(String),
    UnsupportedLanguage(String),
}

impl std::fmt::Display for TranscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelNotLoaded => write!(f, "No transcription model is loaded"),
            Self::AudioTooShort { samples, minimum } => write!(
                f,
                "Audio too short: {} samples (minimum {})",
                samples, minimum
            ),
            Self::EngineFailed(msg) => write!(f, "Transcription engine failed: {}", msg),
            Self::UnsupportedLanguage(lang) => {
                write!(f, "Language '{}' is not supported by this provider", lang)
            }
        }
    }
}

impl std::error::Error for TranscriptionError {}

/// Confidence level of transcript timing, separate from text confidence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingPrecision {
    Exact,
    Approximate,
    #[default]
    Unavailable,
}

impl TimingPrecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Approximate => "approximate",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn from_audio_bounds(start: Option<f64>, end: Option<f64>) -> Self {
        let (Some(start), Some(end)) = (start, end) else {
            return Self::Unavailable;
        };
        if start.is_finite() && end.is_finite() && start >= 0.0 && end >= start {
            Self::Approximate
        } else {
            Self::Unavailable
        }
    }
}

/// Provider-level timed text. No segment is emitted unless the provider supplies timing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedText {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Validate provider-relative word/segment times and move them onto the original recording clock.
/// Invalid or overflowing timing invalidates the whole set so callers can keep their coarse bounds.
pub fn offset_timed_segments(
    segments: Vec<TimedText>,
    source_offset_ms: i64,
    source_duration_ms: i64,
) -> Option<Vec<TimedText>> {
    if segments.is_empty() || source_offset_ms < 0 || source_duration_ms <= 0 {
        return None;
    }
    source_offset_ms.checked_add(source_duration_ms)?;

    let mut previous_start_ms = None;
    let mut mapped = Vec::with_capacity(segments.len());
    for segment in segments {
        if segment.text.trim().is_empty()
            || segment.start_ms < 0
            || segment.end_ms <= segment.start_ms
            || segment.end_ms > source_duration_ms
            || previous_start_ms.is_some_and(|previous| segment.start_ms < previous)
        {
            return None;
        }
        previous_start_ms = Some(segment.start_ms);
        mapped.push(TimedText {
            text: segment.text,
            start_ms: source_offset_ms.checked_add(segment.start_ms)?,
            end_ms: source_offset_ms.checked_add(segment.end_ms)?,
        });
    }
    Some(mapped)
}

/// Unified transcription result across all providers
#[derive(Debug, Clone)]
pub struct TranscriptResult {
    pub text: String,
    pub confidence: Option<f32>, // None if provider doesn't support confidence scores
    pub is_partial: bool,
    pub timed_segments: Vec<TimedText>,
    pub timing_precision: TimingPrecision,
}

impl TranscriptResult {
    pub fn unavailable(text: String, confidence: Option<f32>, is_partial: bool) -> Self {
        Self {
            text,
            confidence,
            is_partial,
            timed_segments: Vec::new(),
            timing_precision: TimingPrecision::Unavailable,
        }
    }
}

#[cfg(test)]
mod timing_tests {
    use super::{TimedText, TimingPrecision, TranscriptResult, offset_timed_segments};

    #[test]
    fn unavailable_timing_remains_unknown() {
        let result = TranscriptResult::unavailable("中文尾句".into(), None, false);
        assert_eq!(result.timing_precision, TimingPrecision::Unavailable);
        assert!(result.timed_segments.is_empty());
    }

    #[test]
    fn only_valid_audio_bounds_are_approximate() {
        assert_eq!(
            TimingPrecision::from_audio_bounds(Some(1.0), Some(2.0)),
            TimingPrecision::Approximate
        );
        assert_eq!(
            TimingPrecision::from_audio_bounds(Some(2.0), Some(1.0)),
            TimingPrecision::Unavailable
        );
        assert_eq!(
            TimingPrecision::from_audio_bounds(Some(f64::NAN), Some(2.0)),
            TimingPrecision::Unavailable
        );
    }

    #[test]
    fn timed_segments_are_offset_into_the_original_recording_clock() {
        let mapped = offset_timed_segments(
            vec![
                TimedText {
                    text: "first".into(),
                    start_ms: 120,
                    end_ms: 480,
                },
                TimedText {
                    text: "second".into(),
                    start_ms: 480,
                    end_ms: 800,
                },
            ],
            12_000,
            1_000,
        )
        .unwrap();

        assert_eq!(mapped[0].start_ms, 12_120);
        assert_eq!(mapped[0].end_ms, 12_480);
        assert_eq!(mapped[1].start_ms, 12_480);
        assert_eq!(mapped[1].end_ms, 12_800);
    }

    #[test]
    fn invalid_local_timing_falls_back_as_a_whole_without_partial_offsets() {
        let invalid = vec![
            TimedText {
                text: "valid".into(),
                start_ms: 100,
                end_ms: 200,
            },
            TimedText {
                text: "outside audio".into(),
                start_ms: 900,
                end_ms: 1_100,
            },
        ];

        assert!(offset_timed_segments(invalid, 12_000, 1_000).is_none());
        assert!(
            offset_timed_segments(
                vec![TimedText {
                    text: "overflow".into(),
                    start_ms: 1,
                    end_ms: 2,
                }],
                i64::MAX - 1,
                10,
            )
            .is_none()
        );
    }
}

/// Trait for transcription providers (Whisper, Parakeet, future providers)
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Transcribe audio samples to text
    ///
    /// # Arguments
    /// * `audio` - Audio samples (16kHz mono, f32 format)
    /// * `language` - Optional language hint (e.g., "en", "es", "fr")
    ///
    /// # Returns
    /// * `TranscriptResult` with text, optional confidence, and partial flag
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError>;

    /// Check if a model is currently loaded
    async fn is_model_loaded(&self) -> bool;

    /// Get the name of the currently loaded model
    async fn get_current_model(&self) -> Option<String>;

    /// Get the provider name (for logging/debugging)
    fn provider_name(&self) -> &'static str;
}
