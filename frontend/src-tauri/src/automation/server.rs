use super::types::ClientBrief;
use super::{
    auth::ClientService,
    discovery,
    types::{Discovery, Scope},
};
use crate::meeting::EnhancementError;
use axum::{
    extract::{Extension, Path, Request, State},
    http::{StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::Manager;
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub(crate) struct GatewayState {
    pub pool: SqlitePool,
    pub clients: ClientService,
    pub discovery: Discovery,
    pub enabled: Arc<AtomicBool>,
    pub rates: Arc<std::sync::Mutex<HashMap<String, (Instant, u32)>>>,
    pub reads: Arc<tokio::sync::Semaphore>,
    pub app: Option<tauri::AppHandle>,
}
struct Running {
    discovery: Discovery,
    cancel: CancellationToken,
    enabled: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
    dispatcher: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
pub struct AutomationServer {
    pub clients: ClientService,
    pool: SqlitePool,
    root: PathBuf,
    running: Arc<tokio::sync::Mutex<Option<Running>>>,
    preference: Arc<tokio::sync::Mutex<()>>,
    app: Arc<tokio::sync::RwLock<Option<tauri::AppHandle>>>,
}
pub(crate) struct ApiError(pub EnhancementError);
impl From<EnhancementError> for ApiError {
    fn from(e: EnhancementError) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "UNAUTHORIZED" => 401,
            "FORBIDDEN" => 403,
            "NOT_FOUND" => 404,
            "CONFLICT" => 409,
            "INVALID_INPUT" => 422,
            "RATE_LIMITED" => 429,
            _ => 503,
        };
        (StatusCode::from_u16(status).unwrap(),Json(serde_json::json!({"error":{"code":self.0.code,"message":self.0.message},"request_id":uuid::Uuid::new_v4().to_string()}))).into_response()
    }
}
fn error(code: &str, message: &str) -> Response {
    ApiError(EnhancementError::new(code, message)).into_response()
}
impl AutomationServer {
    pub fn new(pool: SqlitePool, clients: ClientService, app_data: PathBuf) -> Self {
        Self {
            pool,
            clients,
            root: app_data.join("automation"),
            running: Arc::new(tokio::sync::Mutex::new(None)),
            preference: Arc::new(tokio::sync::Mutex::new(())),
            app: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
    pub async fn attach_app_handle(&self, app: tauri::AppHandle) {
        *self.app.write().await = Some(app);
    }
    pub async fn restore_preference(&self) -> Result<(), EnhancementError> {
        let enabled: bool = sqlx::query_scalar("SELECT enabled FROM automation_config WHERE id=1")
            .fetch_one(&self.pool)
            .await?;
        if enabled {
            self.start().await?;
        }
        Ok(())
    }
    pub async fn set_enabled(&self, enabled: bool) -> Result<Option<Discovery>, EnhancementError> {
        let _preference = self.preference.lock().await;
        if enabled {
            let discovery = self.start().await?;
            if let Err(error) = sqlx::query("UPDATE automation_config SET enabled=1 WHERE id=1")
                .execute(&self.pool)
                .await
            {
                let _ = self.stop().await;
                return Err(error.into());
            }
            Ok(Some(discovery))
        } else {
            let persisted = sqlx::query("UPDATE automation_config SET enabled=0 WHERE id=1")
                .execute(&self.pool)
                .await;
            self.stop().await?;
            persisted?;
            Ok(None)
        }
    }
    pub async fn status(&self) -> Option<Discovery> {
        self.running
            .lock()
            .await
            .as_ref()
            .map(|r| r.discovery.clone())
    }
    pub async fn start(&self) -> Result<Discovery, EnhancementError> {
        let mut running = self.running.lock().await;
        if let Some(r) = running.as_ref() {
            return Ok(r.discovery.clone());
        }
        // Only consult previous instance on an exact loopback discovery endpoint.
        if let Ok(bytes) = std::fs::read(self.root.join("discovery.json")) {
            if bytes.len() <= 4096 {
                if let Ok(previous) = serde_json::from_slice::<Discovery>(&bytes) {
                    if let Ok(url) = url::Url::parse(&previous.base_url) {
                        if url.scheme() == "http"
                            && url.host_str() == Some("127.0.0.1")
                            && url.path() == "/api/internal/v1"
                            && url.port().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                        {
                            if let Ok(client) = reqwest::Client::builder()
                                .redirect(reqwest::redirect::Policy::none())
                                .timeout(Duration::from_millis(500))
                                .build()
                            {
                                if let Ok(response) = client
                                    .get(format!("{}/health", previous.base_url))
                                    .send()
                                    .await
                                {
                                    if let Ok(health) = response.json::<serde_json::Value>().await {
                                        if health["instance_id"] == previous.instance_id {
                                            return Err(EnhancementError::new("BUSY","Another desktop instance has an automation listener"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| EnhancementError::new("BUSY", "Loopback listener could not be opened"))?;
        let port = listener
            .local_addr()
            .map_err(|_| EnhancementError::new("IO_ERROR", "Listener address unavailable"))?
            .port();
        let value = Discovery {
            pid: std::process::id(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            base_url: format!("http://127.0.0.1:{port}/api/internal/v1"),
            protocol_version: 1,
        };
        discovery::write(&self.root, &value)?;
        let enabled = Arc::new(AtomicBool::new(true));
        let state = GatewayState {
            pool: self.pool.clone(),
            clients: self.clients.clone(),
            discovery: value.clone(),
            enabled: enabled.clone(),
            rates: Arc::new(std::sync::Mutex::new(HashMap::new())),
            reads: Arc::new(tokio::sync::Semaphore::new(4)),
            app: self.app.read().await.clone(),
        };
        let routes = Router::new()
            .route("/health", get(health))
            .route("/meetings", get(list_meetings))
            .route("/search", get(search_meetings))
            .route("/meetings/{id}/transcript", get(get_transcript))
            .route("/meetings/{id}/summary", get(get_summary))
            .route("/jobs/{id}", get(get_job))
            .route("/summary-jobs", axum::routing::post(create_summary_job))
            .route("/jobs/{id}/cancel", axum::routing::post(cancel_job))
            .route("/recordings/start", axum::routing::post(start_recording))
            .route("/recordings/{id}/stop", axum::routing::post(stop_recording))
            .fallback(|| async { error("NOT_FOUND", "Unknown automation endpoint") });
        let router = Router::new()
            .nest("/api/internal/v1", routes)
            .fallback(|| async { error("NOT_FOUND", "Unknown automation endpoint") })
            .layer(middleware::from_fn_with_state(state.clone(), guard))
            .with_state(state);
        let cancel = CancellationToken::new();
        let dispatch_cancel = cancel.clone();
        let dispatch_pool = self.pool.clone();
        let dispatcher = tokio::spawn(async move {
            super::outbox::deliver_due(dispatch_pool, dispatch_cancel).await;
        });
        let shutdown = cancel.clone();
        let active = enabled.clone();
        let root = self.root.clone();
        let instance = value.instance_id.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
            {
                log::error!("Automation listener stopped: {e}");
            }
            active.store(false, Ordering::SeqCst);
            let _ = discovery::remove_owned(&root, &instance);
        });
        *running = Some(Running {
            discovery: value.clone(),
            cancel,
            enabled,
            handle,
            dispatcher,
        });
        Ok(value)
    }
    pub async fn stop(&self) -> Result<(), EnhancementError> {
        let mut running = self.running.lock().await;
        if let Some(r) = running.take() {
            r.enabled.store(false, Ordering::SeqCst);
            r.cancel.cancel();
            r.handle
                .await
                .map_err(|_| EnhancementError::new("IO_ERROR", "Listener shutdown failed"))?;
            r.dispatcher.await.map_err(|_| {
                EnhancementError::new("IO_ERROR", "Webhook dispatcher shutdown failed")
            })?;
            discovery::remove_owned(&self.root, &r.discovery.instance_id)?;
        }
        Ok(())
    }
}
async fn health(State(state): State<GatewayState>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"protocol_version":1,"instance_id":state.discovery.instance_id,"capabilities":["gateway","meeting_read","jobs"]}),
    )
}
fn page_request(uri: &Uri) -> Result<super::read_service::PageRequest, ApiError> {
    let mut limit = None;
    let mut cursor = None;
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "limit" => {
                if limit.is_some() {
                    return Err(ApiError(EnhancementError::invalid(
                        "limit must appear once",
                    )));
                }
                limit = Some(value.parse::<usize>().map_err(|_| {
                    ApiError(EnhancementError::invalid("limit must be an integer"))
                })?);
            }
            "cursor" => {
                if cursor.is_some() {
                    return Err(ApiError(EnhancementError::invalid(
                        "cursor must appear once",
                    )));
                }
                cursor = Some(value.into_owned());
            }
            _ => {
                return Err(ApiError(EnhancementError::invalid(
                    "unknown query parameter",
                )))
            }
        }
    }
    Ok(super::read_service::PageRequest { limit, cursor })
}
async fn list_meetings(
    State(state): State<GatewayState>,
    uri: Uri,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = super::read_service::MeetingReadService::new(state.pool)
        .list(page_request(&uri)?)
        .await?;
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}
async fn search_meetings(
    State(state): State<GatewayState>,
    uri: Uri,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut query = None;
    let mut request = super::read_service::PageRequest::default();
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "q" if query.is_none() => query = Some(value.into_owned()),
            "limit" if request.limit.is_none() => {
                request.limit = Some(value.parse().map_err(|_| {
                    ApiError(EnhancementError::invalid("limit must be an integer"))
                })?);
            }
            "cursor" if request.cursor.is_none() => request.cursor = Some(value.into_owned()),
            "q" | "limit" | "cursor" => {
                return Err(ApiError(EnhancementError::invalid(
                    "query parameter must appear once",
                )))
            }
            _ => {
                return Err(ApiError(EnhancementError::invalid(
                    "unknown query parameter",
                )))
            }
        }
    }
    let query = query.ok_or_else(|| ApiError(EnhancementError::invalid("q is required")))?;
    let page = super::read_service::MeetingReadService::new(state.pool)
        .search(&query, request)
        .await?;
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}
async fn get_transcript(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = super::read_service::MeetingReadService::new(state.pool)
        .transcript(&id, page_request(&uri)?)
        .await?;
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}
async fn get_summary(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let doc = super::read_service::MeetingReadService::new(state.pool)
        .summary(&id)
        .await?;
    Ok(Json(
        serde_json::json!({"meeting_id":doc.meeting_id,"title":doc.title,"created_at":doc.created_at,"versions":doc.versions,"summary_markdown":doc.summary_markdown,"summary_stale":doc.summary_stale}),
    ))
}
async fn get_job(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        super::read_service::MeetingReadService::new(state.pool)
            .job(&id)
            .await?,
    ))
}
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SummaryJobRequest {
    meeting_id: String,
    expected_revision: i64,
    model: String,
    model_name: String,
    custom_prompt: Option<String>,
    template_id: Option<String>,
    summary_language: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RecordingStartRequest {
    meeting_name: Option<String>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
}

fn command_error(message: String) -> ApiError {
    if let Some((code, text)) = message.split_once(": ") {
        if matches!(
            code,
            "CONFLICT" | "INVALID_ARGUMENT" | "INVALID_INPUT" | "NOT_FOUND" | "BUSY" | "NOT_READY"
        ) {
            return ApiError(EnhancementError::new(code, text));
        }
    }
    ApiError(EnhancementError::new(
        "IO_ERROR",
        "The local operation could not be completed.",
    ))
}

fn idempotency_key(headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError(EnhancementError::invalid(
                "Idempotency-Key header is required.",
            ))
        })
}

async fn create_summary_job(
    State(state): State<GatewayState>,
    Extension(client): Extension<ClientBrief>,
    headers: axum::http::HeaderMap,
    Json(request): Json<SummaryJobRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.meeting_id.trim().is_empty()
        || request.meeting_id.len() > 256
        || request.model.len() > 100
        || request.model_name.len() > 200
        || request
            .custom_prompt
            .as_ref()
            .is_some_and(|s| s.len() > 100_000)
    {
        return Err(ApiError(EnhancementError::invalid(
            "Summary job fields exceed their limits.",
        )));
    }
    let app = state.app.ok_or_else(|| {
        ApiError(EnhancementError::new(
            "NOT_READY",
            "Desktop state is unavailable.",
        ))
    })?;
    let payload = serde_json::to_value(&request)
        .map_err(|_| ApiError(EnhancementError::invalid("Invalid request body.")))?;
    create_summary_job_with_action(
        state.pool,
        client.id,
        headers,
        payload,
        move || async move {
            let app_state = app.try_state::<crate::state::AppState>().ok_or_else(|| {
                EnhancementError::new("NOT_READY", "Desktop state is unavailable.")
            })?;
            crate::summary::commands::api_process_transcript(
                app.clone(),
                app_state,
                String::new(),
                request.model,
                request.model_name,
                Some(request.meeting_id),
                None,
                None,
                request.custom_prompt,
                request.template_id,
                request.summary_language,
                None,
                Some(request.expected_revision),
            )
            .await
            .map_err(|error| command_error(error).0)
        },
    )
    .await
}

// Share request ownership with database-backed, provider-free regression tests.
pub(super) async fn create_summary_job_with_action<F, Fut>(
    pool: SqlitePool,
    client_id: String,
    headers: axum::http::HeaderMap,
    payload: serde_json::Value,
    action: F,
) -> Result<Json<serde_json::Value>, ApiError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<
            Output = Result<crate::summary::commands::ProcessTranscriptResponse, EnhancementError>,
        > + Send
        + 'static,
{
    let key = idempotency_key(&headers)?;
    let response =
        super::idempotency::run_detached(&pool, &client_id, &key, &payload, action).await?;
    Ok(Json(serde_json::to_value(response).unwrap_or_default()))
}

async fn cancel_job(
    State(state): State<GatewayState>,
    Extension(client): Extension<ClientBrief>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let app = state.app.ok_or_else(|| {
        ApiError(EnhancementError::new(
            "NOT_READY",
            "Desktop state is unavailable.",
        ))
    })?;
    cancel_job_with_action(state.pool, client.id, id, headers, move |id| async move {
        let app_state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| EnhancementError::new("NOT_READY", "Desktop state is unavailable."))?;
        app_state.jobs.cancel(&id).await
    })
    .await
}

pub(super) async fn cancel_job_with_action<F, Fut>(
    pool: SqlitePool,
    client_id: String,
    id: String,
    headers: axum::http::HeaderMap,
    action: F,
) -> Result<Json<serde_json::Value>, ApiError>
where
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<crate::jobs::JobSnapshot, EnhancementError>>
        + Send
        + 'static,
{
    let key = idempotency_key(&headers)?;
    let payload = serde_json::json!({"job_id":id,"operation":"cancel"});
    let response =
        super::idempotency::run_detached(&pool, &client_id, &key, &payload, move || action(id))
            .await?;
    Ok(Json(serde_json::to_value(response).unwrap_or_default()))
}

async fn start_recording(
    State(state): State<GatewayState>,
    Extension(client): Extension<ClientBrief>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RecordingStartRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.meeting_name.as_ref().is_some_and(|s| {
        s.trim().is_empty() || s.chars().count() > 200 || s.chars().any(char::is_control)
    }) {
        return Err(ApiError(EnhancementError::invalid(
            "meeting_name must be at most 200 visible characters.",
        )));
    }
    for device in [&request.mic_device_name, &request.system_device_name]
        .into_iter()
        .flatten()
    {
        if device.chars().count() > 200 || device.chars().any(char::is_control) {
            return Err(ApiError(EnhancementError::invalid(
                "Device name exceeds its limit.",
            )));
        }
    }
    let app = state.app.ok_or_else(|| {
        ApiError(EnhancementError::new(
            "NOT_READY",
            "Desktop state is unavailable.",
        ))
    })?;
    let payload = serde_json::to_value(&request)
        .map_err(|_| ApiError(EnhancementError::invalid("Invalid request body.")))?;
    start_recording_with_action(
        state.pool,
        client.id,
        headers,
        payload,
        move || async move {
            crate::audio::recording_commands::automation_start_recording(
                app,
                request.mic_device_name,
                request.system_device_name,
                request.meeting_name,
            )
            .await
        },
    )
    .await
}

// Share HTTP ownership and result persistence with hardware-free regression tests.
pub(super) async fn start_recording_with_action<F, Fut>(
    pool: SqlitePool,
    client_id: String,
    headers: axum::http::HeaderMap,
    payload: serde_json::Value,
    start: F,
) -> Result<Json<serde_json::Value>, ApiError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    let key = idempotency_key(&headers)?;
    let response =
        super::idempotency::run_detached(&pool, &client_id, &key, &payload, move || async move {
            let session_id = start().await.map_err(|e| {
                if e.starts_with("CONFLICT:") {
                    EnhancementError::new("CONFLICT", e.trim_start_matches("CONFLICT: "))
                } else if e.starts_with("BUSY:") {
                    EnhancementError::new("BUSY", e.trim_start_matches("BUSY: "))
                } else {
                    EnhancementError::new("IO_ERROR", "Recording could not be started.")
                }
            })?;
            Ok(serde_json::json!({"session_id":session_id,"status":"recording"}))
        })
        .await?;
    Ok(Json(response))
}

