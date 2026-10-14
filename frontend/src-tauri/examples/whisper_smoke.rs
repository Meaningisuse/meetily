//! Manual CPU/Metal inference check using the licensed JFK fixture.
//! cargo run --locked -p meetily --example whisper_smoke -- MODEL cpu|metal
use anyhow::{bail, Context, Result};
use std::{path::PathBuf, time::Instant};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || !matches!(args[1].as_str(), "cpu" | "metal") {
        bail!("usage: whisper_smoke MODEL cpu|metal");
    }
    if args[1] == "metal" && !cfg!(target_os = "macos") {
        bail!("Metal smoke check requires macOS");
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jfk.wav");
    let bytes = std::fs::read(fixture)?;
    // Locate the data chunk instead of assuming a fixed WAV header size.
    let mut offset = 12;
    let mut samples = Vec::new();
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let data = bytes
            .get(offset + 8..offset + 8 + size)
            .context("invalid WAV chunk")?;
        if &bytes[offset..offset + 4] == b"data" {
            samples = data
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();
            break;
        }
        offset += 8 + size + (size % 2);
    }
    if samples.is_empty() {
        bail!("fixture has no PCM samples");
    }
    let started = Instant::now();
    let context = WhisperContext::new_with_params(
        &args[0],
        WhisperContextParameters {
            use_gpu: args[1] == "metal",
            flash_attn: false,
            ..Default::default()
        },
    )?;
    let mut state = context.create_state()?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(4);
    params.set_language(Some("en"));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state.full(params, &samples)?;
    let mut text = String::new();
    let mut segments = Vec::new();
    for i in 0..state.full_n_segments()? {
        let part = state.full_get_segment_text(i)?;
        text.push_str(&part);
        segments.push(serde_json::json!({"text": part, "start_ms": state.full_get_segment_t0(i)? * 10, "end_ms": state.full_get_segment_t1(i)? * 10}));
    }
    if !text.to_lowercase().contains("country") {
        bail!("JFK smoke inference did not recover expected speech: {text}");
    }
    println!(
        "{}",
        serde_json::json!({"mode": args[1], "elapsed_ms": started.elapsed().as_millis(), "audio_ms": samples.len() * 1000 / 16000, "segments": segments})
    );
    Ok(())
}
