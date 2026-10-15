use crate::meeting::EnhancementError;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::future::Future;

/// Own the entire accepted operation, including its idempotency reservation and
/// result write. Dropping the HTTP waiter must not cancel recording finalization.
/// Process shutdown and panics still require the existing recovery workflow.
pub(crate) async fn run_detached<T, F, Fut>(
    pool: &SqlitePool,
    client_id: &str,
    key: &str,
    request: &Value,
    action: F,
) -> Result<T, EnhancementError>
where
    T: Serialize + DeserializeOwned + Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, EnhancementError>> + Send + 'static,
{
    let pool = pool.clone();
    let client_id = client_id.to_owned();
    let key = key.to_owned();
    let request = request.clone();
    // Spawn before the first database await so cancellation cannot leave a
    // committed reservation without an owner. A dropped JoinHandle detaches;
    // this task deliberately does not share the gateway's shutdown token.
    tokio::spawn(async move { run(&pool, &client_id, &key, &request, action).await })
        .await
        .map_err(|_| {
            EnhancementError::new("IO_ERROR", "The local operation did not complete normally.")
        })?
}

pub async fn run<T, F, Fut>(
    pool: &SqlitePool,
    client_id: &str,
    key: &str,
    request: &Value,
    action: F,
) -> Result<T, EnhancementError>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, EnhancementError>>,
{
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err(EnhancementError::invalid(
            "Idempotency-Key must be 1 to 128 safe ASCII characters.",
        ));
    }
    let body = serde_json::to_vec(request)
        .map_err(|_| EnhancementError::invalid("Request could not be normalized."))?;
    let digest = format!("{:x}", Sha256::digest(body));
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM automation_requests WHERE expires_at<=?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let existing: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT request_hash,response_json FROM automation_requests WHERE client_id=? AND key=?",
    )
    .bind(client_id)
    .bind(key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((prior_hash, response)) = existing {
        tx.commit().await?;
        if prior_hash != digest {
            return Err(EnhancementError::new(
                "CONFLICT",
                "Idempotency key was already used with a different request.",
            ));
        }
        return match response {
            Some(response) => serde_json::from_str(&response)
                .map_err(|_| EnhancementError::new("IO_ERROR", "Saved request result is invalid.")),
            None => Err(EnhancementError::new(
                "BUSY",
                "A request with this idempotency key is still being finalized.",
            )),
        };
    }
    sqlx::query("INSERT INTO automation_requests(client_id,key,request_hash,response_json,expires_at,created_at) VALUES(?,?,?,NULL,?,?)")
        .bind(client_id).bind(key).bind(&digest).bind(now+86400).bind(now)
        .execute(&mut *tx).await?;
    tx.commit().await?;

    let result = action().await;
    match result {
        Ok(value) => {
            let encoded = serde_json::to_string(&value).map_err(|_| {
                EnhancementError::new("IO_ERROR", "Request result could not be saved.")
            })?;
            if encoded.len() > 64 * 1024 {
                return Err(EnhancementError::new(
                    "INVALID_INPUT",
                    "Request result exceeds the idempotency storage limit.",
                ));
            }
            sqlx::query("UPDATE automation_requests SET response_json=? WHERE client_id=? AND key=? AND request_hash=? AND response_json IS NULL")
                .bind(encoded).bind(client_id).bind(key).bind(digest).execute(pool).await?;
            Ok(value)
        }
        Err(error) => {
            sqlx::query("DELETE FROM automation_requests WHERE client_id=? AND key=? AND response_json IS NULL")
                .bind(client_id).bind(key).execute(pool).await?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{run, run_detached};
    use crate::meeting::tests::pool;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn client(pool: &sqlx::SqlitePool) {
        sqlx::query("INSERT INTO automation_clients(id,name,token_hash,scopes_json,secret_ref,expires_at,created_at) VALUES('idem-client','test',zeroblob(32),'[]','idem-secret',?,?)")
            .bind(chrono::Utc::now().timestamp()+3600).bind(chrono::Utc::now().timestamp())
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_request_returns_saved_result_without_repeating_action() {
        let pool = pool().await;
        client(&pool).await;
        let calls = AtomicUsize::new(0);
        let first: serde_json::Value = run(
            &pool,
            "idem-client",
            "request-1",
            &json!({"meeting_id":"m"}),
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"job_id":"job-1"}))
            },
        )
        .await
        .unwrap();
        let second: serde_json::Value = run(
            &pool,
            "idem-client",
            "request-1",
            &json!({"meeting_id":"m"}),
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"job_id":"job-2"}))
            },
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let (hash,body):(String,Option<String>)=sqlx::query_as("SELECT request_hash,response_json FROM automation_requests WHERE client_id='idem-client'").fetch_one(&pool).await.unwrap();
        assert_eq!(hash.len(), 64);
        assert!(body.unwrap().contains("job-1"));
        assert!(!hash.contains("meeting_id"));
    }

    #[tokio::test]
    async fn same_key_with_a_different_body_conflicts_before_action() {
        let pool = pool().await;
        client(&pool).await;
        run::<serde_json::Value, _, _>(
            &pool,
            "idem-client",
            "request-2",
            &json!({"meeting_id":"one"}),
            || async { Ok(json!({"ok":true})) },
        )
        .await
        .unwrap();
        let calls = AtomicUsize::new(0);
        let error = run::<serde_json::Value, _, _>(
            &pool,
            "idem-client",
            "request-2",
            &json!({"meeting_id":"two"}),
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "CONFLICT");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    async fn saved_response(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT response_json FROM automation_requests WHERE client_id='idem-client' AND key=?",
        )
        .bind(key)
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
    }

    async fn wait_for_response(pool: &sqlx::SqlitePool, key: &str) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(response) = saved_response(pool, key).await {
                    return serde_json::from_str(&response).unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("accepted stop must persist its result after its waiter is cancelled")
    }

    #[tokio::test]
    async fn detached_stop_finishes_and_replays_after_request_is_cancelled() {
        let pool = pool().await;
        client(&pool).await;
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task_pool = pool.clone();
        let task_calls = calls.clone();
        let waiter = tokio::spawn(async move {
            run_detached(
                &task_pool,
                "idem-client",
                "cancelled-stop",
                &json!({"session_id":"one"}),
                move || async move {
                    task_calls.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                    Ok(json!({"session_id":"one","status":"stopped"}))
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let _ = finish_tx.send(());
        let saved = wait_for_response(&pool, "cancelled-stop").await;
        let replay_calls = calls.clone();
        let replay = run_detached(
            &pool,
            "idem-client",
            "cancelled-stop",
            &json!({"session_id":"one"}),
            move || async move {
                replay_calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"unexpected":"duplicate stop"}))
            },
        )
        .await
        .unwrap();
        assert_eq!(saved, json!({"session_id":"one","status":"stopped"}));
        assert_eq!(replay, saved);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn detached_stop_keeps_in_flight_idempotency_and_body_guards() {
        let pool = pool().await;
        client(&pool).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task_pool = pool.clone();
        let waiter = tokio::spawn(async move {
            run_detached(
                &task_pool,
                "idem-client",
                "pending-stop",
                &json!({"session_id":"one"}),
                move || async move {
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                    Ok(json!({"status":"stopped"}))
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        for (session_id, expected) in [("one", "BUSY"), ("two", "CONFLICT")] {
            let error = run_detached::<serde_json::Value, _, _>(
                &pool,
                "idem-client",
                "pending-stop",
                &json!({"session_id":session_id}),
                || async {
                    panic!("a duplicate or conflicting stop must not execute");
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, expected);
        }
        finish_tx.send(()).unwrap();
        assert_eq!(waiter.await.unwrap().unwrap(), json!({"status":"stopped"}));
    }

    #[tokio::test]
    async fn detached_stop_cleans_normal_error_after_waiter_is_cancelled() {
        let pool = pool().await;
        client(&pool).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task_pool = pool.clone();
        let waiter = tokio::spawn(async move {
            run_detached::<serde_json::Value, _, _>(
                &task_pool,
                "idem-client",
                "failed-stop",
                &json!({}),
                move || async move {
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                    Err(crate::meeting::EnhancementError::new(
                        "CONFLICT",
                        "Session no longer active",
                    ))
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let _ = finish_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM automation_requests WHERE key='failed-stop'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                if count == 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("normal stop errors must finish reservation cleanup");
        let retry = run_detached(&pool, "idem-client", "failed-stop", &json!({}), || async {
            Ok(json!({"retried":true}))
        })
        .await
        .unwrap();
        assert_eq!(retry, json!({"retried":true}));
    }

    #[tokio::test]
    async fn detached_stop_panic_retains_reservation_and_returns_controlled_error() {
        let pool = pool().await;
        client(&pool).await;
        let error = run_detached::<serde_json::Value, _, _>(
            &pool,
            "idem-client",
            "panicked-stop",
            &json!({}),
            || async {
                panic!("private native stop diagnostic");
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "IO_ERROR");
        assert!(!error.message.contains("private native stop diagnostic"));
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_requests WHERE key='panicked-stop' AND response_json IS NULL")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(pending, 1);
        let retry = run_detached::<serde_json::Value, _, _>(
            &pool,
            "idem-client",
            "panicked-stop",
            &json!({}),
            || async {
                panic!("must not repeat a stop after a panic");
            },
        )
        .await
        .unwrap_err();
        assert_eq!(retry.code, "BUSY");
    }

    #[tokio::test]
    async fn detached_stop_does_not_repeat_side_effect_if_result_write_fails() {
        let pool = pool().await;
        client(&pool).await;
        sqlx::query("CREATE TRIGGER reject_stop_result BEFORE UPDATE OF response_json ON automation_requests WHEN NEW.key='write-failure-stop' BEGIN SELECT RAISE(FAIL, 'injected result write failure'); END")
            .execute(&pool).await.unwrap();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let action_calls = calls.clone();
        let error = run_detached(
            &pool,
            "idem-client",
            "write-failure-stop",
            &json!({}),
            move || async move {
                action_calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"status":"stopped"}))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "IO_ERROR");
        let retry = run_detached::<serde_json::Value, _, _>(
            &pool,
            "idem-client",
            "write-failure-stop",
            &json!({}),
            || async {
                panic!("a saved stop must not repeat when its response write failed");
            },
        )
        .await
        .unwrap_err();
        assert_eq!(retry.code, "BUSY");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