async fn stop_recording(
    State(state): State<GatewayState>,
    Extension(client): Extension<ClientBrief>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let app = state.app.ok_or_else(|| {
        ApiError(EnhancementError::new(
            "NOT_READY",
            "Desktop state is unavailable.",
        ))
    })?;
    stop_recording_with_action(
        state.pool,
        client.id,
        id,
        headers,
        move |session_id| async move {
            crate::audio::recording_commands::automation_stop_recording(app, &session_id).await
        },
    )
    .await
}

// Keep the HTTP orchestration testable without starting capture hardware. Both
// the desktop action and synthetic slow-action tests use this same boundary.
pub(super) async fn stop_recording_with_action<F, Fut>(
    pool: SqlitePool,
    client_id: String,
    id: String,
    headers: axum::http::HeaderMap,
    stop: F,
) -> Result<Json<serde_json::Value>, ApiError>
where
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let key = idempotency_key(&headers)?;
    let payload = serde_json::json!({"session_id":id,"operation":"stop"});
    let response =
        super::idempotency::run_detached(&pool, &client_id, &key, &payload, move || async move {
            stop(id.clone()).await.map_err(recording_stop_error)?;
            Ok(serde_json::json!({"session_id":id,"status":"stopped"}))
        })
        .await?;
    Ok(Json(response))
}

