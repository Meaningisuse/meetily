use anyhow::Result;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;

use super::audio_processing::create_meeting_folder;
use super::incremental_saver::IncrementalAudioSaver;
use super::recording_state::AudioChunk;

fn default_recording_timing_precision() -> super::transcription::provider::TimingPrecision {
    super::transcription::provider::TimingPrecision::Approximate
}

/// Structured transcript segment for JSON export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub audio_start_time: f64, // Seconds from recording start
    pub audio_end_time: f64,   // Seconds from recording start
    pub duration: f64,         // Segment duration in seconds
    pub display_time: String,  // Formatted time for display like "[02:15]"
    pub confidence: f32,
    pub sequence_id: u64,
    #[serde(default)]
    pub segment_index: u32,
    #[serde(default = "default_recording_timing_precision")]
    pub timing_precision: super::transcription::provider::TimingPrecision,
}

pub(crate) fn transcript_update_segments(
    update: &super::transcription::worker::TranscriptUpdate,
) -> Vec<TranscriptSegment> {
    if update.timed_segments.is_empty() {
        return vec![TranscriptSegment {
            id: format!("seg_{}", update.sequence_id),
            text: update.text.clone(),
            audio_start_time: update.audio_start_time,
            audio_end_time: update.audio_end_time,
            duration: update.duration,
            display_time: update.timestamp.clone(),
            confidence: update.confidence,
            sequence_id: update.sequence_id,
            segment_index: 0,
            timing_precision: super::transcription::provider::TimingPrecision::from_audio_bounds(
                Some(update.audio_start_time),
                Some(update.audio_end_time),
            ),
        }];
    }

    update
        .timed_segments
        .iter()
        .enumerate()
        .map(|(index, timed)| {
            let start = timed.start_ms as f64 / 1000.0;
            let end = timed.end_ms as f64 / 1000.0;
            TranscriptSegment {
                id: format!("seg_{}_{}", update.sequence_id, index),
                text: timed.text.clone(),
                audio_start_time: start,
                audio_end_time: end,
                duration: end - start,
                display_time: update.timestamp.clone(),
                confidence: update.confidence,
                sequence_id: update.sequence_id,
                segment_index: index as u32,
                timing_precision: super::transcription::provider::TimingPrecision::Exact,
            }
        })
        .collect()
}

/// Meeting metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub version: String,
    pub meeting_id: Option<String>,
    pub meeting_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<f64>,
    pub devices: DeviceInfo,
    pub audio_file: String,
    pub transcript_file: String,
    pub sample_rate: u32,
    pub status: String, // "recording", "completed", "error"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
}

/// New recording saver using incremental saving strategy
pub struct RecordingSaver {
    incremental_saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    transcript_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    accumulation_handle: Option<tokio::task::JoinHandle<Result<()>>>,
    capture_error: Option<String>,
    transcription_incomplete: bool,
}

impl RecordingSaver {
    pub fn new() -> Self {
        Self {
            incremental_saver: None,
            meeting_folder: None,
            meeting_name: None,
            metadata: None,
            transcript_segments: Arc::new(Mutex::new(Vec::new())),
            accumulation_handle: None,
            capture_error: None,
            transcription_incomplete: false,
        }
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.meeting_name = name;
    }

    /// Set device information in metadata
    pub fn set_device_info(&mut self, mic_name: Option<String>, sys_name: Option<String>) {
        if let Some(ref mut metadata) = self.metadata {
            metadata.devices.microphone = mic_name;
            metadata.devices.system_audio = sys_name;

            // Write updated metadata to disk if folder exists
            if let Some(folder) = &self.meeting_folder {
                let metadata_clone = metadata.clone();
                if let Err(e) = self.write_metadata(folder, &metadata_clone) {
                    warn!("Failed to update metadata with device info: {}", e);
                }
            }
        }
    }

    pub(crate) fn set_capture_error(&mut self, error: Option<String>) {
        self.capture_error = error;
    }

    pub(crate) fn set_transcription_incomplete(&mut self, incomplete: bool) {
        self.transcription_incomplete = incomplete;
    }

