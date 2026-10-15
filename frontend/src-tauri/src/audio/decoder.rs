// Audio file decoder for retranscription feature
// Uses Symphonia to decode MP4/AAC audio files, with ffmpeg fallback for
// formats Symphonia can't handle (MKV, WebM, WMA)

use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use std::borrow::Cow;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::audio_processing::resample;
use super::batch_limits::{reserve_samples, AudioBudgetExceeded, BatchAudioLimits};
use super::ffmpeg::find_ffmpeg_path;

/// Extensions requiring ffmpeg pre-conversion (Symphonia lacks these demuxers/codecs)
const FFMPEG_ONLY_EXTENSIONS: &[&str] = &["mkv", "webm", "wma"];

/// Progress callback for long-running operations
/// Returns current progress (0-100) and a message
pub type ProgressCallback = Box<dyn Fn(u32, &str) + Send>;

/// Decoded audio data from a file
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Raw audio samples (interleaved if stereo)
    pub samples: Vec<f32>,
    /// Sample rate of the decoded audio
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u16,
    /// Duration in seconds
    pub duration_seconds: f64,
}

impl DecodedAudio {
    /// Consume the source PCM. The mono/16k fast path reuses its allocation.
    pub fn into_whisper_format(self) -> Result<Vec<f32>> {
        self.into_whisper_format_controlled(None, BatchAudioLimits::default(), &|| false)
    }

