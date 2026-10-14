use super::types::{ClientBrief, ClientTokenOnce, Discovery, Scope};
use crate::{meeting::EnhancementError, state::AppState};
#[tauri::command]
pub async fn automation_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<Discovery>, EnhancementError> {
    Ok(state.automation.status().await)
}
#[tauri::command]
pub async fn automation_set_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<Option<Discovery>, EnhancementError> {
    state.automation.set_enabled(enabled).await
}
#[tauri::command]
pub async fn automation_create_client(
    state: tauri::State<'_, AppState>,
    name: String,
    scopes: Vec<Scope>,
    expires_at: i64,
) -> Result<ClientTokenOnce, EnhancementError> {
    let expires = chrono::DateTime::from_timestamp(expires_at, 0)
        .ok_or_else(|| EnhancementError::invalid("Invalid expiry"))?;
    state
        .automation
        .clients
        .create(&name, scopes, expires)
        .await
}
#[tauri::command]
pub async fn automation_list_clients(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ClientBrief>, EnhancementError> {
    state.automation.clients.list().await
}
#[tauri::command]
pub async fn automation_revoke_client(
    state: tauri::State<'_, AppState>,
    client_id: String,
) -> Result<(), EnhancementError> {
    state.automation.clients.revoke(&client_id).await
}
