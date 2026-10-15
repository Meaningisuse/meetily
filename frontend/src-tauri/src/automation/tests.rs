use super::{
    auth::{ClientService, Vault},
    server::AutomationServer,
    types::Scope,
};
use crate::meeting::tests::pool;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct TestVault {
    secrets: Mutex<std::collections::HashMap<String, String>>,
    fail: bool,
    read_fail: bool,
}

#[tokio::test]
async fn stop_deadline_only_ends_http_wait_and_retries_replay_final_result() {
    use super::server::{guard, stop_recording_with_action, GatewayState};
    use super::types::{ClientBrief, Discovery};
    use axum::{
        extract::{Extension, Path, State},
        middleware,
        routing::post,
        Router,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let token = clients
        .create(
            "stop regression",
            vec![Scope::Record],
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let base_url = format!("http://{}/api/internal/v1", listener.local_addr().unwrap());
    let enabled = Arc::new(AtomicBool::new(true));
    let state = GatewayState {
        pool: pool.clone(),
        clients,
        discovery: Discovery {
            pid: std::process::id(),
            instance_id: "stop-regression".into(),
            base_url: base_url.clone(),
            protocol_version: 1,
        },
        enabled: enabled.clone(),
        rates: Arc::new(Mutex::new(std::collections::HashMap::new())),
        reads: Arc::new(tokio::sync::Semaphore::new(4)),
        app: None,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let action_calls = calls.clone();
    let action_started = started.clone();
    let action_finish = finish.clone();
    let router = Router::new()
        .route(
            "/api/internal/v1/recordings/{id}/stop",
            post(
                move |State(state): State<GatewayState>,
                      Extension(client): Extension<ClientBrief>,
                      Path(id): Path<String>,
                      headers: axum::http::HeaderMap| {
                    let calls = action_calls.clone();
                    let started = action_started.clone();
                    let finish = action_finish.clone();
                    async move {
                        stop_recording_with_action(
                            state.pool,
                            client.id,
                            id,
                            headers,
                            move |session_id| async move {
                                assert_eq!(session_id, "session-one");
                                calls.fetch_add(1, Ordering::SeqCst);
                                started.notify_one();
                                finish.notified().await;
                                Ok(())
                            },
                        )
                        .await
                    }
                },
            ),
        )
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let url = format!("{base_url}/recordings/session-one/stop");
    let request = || {
        http.post(&url)
            .bearer_auth(&token.token)
            .header("Idempotency-Key", "http-stop")
    };
    let before = Instant::now();
    let first_request = tokio::spawn(request().send());
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("stop action must start");
    let first = first_request.await.unwrap().unwrap();
    assert_eq!(first.status(), 503);
    let first: serde_json::Value = first.json().await.unwrap();
    assert_eq!(first["error"]["code"], "BUSY");
    assert_eq!(first["error"]["message"], "Request deadline exceeded");
    assert!(
        before.elapsed() >= Duration::from_secs(19),
        "must exercise the production 20-second deadline"
    );

    let pending = request().send().await.unwrap();
    assert_eq!(pending.status(), 503);
    let pending: serde_json::Value = pending.json().await.unwrap();
    assert_eq!(pending["error"]["code"], "BUSY");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Disabling new requests must not cancel an already accepted safe stop.
    enabled.store(false, Ordering::SeqCst);
    let disabled: serde_json::Value = request().send().await.unwrap().json().await.unwrap();
    assert_eq!(disabled["error"]["code"], "NOT_READY");
    finish.notify_one();
    let saved = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result: Option<String> = sqlx::query_scalar("SELECT response_json FROM automation_requests WHERE client_id=? AND key='http-stop'")
                .bind(&token.client.id).fetch_one(&pool).await.unwrap();
            if let Some(result) = result { break serde_json::from_str::<serde_json::Value>(&result).unwrap(); }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("stop must finish and persist after HTTP timeout and gateway disable");
    assert_eq!(
        saved,
        serde_json::json!({"session_id":"session-one","status":"stopped"})
    );
    enabled.store(true, Ordering::SeqCst);
    let replay = request().send().await.unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), saved);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let _ = shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

impl Vault for TestVault {
    fn put(&self, id: &str, value: &str) -> Result<(), String> {
        if self.fail {
            return Err("vault unavailable".into());
        }
        self.secrets.lock().unwrap().insert(id.into(), value.into());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<String, String> {
        if self.read_fail {
            return Err("readback unavailable".into());
        }
        self.secrets
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("missing".into())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.secrets.lock().unwrap().remove(id);
        Ok(())
    }
}
#[tokio::test]
async fn disabled_by_default_and_loopback_only() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let server = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    assert!(server.status().await.is_none());
    assert!(!folder.path().join("automation/discovery.json").exists());
    let first = server.start().await.unwrap();
    assert!(first.base_url.starts_with("http://127.0.0.1:"));
    assert_eq!(server.start().await.unwrap().instance_id, first.instance_id);
    let discovery = folder.path().join("automation/discovery.json");
    assert!(discovery.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&discovery).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(discovery.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    server.stop().await.unwrap();
    assert!(!discovery.exists());
    assert!(reqwest::get(format!("{}/health", first.base_url))
        .await
        .is_err());
    std::fs::write(&discovery, b"stale pid/instance").unwrap();
    let second = server.start().await.unwrap();
    assert_ne!(first.instance_id, second.instance_id);
    server.stop().await.unwrap();
}
#[tokio::test]
async fn auth_revocation_origin_and_host() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let vault = Arc::new(TestVault::default());
    let clients = ClientService::new(pool.clone(), vault.clone());
    let token = clients
        .create(
            "internal reader",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(7),
        )
        .await
        .unwrap();
    assert_eq!(token.token.len(), 64);
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    let discovery = server.start().await.unwrap();
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{}/meetings", discovery.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{}/health", discovery.base_url))
            .header("Origin", "https://evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{}/health", discovery.base_url))
            .header("Host", "attacker.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let health: serde_json::Value = http
        .get(format!("{}/health", discovery.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["instance_id"], discovery.instance_id);
    assert!(!health.to_string().contains("export"));
    let file = std::fs::read_to_string(folder.path().join("automation/discovery.json")).unwrap();
    assert!(!file.contains(&token.token));
    clients.verify(&token.token, Scope::Read).await.unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Record)
            .await
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    sqlx::query("UPDATE automation_clients SET expires_at=0 WHERE id=?")
        .bind(&token.client.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Read)
            .await
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    sqlx::query("UPDATE automation_clients SET expires_at=? WHERE id=?")
        .bind(chrono::Utc::now().timestamp() + 3600)
        .bind(&token.client.id)
        .execute(&pool)
        .await
        .unwrap();
    clients.revoke(&token.client.id).await.unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Read)
            .await
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert!(vault.secrets.lock().unwrap().is_empty());
    server.stop().await.unwrap();
}
#[tokio::test]
async fn credential_failure_never_creates_a_client() {
    let pool = pool().await;
    let clients = ClientService::new(
        pool.clone(),
        Arc::new(TestVault {
            fail: true,
            ..Default::default()
        }),
    );
    assert!(clients
        .create(
            "fail",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1)
        )
        .await
        .is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_clients")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn failed_credential_readback_cleans_the_orphan_secret() {
    let pool = pool().await;
    let vault = Arc::new(TestVault {
        read_fail: true,
        ..Default::default()
    });
    let clients = ClientService::new(pool.clone(), vault.clone());
    assert!(clients
        .create(
            "readback",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1)
        )
        .await
        .is_err());
    assert!(vault.secrets.lock().unwrap().is_empty());
}
#[tokio::test]
async fn rate_body_limits_and_other_instances_are_rejected() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let token = clients
        .create(
            "limits",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    let discovery = server.start().await.unwrap();
    let other = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    assert_eq!(other.start().await.unwrap_err().code, "BUSY");
    let http = reqwest::Client::new();
    let large = http
        .get(format!("{}/unregistered-endpoint", discovery.base_url))
        .bearer_auth(&token.token)
        .body(vec![0u8; 1024 * 1024 + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(large.status(), 422);
    for _ in 0..59 {
        let reply = http
            .get(format!("{}/unregistered-endpoint", discovery.base_url))
            .bearer_auth(&token.token)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 404);
    }
    assert_eq!(
        http.get(format!("{}/unregistered-endpoint", discovery.base_url))
            .bearer_auth(&token.token)
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn only_explicit_enable_is_restored_after_restart() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    server.restore_preference().await.unwrap();
    assert!(server.status().await.is_none());
    server.set_enabled(true).await.unwrap();
    let first = server.status().await.unwrap();
    server.stop().await.unwrap();
    let restarted = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    restarted.restore_preference().await.unwrap();
    assert_ne!(
        first.instance_id,
        restarted.status().await.unwrap().instance_id
    );
    restarted.set_enabled(false).await.unwrap();
    restarted.restore_preference().await.unwrap();
    assert!(restarted.status().await.is_none());
}

#[tokio::test]
async fn pagination_is_stable_and_search_is_bounded() {
    use super::read_service::{MeetingReadService, PageRequest};
    let pool = pool().await;
    for id in ["a", "b", "c"] {
        crate::meeting::tests::seed(&pool, id).await;
    }
    sqlx::query("UPDATE meetings SET created_at='2026-10-01T00:00:00Z'")
        .execute(&pool)
        .await
        .unwrap();
    let service = MeetingReadService::new(pool.clone());
    let first = service
        .list(PageRequest {
            limit: Some(1),
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(first.items[0].id, "a");
    sqlx::query("DELETE FROM meetings WHERE id='a'")
        .execute(&pool)
        .await
        .unwrap();
    let second = service
        .list(PageRequest {
            limit: Some(1),
            cursor: first.next_cursor,
        })
        .await
        .unwrap();
    assert_eq!(second.items[0].id, "b");
    for i in 0..251 {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time,audio_end_time,timing_precision) VALUES(?, 'b', ?, '10:00', ?, ?, ?)")
            .bind(format!("seg{i:03}"))
            .bind(if i==250{"最后行动项 100%_literal".to_string()}else{"中文".repeat(200)})
            .bind(if i<250{Some((i/2) as f64)}else{None})
            .bind(if i<250{Some((i/2) as f64+1.0)}else{None})
            .bind(if i<250 {"approximate"} else {"unavailable"})
            .execute(&pool).await.unwrap();
    }
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = service
            .transcript(
                "b",
                PageRequest {
                    limit: Some(50),
                    cursor,
                },
            )
            .await
            .unwrap();
        if ids.is_empty() {
            assert_eq!(page.items[0].timing_precision, "approximate");
        }
        ids.extend(page.items.iter().map(|s| s.id.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 251);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        251
    );
    assert_eq!(ids.last().unwrap(), "seg250");
    assert!(service
        .search(&"q".repeat(257), PageRequest::default())
        .await
        .is_err());
    assert!(service.search("", PageRequest::default()).await.is_err());
    assert!(service
        .list(PageRequest {
            limit: Some(201),
            cursor: None
        })
        .await
        .is_err());
    let hits = service
        .search("%_literal", PageRequest::default())
        .await
        .unwrap();
    assert_eq!(hits.items.len(), 1);
    assert!(hits.items[0].snippet.contains("最后行动项"));
    assert_eq!(hits.items[0].start_ms, None);
    let hits = service
        .search("中文", PageRequest::default())
        .await
        .unwrap();
    assert!(hits.items.iter().all(|h| h.snippet.chars().count() <= 300));
}
#[tokio::test]
async fn read_scope_cannot_read_keys_audio_or_paths() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    crate::meeting::tests::seed(&pool, "bounded").await;
    sqlx::query("UPDATE meetings SET folder_path='/private/secret-recordings'")
        .execute(&pool)
        .await
        .unwrap();
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let token = clients
        .create(
            "reader",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    let server = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    let discovery = server.start().await.unwrap();
    let http = reqwest::Client::new();
    let reply = http
        .get(format!("{}/meetings", discovery.base_url))
        .bearer_auth(&token.token)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    let body = reply.text().await.unwrap();
    assert!(!body.contains("secret-recordings"));
    assert!(!body.contains(&token.token));
    for path in [
        "settings",
        "keys",
        "audio",
        "exports",
        "exports/id/download",
    ] {
        assert_eq!(
            http.get(format!("{}/{path}", discovery.base_url))
                .bearer_auth(&token.token)
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    let summary_write = http
        .post(format!("{}/summary-jobs", discovery.base_url))
        .bearer_auth(&token.token)
        .json(&serde_json::json!({"meeting_id":"bounded","expected_revision":0,"model":"builtin-ai","model_name":"local"}))
        .send()
        .await
        .unwrap();
    assert_eq!(summary_write.status(), 403);
    let recording_write = http
        .post(format!("{}/recordings/start", discovery.base_url))
        .bearer_auth(&token.token)
        .json(&serde_json::json!({"meeting_name":"test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(recording_write.status(), 403);
    let reply = http
        .get(format!("{}/meetings?limit=999", discovery.base_url))
        .bearer_auth(&token.token)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 422);
    let body: serde_json::Value = reply.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_INPUT");
    assert!(body["request_id"].is_string());
    assert_eq!(
        http.get(format!("{}/search?q=first&q=second", discovery.base_url))
            .bearer_auth(&token.token)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn transcript_cursor_is_bound_to_transcript_revision() {
    use super::read_service::{MeetingReadService, PageRequest};
    let pool = pool().await;
    crate::meeting::tests::seed(&pool, "versioned").await;
    sqlx::query("INSERT INTO meeting_content_versions(meeting_id) VALUES('versioned')")
        .execute(&pool)
        .await
        .unwrap();
    for (id, start) in [("one", 1.0), ("two", 2.0)] {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time) VALUES(?, 'versioned', ?, ?, ?)")
            .bind(id)
            .bind(id)
            .bind(id)
            .bind(start)
            .execute(&pool)
            .await
            .unwrap();
    }
    let service = MeetingReadService::new(pool.clone());
    let first = service
        .transcript(
            "versioned",
            PageRequest {
                limit: Some(1),
                cursor: None,
            },
        )
        .await
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    sqlx::query(
        "UPDATE meeting_content_versions SET transcript_revision=1 WHERE meeting_id='versioned'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let error = service
        .transcript(
            "versioned",
            PageRequest {
                limit: Some(1),
                cursor: Some(cursor),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "CONFLICT");
}
