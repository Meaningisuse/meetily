//! Explicit replay after a crash. Original inputs remain until acceptance.
use super::{
    transcription::{
        engine,
        provider::{offset_timed_segments, TimingPrecision},
        worker::{transcribe_chunk_with_provider, TranscriptUpdate},
    },
    work_queue::AudioWorkReceiver,
};
use tauri::Manager;

fn verified_folder(
    base: &std::path::Path,
    folder: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let folder = folder.canonicalize().map_err(|e| e.to_string())?;
    if folder == base || !folder.starts_with(&base) || !folder.join("metadata.json").is_file() {
        return Err(
            "Recovery folder must be a meeting under the configured recordings directory".into(),
        );
    }
    Ok(folder)
}

#[tauri::command]
pub async fn recover_transcription_backlog(
    app: tauri::AppHandle,
    meeting_folder: String,
) -> Result<Vec<TranscriptUpdate>, String> {
    if super::recording_commands::is_recording().await {
        return Err("Stop recording before replaying saved audio".into());
    }
    let _lifecycle = super::common::acquire_engine_lifecycle_lock().await;
    if super::recording_commands::is_recording().await {
        return Err("Recording is active".into());
    }
    let folder = verified_folder(
        &super::recording_preferences::get_default_recordings_folder(),
        std::path::Path::new(&meeting_folder),
    )?;
    if !folder.join(".catchup").is_dir() {
        return Ok(Vec::new());
    }
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("Database is not ready")?;
    let _permit = state
        .jobs
        .try_audio_recovery_slot()
        .map_err(|e| e.message)?;
    let mut receiver = AudioWorkReceiver::recover(folder).map_err(|e| e.to_string())?;
    let store = receiver.store().ok_or("Catch-up store unavailable")?;
    // Validate before inference; retries are explicit and never run at startup.
    let pending = store.pending().map_err(|e| e.to_string())?;
    if !pending.is_empty() {
        engine::validate_transcription_model_ready(&app).await?;
        let engine = engine::get_or_init_transcription_engine(&app).await?;
        while let Some(chunk) = receiver.recv().await.map_err(|e| e.to_string())? {
            let id = chunk.chunk_id;
            let start = chunk.timestamp;
            let duration = chunk.data.len() as f64 / chunk.sample_rate as f64;
            let (text, confidence, is_partial, timed_segments, provider_precision) =
                transcribe_chunk_with_provider(&engine, chunk, &app)
                    .await
                    .map_err(|e| e.to_string())?;
            if is_partial {
                return Err(
                    "Provider returned a partial transcript; source retained for another attempt"
                        .into(),
                );
            }
            let mapped_timed_segments =
                if provider_precision != TimingPrecision::Unavailable {
                    offset_timed_segments(
                        timed_segments,
                        (start * 1000.0).round() as i64,
                        (duration * 1000.0).round() as i64,
                    )
                } else {
                    None
                };
            let result = TranscriptUpdate {
                text,
                timestamp: format!("{:02}:{:02}", start as u64 / 60, start as u64 % 60),
                source: "Saved audio".into(),
                sequence_id: id,
                chunk_start_time: start,
                is_partial: false,
                confidence: confidence.unwrap_or(1.0),
                audio_start_time: start,
                audio_end_time: start + duration,
                duration,
                timing_precision: if mapped_timed_segments.is_some() {
                    provider_precision
                } else {
                    TimingPrecision::from_audio_bounds(Some(start), Some(start + duration))
                },
                timed_segments: mapped_timed_segments.unwrap_or_default(),
            };
            store
                .complete(
                    id,
                    &serde_json::to_value(result).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
        }
    }
    store
        .completed_results()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_path_rejects_escape_and_base() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("metadata.json"), b"{}").unwrap();
        assert!(verified_folder(base.path(), outside.path()).is_err());
        assert!(verified_folder(base.path(), base.path()).is_err());
        let meeting = base.path().join("meeting");
        std::fs::create_dir(&meeting).unwrap();
        std::fs::write(meeting.join("metadata.json"), b"{}").unwrap();
        assert!(verified_folder(base.path(), &meeting).is_ok());
    }
}