    pub(crate) fn history_handle(&self) -> Arc<Mutex<Vec<TranscriptSegment>>> {
        self.transcript_segments.clone()
    }

    /// Add or update a structured transcript segment (upserts based on sequence_id)
    /// Also saves incrementally to disk
    pub fn add_transcript_segment(&self, segment: TranscriptSegment) {
        if let Ok(mut segments) = self.transcript_segments.lock() {
            // Check if segment with same sequence_id exists (update it)
            if let Some(existing) = segments
                .iter_mut()
            .find(|s| s.sequence_id == segment.sequence_id && s.segment_index == segment.segment_index)
            {
                *existing = segment.clone();
                info!(
                    "Updated transcript segment {} (seq: {}) - total segments: {}",
                    segment.id,
                    segment.sequence_id,
                    segments.len()
                );
            } else {
                // New segment, add it
                segments.push(segment.clone());
                info!(
                    "Added new transcript segment {} (seq: {}) - total segments: {}",
                    segment.id,
                    segment.sequence_id,
                    segments.len()
                );
            }
        } else {
            error!(
                "Failed to lock transcript segments for adding segment {}",
                segment.id
            );
        }

        // NEW: Save incrementally to disk
        if let Some(folder) = &self.meeting_folder {
            if let Err(e) = self.write_transcripts_json(folder) {
                warn!("Failed to write incremental transcript update: {}", e);
            }
        }
    }

    /// Legacy method for backward compatibility - converts text to basic segment
    pub fn add_transcript_chunk(&self, text: String) {
        let segment = TranscriptSegment {
            id: format!("seg_{}", chrono::Utc::now().timestamp_millis()),
            text,
            audio_start_time: 0.0,
            audio_end_time: 0.0,
            duration: 0.0,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 0,
            segment_index: 0,
            timing_precision: super::transcription::provider::TimingPrecision::Unavailable,
        };
        self.add_transcript_segment(segment);
    }

