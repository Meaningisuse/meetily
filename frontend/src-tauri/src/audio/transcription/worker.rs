// audio/transcription/worker.rs
//
// Parallel transcription worker pool and chunk processing logic.

use super::engine::TranscriptionEngine;
use super::provider::{TimedText, TimingPrecision, TranscriptionError};
use crate::audio::AudioChunk;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};

static ACTIVE_CANCELLATION: std::sync::LazyLock<std::sync::Mutex<Option<tokio_util::sync::CancellationToken>>> = std::sync::LazyLock::new(||std::sync::Mutex::new(None));
/// Stop scheduling more inputs, then let the current native inference finish.
/// The caller must join the worker before unloading a model.
pub fn cancel_pending_transcription() {
    TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);
    if let Some(token)=ACTIVE_CANCELLATION.lock().unwrap_or_else(|p|p.into_inner()).as_ref(){token.cancel();}
}


// Speech detection flag - reset per recording session
static SPEECH_DETECTED_EMITTED: AtomicBool = AtomicBool::new(false);
static TRANSCRIPTION_INCOMPLETE:AtomicBool=AtomicBool::new(false);
pub(crate) fn has_incomplete_transcription()->bool {TRANSCRIPTION_INCOMPLETE.load(Ordering::SeqCst)}

/// Reset the speech detected flag for a new recording session
pub fn reset_speech_detected_flag() {
    SPEECH_DETECTED_EMITTED.store(false, Ordering::SeqCst);
    info!("🔍 SPEECH_DETECTED_EMITTED reset to: {}", SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst));
}

