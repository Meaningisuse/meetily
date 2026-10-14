use super::{
    auth::ClientService,
    discovery,
    types::{Discovery, Scope},
};
use crate::meeting::EnhancementError;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
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
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub(crate) struct GatewayState {
    pub pool: SqlitePool,
    pub clients: ClientService,
    pub discovery: Discovery,
    pub enabled: Arc<AtomicBool>,
    pub rates: Arc<std::sync::Mutex<HashMap<String, (Instant, u32)>>>,
    pub reads: Arc<tokio::sync::Semaphore>,
}
struct Running {
    discovery: Discovery,
    cancel: CancellationToken,
    enabled: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
pub struct AutomationServer {
    pub clients: ClientService,
    pool: SqlitePool,
    root: PathBuf,
    running: Arc<tokio::sync::Mutex<Option<Running>>>,
    preference: Arc<tokio::sync::Mutex<()>>,
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
        }
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
        };
        let routes = Router::new()
            .route("/health", get(health))
            .fallback(|| async { error("NOT_FOUND", "Unknown automation endpoint") });
        let router = Router::new()
            .nest("/api/internal/v1", routes)
            .fallback(|| async { error("NOT_FOUND", "Unknown automation endpoint") })
            .layer(middleware::from_fn_with_state(state.clone(), guard))
            .with_state(state);
        let cancel = CancellationToken::new();
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
            discovery::remove_owned(&self.root, &r.discovery.instance_id)?;
        }
        Ok(())
    }
}
async fn health(State(state): State<GatewayState>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"protocol_version":1,"instance_id":state.discovery.instance_id,"capabilities":["gateway"]}),
    )
}
async fn guard(State(state): State<GatewayState>, mut request: Request, next: Next) -> Response {
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