    /// Start accumulation with optional incremental saving
    ///
    /// # Arguments
    /// * `auto_save` - If true, creates checkpoints and enables saving. If false, audio chunks are discarded.
    pub fn start_accumulation(
        &mut self,
        auto_save: bool,
        receiver: mpsc::Receiver<AudioChunk>,
        state: Arc<super::recording_state::RecordingState>,
    ) -> Result<()> {
        if self.accumulation_handle.is_some() {
            return Err(anyhow::anyhow!("Previous audio drain is still active"));
        }
        let name = self
            .meeting_name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Meeting name is required before audio capture"))?;
        self.initialize_meeting_folder(&name, auto_save)?;
        self.accumulation_handle = Some(Self::spawn_accumulation_task(
            receiver,
            self.incremental_saver.clone(),
            auto_save,
            state,
        ));
        Ok(())
    }
    fn spawn_accumulation_task(
        mut receiver: mpsc::Receiver<AudioChunk>,
        saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
        save_audio: bool,
        state: Arc<super::recording_state::RecordingState>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move {
            // Sender closure defines the end. A stop flag must never discard
            // chunks already accepted from the mixing pipeline.
            while let Some(chunk) = receiver.recv().await {
                if save_audio {
                    let saver = saver
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Audio saver was not initialized"))?;
                    let result =
                        tokio::task::spawn_blocking(move || saver.blocking_lock().add_chunk(chunk))
                            .await;
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => Err(anyhow::anyhow!(error)),
                    };
                    if let Err(error) = result {
                        state.report_error(super::recording_state::AudioError::StorageFailed);
                        return Err(error);
                    }
                }
            }
            Ok(())
        })
    }
    async fn wait_for_audio_drain(&mut self) -> Result<(), String> {
        if let Some(handle) = self.accumulation_handle.take() {
            handle
                .await
                .map_err(|e| format!("Audio drain failed: {e}"))?
                .map_err(|e| format!("Audio could not be saved: {e}"))?;
        }
        Ok(())
    }

    fn restore_catchup_transcripts(&self) -> Result<()> {
        let Some(folder) = &self.meeting_folder else {
            return Ok(());
        };
        if !folder.join(".catchup").is_dir() {
            return Ok(());
        }
        let store = super::work_queue::CatchupStore::open(folder.clone())?;
        let mut segments = self
            .transcript_segments
            .lock()
            .map_err(|_| anyhow::anyhow!("Transcript history lock failed"))?;
        let mut ids: std::collections::HashSet<(u64, u32)> = segments
            .iter()
            .map(|s| (s.sequence_id, s.segment_index))
            .collect();
        for value in store.completed_results()? {
            if value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| text.trim().is_empty())
            {
                continue;
            }
            let update: super::transcription::worker::TranscriptUpdate =
                serde_json::from_value(value)?;
            for segment in transcript_update_segments(&update) {
                if ids.insert((segment.sequence_id, segment.segment_index)) {
                    segments.push(segment);
                }
            }
        }
        segments.sort_by_key(|s| (s.sequence_id, s.segment_index));
        Ok(())
    }

    /// Initialize meeting folder structure and metadata
    ///
    /// # Arguments
    /// * `meeting_name` - Name of the meeting
    /// * `create_checkpoints` - Whether to create .checkpoints/ directory and IncrementalAudioSaver
    fn initialize_meeting_folder(
        &mut self,
        meeting_name: &str,
        create_checkpoints: bool,
    ) -> Result<()> {
        // Load preferences to get base recordings folder
        let base_folder = super::recording_preferences::get_default_recordings_folder();

        // Create meeting folder structure (with or without .checkpoints/ subdirectory)
        let meeting_folder = create_meeting_folder(&base_folder, meeting_name, create_checkpoints)?;

        // Only initialize incremental saver if checkpoints are needed (auto_save is true)
        if create_checkpoints {
            let incremental_saver = IncrementalAudioSaver::new(meeting_folder.clone(), 48000)?;
            self.incremental_saver = Some(Arc::new(AsyncMutex::new(incremental_saver)));
            info!(
                "✅ Incremental audio saver initialized for meeting: {}",
                meeting_name
            );
        } else {
            info!("⚠️  Skipped incremental audio saver (auto-save disabled)");
        }

        // Create initial metadata
        let metadata = MeetingMetadata {
            version: "1.0".to_string(),
            meeting_id: None, // Will be set by backend
            meeting_name: Some(meeting_name.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_seconds: None,
            devices: DeviceInfo {
                microphone: None, // Could be enhanced to store actual device names
                system_audio: None,
            },
            audio_file: if create_checkpoints {
                "audio.mp4".to_string()
            } else {
                "".to_string()
            },
            transcript_file: "transcripts.json".to_string(),
            sample_rate: 48000,
            status: "recording".to_string(),
        };

        // Write initial metadata.json
        self.write_metadata(&meeting_folder, &metadata)?;

        self.meeting_folder = Some(meeting_folder);
        self.metadata = Some(metadata);

        Ok(())
    }

    /// Write metadata.json to disk (atomic write with temp file)
    fn write_metadata(&self, folder: &PathBuf, metadata: &MeetingMetadata) -> Result<()> {
        Self::atomic_private_write(
            folder,
            "metadata.json",
            &serde_json::to_vec_pretty(metadata)?,
        )
    }

    fn atomic_private_write(folder: &std::path::Path, name: &str, data: &[u8]) -> Result<()> {
        use std::io::Write;
        let partial = folder.join(format!(".{}.incomplete", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&partial)?;
            file.write_all(data)?;
            file.sync_all()?;
            std::fs::rename(&partial, folder.join(name))?;
            std::fs::File::open(folder)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        result
    }

    /// Write transcripts.json to disk (atomic write with temp file and validation)
    fn write_transcripts_json(&self, folder: &PathBuf) -> Result<()> {
        // Clone segments to avoid holding lock during I/O
        let segments_clone = if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            error!("Failed to lock transcript segments for writing");
            return Err(anyhow::anyhow!("Failed to lock transcript segments"));
        };

        info!(
            "Writing {} transcript segments to JSON",
            segments_clone.len()
        );

        let json = serde_json::json!({"version":"1.0","segments":segments_clone,"last_updated":chrono::Utc::now().to_rfc3339(),"total_segments":segments_clone.len()});
        Self::atomic_private_write(
            folder,
            "transcripts.json",
            &serde_json::to_vec_pretty(&json)?,
        )
    }

    fn persist_final_documents(&mut self, recording_duration: Option<f64>) -> Result<()> {
        let folder = self
            .meeting_folder
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Recording folder unavailable"))?;
        self.write_transcripts_json(folder)?;
        let mut metadata = self
            .metadata
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Recording metadata unavailable"))?;
        let pending = if folder.join(".catchup").is_dir() {
            super::work_queue::CatchupStore::open(folder.clone())?
                .pending()?
                .len()
        } else {
            0
        };
        metadata.status = if self.capture_error.is_some() {
            "recording_error"
        } else if pending > 0 || self.transcription_incomplete {
            "transcription_incomplete"
        } else {
            "completed"
        }
        .into();
        metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());
        metadata.duration_seconds = recording_duration.or_else(|| {
            self.transcript_segments
                .lock()
                .ok()?
                .iter()
                .map(|s| s.audio_end_time)
                .max_by(f64::total_cmp)
        });
        self.write_metadata(folder, &metadata)?;
        self.metadata = Some(metadata);
        Ok(())
    }

    // in frontend/src-tauri/src/audio/recording_saver.rs
    pub fn get_stats(&self) -> (usize, u32) {
        if let Some(ref saver) = self.incremental_saver {
            if let Ok(guard) = saver.try_lock() {
                (guard.get_checkpoint_count() as usize, 48000)
            } else {
                (0, 48000)
            }
        } else {
            (0, 48000)
        }
    }

    /// Stop and save using incremental saving approach
    ///
    /// # Arguments
    /// * `app` - Tauri app handle for emitting events
    /// * `recording_duration` - Actual recording duration in seconds (from RecordingState)
    pub async fn stop_and_save<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        recording_duration: Option<f64>,
    ) -> Result<Option<String>, String> {
        info!("Stopping recording saver");

        self.wait_for_audio_drain().await?;
        self.restore_catchup_transcripts()
            .map_err(|e| format!("Saved transcript readback failed: {e}"))?;

        // Check if incremental saver exists (indicates auto_save was enabled)
        let should_save_audio = self.incremental_saver.is_some();

        if !should_save_audio {
            info!("⚠️  No audio saver initialized (auto-save was disabled) - skipping audio finalization");
            self.persist_final_documents(recording_duration)
                .map_err(|e| format!("Failed to save final transcript/metadata: {e}"))?;
            return Ok(None);
        }

        // Finalize incremental saver (merge checkpoints into final audio.mp4)
        let final_audio_path = if let Some(saver_arc) = &self.incremental_saver {
            let mut saver = saver_arc.lock().await;
            match saver.finalize().await {
                Ok(path) => {
                    info!("✅ Successfully finalized audio: {}", path.display());
                    path
                }
                Err(e) => {
                    error!("❌ Failed to finalize incremental saver: {}", e);
                    return Err(format!("Failed to finalize audio: {}", e));
                }
            }
        } else {
            error!("No incremental saver initialized - cannot save recording");
            return Err("No incremental saver initialized".to_string());
        };

        self.persist_final_documents(recording_duration)
            .map_err(|e| format!("Failed to save final transcript/metadata: {e}"))?;
        if self
            .metadata
            .as_ref()
            .is_some_and(|m| m.status == "completed")
        {
            if let Some(saver) = &self.incremental_saver {
                if let Err(error) = saver.lock().await.cleanup_checkpoints_after_acceptance() {
                    warn!("Meeting saved; retained checkpoints because cleanup failed: {error}");
                }
            }
        }

        // Emit save event with audio and transcript paths
        let save_event = serde_json::json!({
            "audio_file": final_audio_path.to_string_lossy(),
            "transcript_file": self.meeting_folder.as_ref()
                .map(|f| f.join("transcripts.json").to_string_lossy().to_string()),
            "meeting_name": self.meeting_name,
            "meeting_folder": self.meeting_folder.as_ref()
                .map(|f| f.to_string_lossy().to_string())
        });

        if let Err(e) = app.emit("recording-saved", &save_event) {
            warn!("Failed to emit recording-saved event: {}", e);
        }

        // Clean up transcript segments
        if let Ok(mut segments) = self.transcript_segments.lock() {
            segments.clear();
        }

        Ok(Some(final_audio_path.to_string_lossy().to_string()))
    }

    /// Get the meeting folder path (for passing to backend)
    pub fn get_meeting_folder(&self) -> Option<&PathBuf> {
        self.meeting_folder.as_ref()
    }

    /// Get accumulated transcript segments (for reload sync)
    pub fn get_transcript_segments(&self) -> Vec<TranscriptSegment> {
        if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            Vec::new()
        }
    }

    /// Get meeting name (for reload sync)
    pub fn get_meeting_name(&self) -> Option<String> {
        self.meeting_name.clone()
    }
}

