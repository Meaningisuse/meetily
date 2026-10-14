use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::Manager;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp4", "m4a", "wav", "mp3", "flac", "ogg", "mkv", "webm", "wma",
];

pub fn resolve_meeting_audio(folder: &Path) -> Result<PathBuf, String> {
    let folder =
        fs::canonicalize(folder).map_err(|e| format!("Meeting folder unavailable: {e}"))?;
    if !folder.is_dir() {
        return Err("Meeting folder is not a directory".into());
    }

    let metadata = folder.join("metadata.json");
    if let Ok(contents) = fs::read_to_string(metadata) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(name) = value.get("audio_file").and_then(|v| v.as_str()) {
                let candidate = Path::new(name);
                let candidate = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    folder.join(candidate)
                };
                if let Some(path) = validate_audio_candidate(&folder, &candidate) {
                    return Ok(path);
                }
            }
        }
    }

    let candidates = [
        "audio.mp4",
        "audio.m4a",
        "audio.wav",
        "audio.mp3",
        "audio.flac",
        "audio.ogg",
        "recording.mp4",
        "audio.mkv",
        "audio.webm",
        "audio.wma",
    ];
    for name in candidates {
        if let Some(path) = validate_audio_candidate(&folder, &folder.join(name)) {
            return Ok(path);
        }
    }

    let entries = fs::read_dir(&folder).map_err(|e| format!("Cannot list meeting audio: {e}"))?;
    for entry in entries.flatten() {
        let candidate = entry.path();
        let supported = candidate
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                AUDIO_EXTENSIONS
                    .iter()
                    .any(|supported| ext.eq_ignore_ascii_case(supported))
            })
            .unwrap_or(false);
        if supported {
            if let Some(path) = validate_audio_candidate(&folder, &candidate) {
                return Ok(path);
            }
        }
    }

    Err("No playable audio file found for this meeting".into())
}

fn validate_audio_candidate(folder: &Path, candidate: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(candidate).ok()?;
    let extension = path.extension()?.to_str()?;
    if !AUDIO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
        || !path.is_file()
        || !path.starts_with(folder)
    {
        return None;
    }
    Some(path)
}

#[tauri::command]
pub async fn get_meeting_audio_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<String, String> {
    let folder: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(&meeting_id)
            .fetch_optional(state.db_manager.pool())
            .await
            .map_err(|error| format!("Failed to look up meeting audio: {error}"))?
            .flatten();
    let folder = folder.ok_or_else(|| "Meeting audio is not available".to_string())?;
    let path = resolve_meeting_audio(Path::new(&folder))?;
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|error| format!("Failed to authorize meeting audio: {error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::resolve_meeting_audio;
    use std::fs;

    #[test]
    fn resolves_audio_only_from_inside_meeting_folder() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio.wav");
        fs::write(&audio, b"audio").unwrap();

        assert_eq!(
            resolve_meeting_audio(dir.path()).unwrap(),
            fs::canonicalize(audio).unwrap()
        );
    }

    #[test]
    fn rejects_missing_audio_and_non_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_meeting_audio(dir.path()).is_err());
        assert!(resolve_meeting_audio(&dir.path().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_audio_symlink_that_escapes_meeting_folder() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("audio.wav")).unwrap();
        assert!(resolve_meeting_audio(dir.path()).is_err());
    }
}
