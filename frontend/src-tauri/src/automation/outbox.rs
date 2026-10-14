use super::{
    auth::{SystemVault, Vault},
    signing, target_policy,
};
use sqlx::SqlitePool;
use std::time::Duration;

const BACKOFF_SECONDS: [i64; 5] = [10, 60, 300, 1_800, 7_200];

#[derive(Debug)]
struct Delivery {
    id: String,
    event_id: String,
    attempts: i64,
    payload: String,
    url: String,
    secret_ref: String,
    allowed_hosts_json: String,
}

fn retry_after(attempts: i64, now: i64) -> Option<i64> {
    BACKOFF_SECONDS
        .get(attempts.saturating_sub(1) as usize)
        .map(|delay| now + delay)
}

async fn claim(pool: &SqlitePool) -> Result<Option<Delivery>, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = sqlx::query_as::<_, (String, String, i64, String, String, String, String)>(
        "SELECT d.id,d.event_id,d.attempts,e.payload_json,w.url,w.secret_ref,w.allowed_hosts_json FROM automation_deliveries d JOIN automation_events e ON e.id=d.event_id JOIN automation_webhooks w ON w.id=d.webhook_id WHERE d.status='pending' AND d.next_attempt_at<=? AND w.enabled=1 ORDER BY d.next_attempt_at,d.id LIMIT 1",
    )
    .bind(chrono::Utc::now().timestamp())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, event_id, attempts, payload, url, secret_ref, allowed_hosts_json)) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let updated = sqlx::query(
        "UPDATE automation_deliveries SET status='delivering' WHERE id=? AND status='pending'",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    Ok(Some(Delivery {
        id,
        event_id,
        attempts,
        payload,
        url,
        secret_ref,
        allowed_hosts_json,
    }))
}

async fn secret(vault: std::sync::Arc<dyn Vault>, id: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || vault.get(&id))
        .await
        .map_err(|_| "Keychain task failed".to_owned())?
}

async fn deliver(delivery: &Delivery, secret: &str) -> Result<u16, String> {
    let approved: Vec<String> = serde_json::from_str(&delivery.allowed_hosts_json)
        .map_err(|_| "Webhook allowlist is invalid".to_owned())?;
    let target = target_policy::resolve_and_validate(&delivery.url, &approved).await?;
    let client = http_client(&target)?;
    let timestamp = chrono::Utc::now().timestamp();
    let response = client
        .post(target.url)
        .header("content-type", "application/json")
        .header("x-meetily-event", &delivery.event_id)
        .header("x-meetily-timestamp", timestamp.to_string())
        .header(
            "x-meetily-signature",
            format!(
                "sha256={}",
                signing::signature(secret, timestamp, delivery.payload.as_bytes())
            ),
        )
        .body(delivery.payload.clone())
        .send()
        .await
        .map_err(|_| "Webhook request failed".to_owned())?;
    let status = response.status().as_u16();
    let mut body = response.bytes_stream();
    use futures_util::StreamExt;
    let mut size = 0usize;
    while let Some(chunk) = body.next().await {
        let Ok(chunk) = chunk else { break };
        size = size.saturating_add(chunk.len());
        if size > 16 * 1024 {
            break;
        }
    }
    Ok(status)
}

fn http_client(target: &target_policy::PinnedTarget) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .resolve_to_addrs(&target.host, &target.addresses)
        .build()
        .map_err(|_| "Webhook client could not be configured".to_owned())
}

async fn finish(
    pool: &SqlitePool,
    delivery: &Delivery,
    status: Option<u16>,
) -> Result<(), sqlx::Error> {
    let attempts = delivery.attempts + 1;
    let now = chrono::Utc::now().timestamp();
    match status {
        Some(status) if (200..300).contains(&status) => {
            sqlx::query("UPDATE automation_deliveries SET attempts=?,last_status=?,status='succeeded' WHERE id=? AND status='delivering'")
                .bind(attempts).bind(status as i64).bind(&delivery.id).execute(pool).await?;
        }
        Some(status) if !(status == 429 || status >= 500) => {
            sqlx::query("UPDATE automation_deliveries SET attempts=?,last_status=?,status='failed' WHERE id=? AND status='delivering'")
                .bind(attempts).bind(status as i64).bind(&delivery.id).execute(pool).await?;
        }
        _ => {
            let next = retry_after(attempts, now);
            let state = if next.is_some() { "pending" } else { "failed" };
            sqlx::query("UPDATE automation_deliveries SET attempts=?,last_status=?,next_attempt_at=COALESCE(?,next_attempt_at),status=? WHERE id=? AND status='delivering'")
                .bind(attempts).bind(status.map(i64::from)).bind(next).bind(state).bind(&delivery.id).execute(pool).await?;
        }
    }
    Ok(())
}

