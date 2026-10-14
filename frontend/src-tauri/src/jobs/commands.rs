use crate::{jobs::JobSnapshot, meeting::EnhancementError, state::AppState};
#[tauri::command]
pub async fn api_get_job(
    state: tauri::State<'_, AppState>,
    job_id: String,
) -> Result<JobSnapshot, EnhancementError> {
    state.jobs.get(&job_id).await
}
#[tauri::command]
pub async fn api_cancel_job(
    state: tauri::State<'_, AppState>,
    job_id: String,
) -> Result<JobSnapshot, EnhancementError> {
    state.jobs.cancel(&job_id).await
}
