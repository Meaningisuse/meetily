// audio/transcription/parakeet_provider.rs
//
// Parakeet transcription provider implementation.

use super::provider::{
    TimedText, TimingPrecision, TranscriptResult, TranscriptionError, TranscriptionProvider,
};
use async_trait::async_trait;
use log::warn;
use std::sync::Arc;

/// Parakeet transcription provider (wraps ParakeetEngine)
pub struct ParakeetProvider {
    engine: Arc<crate::parakeet_engine::ParakeetEngine>,
}

impl ParakeetProvider {
    pub fn new(engine: Arc<crate::parakeet_engine::ParakeetEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TranscriptionProvider for ParakeetProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        // Log language preference warning if set (Parakeet doesn't support it yet)
        if let Some(ref lang) = language {
            warn!(
                "Parakeet doesn't support language preference '{}' yet - transcribing in default language",
                lang
            );
        }

        match self.engine.transcribe_audio_with_timestamps(audio).await {
            Ok(result) => {
                let (timed_segments, timing_precision) = token_timestamps_to_segments(
                    &result.text,
                    &result.tokens,
                    &result.timestamps,
                    result.audio_duration_ms,
                );
                Ok(TranscriptResult {
                    text: result.text.trim().to_string(),
                    confidence: None,
                    is_partial: false,
                    timed_segments,
                    timing_precision,
                })
            }
            Err(e) => Err(TranscriptionError::EngineFailed(e.to_string())),
        }
    }

    async fn is_model_loaded(&self) -> bool {
        self.engine.is_model_loaded().await
    }

    async fn get_current_model(&self) -> Option<String> {
        self.engine.get_current_model().await
    }

    fn provider_name(&self) -> &'static str {
        "Parakeet"
    }
}

pub(crate) fn token_timestamps_to_segments(
    canonical_text: &str,
    tokens: &[String],
    timestamps_seconds: &[f32],
    audio_duration_ms: i64,
) -> (Vec<TimedText>, TimingPrecision) {
    const FRAME_MS: i64 = 80;

    if tokens.is_empty() || tokens.len() != timestamps_seconds.len() || audio_duration_ms <= 0 {
        return (Vec::new(), TimingPrecision::Unavailable);
    }

    let mut groups: Vec<(String, i64)> = Vec::new();
    let mut previous_start_ms = None;
    for (token, timestamp) in tokens.iter().zip(timestamps_seconds) {
        if !timestamp.is_finite() || *timestamp < 0.0 {
            return (Vec::new(), TimingPrecision::Unavailable);
        }
        let start_ms = (*timestamp as f64 * 1000.0).round() as i64;
        if start_ms >= audio_duration_ms
            || previous_start_ms.is_some_and(|previous| start_ms < previous)
        {
            return (Vec::new(), TimingPrecision::Unavailable);
        }
        previous_start_ms = Some(start_ms);

        let begins_word = token.chars().next().is_some_and(char::is_whitespace);
        let text = token.trim();
        if text.is_empty() {
            continue;
        }
        let is_punctuation = text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_punctuation() || "，。！？；：、）》】」』".contains(ch));

        if begins_word && !is_punctuation {
            if let Some((current, current_start)) = groups.last() {
                if start_ms > *current_start {
                    groups.push((text.to_owned(), start_ms));
                    continue;
                }
                if !current.is_empty() {
                    groups.last_mut().expect("group exists").0.push(' ');
                }
            }
        }

        if let Some((current, _)) = groups.last_mut() {
            current.push_str(text);
        } else {
            groups.push((text.to_owned(), start_ms));
        }
    }

    if groups.is_empty() {
        return (Vec::new(), TimingPrecision::Unavailable);
    }

    let mut segments = Vec::with_capacity(groups.len());
    for index in 0..groups.len() {
        let (text, start_ms) = &groups[index];
        let end_ms = groups
            .get(index + 1)
            .map(|(_, next_start)| *next_start)
            .unwrap_or_else(|| {
                timestamps_seconds
                    .last()
                    .map(|last| {
                        ((*last as f64 * 1000.0).round() as i64 + FRAME_MS).min(audio_duration_ms)
                    })
                    .unwrap_or(*start_ms)
            });
        if text.is_empty() || end_ms <= *start_ms {
            return (Vec::new(), TimingPrecision::Unavailable);
        }
        segments.push(TimedText {
            text: text.clone(),
            start_ms: *start_ms,
            end_ms,
        });
    }

    let reconstructed = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if reconstructed != canonical_text.trim() {
        return (Vec::new(), TimingPrecision::Unavailable);
    }

    (segments, TimingPrecision::Approximate)
}

