use crate::meeting::EnhancementError;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::future::Future;

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
    use super::run;
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
}