/// Returns true if the transcript text is non-trivial and should be emitted.
/// Filters empty/whitespace-only text; no confidence gating is applied.
fn should_emit_transcript(text: &str) -> bool {
    !text.trim().is_empty()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranscriptUpdate {
    pub text: String,
    pub timestamp: String, // Wall-clock time for reference (e.g., "14:30:05")
    pub source: String,
    pub sequence_id: u64,
    pub chunk_start_time: f64, // Legacy field, kept for compatibility
    pub is_partial: bool,
    pub confidence: f32,
    // NEW: Recording-relative timestamps for playback sync
    pub audio_start_time: f64, // Seconds from recording start (e.g., 125.3)
    pub audio_end_time: f64,   // Seconds from recording start (e.g., 128.6)
    pub duration: f64,          // Segment duration in seconds (e.g., 3.3)
    #[serde(default)]
    pub timing_precision: TimingPrecision,
    #[serde(default)]
    pub timed_segments: Vec<TimedText>,
}

// NOTE: get_transcript_history and get_recording_meeting_name functions
// have been moved to recording_commands.rs where they have access to RECORDING_MANAGER

/// Bounded transcription worker with durable retries for saved audio.
pub fn start_transcription_task<R: Runtime>(
    app: AppHandle<R>,
    transcription_receiver: crate::audio::work_queue::AudioWorkReceiver,
) -> tokio::task::JoinHandle<()> {
    let cancellation=tokio_util::sync::CancellationToken::new();
    *ACTIVE_CANCELLATION.lock().unwrap_or_else(|p|p.into_inner())=Some(cancellation.clone());
    TRANSCRIPTION_INCOMPLETE.store(false,Ordering::SeqCst);
    tokio::spawn(async move {
        info!("🚀 Starting bounded transcription task with saved-audio catch-up");

        // Initialize transcription engine (Whisper or Parakeet based on config)
        let transcription_engine = match super::engine::get_or_init_transcription_engine(&app).await {
            Ok(engine) => engine,
            Err(e) => {
                TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);
                error!("Failed to initialize transcription engine: {}", e);
                let _ = app.emit("transcription-error", serde_json::json!({
                    "error": e,
                    "userMessage": "Recording failed: Unable to initialize speech recognition. Please check your model settings.",
                    "actionable": true,
                    "phase": "active"
                }));
                return;
            }
        };

        // Create parallel workers for faster processing while preserving ALL chunks
        const NUM_WORKERS: usize = 1; // Serial processing ensures transcripts emit in chronological order
        let catchup_store=transcription_receiver.store();
        let (work_sender, work_receiver) = tokio::sync::mpsc::channel::<AudioChunk>(1);
        let work_receiver = Arc::new(tokio::sync::Mutex::new(work_receiver));

        // Track completion: AtomicU64 for chunks queued, AtomicU64 for chunks completed
        let chunks_queued = Arc::new(AtomicU64::new(0));
        let chunks_completed = Arc::new(AtomicU64::new(0));
        let input_finished = Arc::new(AtomicBool::new(false));

        info!("📊 Starting {} transcription worker{} (serial mode for ordered emission)", NUM_WORKERS, if NUM_WORKERS == 1 { "" } else { "s" });

        // Spawn worker tasks
        let mut worker_handles = Vec::new();
        for worker_id in 0..NUM_WORKERS {
            let engine_clone = match &transcription_engine {
                TranscriptionEngine::Whisper(e) => TranscriptionEngine::Whisper(e.clone()),
                TranscriptionEngine::Parakeet(e) => TranscriptionEngine::Parakeet(e.clone()),
                TranscriptionEngine::Provider(p) => TranscriptionEngine::Provider(p.clone()),
            };
            let app_clone = app.clone();
            let work_receiver_clone = work_receiver.clone();
            let chunks_completed_clone = chunks_completed.clone();
            let input_finished_clone = input_finished.clone();
            let chunks_queued_clone = chunks_queued.clone();

            let catchup_store=catchup_store.clone();
            let worker_cancellation=cancellation.clone();
            let worker_handle = tokio::spawn(async move {
                info!("👷 Worker {} started", worker_id);

                // PRE-VALIDATE model state to avoid repeated async calls per chunk
                let initial_model_loaded = engine_clone.is_model_loaded().await;
                let current_model = engine_clone
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());

                let engine_name = engine_clone.provider_name();

                if initial_model_loaded {
                    info!(
                        "✅ Worker {} pre-validation: {} model '{}' is loaded and ready",
                        worker_id, engine_name, current_model
                    );
                } else {
                    warn!("⚠️ Worker {} pre-validation: {} model not loaded - chunks may be skipped", worker_id, engine_name);
                }

                loop {
                    if worker_cancellation.is_cancelled(){break;}
                    // Try to get a chunk to process
                    let chunk = {
                        let mut receiver = work_receiver_clone.lock().await;
                        receiver.recv().await
                    };

                    match chunk {
                        Some(chunk) => {
                            // PERFORMANCE OPTIMIZATION: Reduce logging in hot path
                            // Only log every 10th chunk per worker to reduce I/O overhead
                            let should_log_this_chunk = chunk.chunk_id % 10 == 0;

                            if should_log_this_chunk {
                                info!(
                                    "👷 Worker {} processing chunk {} with {} samples",
                                    worker_id,
                                    chunk.chunk_id,
                                    chunk.data.len()
                                );
                            }

                            // Check if model is still loaded before processing
                            if !engine_clone.is_model_loaded().await {
                                TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);
                                warn!("⚠️ Worker {}: Model unloaded, but continuing to preserve chunk {}", worker_id, chunk.chunk_id);
                                // Still count as completed even if we can't process
                                chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                continue;
                            }

                            let chunk_id=chunk.chunk_id;
                            let chunk_timestamp = chunk.timestamp;
                            let chunk_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;

                            // Transcribe with provider-agnostic approach
                            match transcribe_chunk_with_provider(
                                &engine_clone,
                                chunk,
                                &app_clone,
                            )
                            .await
                            {
                                Ok((transcript, confidence_opt, is_partial, timed_segments, provider_precision)) => {
                                    if is_partial {TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);}
                                    let confidence_str = match confidence_opt {
                                        Some(c) => format!("{:.2}", c),
                                        None => "N/A".to_string(),
                                    };

                                    info!("🔍 Worker {} transcription result: text='{}', confidence={}, partial={}",
                worker_id, transcript, confidence_str, is_partial);

                                    if should_emit_transcript(&transcript) {
                                        // PERFORMANCE: Only log transcription results, not every processing step
                                        info!("✅ Worker {} transcribed: {} (confidence: {}, partial: {})",
                                              worker_id, transcript, confidence_str, is_partial);

                                        // Emit speech-detected event for frontend UX (only on first detection per session)
                                        // This is lightweight and provides better user feedback
                                        let current_flag = SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst);
                                        info!("🔍 Checking speech-detected flag: current={}, will_emit={}", current_flag, !current_flag);

                                        if !current_flag {
                                            SPEECH_DETECTED_EMITTED.store(true, Ordering::SeqCst);
                                            match app_clone.emit("speech-detected", serde_json::json!({
                                                "message": "Speech activity detected"
                                            })) {
                                                Ok(_) => info!("🎤 ✅ First speech detected - successfully emitted speech-detected event"),
                                                Err(e) => error!("🎤 ❌ Failed to emit speech-detected event: {}", e),
                                            }
                                        } else {
                                            info!("🔍 Speech already detected in this session, not re-emitting");
                                        }

                                        // Generate sequence ID and calculate timestamps FIRST
                                        let sequence_id = chunk_id;
                                        let audio_start_time = chunk_timestamp; // Already in seconds from recording start
                                        let audio_end_time = chunk_timestamp + chunk_duration;

                                        // Save structured transcript segment to recording manager (only final results)
                                        // Save ALL segments (partial and final) to ensure complete JSON
                                        // Create structured segment with full timestamp data
                                        // NOTE: This is now handled via the transcript-update event emission below
                                        // The recording_commands module listens to these events and saves them
                                        // This decouples the transcription worker from direct RECORDING_MANAGER access

                                        // Emit transcript update with NEW recording-relative timestamps

                                        let update = TranscriptUpdate {
                                            text: transcript,
                                            timestamp: format_current_timestamp(), // Wall-clock for reference
                                            source: "Audio".to_string(),
                                            sequence_id,
                                            chunk_start_time: chunk_timestamp, // Legacy compatibility
                                            is_partial,
                                            confidence: confidence_opt.unwrap_or(0.85), // Default for providers without confidence
                                            // NEW: Recording-relative timestamps for sync
                                            audio_start_time,
                                            audio_end_time,
                                            duration: chunk_duration,
                                            timing_precision: if !timed_segments.is_empty()
                                                && provider_precision != TimingPrecision::Unavailable
                                            {
                                                provider_precision
                                            } else {
                                                TimingPrecision::from_audio_bounds(
                                                    Some(audio_start_time),
                                                    Some(audio_end_time),
                                                )
                                            },
                                            timed_segments: if !timed_segments.is_empty()
                                                && provider_precision != TimingPrecision::Unavailable
                                            {
                                                let offset_ms = (chunk_timestamp * 1000.0).round() as i64;
                                                timed_segments.into_iter().map(|mut timed| {
                                                    timed.start_ms = timed.start_ms.saturating_add(offset_ms);
                                                    timed.end_ms = timed.end_ms.saturating_add(offset_ms);
                                                    timed
                                                }).collect()
                                            } else {
                                                Vec::new()
                                            },
                                        };

                                        if let Some(store)=&catchup_store {
                                            if is_partial {let _=app_clone.emit("transcription-warning","Partial transcript retained for retry");} else if let Err(error)=store.complete(chunk_id,&serde_json::to_value(&update).unwrap()) {
                                                TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);
                                                let _=app_clone.emit("transcription-warning",format!("Transcript could not be checkpointed; recovery input retained: {error}"));
                                                chunks_completed_clone.fetch_add(1,Ordering::SeqCst);continue;
                                            }
                                        }
                                        if let Err(e) = app_clone.emit("transcript-update", &update)
                                        {
                                            error!(
                                                "Worker {}: Failed to emit transcript update: {}",
                                                worker_id, e
                                            );
                                        }
                                        // PERFORMANCE: Removed verbose logging of every emission
                                    } else if let Some(store)=&catchup_store {
                                        if is_partial{chunks_completed_clone.fetch_add(1,Ordering::SeqCst);continue;}
                                        let audio_end_time = chunk_timestamp + chunk_duration;
                                        let update = TranscriptUpdate {
                                            text: String::new(),
                                            timestamp: format_current_timestamp(),
                                            source: "Audio".into(),
                                            sequence_id: chunk_id,
                                            chunk_start_time: chunk_timestamp,
                                            is_partial: false,
                                            confidence: confidence_opt.unwrap_or(1.0),
                                            audio_start_time: chunk_timestamp,
                                            audio_end_time,
                                            duration: chunk_duration,
                                            timing_precision: TimingPrecision::from_audio_bounds(
                                                Some(chunk_timestamp),
                                                Some(audio_end_time),
                                            ),
                                            timed_segments: Vec::new(),
                                        };
                                        let _=store.complete(chunk_id,&serde_json::to_value(update).unwrap());
                                    }
                                }
                                Err(e) => {
                                    TRANSCRIPTION_INCOMPLETE.store(true,Ordering::SeqCst);
                                    // Improved error handling with specific cases
                                    match e {
                                        TranscriptionError::AudioTooShort { .. } => {
                                            // Skip silently, this is expected for very short chunks
                                            info!("Worker {}: {}", worker_id, e);
                                            chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                            continue;
                                        }
                                        TranscriptionError::ModelNotLoaded => {
                                            warn!("Worker {}: Model unloaded during transcription", worker_id);
                                            chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                            continue;
                                        }
                                        _ => {
                                            warn!("Worker {}: Transcription failed: {}", worker_id, e);
                                            let _ = app_clone.emit("transcription-warning", e.to_string());
                                        }
                                    }
                                }
                            }

                            // Mark chunk as completed
                            let completed =
                                chunks_completed_clone.fetch_add(1, Ordering::SeqCst) + 1;
                            let queued = chunks_queued_clone.load(Ordering::SeqCst);

                            // PERFORMANCE: Only log progress every 5th chunk to reduce I/O overhead
                            if completed % 5 == 0 || should_log_this_chunk {
                                info!(
                                    "Worker {}: Progress {}/{} chunks ({:.1}%)",
                                    worker_id,
                                    completed,
                                    queued,
                                    (completed as f64 / queued.max(1) as f64 * 100.0)
                                );
                            }

                            // Emit progress event for frontend
                            let progress_percentage = if queued > 0 {
                                (completed as f64 / queued as f64 * 100.0) as u32
                            } else {
                                100
                            };

                            let _ = app_clone.emit("transcription-progress", serde_json::json!({
                                "worker_id": worker_id,
                                "chunks_completed": completed,
                                "chunks_queued": queued,
                                "progress_percentage": progress_percentage,
                                "message": format!("Worker {} processing... ({}/{})", worker_id, completed, queued)
                            }));
                        }
                        None => {
                            // No more chunks available
                            if input_finished_clone.load(Ordering::SeqCst) {
                                // Double-check that all queued chunks are actually completed
                                let final_queued = chunks_queued_clone.load(Ordering::SeqCst);
                                let final_completed = chunks_completed_clone.load(Ordering::SeqCst);

                                if final_completed >= final_queued {
                                    info!(
                                        "👷 Worker {} finishing - all {}/{} chunks processed",
                                        worker_id, final_completed, final_queued
                                    );
                                    break;
                                } else {
                                    warn!("👷 Worker {} detected potential chunk loss: {}/{} completed, waiting...", worker_id, final_completed, final_queued);
                                    // AGGRESSIVE POLLING: Reduced from 50ms to 5ms for faster chunk detection during shutdown
                                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                                }
                            } else {
                                // AGGRESSIVE POLLING: Reduced from 10ms to 1ms for faster response during shutdown
                                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                            }
                        }
                    }
                }

                info!("👷 Worker {} completed", worker_id);
            });

            worker_handles.push(worker_handle);
        }

        // Main dispatcher: receive chunks and distribute to workers
        let mut receiver = transcription_receiver;
        loop {
            let received=tokio::select! {value=receiver.recv()=>value,_=cancellation.cancelled()=>break};
            let chunk=match received {Ok(Some(chunk))=>chunk,Ok(None)=>break,Err(error)=>{let _=app.emit("transcription-warning",format!("Catch-up data could not be read; originals retained: {error}"));break;}};
            let queued = chunks_queued.fetch_add(1, Ordering::SeqCst) + 1;
            info!(
                "📥 Dispatching chunk {} to workers (total queued: {})",
                chunk.chunk_id, queued
            );

            let sent=tokio::select! {value=work_sender.send(chunk)=>value,_=cancellation.cancelled()=>break};
            if sent.is_err() {
                error!("❌ Failed to send chunk to workers - this should not happen!");
                break;
            }
        }

        // Signal that input is finished
        input_finished.store(true, Ordering::SeqCst);
        drop(work_sender); // Close the channel to signal workers

        let total_chunks_queued = chunks_queued.load(Ordering::SeqCst);
        info!("📭 Input finished with {} total chunks queued. Waiting for all {} workers to complete...",
              total_chunks_queued, NUM_WORKERS);

        // Emit final chunk count to frontend
        let _ = app.emit("transcription-queue-complete", serde_json::json!({
            "total_chunks": total_chunks_queued,
            "message": format!("{} chunks queued for processing - waiting for completion", total_chunks_queued)
        }));

        // Wait for all workers to complete
        for (worker_id, handle) in worker_handles.into_iter().enumerate() {
            if let Err(e) = handle.await {
                error!("❌ Worker {} panicked: {:?}", worker_id, e);
            } else {
                info!("✅ Worker {} completed successfully", worker_id);
            }
        }

        // Final verification with retry logic to catch any stragglers
        let mut verification_attempts = 0;
        const MAX_VERIFICATION_ATTEMPTS: u32 = 10;

        loop {
            let final_queued = chunks_queued.load(Ordering::SeqCst);
            let final_completed = chunks_completed.load(Ordering::SeqCst);

            if cancellation.is_cancelled(){break;}
            if final_queued == final_completed {
                info!(
                    "All {} queued chunks were attempted; verify durable pending intervals separately",
                    final_completed
                );
                break;
            } else if verification_attempts < MAX_VERIFICATION_ATTEMPTS {
                verification_attempts += 1;
                warn!("⚠️ Chunk count mismatch (attempt {}): {} queued, {} completed - waiting for stragglers...",
                     verification_attempts, final_queued, final_completed);

                // Wait a bit for any remaining chunks to be processed
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            } else {
                error!(
                    "❌ CRITICAL: After {} attempts, chunk loss detected: {} queued, {} completed",
                    MAX_VERIFICATION_ATTEMPTS, final_queued, final_completed
                );

                // Emit critical error event
                let _ = app.emit(
                    "transcript-chunk-loss-detected",
                    serde_json::json!({
                        "chunks_queued": final_queued,
                        "chunks_completed": final_completed,
                        "chunks_lost": final_queued - final_completed,
                        "message": "Some transcript chunks may have been lost during shutdown"
                    }),
                );
                break;
            }
        }

        if let Some(store)=catchup_store {
            match store.pending() {Ok(pending) if !pending.is_empty()=>{let _=app.emit("transcription-warning",format!("{} audio intervals remain untranscribed; saved catch-up audio retained for retry.",pending.len()));},Err(error)=>{let _=app.emit("transcription-warning",format!("Could not verify catch-up completion: {error}"));},_=>{}}
        }
        info!("Transcription workers finished; attempted chunks are distinct from persisted successful results.");
    })
}