    pub(crate) fn into_whisper_format_controlled(
        self,
        progress: Option<ProgressCallback>,
        limits: BatchAudioLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<f32>> {
        check_cancelled(cancelled)?;
        limits.check_source(self.samples.capacity(), self.sample_rate, self.channels)?;
        let mut mono = self.samples;
        if mono.len() % self.channels as usize != 0 {
            return Err(anyhow!("Incomplete interleaved audio frame"));
        }
        if self.channels > 1 {
            let channels = self.channels as usize;
            let frames = mono.len() / channels;
            for frame in 0..frames {
                if frame % 16_000 == 0 {
                    check_cancelled(cancelled)?;
                }
                mono[frame] = mono[frame * channels..(frame + 1) * channels]
                    .iter()
                    .sum::<f32>()
                    / channels as f32;
            }
            mono.truncate(frames);
        }
        normalize_in_place(&mut mono, cancelled)?;
        if self.sample_rate == 16_000 {
            return Ok(mono);
        }
        let maximum = limits.max_target_capacity(mono.capacity())?;
        chunked_resample_controlled(
            &mono,
            self.sample_rate,
            16_000,
            progress,
            maximum,
            cancelled,
        )
    }
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<()> {
    if cancelled() {
        return Err(anyhow!("Audio processing cancelled"));
    }
    Ok(())
}

fn normalize_in_place(samples: &mut [f32], cancelled: &dyn Fn() -> bool) -> Result<()> {
    let mut max_abs = 0.0f32;
    for chunk in samples.chunks(16_000) {
        check_cancelled(cancelled)?;
        for sample in chunk {
            if sample.is_finite() {
                max_abs = max_abs.max(sample.abs());
            }
        }
    }
    let scale = if max_abs > 1.0 { 1.0 / max_abs } else { 1.0 };
    for chunk in samples.chunks_mut(16_000) {
        check_cancelled(cancelled)?;
        for sample in chunk {
            *sample = if sample.is_finite() {
                (*sample * scale).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
    }
    Ok(())
}

/// One fixed-size sinc window at a time. Preserve the established 60s windows,
/// 100ms overlap and cross-fade, without retaining every window's result.
fn chunked_resample_controlled(
    input: &[f32],
    from_rate: u32,
    to_rate: u32,
    progress: Option<ProgressCallback>,
    maximum: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>> {
    check_cancelled(cancelled)?;
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if from_rate == 0 || to_rate == 0 {
        return Err(anyhow!("Invalid resampling rate"));
    }
    if from_rate == to_rate {
        let mut output = Vec::new();
        reserve_samples(&mut output, input.len(), maximum)?;
        output.extend_from_slice(input);
        return Ok(output);
    }
    let window = from_rate as usize * 60;
    let overlap = from_rate as usize / 10;
    let overlap_output = (overlap as u64 * to_rate as u64 / from_rate as u64) as usize;
    let chunks = input.len().div_ceil(window);
    let mut output = Vec::new();
    for (index, start) in (0..input.len()).step_by(window).enumerate() {
        check_cancelled(cancelled)?;
        let end = start
            .saturating_add(window)
            .saturating_add(overlap)
            .min(input.len());
        // Never fall back to a full-file allocation or the wrong sample rate.
        let mut current = resample(&input[start..end], from_rate, to_rate)?;
        check_cancelled(cancelled)?;
        for sample in &mut current {
            *sample = sample.clamp(-1.0, 1.0);
        }
        let fade = if index == 0 {
            0
        } else {
            overlap_output.min(current.len()).min(output.len())
        };
        reserve_samples(&mut output, current.len() - fade, maximum)?;
        let tail = output.len() - fade;
        for i in 0..fade {
            let t = i as f32 / fade as f32;
            output[tail + i] = output[tail + i] * (1.0 - t) + current[i] * t;
        }
        output.extend_from_slice(&current[fade..]);
        if let Some(callback) = &progress {
            callback(((index + 1) * 100 / chunks) as u32, "Resampling audio");
        }
    }
    check_cancelled(cancelled)?;
    Ok(output)
}

#[cfg(test)]
fn chunked_resample_with_progress(
    input: &[f32],
    from: u32,
    to: u32,
    progress: Option<ProgressCallback>,
) -> Vec<f32> {
    chunked_resample_controlled(
        input,
        from,
        to,
        progress,
        BatchAudioLimits::default().max_target_capacity(0).unwrap(),
        &|| false,
    )
    .unwrap()
}

/// Normalize audio samples to the valid range (-1.0 to 1.0)
/// This handles audio files that may have samples slightly outside the expected range
#[cfg(test)]
fn normalize_audio_samples(mut samples: Vec<f32>) -> Vec<f32> {
    normalize_in_place(&mut samples, &|| false).unwrap();
    samples
}

/// Check if a file extension requires ffmpeg pre-conversion
fn needs_ffmpeg_conversion(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| FFMPEG_ONLY_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Convert an audio file to WAV using ffmpeg for formats Symphonia can't decode.
///
/// Returns a `TempPath` that auto-deletes the temporary WAV file when dropped.
/// The caller must keep the `TempPath` alive until decoding of the WAV is complete.
fn convert_to_wav_with_ffmpeg(
    input_path: &Path,
    progress_callback: Option<&ProgressCallback>,
    limits: BatchAudioLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<tempfile::TempPath> {
    check_cancelled(cancelled)?;
    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow!(
            "FFmpeg not found. FFmpeg is required to decode .{} files. \
             It will be downloaded automatically on next launch, or install it manually.",
            input_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("this format")
        )
    })?;

    // Create temp file in the same directory as the input to avoid cross-device issues
    let parent_dir = input_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_file = tempfile::Builder::new()
        .prefix(".meetily_decode_")
        .suffix(".wav")
        .tempfile_in(parent_dir)
        .map_err(|e| anyhow!("Failed to create temporary WAV file: {}", e))?;

    let temp_path = temp_file.into_temp_path();

    info!(
        "Converting .{} to temporary WAV via ffmpeg: {} -> {}",
        input_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown"),
        input_path.display(),
        temp_path.display()
    );

    if let Some(cb) = progress_callback {
        cb(0, "Converting audio format with FFmpeg...");
    }

    let input_str = input_path
        .to_str()
        .ok_or_else(|| anyhow!("Invalid input path (non-UTF8)"))?;
    let output_str = temp_path
        .to_str()
        .ok_or_else(|| anyhow!("Invalid temp path (non-UTF8)"))?;

    let mut command = Command::new(&ffmpeg_path);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            input_str,
            "-vn", // Strip video tracks
            "-acodec",
            "pcm_s16le", // Output PCM WAV (Symphonia handles natively)
            "-fs",
            &limits.temp_bytes.to_string(),
            "-y", // Overwrite without prompt
            output_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Hide console window on Windows
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    debug!("FFmpeg conversion command: {:?}", command);

    let child = command
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn ffmpeg process: {e}"))?;
    let mut child = ConversionChild {
        child,
        reaped: false,
    };
    let status = wait_for_conversion(&mut child, &temp_path, limits.temp_bytes, cancelled)?;
    if !status.success() {
        return Err(anyhow!(
            "FFmpeg conversion failed. The file may be corrupted or unsupported."
        ));
    }

    // Verify output file exists and has content
    let output_meta = std::fs::metadata(&temp_path)
        .map_err(|e| anyhow!("FFmpeg output file not found: {}", e))?;

    if output_meta.len() >= limits.temp_bytes {
        return Err(anyhow!(
            "Audio conversion exceeds the temporary-file size limit; trim the recording first"
        ));
    }
    if output_meta.len() == 0 {
        return Err(anyhow!(
            "FFmpeg produced an empty output file. The input may contain no audio."
        ));
    }

    if let Some(cb) = progress_callback {
        cb(100, "FFmpeg conversion complete");
    }

    info!(
        "FFmpeg conversion complete: {} bytes output",
        output_meta.len()
    );

    Ok(temp_path)
}

/// Kill and reap on cancellation/error. Never collect unbounded process output.
struct ConversionChild {
    child: Child,
    reaped: bool,
}

impl ConversionChild {
    fn terminate(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

impl Drop for ConversionChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn wait_for_conversion(
    child: &mut ConversionChild,
    path: &Path,
    maximum: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExitStatus> {
    loop {
        if cancelled() {
            child.terminate();
            return Err(anyhow!("Audio processing cancelled"));
        }
        if std::fs::metadata(path).is_ok_and(|m| m.len() >= maximum) {
            child.terminate();
            return Err(anyhow!(
                "Audio conversion exceeds the temporary-file size limit; trim the recording first"
            ));
        }
        if let Some(status) = child.child.try_wait()? {
            child.reaped = true;
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Decode an audio file (MP4, M4A, WAV, etc.) to raw samples
pub fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    decode_audio_file_with_progress(path, None)
}

/// Decode with a default budget for callers without a cancellation source.
pub fn decode_audio_file_with_progress(
    path: &Path,
    progress: Option<ProgressCallback>,
) -> Result<DecodedAudio> {
    decode_audio_file_controlled(path, progress, BatchAudioLimits::default(), &|| false)
}

pub(crate) fn decode_audio_file_controlled(
    path: &Path,
    progress_callback: Option<ProgressCallback>,
    limits: BatchAudioLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<DecodedAudio> {
    check_cancelled(cancelled)?;
    info!("Decoding audio file: {}", path.display());

    // FFmpeg pre-conversion for unsupported formats (MKV, WebM, WMA).
    // If the file is in a format Symphonia can't decode, use ffmpeg to convert
    // it to a temporary WAV file first, then decode the WAV with Symphonia.
    // The _temp_wav_guard keeps the temp file alive until decoding completes,
    // then auto-deletes it when dropped (even on error/panic).
    let (_temp_wav_guard, decode_path): (Option<tempfile::TempPath>, Cow<'_, Path>) =
        if needs_ffmpeg_conversion(path) {
            info!(
                "Format requires ffmpeg pre-conversion: .{}",
                path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
            );
            let temp_path =
                convert_to_wav_with_ffmpeg(path, progress_callback.as_ref(), limits, cancelled)?;
            let wav_path = temp_path.to_path_buf();
            (Some(temp_path), Cow::Owned(wav_path))
        } else {
            (None, Cow::Borrowed(path))
        };

    // Open the file (use decode_path which may be the temp WAV)
    let file = std::fs::File::open(decode_path.as_ref()).map_err(|e| {
        anyhow!(
            "Failed to open audio file '{}': {}",
            decode_path.display(),
            e
        )
    })?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Set up format hint based on file extension
    let mut hint = Hint::new();
    if let Some(ext) = decode_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe the file format
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

    let mut format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("No audio track found in file"))?;

    let track_id = track.id;

    // Get audio parameters
    let mut sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("Unknown sample rate"))?;

    let mut channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(1);

    debug!(
        "Audio track: {}Hz, {} channels (from metadata)",
        sample_rate, channels
    );

    // Create the decoder
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| anyhow!("Failed to create decoder: {}", e))?;

    // Decode all packets
    let mut all_samples: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    // Calculate expected samples for progress tracking
    let expected_duration = track
        .codec_params
        .n_frames
        .map(|frames| frames as f64 / sample_rate as f64);
    let expected_samples =
        expected_duration.map(|dur| (dur * sample_rate as f64 * channels as f64) as usize);

    let mut last_progress = 0u32;
    let mut actual_spec = None;
    let mut maximum_samples = 0;

    loop {
        check_cancelled(cancelled)?;
        // Get the next packet
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // End of file
                break;
            }
            Err(e) => {
                warn!("Error reading packet: {}", e);
                break;
            }
        };

        // Skip packets from other tracks
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let actual_channels =
                    u16::try_from(spec.channels.count()).map_err(|_| AudioBudgetExceeded)?;
                limits.check_format(spec.rate, actual_channels)?;
                limits.check_packet(decoded.capacity(), actual_channels)?;
                if let Some(previous) = actual_spec {
                    if previous != spec {
                        return Err(anyhow!("Audio format changed during decoding"));
                    }
                } else {
                    sample_rate = spec.rate;
                    channels = actual_channels;
                    maximum_samples = limits.max_source_samples(sample_rate, channels)?;
                    actual_spec = Some(spec);
                }
                let packet_samples = decoded
                    .frames()
                    .checked_mul(channels as usize)
                    .ok_or(AudioBudgetExceeded)?;
                reserve_samples(&mut all_samples, packet_samples, maximum_samples)?;
                if sample_buf
                    .as_ref()
                    .is_none_or(|buffer| buffer.capacity() < decoded.capacity() * channels as usize)
                {
                    sample_buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                }
                check_cancelled(cancelled)?;
                let buffer = sample_buf.as_mut().unwrap();
                buffer.copy_interleaved_ref(decoded);
                all_samples.extend_from_slice(buffer.samples());
                check_cancelled(cancelled)?;

                // Emit progress updates (every 10%)
                if let (Some(callback), Some(expected)) = (&progress_callback, expected_samples) {
                    let current_progress =
                        ((all_samples.len() as f64 / expected as f64) * 100.0) as u32;
                    if current_progress >= last_progress + 10 && current_progress <= 100 {
                        last_progress = current_progress;
                        callback(
                            current_progress,
                            &format!("Decoding audio: {}%", current_progress),
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Error decoding packet: {}", e);
                continue;
            }
        }
    }

    check_cancelled(cancelled)?;
    // Ensure we report 100% completion
    if let Some(callback) = &progress_callback {
        callback(100, "Decoding complete");
    }

    if all_samples.is_empty() {
        return Err(anyhow!("No audio samples decoded from file"));
    }

    let total_frames = all_samples.len() / channels as usize;
    let duration_seconds = total_frames as f64 / sample_rate as f64;

    info!(
        "Decoded {} samples ({:.2}s) at {}Hz, {} channels",
        all_samples.len(),
        duration_seconds,
        sample_rate,
        channels
    );

    Ok(DecodedAudio {
        samples: all_samples,
        sample_rate,
        channels,
        duration_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::super::audio_processing::resample_audio;
    use super::*;

    fn small_pcm_fixture() -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let size = 8_000u32;
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(size + 36).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00")
            .unwrap();
        file.write_all(&16_000u32.to_le_bytes()).unwrap();
        file.write_all(&32_000u32.to_le_bytes()).unwrap();
        file.write_all(b"\x02\x00\x10\x00data").unwrap();
        file.write_all(&size.to_le_bytes()).unwrap();
        file.write_all(&vec![0; size as usize]).unwrap();
        file
    }

    #[test]
    fn batch_decode_enforces_actual_pcm_budget() {
        let file = small_pcm_fixture();
        let limits = super::super::batch_limits::BatchAudioLimits {
            working_bytes: 4_096,
            scratch_bytes: 256,
            ..Default::default()
        };
        let error = decode_audio_file_controlled(file.path(), None, limits, &|| false).unwrap_err();
        assert!(error.to_string().contains("memory budget"), "{error}");
    }

    #[test]
    fn batch_decode_honors_cancellation_before_processing() {
        let file = small_pcm_fixture();
        let error = decode_audio_file_controlled(file.path(), None, Default::default(), &|| true)
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    #[test]
    fn batch_decode_cancels_between_real_packets() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let file = small_pcm_fixture();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let progress = Box::new(move |_: u32, _: &str| {
            signal.store(true, Ordering::SeqCst);
        });
        let error =
            decode_audio_file_controlled(file.path(), Some(progress), Default::default(), &|| {
                cancelled.load(Ordering::SeqCst)
            })
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn batch_mono_conversion_moves_its_allocation() {
        let samples = vec![0.5f32; 1000];
        let pointer = samples.as_ptr();
        let audio = DecodedAudio {
            samples,
            sample_rate: 16_000,
            channels: 1,
            duration_seconds: 0.0625,
        };
        let converted = audio.into_whisper_format().unwrap();
        assert_eq!(converted.as_ptr(), pointer);
        assert_eq!(converted, vec![0.5; 1000]);
    }

    #[test]
    fn batch_resample_cancels_after_first_window_without_fallback() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let progress = Box::new(move |_: u32, _: &str| {
            signal.store(true, Ordering::SeqCst);
        });
        let input = vec![0.1f32; 48_000 * 61];
        let result = chunked_resample_controlled(
            &input,
            48_000,
            16_000,
            Some(progress),
            16_000 * 63,
            &|| cancelled.load(Ordering::SeqCst),
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn batch_sinc_resampling_preserves_speech_band_and_rejects_aliases() {
        let rms = |frequency: f32| {
            let samples = (0..48_000)
                .map(|i| (2.0 * std::f32::consts::PI * frequency * i as f32 / 48_000.0).sin() * 0.5)
                .collect();
            let output = DecodedAudio {
                samples,
                sample_rate: 48_000,
                channels: 1,
                duration_seconds: 1.0,
            }
            .into_whisper_format()
            .unwrap();
            let core = &output[1000..output.len() - 1000];
            (core.iter().map(|s| s * s).sum::<f32>() / core.len() as f32).sqrt()
        };
        assert!(rms(1_000.0) > 0.25);
        assert!(rms(14_000.0) < 0.05);
    }

    #[test]
    fn batch_ffmpeg_size_limit_rejects_truncation_and_cleans_temp_file() {
        let input = small_pcm_fixture();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("source.wav");
        std::fs::copy(input.path(), &path).unwrap();
        let limits = BatchAudioLimits {
            temp_bytes: 4096,
            ..Default::default()
        };
        let error = convert_to_wav_with_ffmpeg(&path, None, limits, &|| false).unwrap_err();
        assert!(
            error.to_string().contains("temporary-file size limit"),
            "{error}"
        );
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn batch_cancelled_conversion_reaps_child() {
        let file = small_pcm_fixture();
        let child = Command::new("sh")
            .args(["-c", "exec sleep 30"])
            .spawn()
            .unwrap();
        let mut child = ConversionChild {
            child,
            reaped: false,
        };
        let error = wait_for_conversion(&mut child, file.path(), u64::MAX, &|| true).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(child.reaped);
        assert!(child.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn test_decode_he_aac_uses_decoded_rate_not_container_rate() {
        // HE-AAC (SBR) fixture: the container declares 48 kHz stereo, but
        // Symphonia has no SBR support and decodes only the AAC-LC core at
        // 24 kHz. Trusting the container rate halves the duration and makes
        // the audio play at 2x speed, which truncated long imported
        // recordings to half their length.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/he_aac_48k_5s.m4a");
        let decoded = decode_audio_file(&path).expect("failed to decode HE-AAC fixture");

        assert_eq!(
            decoded.sample_rate, 24000,
            "sample_rate must come from the decoder output, not container metadata"
        );
        assert!(
            (decoded.duration_seconds - 5.16).abs() < 0.5,
            "duration must be ~5s (was reported as ~2.6s before the fix), got {:.2}s",
            decoded.duration_seconds
        );

        // The 16 kHz conversion must preserve the real duration as well.
        let whisper_samples = decoded.into_whisper_format().unwrap();
        let duration_16k = whisper_samples.len() as f64 / 16000.0;
        assert!(
            (duration_16k - 5.16).abs() < 0.5,
            "whisper-format duration must be ~5s, got {:.2}s",
            duration_16k
        );
    }

    #[test]
    fn test_to_whisper_format_mono_16k() {
        // Already in correct format
        let audio = DecodedAudio {
            samples: vec![0.1, 0.2, 0.3],
            sample_rate: 16000,
            channels: 1,
            duration_seconds: 0.0001875,
        };

        let result = audio.into_whisper_format().unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_to_whisper_format_stereo_to_mono() {
        // Stereo input
        let audio = DecodedAudio {
            samples: vec![0.2, 0.4, 0.6, 0.8], // 2 stereo frames
            sample_rate: 16000,
            channels: 2,
            duration_seconds: 0.000125,
        };

        let result = audio.into_whisper_format().unwrap();
        assert_eq!(result.len(), 2); // Should be mono now
                                     // Average of (0.2, 0.4) = 0.3 and (0.6, 0.8) = 0.7
        assert!((result[0] - 0.3).abs() < 0.001);
        assert!((result[1] - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_to_whisper_format_resamples_48k_to_16k() {
        // 48kHz mono input - should be downsampled to 16kHz
        // Use a larger sample to ensure resampler works correctly
        // 48000 samples at 48kHz = 1 second → 16000 samples at 16kHz
        let audio = DecodedAudio {
            samples: vec![0.5; 4800], // 0.1 seconds at 48kHz
            sample_rate: 48000,
            channels: 1,
            duration_seconds: 4800.0 / 48000.0,
        };

        let result = audio.into_whisper_format().unwrap();
        // Output length should be approximately input_len / 3 (16000/48000 ratio)
        // 4800 / 3 = 1600
        assert!(!result.is_empty(), "Result should not be empty");
        assert!(
            result.len() > 1000 && result.len() < 2000,
            "Expected ~1600 samples, got {}",
            result.len()
        );
    }

    #[test]
    fn test_chunked_resample_same_rate() {
        let input = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let result = chunked_resample_with_progress(&input, 16000, 16000, None);
        assert_eq!(result.len(), input.len());
        for (i, &sample) in result.iter().enumerate() {
            assert!((sample - input[i]).abs() < 0.001);
        }
    }

    #[test]
    fn test_chunked_resample_empty_input() {
        let input: Vec<f32> = vec![];
        let result = chunked_resample_with_progress(&input, 48000, 16000, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_chunked_resample_downsamples_correctly() {
        // 48kHz to 16kHz = 3x downsampling with a 2-second signal
        let input: Vec<f32> = (0..96000).map(|i| (i as f32 / 96000.0)).collect();
        let result = chunked_resample_with_progress(&input, 48000, 16000, None);

        // Output should be approximately 1/3 the length
        let expected_len = 96000.0 * (16000.0 / 48000.0);
        assert!(
            (result.len() as f64 - expected_len).abs() < 200.0,
            "Expected ~{} samples, got {}",
            expected_len,
            result.len()
        );
    }

    #[test]
    fn test_chunked_resample_preserves_signal_range() {
        // 1 second of sine wave at 44100Hz
        let input: Vec<f32> = (0..44100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let result = chunked_resample_with_progress(&input, 44100, 16000, None);

        for sample in &result {
            assert!(
                *sample >= -1.1 && *sample <= 1.1,
                "Sample {} out of expected range",
                sample
            );
        }
    }

    #[test]
    fn test_chunked_resample_matches_single_pass() {
        // Verify chunked output is close to single-pass for small files
        let input: Vec<f32> = (0..48000)
            .map(|i| (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 48000.0).sin() * 0.5)
            .collect();

        let single_pass = resample_audio(&input, 48000, 16000);
        let chunked = chunked_resample_with_progress(&input, 48000, 16000, None);

        // Lengths should be very close
        let len_diff = (single_pass.len() as i64 - chunked.len() as i64).unsigned_abs();
        assert!(
            len_diff < 50,
            "Length mismatch: single_pass={}, chunked={}",
            single_pass.len(),
            chunked.len()
        );

        // Compare overlapping samples (allow some tolerance at chunk boundaries)
        let compare_len = single_pass.len().min(chunked.len());
        let mut max_diff = 0.0f32;
        for i in 0..compare_len {
            let diff = (single_pass[i] - chunked[i]).abs();
            max_diff = max_diff.max(diff);
        }
        // Chunk boundaries may introduce small discontinuities
        assert!(
            max_diff < 0.15,
            "Max sample difference too large: {}",
            max_diff
        );
    }

    #[test]
    fn test_decoded_audio_duration_calculation() {
        let audio = DecodedAudio {
            samples: vec![0.0; 48000], // 1 second at 48kHz mono
            sample_rate: 48000,
            channels: 1,
            duration_seconds: 1.0,
        };

        // Duration should be samples / sample_rate for mono
        let calculated_duration = audio.samples.len() as f64 / audio.sample_rate as f64;
        assert!((calculated_duration - audio.duration_seconds).abs() < 0.001);
    }

    #[test]
    fn test_decoded_audio_stereo_duration() {
        let audio = DecodedAudio {
            samples: vec![0.0; 96000], // 1 second at 48kHz stereo (2 channels)
            sample_rate: 48000,
            channels: 2,
            duration_seconds: 1.0,
        };

        // Duration should be samples / (sample_rate * channels) for stereo
        let frames = audio.samples.len() / audio.channels as usize;
        let calculated_duration = frames as f64 / audio.sample_rate as f64;
        assert!((calculated_duration - audio.duration_seconds).abs() < 0.001);
    }

    #[test]
    fn test_to_whisper_format_handles_large_file_threshold() {
        // Test that large files use chunked sinc resampling path
        // LARGE_FILE_THRESHOLD is 14_400_000 samples
        // We'll test with a smaller sample to verify the path selection logic works
        let audio = DecodedAudio {
            samples: vec![0.5; 1000], // Small file
            sample_rate: 48000,
            channels: 1,
            duration_seconds: 1000.0 / 48000.0,
        };

        let result = audio.into_whisper_format().unwrap();
        // Should complete without error and produce valid output
        assert!(!result.is_empty());
        assert!(result.len() < 1000); // Downsampled
    }

    #[test]
    fn test_normalize_audio_samples_already_normalized() {
        let samples = vec![0.5, -0.5, 0.0, 0.9, -0.9];
        let result = normalize_audio_samples(samples.clone());
        // Should be unchanged (already in range)
        for (i, &s) in result.iter().enumerate() {
            assert!((s - samples[i]).abs() < 0.001);
        }
    }

    #[test]
    fn test_normalize_audio_samples_exceeds_range() {
        let samples = vec![0.5, -0.5, 2.0, -1.5]; // max_abs = 2.0
        let result = normalize_audio_samples(samples);
        // All samples should be scaled by 0.5 (1.0 / 2.0)
        assert!((result[0] - 0.25).abs() < 0.001);
        assert!((result[1] - -0.25).abs() < 0.001);
        assert!((result[2] - 1.0).abs() < 0.001);
        assert!((result[3] - -0.75).abs() < 0.001);
    }

    #[test]
    fn test_normalize_audio_samples_handles_nan() {
        let samples = vec![0.5, f32::NAN, 0.3];
        let result = normalize_audio_samples(samples);
        assert!((result[0] - 0.5).abs() < 0.001);
        assert_eq!(result[1], 0.0); // NaN replaced with 0
        assert!((result[2] - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_normalize_audio_samples_handles_infinity() {
        let samples = vec![0.5, f32::INFINITY, -0.3];
        let result = normalize_audio_samples(samples);
        assert!((result[0] - 0.5).abs() < 0.001); // preserved
        assert_eq!(result[1], 0.0); // infinity → 0
        assert!((result[2] - (-0.3)).abs() < 0.001); // preserved
    }

    #[test]
    fn test_needs_ffmpeg_conversion() {
        assert!(needs_ffmpeg_conversion(Path::new("video.mkv")));
        assert!(needs_ffmpeg_conversion(Path::new("audio.webm")));
        assert!(needs_ffmpeg_conversion(Path::new("audio.wma")));
        // Case insensitive
        assert!(needs_ffmpeg_conversion(Path::new("meeting.MKV")));
        assert!(needs_ffmpeg_conversion(Path::new("audio.WMA")));
        assert!(needs_ffmpeg_conversion(Path::new("audio.WebM")));
        // Symphonia-native formats should NOT need ffmpeg
        assert!(!needs_ffmpeg_conversion(Path::new("audio.mp4")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.wav")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.mp3")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.flac")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.ogg")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.aac")));
        assert!(!needs_ffmpeg_conversion(Path::new("audio.m4a")));
        // No extension
        assert!(!needs_ffmpeg_conversion(Path::new("noext")));
    }
}
