//! Recovery copies are evidence, never a license to discard a committed WAL.
//! Offline capture requires the caller to stop writers and close its pool.
use crate::meeting::EnhancementError;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct RecoveryService;
fn io_error(error: std::io::Error) -> EnhancementError {
    log::error!("Database recovery file operation failed: {error}");
    EnhancementError::new(
        "IO_ERROR",
        "Recovery materials could not be saved. Original files were retained.",
    )
}
fn private_directory(path: &Path) -> Result<(), EnhancementError> {
    fs::create_dir_all(path).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}
fn new_private_file(path: &Path) -> Result<File, EnhancementError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io_error)
}
fn sync_directory(path: &Path) -> Result<(), EnhancementError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}

impl RecoveryService {
    pub fn backup_before_recovery(db_path: &Path) -> Result<PathBuf, EnhancementError> {
        let parent = db_path
            .parent()
            .ok_or_else(|| EnhancementError::invalid("Database needs a parent directory."))?;
        Self::backup_to(db_path, &parent.join("recovery-backups"))
    }
    pub(crate) fn backup_to(db_path: &Path, root: &Path) -> Result<PathBuf, EnhancementError> {
        private_directory(root)?;
        let id = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S"),
            uuid::Uuid::new_v4()
        );
        let partial = root.join(format!("{id}.incomplete"));
        private_directory(&partial)?;
        let name = db_path
            .file_name()
            .ok_or_else(|| EnhancementError::invalid("Database filename required."))?;
        let mut manifest = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut source = db_path.as_os_str().to_os_string();
            source.push(suffix);
            let source = PathBuf::from(source);
            let before = match fs::symlink_metadata(&source) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {
                    continue
                }
                Err(e) => return Err(io_error(e)),
            };
            if !before.is_file() {
                return Err(EnhancementError::new(
                    "IO_ERROR",
                    "Recovery source must be a regular file.",
                ));
            }
            let mut target_name = name.to_os_string();
            target_name.push(suffix);
            let target = partial.join(&target_name);
            let mut input = File::open(&source).map_err(io_error)?;
            let mut output = new_private_file(&target)?;
            let mut hash = Sha256::new();
            let mut copied = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let n = input.read(&mut buffer).map_err(io_error)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buffer[..n]).map_err(io_error)?;
                hash.update(&buffer[..n]);
                copied += n as u64;
            }
            output.sync_all().map_err(io_error)?;
            let after = fs::metadata(&source).map_err(io_error)?;
            if before.len() != copied
                || after.len() != before.len()
                || after.modified().ok() != before.modified().ok()
            {
                return Err(EnhancementError::new(
                    "BUSY",
                    "Recovery source changed while copying; stop other writers and retry.",
                ));
            }
            manifest.push(serde_json::json!({"file":target_name.to_string_lossy(),"bytes":copied,"sha256":format!("{:x}",hash.finalize())}));
        }
        let mut file = new_private_file(&partial.join("manifest.json"))?;
        file.write_all(serde_json::json!({"schema_version":1,"created_at":chrono::Utc::now(),"mode":"offline_evidence","files":manifest}).to_string().as_bytes()).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        sync_directory(&partial)?;
        let completed = root.join(id);
        fs::rename(&partial, &completed).map_err(io_error)?;
        sync_directory(root)?;
        Ok(completed)
    }
    /// SQLite creates a consistent logical snapshot including committed WAL.
    /// Validate and sync the output before publishing it without overwriting.
    pub async fn online_backup(
        pool: &SqlitePool,
        destination: &Path,
    ) -> Result<(), EnhancementError> {
        if destination.exists() {
            return Err(EnhancementError::new("CONFLICT", "Backup already exists."));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| EnhancementError::invalid("Backup parent required."))?;
        let partial = parent.join(format!(".backup-{}.incomplete", uuid::Uuid::new_v4()));
        drop(new_private_file(&partial)?);
        let result = async {
            let path = partial
                .to_str()
                .ok_or_else(|| EnhancementError::invalid("Backup path must be UTF-8."))?;
            sqlx::query("VACUUM main INTO ?")
                .bind(path)
                .execute(pool)
                .await?;
            let verification = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    sqlx::sqlite::SqliteConnectOptions::new()
                        .filename(&partial)
                        .read_only(true),
                )
                .await?;
            let check = sqlx::query_scalar::<_, String>("PRAGMA quick_check")
                .fetch_one(&verification)
                .await;
            verification.close().await;
            if check? != "ok" {
                return Err(EnhancementError::new(
                    "IO_ERROR",
                    "Backup failed SQLite integrity verification.",
                ));
            }
            File::open(&partial)
                .and_then(|f| f.sync_all())
                .map_err(io_error)?;
            // Atomic no-clobber publication, on the destination filesystem.
            fs::hard_link(&partial, destination).map_err(io_error)?;
            fs::remove_file(&partial).map_err(io_error)?;
            sync_directory(parent)?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result
    }
    pub async fn migration_backup(
        pool: &SqlitePool,
        db_path: &Path,
    ) -> Result<PathBuf, EnhancementError> {
        let parent = db_path
            .parent()
            .ok_or_else(|| EnhancementError::invalid("Database parent required."))?;
        let root = parent.join("recovery-backups");
        private_directory(&root)?;
        let destination = root.join(format!(
            "pre-migration-{}-{}.sqlite",
            chrono::Utc::now().format("%Y%m%dT%H%M%S"),
            uuid::Uuid::new_v4()
        ));
        Self::online_backup(pool, &destination).await?;
        Ok(destination)
    }
    pub async fn copy_online_path(
        source: &Path,
        destination: &Path,
    ) -> Result<(), EnhancementError> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(source)
                    .read_only(true),
            )
            .await?;
        let result = Self::online_backup(&pool, destination).await;
        pool.close().await;
        result
    }
}
