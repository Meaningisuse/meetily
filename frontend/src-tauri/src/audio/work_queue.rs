//! Bounded live PCM plus a lossless, durable catch-up store. Called by the
//! processing pipeline, never by the real-time capture callback.
use super::recording_state::{AudioChunk, DeviceType};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::mpsc;
const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const MAX_SPOOL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20000;
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u8,
    chunk_id: u64,
    sample_rate: u32,
    timestamp: f64,
    samples: usize,
    device: String,
    sha256: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct PendingRange {
    pub chunk_id: u64,
    pub start_ms: i64,
    pub end_ms: i64,
}
pub struct CatchupStore {
    root: PathBuf,
    bytes: AtomicU64,
    entries: AtomicU64,
    writer: Mutex<()>,
}
#[derive(Clone)]
pub struct AudioWorkSender {
    sender: mpsc::Sender<AudioChunk>,
    store: Option<Arc<CatchupStore>>,
    disk_only: Arc<AtomicBool>,
}
pub struct AudioWorkReceiver {
    receiver: mpsc::Receiver<AudioChunk>,
    store: Option<Arc<CatchupStore>>,
    seen: HashSet<u64>,
    replay: Option<VecDeque<PendingRange>>,
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
impl CatchupStore {
    pub(crate) fn open(meeting_folder: PathBuf) -> Result<Arc<Self>> {
        let root = meeting_folder.join(".catchup");
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let mut bytes = 0u64;
        let mut entries = 0usize;
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "pcm") {
                bytes = bytes
                    .checked_add(entry.metadata()?.len())
                    .ok_or_else(|| anyhow!("Catch-up size overflow"))?;
                entries += 1;
            }
        }
        if bytes > MAX_SPOOL_BYTES || entries > MAX_ENTRIES {
            return Err(anyhow!("Catch-up storage limit exceeded"));
        }
        Ok(Arc::new(Self {
            root,
            bytes: AtomicU64::new(bytes),
            entries: AtomicU64::new(entries as u64),
            writer: Mutex::new(()),
        }))
    }
    fn pcm_path(&self, id: u64) -> PathBuf {
        self.root.join(format!("{id:020}.pcm"))
    }
    fn result_path(&self, id: u64) -> PathBuf {
        self.root.join(format!("{id:020}.result.json"))
    }
    fn read_header(path: &Path) -> Result<(Header, File)> {
        let mut file = File::open(path)?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != b"MEETILY1" {
            return Err(anyhow!("Invalid catch-up audio signature"));
        }
        let mut length = [0u8; 4];
        file.read_exact(&mut length)?;
        let length = u32::from_le_bytes(length) as usize;
        if length > 1024 {
            return Err(anyhow!("Catch-up header exceeds limit"));
        }
        let mut data = vec![0u8; length];
        file.read_exact(&mut data)?;
        let header: Header = serde_json::from_slice(&data)?;
        if header.version != 1
            || header.sample_rate == 0
            || !header.timestamp.is_finite()
            || header.timestamp < 0.0
            || header.samples > MAX_CHUNK_BYTES / 4
        {
            return Err(anyhow!("Invalid catch-up audio metadata"));
        }
        if file.metadata()?.len() != 12 + length as u64 + (header.samples as u64) * 4 {
            return Err(anyhow!("Incomplete catch-up audio"));
        }
        Ok((header, file))
    }
    fn persist(&self, chunk: &AudioChunk) -> Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Catch-up writer unavailable"))?;
        if chunk.sample_rate == 0
            || !chunk.timestamp.is_finite()
            || chunk.timestamp < 0.0
            || chunk.data.len() > MAX_CHUNK_BYTES / 4
        {
            return Err(anyhow!("Audio chunk exceeds catch-up bounds"));
        }
        let mut hash = Sha256::new();
        for sample in &chunk.data {
            hash.update(sample.to_le_bytes());
        }
        let digest = format!("{:x}", hash.finalize());
        let destination = self.pcm_path(chunk.chunk_id);
        if destination.exists() {
            let (old, _) = Self::read_header(&destination)?;
            if old.sha256 == digest
                && old.timestamp == chunk.timestamp
                && old.sample_rate == chunk.sample_rate
            {
                return Ok(());
            }
            return Err(anyhow!("Conflicting catch-up chunk identifier"));
        }
        let header = Header {
            version: 1,
            chunk_id: chunk.chunk_id,
            sample_rate: chunk.sample_rate,
            timestamp: chunk.timestamp,
            samples: chunk.data.len(),
            device: if chunk.device_type == DeviceType::System {
                "system"
            } else {
                "microphone"
            }
            .into(),
            sha256: digest,
        };
        let json = serde_json::to_vec(&header)?;
        let required = 12 + json.len() as u64 + (chunk.data.len() as u64) * 4;
        if self.entries.load(Ordering::SeqCst) >= MAX_ENTRIES as u64
            || self.bytes.load(Ordering::SeqCst).saturating_add(required) > MAX_SPOOL_BYTES
        {
            return Err(anyhow!(
                "Catch-up disk budget of 1 GiB exhausted; stop recording"
            ));
        }
        let partial = self
            .root
            .join(format!(".{}.incomplete", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_file(&partial)?;
            file.write_all(b"MEETILY1")?;
            file.write_all(&(json.len() as u32).to_le_bytes())?;
            file.write_all(&json)?;
            let mut buffer = Vec::with_capacity(64 * 1024);
            for samples in chunk.data.chunks(16384) {
                buffer.clear();
                for sample in samples {
                    buffer.extend_from_slice(&sample.to_le_bytes());
                }
                file.write_all(&buffer)?;
            }
            file.sync_all()?;
            fs::hard_link(&partial, &destination)?;
            fs::remove_file(&partial)?;
            sync_directory(&self.root)?;
            self.bytes.fetch_add(required, Ordering::SeqCst);
            self.entries.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result
    }
    pub fn pending(&self) -> Result<Vec<PendingRange>> {
        let mut pending = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if !path.extension().is_some_and(|e| e == "pcm") {
                continue;
            }
            let (header, _) = Self::read_header(&path)?;
            if self.result_path(header.chunk_id).exists() {
                let path = self.result_path(header.chunk_id);
                if fs::metadata(&path)?.len() > 512 * 1024 {
                    return Err(anyhow!("Catch-up result exceeds limit"));
                }
                let result: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                if !result.get("text").is_some_and(|v| v.is_string()) {
                    return Err(anyhow!("Invalid catch-up result"));
                }
                continue;
            }
            let start = (header.timestamp * 1000.0).round() as i64;
            let duration = (header.samples as u64 * 1000 / header.sample_rate as u64) as i64;
            pending.push(PendingRange {
                chunk_id: header.chunk_id,
                start_ms: start,
                end_ms: start.saturating_add(duration),
            });
            if pending.len() > MAX_ENTRIES {
                return Err(anyhow!("Too many catch-up entries"));
            }
        }
        pending.sort_by_key(|r| r.chunk_id);
        Ok(pending)
    }
    fn read_chunk(&self, id: u64) -> Result<AudioChunk> {
        let (header, mut file) = Self::read_header(&self.pcm_path(id))?;
        let mut bytes = vec![0u8; header.samples * 4];
        file.read_exact(&mut bytes)?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != header.sha256 {
            return Err(anyhow!("Catch-up audio checksum mismatch"));
        }
        let data = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        Ok(AudioChunk {
            data,
            sample_rate: header.sample_rate,
            timestamp: header.timestamp,
            chunk_id: header.chunk_id,
            device_type: if header.device == "system" {
                DeviceType::System
            } else {
                DeviceType::Microphone
            },
        })
    }
    /// The transcript is durable before a chunk stops appearing as pending.
    /// Retain both input and result until the meeting has been accepted/saved.
    pub fn complete(&self, id: u64, result: &serde_json::Value) -> Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Catch-up writer unavailable"))?;
        Self::read_header(&self.pcm_path(id))?;
        let destination = self.result_path(id);
        if destination.exists() {
            return Ok(());
        }
        let data = serde_json::to_vec(result)?;
        if data.len() > 512 * 1024 {
            return Err(anyhow!("Catch-up transcript exceeds limit"));
        }
        let partial = self
            .root
            .join(format!(".{}.result.incomplete", uuid::Uuid::new_v4()));
        let write = (|| -> Result<()> {
            let mut file = private_file(&partial)?;
            file.write_all(&data)?;
            file.sync_all()?;
            fs::hard_link(&partial, &destination)?;
            fs::remove_file(&partial)?;
            sync_directory(&self.root)?;
            Ok(())
        })();
        if write.is_err() {
            let _ = fs::remove_file(&partial);
        }
        write
    }
    pub fn completed_results(&self) -> Result<Vec<serde_json::Value>> {
        let mut paths = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".result.json"))
            {
                total = total
                    .checked_add(fs::metadata(&path)?.len())
                    .ok_or_else(|| anyhow!("Result size overflow"))?;
                if total > 16 * 1024 * 1024 {
                    return Err(anyhow!("Catch-up transcript readback exceeds 16 MiB"));
                }
                paths.push(path);
            }
            if paths.len() > MAX_ENTRIES {
                return Err(anyhow!("Too many catch-up results"));
            }
        }
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                if fs::metadata(&path)?.len() > 512 * 1024 {
                    return Err(anyhow!("Catch-up result exceeds limit"));
                }
                Ok(serde_json::from_slice(&fs::read(path)?)?)
            })
            .collect()
    }
}
pub fn audio_work_queue(
    capacity: usize,
    meeting_folder: Option<PathBuf>,
) -> Result<(AudioWorkSender, AudioWorkReceiver)> {
    if !(1..=16).contains(&capacity) {
        return Err(anyhow!("Audio work capacity must be 1–16"));
    }
    let (sender, receiver) = mpsc::channel(capacity);
    let store = meeting_folder.map(CatchupStore::open).transpose()?;
    Ok((
        AudioWorkSender {
            sender,
            store: store.clone(),
            disk_only: Arc::new(AtomicBool::new(false)),
        },
        AudioWorkReceiver {
            receiver,
            store,
            seen: HashSet::new(),
            replay: None,
        },
    ))
}
impl AudioWorkSender {
    pub fn has_recovery_audio(&self) -> bool {
        self.store.is_some()
    }
    pub fn send(&self, chunk: AudioChunk) -> Result<()> {
        if let Some(store) = &self.store {
            store.persist(&chunk)?;
        }
        if self.disk_only.load(Ordering::SeqCst) {
            return Ok(());
        }
        match self.sender.try_send(chunk) {
            Ok(())=>Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) if self.store.is_some()=>{self.disk_only.store(true,Ordering::SeqCst);Ok(())},
            Err(mpsc::error::TrySendError::Full(_))=>Err(anyhow!("Transcription queue is full and audio saving is disabled; stop recording. No recovery audio exists.")),
            Err(mpsc::error::TrySendError::Closed(_))=>Err(anyhow!("Transcription worker has stopped")),
        }
    }
}
impl AudioWorkReceiver {
    pub fn store(&self) -> Option<Arc<CatchupStore>> {
        self.store.clone()
    }
    pub fn recover(meeting_folder: PathBuf) -> Result<Self> {
        let (sender, receiver) = mpsc::channel(1);
        drop(sender);
        Ok(Self {
            receiver,
            store: Some(CatchupStore::open(meeting_folder)?),
            seen: HashSet::new(),
            replay: None,
        })
    }
    pub async fn recv(&mut self) -> Result<Option<AudioChunk>> {
        while let Some(chunk) = self.receiver.recv().await {
            if self.seen.len() >= MAX_ENTRIES {
                return Err(anyhow!("Audio session exceeds 20000 catch-up intervals"));
            }
            if self.seen.insert(chunk.chunk_id) {
                return Ok(Some(chunk));
            }
        }
        let Some(store) = &self.store else {
            return Ok(None);
        };
        if self.replay.is_none() {
            self.replay = Some(store.pending()?.into());
        }
        while let Some(range) = self.replay.as_mut().unwrap().pop_front() {
            if self.seen.insert(range.chunk_id) {
                return Ok(Some(store.read_chunk(range.chunk_id)?));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::recording_state::{AudioChunk, DeviceType};
    fn chunk(id: u64) -> AudioChunk {
        AudioChunk {
            chunk_id: id,
            data: vec![0.125; 1600],
            sample_rate: 16000,
            timestamp: id as f64,
            device_type: DeviceType::Microphone,
        }
    }
    #[tokio::test]
    async fn overflow_marks_saved_audio_for_catchup() {
        let directory = tempfile::tempdir().unwrap();
        let (sender, mut receiver) =
            audio_work_queue(1, Some(directory.path().to_path_buf())).unwrap();
        sender.send(chunk(10)).unwrap();
        sender.send(chunk(11)).unwrap();
        sender.send(chunk(12)).unwrap();
        sender.send(chunk(12)).unwrap();
        let store = receiver.store().unwrap();
        assert_eq!(store.pending().unwrap().len(), 3);
        let ranges = store.pending().unwrap();
        assert_eq!(ranges[0].start_ms, 10000);
        assert_eq!(ranges[0].end_ms, 10100);
        drop(sender);
        let mut seen = Vec::new();
        while let Some(chunk) = receiver.recv().await.unwrap() {
            assert_eq!(chunk.data, vec![0.125; 1600]);
            seen.push(chunk.chunk_id);
            store
                .complete(
                    chunk.chunk_id,
                    &serde_json::json!({"text":format!("chunk{}",chunk.chunk_id)}),
                )
                .unwrap();
        }
        assert_eq!(seen, vec![10, 11, 12]);
        assert!(store.pending().unwrap().is_empty());
        let mut restarted = AudioWorkReceiver::recover(directory.path().to_path_buf()).unwrap();
        assert!(restarted.recv().await.unwrap().is_none());
        assert_eq!(store.completed_results().unwrap().len(), 3);
    }
    #[tokio::test]
    async fn restart_replays_only_unfinished_and_detects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let (sender, receiver) = audio_work_queue(1, Some(directory.path().to_path_buf())).unwrap();
        sender.send(chunk(1)).unwrap();
        sender.send(chunk(2)).unwrap();
        let store = receiver.store().unwrap();
        store
            .complete(1, &serde_json::json!({"text":"first"}))
            .unwrap();
        drop(sender);
        drop(receiver);
        let mut restarted = AudioWorkReceiver::recover(directory.path().to_path_buf()).unwrap();
        assert_eq!(restarted.recv().await.unwrap().unwrap().chunk_id, 2);
        assert!(restarted.recv().await.unwrap().is_none());
        let path = store.pcm_path(2);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        let mut restarted = AudioWorkReceiver::recover(directory.path().to_path_buf()).unwrap();
        assert!(restarted
            .recv()
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        fs::write(store.result_path(1), b"{").unwrap();
        assert!(
            store.pending().is_err(),
            "A damaged result must never suppress pending source silently"
        );
    }
    #[test]
    fn disk_limits_fail_before_acceptance_and_keep_accepted_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let store = CatchupStore::open(directory.path().to_path_buf()).unwrap();
        store.persist(&chunk(1)).unwrap();
        store.bytes.store(MAX_SPOOL_BYTES, Ordering::SeqCst);
        assert!(store.persist(&chunk(2)).is_err());
        assert!(!store.pcm_path(2).exists());
        assert!(store.pcm_path(1).is_file());
    }
    #[tokio::test]
    async fn no_save_overflow_is_explicit_and_never_claims_recovery() {
        let (sender, _receiver) = audio_work_queue(1, None).unwrap();
        sender.send(chunk(0)).unwrap();
        let error = sender.send(chunk(1)).unwrap_err();
        assert!(error.to_string().contains("audio saving"));
    }
    #[tokio::test]
    async fn broken_spool_does_not_report_acceptance() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        assert!(audio_work_queue(1, Some(blocked)).is_err());
        let (sender, receiver) = audio_work_queue(1, Some(directory.path().to_path_buf())).unwrap();
        let store = receiver.store().unwrap();
        std::fs::remove_dir(&store.root).unwrap();
        std::fs::write(&store.root, b"disk unavailable").unwrap();
        assert!(sender.send(chunk(1)).is_err());
    }
}
