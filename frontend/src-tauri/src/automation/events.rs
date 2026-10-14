use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub event_id: String,
    pub meeting_id: String,
    pub kind: String,
    pub revision: i64,
    pub job_id: Option<String>,
    pub occurred_at: String,
}

impl DomainEvent {
    pub fn new(meeting_id: &str, kind: &str, revision: i64, job_id: Option<&str>) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            meeting_id: meeting_id.to_owned(),
            kind: kind.to_owned(),
            revision,
            job_id: job_id.map(str::to_owned),
            occurred_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub async fn append(conn: &mut SqliteConnection, event: &DomainEvent) -> Result<(), sqlx::Error> {
    let payload = serde_json::to_string(event)
        .map_err(|error| sqlx::Error::Protocol(format!("event serialization failed: {error}")))?;
    sqlx::query("INSERT INTO automation_events(id,meeting_id,kind,revision,job_id,occurred_at,payload_json) VALUES(?,?,?,?,?,?,?)")
        .bind(&event.event_id)
        .bind(&event.meeting_id)
        .bind(&event.kind)
        .bind(event.revision)
        .bind(&event.job_id)
        .bind(&event.occurred_at)
        .bind(payload)
        .execute(&mut *conn)
        .await?;

    let webhooks: Vec<(String, String)> =
        sqlx::query_as("SELECT id,kinds_json FROM automation_webhooks WHERE enabled=1 ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    for (webhook_id, kinds_json) in webhooks {
        let kinds: Vec<String> = serde_json::from_str(&kinds_json).map_err(|error| {
            sqlx::Error::Protocol(format!("invalid webhook event filter: {error}"))
        })?;
        if kinds.iter().any(|kind| kind == &event.kind) {
            sqlx::query("INSERT INTO automation_deliveries(id,event_id,webhook_id,attempts,next_attempt_at,status) VALUES(?,?,?,0,?,'pending')")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&event.event_id)
                .bind(webhook_id)
                .bind(chrono::Utc::now().timestamp())
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{append, DomainEvent};

    #[tokio::test]
    async fn event_and_delivery_follow_the_business_transaction() {
        let pool = crate::meeting::tests::pool().await;
        crate::meeting::tests::seed(&pool, "event-meeting").await;
        sqlx::query("INSERT INTO automation_webhooks(id,url,secret_ref,allowed_hosts_json,kinds_json,enabled,created_at) VALUES('hook-one','https://example.test/hook','secret','[]','[\"summary.completed\"]',1,?)")
            .bind(chrono::Utc::now().timestamp()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO automation_webhooks(id,url,secret_ref,allowed_hosts_json,kinds_json,enabled,created_at) VALUES('hook-two','https://example.org/hook','secret-2','[]','[\"summary.completed\"]',1,?)")
            .bind(chrono::Utc::now().timestamp()).execute(&pool).await.unwrap();

        let event = DomainEvent::new("event-meeting", "summary.completed", 2, Some("job-1"));
        let mut tx = pool.begin().await.unwrap();
        append(&mut tx, &event).await.unwrap();
        let inside: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_events WHERE id=?")
            .bind(&event.event_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(inside, 1);
        tx.rollback().await.unwrap();

        let rolled_back: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM automation_events WHERE id=?")
                .bind(&event.event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let rolled_back_deliveries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM automation_deliveries WHERE event_id=?")
                .bind(&event.event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rolled_back, 0);
        assert_eq!(rolled_back_deliveries, 0);

        let mut tx = pool.begin().await.unwrap();
        append(&mut tx, &event).await.unwrap();
        tx.commit().await.unwrap();
        let committed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_deliveries WHERE event_id=? AND webhook_id IN ('hook-one','hook-two') AND status='pending'")
            .bind(&event.event_id).fetch_one(&pool).await.unwrap();
        assert_eq!(committed, 2);
        let stored_payload: String =
            sqlx::query_scalar("SELECT payload_json FROM automation_events WHERE id=?")
                .bind(&event.event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<DomainEvent>(&stored_payload)
                .unwrap()
                .event_id,
            event.event_id
        );
    }
}