impl Default for RecordingSaver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    #[tokio::test]
    async fn queued_audio_is_drained_before_finalization() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join(".checkpoints")).unwrap();
        let saver = Arc::new(AsyncMutex::new(
            IncrementalAudioSaver::new(folder.path().to_path_buf(), 48000).unwrap(),
        ));
        let (sender, receiver) = mpsc::channel(16);
        let handle = RecordingSaver::spawn_accumulation_task(
            receiver,
            Some(saver.clone()),
            true,
            super::super::recording_state::RecordingState::new(),
        );
        for id in 0..11 {
            sender
                .send(AudioChunk {
                    data: vec![0.125; if id < 10 { 144000 } else { 48000 }],
                    sample_rate: 48000,
                    timestamp: id as f64 * 3.0,
                    chunk_id: id,
                    device_type: super::super::recording_state::DeviceType::Microphone,
                })
                .await
                .unwrap();
        }
        drop(sender);
        handle.await.unwrap().unwrap();
        let saver = saver.lock().await;
        assert_eq!(saver.get_checkpoint_count(), 1);
        assert_eq!(saver.buffered_sample_count(), 48000);
    }
}

#[cfg(test)]
mod history_tail_tests {
    use super::*;
    #[test]
    fn detached_stop_history_keeps_the_last_transcript_without_audio_saving() {
        let saver = RecordingSaver::new();
        let history = saver.history_handle();
        drop(saver);
        let segment = TranscriptSegment {
            id: "last".into(),
            text: "最后一句行动项".into(),
            audio_start_time: 120.0,
            audio_end_time: 121.0,
            duration: 1.0,
            display_time: "02:00".into(),
            confidence: 1.0,
            sequence_id: 2,
            segment_index: 0,
            timing_precision: super::super::transcription::provider::TimingPrecision::Approximate,
        };
        let mut history = history.lock().unwrap();
        history.push(segment);
        assert_eq!(history.last().unwrap().text, "最后一句行动项");
    }

