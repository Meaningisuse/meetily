//! Accelerated 120-minute equivalent PCM backlog + real tiny.en inference.
//! Runs only against a temporary directory, never captures devices.
use anyhow::{bail, Context, Result};
use app_lib::audio::{
    recording_state::{AudioChunk, DeviceType},
    work_queue::audio_work_queue,
};
use std::{process::Command, time::Instant};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
fn rss_kib() -> Result<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}
#[tokio::main]
async fn main() -> Result<()> {
    let model = std::env::args()
        .nth(1)
        .context("usage: processing_stress MODEL")?;
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jfk.wav"),
    )?;
    let mut offset = 12;
    let mut fixture = Vec::new();
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let data = bytes
            .get(offset + 8..offset + 8 + size)
            .context("Invalid WAV")?;
        if &bytes[offset..offset + 4] == b"data" {
            fixture = data
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();
            break;
        }
        offset += 8 + size + size % 2;
    }
    if fixture.is_empty() {
        bail!("No fixture PCM");
    }
    let directory = tempfile::tempdir()?;
    let (sender, mut receiver) = audio_work_queue(8, Some(directory.path().to_path_buf()))?;
    let store = receiver.store().context("No store")?;
    let started = Instant::now();
    let mut peak = rss_kib()?;
    // Producer outruns inference: retain 240 actual 30-second chunks on disk.
    for id in 0..240 {
        let data = (0..480000).map(|i| fixture[i % fixture.len()]).collect();
        sender.send(AudioChunk {
            data,
            sample_rate: 16000,
            timestamp: id as f64 * 30.0,
            chunk_id: id,
            device_type: DeviceType::Microphone,
        })?;
        if id % 20 == 0 {
            peak = peak.max(rss_kib()?);
        }
    }
    let peak_pending = store.pending()?.len();
    drop(sender);
    let context = WhisperContext::new_with_params(
        &model,
        WhisperContextParameters {
            use_gpu: false,
            ..Default::default()
        },
    )?;
    let mut state = context.create_state()?;
    let mut ids = Vec::new();
    let mut last_end = 0f64;
    while let Some(chunk) = receiver.recv().await? {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        state.full(params, &chunk.data)?;
        let mut text = String::new();
        for i in 0..state.full_n_segments()? {
            text.push_str(&state.full_get_segment_text(i)?);
        }
        if !text.to_lowercase().contains("country") {
            bail!("Missing expected speech in chunk {}", chunk.chunk_id);
        }
        store.complete(
            chunk.chunk_id,
            &serde_json::json!({"text":text,"sequence_id":chunk.chunk_id}),
        )?;
        ids.push(chunk.chunk_id);
        last_end = chunk.timestamp + chunk.data.len() as f64 / 16000.0;
        peak = peak.max(rss_kib()?);
        if ids.len() % 40 == 0 {
            eprintln!("processed {}/240", ids.len());
        }
    }
    if ids != (0..240).collect::<Vec<_>>() || last_end != 7200.0 || !store.pending()?.is_empty() {
        bail!("Incomplete backlog");
    }
    println!(
        "{}",
        serde_json::json!({"fixture":"JFK repeated, synthetic","input_audio_seconds":7200,"chunks":240,"chunk_seconds":30,"live_capacity":8,"peak_pending":peak_pending,"completed":ids.len(),"last_end_seconds":last_end,"peak_rss_kib":peak,"elapsed_seconds":started.elapsed().as_secs_f64(),"pending_after":store.pending()?.len(),"captures_devices":false,"real_time_120_minutes":false})
    );
    Ok(())
}
