use super::types::Discovery;
use crate::meeting::EnhancementError;
use std::{io::Write, path::Path};
fn io_error() -> EnhancementError {
    EnhancementError::new(
        "IO_ERROR",
        "Private automation discovery could not be saved",
    )
}
pub fn write(root: &Path, discovery: &Discovery) -> Result<(), EnhancementError> {
    if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io_error());
    }
    std::fs::create_dir_all(root).map_err(|_| io_error())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| io_error())?;
    }
    let partial = root.join(format!(".{}.incomplete", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), EnhancementError> {
        let mut opts = std::fs::OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&partial).map_err(|_| io_error())?;
        file.write_all(&serde_json::to_vec(discovery).map_err(|_| io_error())?)
            .map_err(|_| io_error())?;
        file.sync_all().map_err(|_| io_error())?;
        std::fs::rename(&partial, root.join("discovery.json")).map_err(|_| io_error())?;
        std::fs::File::open(root)
            .and_then(|f| f.sync_all())
            .map_err(|_| io_error())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}
pub fn remove_owned(root: &Path, instance: &str) -> Result<(), EnhancementError> {
    let path = root.join("discovery.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(());
    };
    let Ok(value) = serde_json::from_slice::<Discovery>(&bytes) else {
        return Ok(());
    };
    if value.instance_id == instance {
        std::fs::remove_file(path).map_err(|_| io_error())?;
        std::fs::File::open(root)
            .and_then(|f| f.sync_all())
            .map_err(|_| io_error())?;
    }
    Ok(())
}
