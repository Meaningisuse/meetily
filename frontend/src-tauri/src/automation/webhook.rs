use super::{
    auth::{SystemVault, Vault},
    target_policy,
};
use crate::{meeting::EnhancementError, state::AppState};
use rand::RngCore;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;

const EVENT_KINDS: &[&str] = &[
    "recording.stopped",
    "transcript.completed",
    "summary.completed",
    "summary.failed",
];

#[derive(Debug, Serialize)]
pub struct WebhookBrief {
    pub id: String,
    pub url: String,
    pub kinds: Vec<String>,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct WebhookSecretOnce {
    pub webhook: WebhookBrief,
    pub secret: String,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<WebhookBrief>, EnhancementError> {
    let rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id,url,kinds_json,enabled,created_at FROM automation_webhooks ORDER BY created_at DESC,id",
    ).fetch_all(pool).await?;
    rows.into_iter()
        .map(|(id, url, kinds, enabled, created_at)| {
            let kinds = serde_json::from_str(&kinds).map_err(|_| {
                EnhancementError::new("IO_ERROR", "Webhook event configuration is invalid")
            })?;
            Ok(WebhookBrief {
                id,
                url,
                kinds,
                enabled: enabled != 0,
                created_at,
            })
        })
        .collect()
}

pub async fn create(
    pool: &SqlitePool,
    url: &str,
    approvals: Vec<String>,
    kinds: Vec<String>,
) -> Result<WebhookSecretOnce, EnhancementError> {
    if url.len() > 2048
        || kinds.is_empty()
        || kinds.len() > EVENT_KINDS.len()
        || kinds
            .iter()
            .any(|kind| !EVENT_KINDS.contains(&kind.as_str()))
    {
        return Err(EnhancementError::invalid(
            "Webhook URL or event selection is invalid.",
        ));
    }
    let mut approvals: Vec<String> = approvals
        .into_iter()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .collect();
    approvals.sort();
    approvals.dedup();
    if approvals.len() > 32 {
        return Err(EnhancementError::invalid(
            "Too many webhook target approvals.",
        ));
    }
    let pinned = target_policy::resolve_and_validate(url, &approvals)
        .await
        .map_err(|_| {
            EnhancementError::invalid(
                "Webhook target must use HTTPS and an explicitly approved safe host/IP.",
            )
        })?;
    for address in &pinned.addresses {
        approvals.push(address.ip().to_string());
    }
    approvals.sort();
    approvals.dedup();

    let id = uuid::Uuid::new_v4().to_string();
    let secret_ref = id.clone();
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let secret: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let vault: Arc<dyn Vault> = Arc::new(SystemVault);
    let write_vault = vault.clone();
    let write_id = secret_ref.clone();
    let write_secret = secret.clone();
    tokio::task::spawn_blocking(move || {
        write_vault.put(&write_id, &write_secret)?;
        if write_vault.get(&write_id)? != write_secret {
            let _ = write_vault.delete(&write_id);
            return Err("Keychain verification failed".to_owned());
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| EnhancementError::new("IO_ERROR", "Keychain task failed"))?
    .map_err(|_| EnhancementError::new("IO_ERROR", "Webhook secret could not be saved securely"))?;

    let created_at = chrono::Utc::now().timestamp();
    let result = sqlx::query("INSERT INTO automation_webhooks(id,url,secret_ref,allowed_hosts_json,kinds_json,enabled,created_at) VALUES(?,?,?,?,?,1,?)")
        .bind(&id).bind(pinned.url.as_str()).bind(&secret_ref).bind(serde_json::to_string(&approvals).unwrap()).bind(serde_json::to_string(&kinds).unwrap()).bind(created_at).execute(pool).await;
    if result.is_err() {
        let delete_vault = vault.clone();
        let delete_id = secret_ref.clone();
        let _ = tokio::task::spawn_blocking(move || delete_vault.delete(&delete_id)).await;
        return Err(EnhancementError::new(
            "IO_ERROR",
            "Webhook configuration could not be saved",
        ));
    }
    Ok(WebhookSecretOnce {
        webhook: WebhookBrief {
            id,
            url: pinned.url.to_string(),
            kinds,
            enabled: true,
            created_at,
        },
        secret,
    })
}

pub async fn set_enabled(
    pool: &SqlitePool,
    id: &str,
    enabled: bool,
) -> Result<(), EnhancementError> {
    let result = sqlx::query("UPDATE automation_webhooks SET enabled=? WHERE id=?")
        .bind(enabled)
        .bind(id)
        .execute(pool)
        .await?;
    if result.rows_affected() != 1 {
        return Err(EnhancementError::new("NOT_FOUND", "Webhook does not exist"));
    }
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<(), EnhancementError> {
    let secret_ref: Option<String> =
        sqlx::query_scalar("SELECT secret_ref FROM automation_webhooks WHERE id=?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some(secret_ref) = secret_ref else {
        return Err(EnhancementError::new("NOT_FOUND", "Webhook does not exist"));
    };
    sqlx::query("DELETE FROM automation_webhooks WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    let vault: Arc<dyn Vault> = Arc::new(SystemVault);
    tokio::task::spawn_blocking(move || vault.delete(&secret_ref))
        .await
        .map_err(|_| EnhancementError::new("IO_ERROR", "Keychain task failed"))?
        .map_err(|_| {
            EnhancementError::new(
                "IO_ERROR",
                "Webhook was removed but its Keychain secret could not be deleted",
            )
        })
}

#[tauri::command]
pub async fn automation_list_webhooks(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WebhookBrief>, EnhancementError> {
    list(state.db_manager.pool()).await
}

#[tauri::command]
pub async fn automation_create_webhook(
    state: tauri::State<'_, AppState>,
    url: String,
    approved_hosts: Vec<String>,
    kinds: Vec<String>,
) -> Result<WebhookSecretOnce, EnhancementError> {
    create(state.db_manager.pool(), &url, approved_hosts, kinds).await
}

#[tauri::command]
pub async fn automation_set_webhook_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), EnhancementError> {
    set_enabled(state.db_manager.pool(), &id, enabled).await
}

#[tauri::command]
pub async fn automation_delete_webhook(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), EnhancementError> {
    delete(state.db_manager.pool(), &id).await
}
