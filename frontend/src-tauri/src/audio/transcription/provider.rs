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
    use super::{TimingPrecision, TranscriptResult};

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