/// Attempts one due delivery. Returns false when there is no due work.
pub async fn deliver_one(
    pool: &SqlitePool,
    vault: std::sync::Arc<dyn Vault>,
) -> Result<bool, sqlx::Error> {
    let Some(delivery) = claim(pool).await? else {
        return Ok(false);
    };
    let outcome = match secret(vault, delivery.secret_ref.clone()).await {
        Ok(secret) => deliver(&delivery, &secret).await,
        Err(_) => Err("Webhook secret is unavailable".into()),
    };
    // Response bodies and request secrets are intentionally never persisted or logged.
    let status = outcome.ok();
    finish(pool, &delivery, status).await?;
    Ok(true)
}

pub async fn deliver_due(pool: SqlitePool, cancel: tokio_util::sync::CancellationToken) {
    let vault: std::sync::Arc<dyn Vault> = std::sync::Arc::new(SystemVault);
    let _ =
        sqlx::query("UPDATE automation_deliveries SET status='pending' WHERE status='delivering'")
            .execute(&pool)
            .await;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let enabled =
            sqlx::query_scalar::<_, i64>("SELECT enabled FROM automation_config WHERE id=1")
                .fetch_one(&pool)
                .await
                .unwrap_or(0)
                == 1;
        if enabled {
            match deliver_one(&pool, vault.clone()).await {
                Ok(true) => continue,
                Ok(false) | Err(_) => {}
            }
        }
        tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
    }
}

#[cfg(test)]
mod tests {
    use super::{finish, retry_after, Delivery};

    #[test]
    fn retries_initial_delivery_five_times_with_fixed_backoff() {
        let now = 10_000;
        assert_eq!(retry_after(1, now), Some(now + 10));
        assert_eq!(retry_after(2, now), Some(now + 60));
        assert_eq!(retry_after(3, now), Some(now + 300));
        assert_eq!(retry_after(4, now), Some(now + 1_800));
        assert_eq!(retry_after(5, now), Some(now + 7_200));
        assert_eq!(retry_after(6, now), None);
    }

    #[tokio::test]
    async fn response_policy_retries_transient_status_and_terminates_other_4xx() {
        let pool = crate::meeting::tests::pool().await;
        crate::meeting::tests::seed(&pool, "delivery-policy").await;
        sqlx::query("INSERT INTO automation_webhooks(id,url,secret_ref,allowed_hosts_json,kinds_json,enabled,created_at) VALUES('delivery-hook','https://example.test/hook','secret','[\"example.test\"]','[\"summary.completed\"]',1,?)")
            .bind(chrono::Utc::now().timestamp()).execute(&pool).await.unwrap();
        let event =
            super::super::events::DomainEvent::new("delivery-policy", "summary.completed", 1, None);
        let payload = serde_json::to_string(&event).unwrap();
        sqlx::query("INSERT INTO automation_events(id,meeting_id,kind,revision,job_id,occurred_at,payload_json) VALUES(?,?,?,1,NULL,?,?)")
            .bind(&event.event_id).bind(&event.meeting_id).bind(&event.kind).bind(&event.occurred_at).bind(&payload).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO automation_deliveries(id,event_id,webhook_id,attempts,next_attempt_at,status) VALUES('delivery-one',?,'delivery-hook',0,?,'delivering')")
            .bind(&event.event_id).bind(chrono::Utc::now().timestamp()).execute(&pool).await.unwrap();
        let delivery = Delivery {
            id: "delivery-one".into(),
            event_id: event.event_id.clone(),
            attempts: 0,
            payload,
            url: "https://example.test/hook".into(),
            secret_ref: "secret".into(),
            allowed_hosts_json: "[\"example.test\"]".into(),
        };

        finish(&pool, &delivery, Some(503)).await.unwrap();
        let state: (i64, i64, String) = sqlx::query_as("SELECT attempts,next_attempt_at,status FROM automation_deliveries WHERE id='delivery-one'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(state.0, 1);
        assert_eq!(state.2, "pending");
        assert!(state.1 >= chrono::Utc::now().timestamp() + 9);

        sqlx::query("UPDATE automation_deliveries SET status='delivering' WHERE id='delivery-one'")
            .execute(&pool)
            .await
            .unwrap();
        let retry = Delivery {
            attempts: 1,
            ..delivery
        };
        finish(&pool, &retry, Some(401)).await.unwrap();
        let terminal: (i64, i64, String) = sqlx::query_as(
            "SELECT attempts,last_status,status FROM automation_deliveries WHERE id='delivery-one'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(terminal, (2, 401, "failed".into()));
        let stored: String =
            sqlx::query_scalar("SELECT payload_json FROM automation_events WHERE id=?")
                .bind(event.event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, retry.payload);
    }

    #[tokio::test]
    async fn webhook_http_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let url = url::Url::parse(&format!("http://127.0.0.1:{}/hook", address.port())).unwrap();
        let target = crate::automation::target_policy::PinnedTarget {
            url,
            host: "127.0.0.1".into(),
            addresses: vec![address],
        };
        let response = super::http_client(&target)
            .unwrap()
            .post(target.url)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 302);
        server.await.unwrap();
    }
}