/// Transcribe audio chunk using the appropriate provider (Whisper, Parakeet, or trait-based)
/// Returns text/confidence/partial state plus verified provider timing, if present.
pub(crate) async fn transcribe_chunk_with_provider<R: Runtime>(
    engine: &TranscriptionEngine,
    chunk: AudioChunk,
    app: &AppHandle<R>,
) -> std::result::Result<(String, Option<f32>, bool, Vec<TimedText>, TimingPrecision), TranscriptionError> {
    // Convert to 16kHz mono for transcription
    let transcription_data = if chunk.sample_rate != 16000 {
        crate::audio::audio_processing::resample_audio(&chunk.data, chunk.sample_rate, 16000)
    } else {
        chunk.data
    };

    // Skip VAD processing here since the pipeline already extracted speech using VAD
    let speech_samples = transcription_data;

    // Check for empty samples - improved error handling
    if speech_samples.is_empty() {
        warn!(
            "Audio chunk {} is empty, skipping transcription",
            chunk.chunk_id
        );
        return Err(TranscriptionError::AudioTooShort {
            samples: 0,
            minimum: 1600, // 100ms at 16kHz
        });
    }

    // Calculate energy for logging/monitoring only
    let energy: f32 =
        speech_samples.iter().map(|&x| x * x).sum::<f32>() / speech_samples.len() as f32;
    info!(
        "Processing speech audio chunk {} with {} samples (energy: {:.6})",
        chunk.chunk_id,
        speech_samples.len(),
        energy
    );

    // Transcribe using the appropriate engine (with improved error handling)
    match engine {
        TranscriptionEngine::Whisper(whisper_engine) => {
            // Get language preference from global state
            let language = crate::get_language_preference_internal();

            match whisper_engine
                .transcribe_audio_with_verified_timing(speech_samples, language)
                .await
            {
                Ok((text, confidence, is_partial, timed_segments, timing_precision)) => {
                    let cleaned_text = text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), Some(confidence), is_partial, Vec::new(), TimingPrecision::Unavailable));
                    }

                    info!(
                        "Whisper transcription complete for chunk {}: '{}' (confidence: {:.2}, partial: {})",
                        chunk.chunk_id, cleaned_text, confidence, is_partial
                    );

                    Ok((cleaned_text, Some(confidence), is_partial, timed_segments, timing_precision))
                }
                Err(e) => {
                    error!(
                        "Whisper transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false,
                            "phase": "active"
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Parakeet(parakeet_engine) => {
            match parakeet_engine.transcribe_audio(speech_samples).await {
                Ok(text) => {
                    let cleaned_text = text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), None, false, Vec::new(), TimingPrecision::Unavailable));
                    }

                    info!(
                        "Parakeet transcription complete for chunk {}: '{}'",
                        chunk.chunk_id, cleaned_text
                    );

                    // Parakeet doesn't provide confidence or partial results
                    Ok((cleaned_text, None, false, Vec::new(), TimingPrecision::Unavailable))
                }
                Err(e) => {
                    error!(
                        "Parakeet transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false,
                            "phase": "active"
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Provider(provider) => {
            // NEW: Trait-based provider (clean, unified interface)
            let language = crate::get_language_preference_internal();

            match provider.transcribe(speech_samples, language).await {
                Ok(result) => {
                    let cleaned_text = result.text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), result.confidence, result.is_partial, Vec::new(), TimingPrecision::Unavailable));
                    }

                    let confidence_str = match result.confidence {
                        Some(c) => format!("confidence: {:.2}", c),
                        None => "no confidence".to_string(),
                    };

                    info!(
                        "{} transcription complete for chunk {}: '{}' ({}, partial: {})",
                        provider.provider_name(),
                        chunk.chunk_id,
                        cleaned_text,
                        confidence_str,
                        result.is_partial
                    );

                    Ok((cleaned_text, result.confidence, result.is_partial, result.timed_segments, result.timing_precision))
                }
                Err(e) => {
                    error!(
                        "{} transcription failed for chunk {}: {}",
                        provider.provider_name(),
                        chunk.chunk_id,
                        e
                    );

                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": e.to_string(),
                            "userMessage": format!("Transcription failed: {}", e),
                            "actionable": false,
                            "phase": "active"
                        }),
                    );

                    Err(e)
                }
            }
        }
    }
}

/// Format current timestamp (wall-clock time)
fn format_current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    let hours = (now.as_secs() / 3600) % 24;
    let minutes = (now.as_secs() / 60) % 60;
    let seconds = now.as_secs() % 60;

    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Format recording-relative time as [MM:SS]
#[allow(dead_code)]
fn format_recording_time(seconds: f64) -> String {
    let total_seconds = seconds.floor() as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;

    format!("[{:02}:{:02}]", minutes, secs)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn keeps_short_acknowledgements() {
            assert!(should_emit_transcript("Yes"));
            assert!(should_emit_transcript("ok"));
        }

        #[test]
        fn drops_empty_and_whitespace_only() {
            assert!(!should_emit_transcript(""));
            assert!(!should_emit_transcript("   "));
        }
    }