#[cfg(test)]
mod tests {
    use super::token_timestamps_to_segments;
    use crate::audio::transcription::TimingPrecision;

    #[test]
    fn parakeet_token_emissions_form_approximate_word_spans() {
        let (segments, precision) = token_timestamps_to_segments(
            "Hello world!",
            &[" Hello", " world", "!"].map(str::to_owned),
            &[0.12, 0.48, 0.72],
            1_000,
        );

        assert_eq!(precision, TimingPrecision::Approximate);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Hello", "world!"]
        );
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (120, 480));
        assert_eq!((segments[1].start_ms, segments[1].end_ms), (480, 800));
    }

    #[test]
    fn malformed_or_out_of_audio_parakeet_times_remain_unavailable() {
        for (tokens, times) in [
            (vec![" Hello".to_owned()], vec![]),
            (vec![" Hello".to_owned()], vec![f32::NAN]),
            (vec![" Hello".to_owned()], vec![1.1]),
        ] {
            let (segments, precision) =
                token_timestamps_to_segments("Hello", &tokens, &times, 1_000);
            assert!(segments.is_empty());
            assert_eq!(precision, TimingPrecision::Unavailable);
        }
    }

    #[test]
    fn same_frame_tokens_do_not_create_zero_length_segments() {
        let (segments, precision) = token_timestamps_to_segments(
            "Meetily works",
            &[" Meet", "ily", " works"].map(str::to_owned),
            &[0.10, 0.10, 0.30],
            1_000,
        );

        assert_eq!(precision, TimingPrecision::Approximate);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Meetily");
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (100, 300));
        assert_eq!((segments[1].start_ms, segments[1].end_ms), (300, 380));
    }

    #[test]
    fn token_grouping_that_changes_canonical_text_falls_back_to_unavailable() {
        let (segments, precision) = token_timestamps_to_segments(
            "No separator.",
            &[" No", " separator"].map(str::to_owned),
            &[0.1, 0.4],
            1_000,
        );

        assert!(segments.is_empty());
        assert_eq!(precision, TimingPrecision::Unavailable);
    }

    #[test]
    #[ignore = "requires a local Parakeet ONNX model directory and audio fixture"]
    fn parakeet_model_emissions_stay_bounded_and_approximate() {
        let model_dir = std::env::var("PARAKEET_TIMING_TEST_MODEL_DIR")
            .expect("set PARAKEET_TIMING_TEST_MODEL_DIR");
        let audio_path =
            std::env::var("PARAKEET_TIMING_TEST_AUDIO").expect("set PARAKEET_TIMING_TEST_AUDIO");
        let audio = crate::audio::decoder::decode_audio_file(std::path::Path::new(&audio_path))
            .expect("decode timing fixture");
        let samples = audio.into_whisper_format().expect("convert timing fixture");
        let duration_ms = (samples.len() as f64 / 16_000.0 * 1000.0).round() as i64;
        let mut model = crate::parakeet_engine::ParakeetModel::new(model_dir, true)
            .expect("load Parakeet ONNX model");
        let result = model
            .transcribe_samples(samples)
            .expect("transcribe timing fixture");
        let (segments, precision) = token_timestamps_to_segments(
            &result.text,
            &result.tokens,
            &result.timestamps,
            result.audio_duration_ms,
        );

        assert!(!result.text.trim().is_empty());
        assert!(!segments.is_empty());
        assert_eq!(precision, TimingPrecision::Approximate);
        assert!(segments.iter().all(|segment| {
            segment.start_ms >= 0
                && segment.end_ms > segment.start_ms
                && segment.end_ms <= duration_ms
        }));
        println!("text={:?} segments={segments:?}", result.text);
    }
}
