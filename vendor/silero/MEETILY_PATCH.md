# Meetily VAD corrections

Source: emotechlab/silero-rs commit 26a6460 (previous Cargo.lock source). MIT license and the original embedded model are retained.

Two upstream implementation defects exposed by bounded live audio are corrected in src/lib.rs:

- trim_start_silence preserves VadState::Speech's candidate start before min_speech_time sets speech_start_ms. Otherwise delayed SpeechStart refers to samples already removed.
- take_until updates both VadState::Speech.start_ms and speech_start_ms. Otherwise a later SpeechEnd reads the old removed start and panics.

App-level RED→GREEN coverage lives in audio::vad::bounded_session_tests together with the original timestamp, flush, and chunked/batch equivalence regressions. This is a maintained source dependency, not a restored legacy application backend. Compare this file and these two method patches when upgrading the upstream dependency.