fn recording_stop_error(error: String) -> EnhancementError {
    for code in ["CONFLICT", "BUSY"] {
        if let Some(message) = error.strip_prefix(&format!("{code}:")) {
            return EnhancementError::new(code, message.trim_start());
        }
    }
    EnhancementError::new("IO_ERROR", "Recording could not be stopped or saved.")
}

#[cfg(test)]
mod stop_error_tests {
    #[test]
    fn preserves_retryable_and_stale_session_errors_without_leaking_native_errors() {
        for code in ["BUSY", "CONFLICT"] {
            let error = super::recording_stop_error(format!("{code}: Recording operation"));
            assert_eq!(error.code, code);
            assert_eq!(error.message, "Recording operation");
        }
        let error = super::recording_stop_error("private filesystem diagnostic".into());
        assert_eq!(error.code, "IO_ERROR");
        assert!(!error.message.contains("private filesystem diagnostic"));
    }
}

pub(super) async fn guard(
    State(state): State<GatewayState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !state.enabled.load(Ordering::SeqCst) {
        return error("NOT_READY", "Automation is disabled");
    }
    let authority = state
        .discovery
        .base_url
        .strip_prefix("http://")
        .and_then(|s| s.split('/').next())
        .unwrap_or("");
    if request.headers().get("host").and_then(|h| h.to_str().ok()) != Some(authority) {
        return error("FORBIDDEN", "Invalid local Host");
    }
    if let Some(origin) = request.headers().get("origin") {
        if !origin.to_str().is_ok_and(|s| {
            matches!(
                s,
                "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
            )
        }) {
            return error("FORBIDDEN", "Origin is not permitted");
        }
    }
    let _permit = match state.reads.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return error("BUSY", "Four requests are already active"),
    };
    if request.uri().path() != "/api/internal/v1/health"
        || request.method() != axum::http::Method::GET
    {
        let scope = if request
            .uri()
            .path()
            .starts_with("/api/internal/v1/recordings")
        {
            Scope::Record
        } else if request.method() == axum::http::Method::GET {
            Scope::Read
        } else {
            Scope::Write
        };
        let token = match request
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
        {
            Some(t) => t,
            None => return error("UNAUTHORIZED", "Bearer credential is required"),
        };
        let client = match state.clients.verify(token, scope).await {
            Ok(c) => c,
            Err(e) => return ApiError(e).into_response(),
        };
        {
            let mut rates = state.rates.lock().unwrap_or_else(|p| p.into_inner());
            rates.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
            let entry = rates
                .entry(client.id.clone())
                .or_insert((Instant::now(), 0));
            if entry.1 >= 60 {
                return error("RATE_LIMITED", "Maximum60requests per minute per client");
            }
            entry.1 += 1;
        }
        request.extensions_mut().insert(client);
    }
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(body, 1024 * 1024),
    )
    .await
    {
        Ok(Ok(b)) => b,
        _ => return error("INVALID_INPUT", "Request body is too large or incomplete"),
    };
    let request = Request::from_parts(parts, axum::body::Body::from(bytes));
    match tokio::time::timeout(Duration::from_secs(20), next.run(request)).await {
        Ok(response) => response,
        Err(_) => error("BUSY", "Request deadline exceeded"),
    }
}
