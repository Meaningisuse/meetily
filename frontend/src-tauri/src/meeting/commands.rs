use super::{
    EnhancementError, MeetingDocument, MeetingDocumentService, MeetingWriteService, SourceVersions,
};
use crate::state::AppState;
#[tauri::command]
pub async fn api_get_meeting_document(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<MeetingDocument, EnhancementError> {
    MeetingDocumentService::new(state.db_manager.pool().clone())
        .snapshot(&meeting_id)
        .await
}
#[tauri::command]
pub async fn api_save_meeting_notes(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    markdown: String,
    expected_revision: i64,
) -> Result<SourceVersions, EnhancementError> {
    MeetingWriteService::new(state.db_manager.pool().clone())
        .save_notes_markdown(&meeting_id, &markdown, expected_revision)
        .await
}

#[tauri::command]
pub fn api_native_ready(app: tauri::AppHandle) -> bool {
    use tauri::Manager;
    app.try_state::<AppState>().is_some()
}