    #[test]
    fn verified_provider_times_become_individual_recording_segments() {
        let update = super::super::transcription::worker::TranscriptUpdate {
            text: "会议结论最后行动项".into(),
            timestamp: "00:12".into(),
            source: "Audio".into(),
            sequence_id: 9,
            chunk_start_time: 12.0,
            is_partial: false,
            confidence: 0.9,
            audio_start_time: 12.0,
            audio_end_time: 14.0,
            duration: 2.0,
            timing_precision: super::super::transcription::provider::TimingPrecision::Approximate,
            timed_segments: vec![
                super::super::transcription::provider::TimedText {
                    text: "会议结论".into(),
                    start_ms: 12_100,
                    end_ms: 12_800,
                },
                super::super::transcription::provider::TimedText {
                    text: "最后行动项".into(),
                    start_ms: 13_000,
                    end_ms: 13_900,
                },
            ],
        };

        let segments = transcript_update_segments(&update);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "会议结论");
        assert_eq!(segments[1].text, "最后行动项");
        assert_eq!(segments[1].audio_start_time, 13.0);
        assert_eq!(segments[1].audio_end_time, 13.9);
        assert_eq!(segments[1].timing_precision, super::super::transcription::provider::TimingPrecision::Exact);
        assert_eq!(segments[0].sequence_id, segments[1].sequence_id);
        assert_ne!(segments[0].segment_index, segments[1].segment_index);
    }
    #[test]
    fn final_no_audio_save_persists_detached_tail_and_error_status() {
        let folder = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.meeting_folder = Some(folder.path().to_path_buf());
        saver.metadata = Some(MeetingMetadata {
            version: "1".into(),
            meeting_id: None,
            meeting_name: Some("test".into()),
            created_at: "now".into(),
            completed_at: None,
            duration_seconds: None,
            devices: DeviceInfo {
                microphone: None,
                system_audio: None,
            },
            audio_file: "".into(),
            transcript_file: "transcripts.json".into(),
            sample_rate: 48000,
            status: "recording".into(),
        });
        let history = saver.history_handle();
        history.lock().unwrap().push(TranscriptSegment {
            id: "last".into(),
            text: "最后行动项".into(),
            audio_start_time: 1.0,
            audio_end_time: 2.0,
            duration: 1.0,
            display_time: "00:01".into(),
            confidence: 1.0,
            sequence_id: 7,
            segment_index: 0,
            timing_precision: super::super::transcription::provider::TimingPrecision::Approximate,
        });
        saver.set_capture_error(Some("No recovery audio".into()));
        saver.persist_final_documents(Some(2.0)).unwrap();
        let transcripts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.path().join("transcripts.json")).unwrap())
                .unwrap();
        let metadata: MeetingMetadata =
            serde_json::from_slice(&std::fs::read(folder.path().join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(transcripts["segments"][0]["text"], "最后行动项");
        assert_eq!(metadata.status, "recording_error");
        assert_eq!(metadata.duration_seconds, Some(2.0));
        saver.set_capture_error(None);
        saver.set_transcription_incomplete(true);
        saver.persist_final_documents(Some(2.0)).unwrap();
        assert_eq!(
            saver.metadata.as_ref().unwrap().status,
            "transcription_incomplete"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(folder.path().join("transcripts.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[tokio::test]
    async fn durable_stop_tail_readback_is_idempotent() {
        let folder = tempfile::tempdir().unwrap();
        let (sender, receiver) =
            super::super::work_queue::audio_work_queue(1, Some(folder.path().to_path_buf()))
                .unwrap();
        sender
            .send(AudioChunk {
                data: vec![0.1; 1600],
                sample_rate: 16000,
                timestamp: 120.0,
                chunk_id: 3,
                device_type: super::super::recording_state::DeviceType::Microphone,
            })
            .unwrap();
        let update = super::super::transcription::worker::TranscriptUpdate {
            text: "最后行动项".into(),
            timestamp: "02:00".into(),
            source: "Audio".into(),
            sequence_id: 3,
            chunk_start_time: 120.0,
            is_partial: false,
            confidence: 1.0,
            audio_start_time: 120.0,
            audio_end_time: 120.1,
            duration: 0.1,
            timing_precision: super::super::transcription::provider::TimingPrecision::Approximate,
            timed_segments: Vec::new(),
        };
        receiver
            .store()
            .unwrap()
            .complete(3, &serde_json::to_value(update).unwrap())
            .unwrap();
        let mut saver = RecordingSaver::new();
        saver.meeting_folder = Some(folder.path().to_path_buf());
        saver.restore_catchup_transcripts().unwrap();
        saver.restore_catchup_transcripts().unwrap();
        assert_eq!(saver.get_transcript_segments().len(), 1);
        assert_eq!(saver.get_transcript_segments()[0].text, "最后行动项");
    }
}
