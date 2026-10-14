use crate::database::models::SummaryProcess;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::info as log_info;

pub struct SummaryProcessesRepository;

impl SummaryProcessesRepository {
    /// Retrieves the current summary process state for a given meeting ID.
    pub async fn get_summary_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>("SELECT * FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn update_meeting_summary(
        pool: &SqlitePool,
        meeting_id: &str,
        summary: &Value,
    ) -> Result<bool, sqlx::Error> {
        if sqlx::query("SELECT id FROM meetings WHERE id=?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM meeting_content_versions WHERE meeting_id=?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;
        crate::meeting::MeetingWriteService::new(pool.clone())
            .save_summary_value(meeting_id, summary, revision.unwrap_or(0))
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        Ok(true)
    }

    pub async fn get_summary_data_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>(
            "SELECT p.*,COALESCE(v.revision,0) AS content_revision FROM summary_processes p LEFT JOIN meeting_content_versions v ON v.meeting_id=p.meeting_id WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn create_or_reset_process(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        log_info!(
            "Creating or resetting summary process for meeting_id: {}",
            meeting_id
        );
        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL
            "#
        )
        .bind(meeting_id)
        .bind(started_at)
        .bind(started_at)
        .bind(started_at)
        .execute(pool)
        .await?;
        log_info!(
            "Backed up existing summary before regeneration for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_completed(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
        result: Value,
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now();
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        let job: Option<(String,i64,String)> = sqlx::query_as("SELECT j.id,j.source_revision,j.status FROM enhancement_jobs j JOIN summary_job_context c ON c.job_id=j.id WHERE c.meeting_id=? AND c.started_at=?")
            .bind(meeting_id).bind(started_at).fetch_optional(&mut *transaction).await?;
        if let Some((id, source_revision, status)) = &job {
            if status != "running" {
                return Ok(false);
            }
            let current =
                crate::database::repositories::content_version::read(&mut transaction, meeting_id)
                    .await?;
            if current.revision != *source_revision {
                sqlx::query("INSERT INTO enhancement_job_results(job_id,source_revision,result_json,stale,created_at) VALUES(?,?,?,1,?)")
                    .bind(id).bind(source_revision).bind(&result_str).bind(now).execute(&mut *transaction).await?;
                Self::terminate_in_transaction(&mut transaction, meeting_id, started_at, "failed", "CONFLICT", "Meeting content changed during generation. The generated attempt is retained as stale; current content is preserved.").await?;
                transaction.commit().await?;
                return Ok(false);
            }
        }
        let update = sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ? AND start_time = ? AND LOWER(status) = 'pending'
            "#
        )
        .bind(&result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .bind(started_at)
        .execute(&mut *transaction)
        .await?;
        let mut completion_revision = None;
        if update.rows_affected() == 1 {
            let versions = crate::database::repositories::content_version::bump(
                &mut transaction,
                meeting_id,
                crate::database::repositories::content_version::ContentKind::Summary,
            )
            .await?;
            completion_revision = Some(versions.revision);
            let markdown = crate::meeting::normalize::summary_markdown(&result_str);
            sqlx::query("INSERT INTO meeting_document_content(meeting_id,summary_markdown,summary_source_transcript_revision,summary_source_speaker_revision) VALUES(?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET summary_markdown=excluded.summary_markdown,summary_source_transcript_revision=excluded.summary_source_transcript_revision,summary_source_speaker_revision=excluded.summary_source_speaker_revision")
                .bind(meeting_id).bind(markdown).bind(versions.transcript_revision).bind(versions.speaker_revision).execute(&mut *transaction).await?;
        }
        if update.rows_affected() == 1 {
            if let Some((id, source_revision, _)) = &job {
                sqlx::query("INSERT INTO enhancement_job_results(job_id,source_revision,result_json,stale,created_at) VALUES(?,?,?,0,?)")
                    .bind(id).bind(source_revision).bind(&result_str).bind(now).execute(&mut *transaction).await?;
                sqlx::query("UPDATE enhancement_jobs SET status='succeeded',progress=1,error_code=NULL,updated_at=? WHERE id=? AND status='running'")
                    .bind(now).bind(id).execute(&mut *transaction).await?;
                if let Some(revision) = completion_revision {
                    crate::automation::events::append(
                        &mut transaction,
                        &crate::automation::events::DomainEvent::new(
                            meeting_id,
                            "summary.completed",
                            revision,
                            Some(id),
                        ),
                    )
                    .await?;
                }
            }
            // Generated titles share the completion transaction and source check.
            if let Some(name) = result
                .get("generated_title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.chars().count() <= 256)
            {
                let changed =
                    sqlx::query("UPDATE meetings SET title=?,updated_at=? WHERE id=? AND title!=?")
                        .bind(name)
                        .bind(now)
                        .bind(meeting_id)
                        .bind(name)
                        .execute(&mut *transaction)
                        .await?
                        .rows_affected();
                if changed == 1 {
                    sqlx::query("UPDATE transcript_chunks SET meeting_name=? WHERE meeting_id=?")
                        .bind(name)
                        .bind(meeting_id)
                        .execute(&mut *transaction)
                        .await?;
                    crate::database::repositories::content_version::bump(
                        &mut transaction,
                        meeting_id,
                        crate::database::repositories::content_version::ContentKind::Title,
                    )
                    .await?;
                }
            }
        }
        transaction.commit().await?;
        Ok(update.rows_affected() == 1)
    }

    pub async fn update_process_failed(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
        error: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = Self::terminate_in_transaction(
            &mut tx,
            meeting_id,
            started_at,
            "failed",
            "PROVIDER_ERROR",
            error,
        )
        .await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = Self::terminate_in_transaction(
            &mut tx,
            meeting_id,
            started_at,
            "cancelled",
            "CANCELLED",
            "Generation was cancelled by user",
        )
        .await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub(crate) async fn terminate_in_transaction(
        conn: &mut sqlx::SqliteConnection,
        meeting_id: &str,
        started_at: DateTime<Utc>,
        status: &str,
        code: &str,
        message: &str,
    ) -> Result<bool, sqlx::Error> {
        let job: Option<(String,i64)> = sqlx::query_as("SELECT j.id,j.source_revision FROM enhancement_jobs j JOIN summary_job_context c ON c.job_id=j.id WHERE c.meeting_id=? AND c.started_at=?")
            .bind(meeting_id).bind(started_at).fetch_optional(&mut *conn).await?;
        let restore = match &job {
            Some((_, revision)) => {
                crate::database::repositories::content_version::read(conn, meeting_id)
                    .await?
                    .revision
                    == *revision
            }
            None => true,
        };
        let now = Utc::now();
        let update = sqlx::query("UPDATE summary_processes SET status=?,error=?,updated_at=?,end_time=?,result=CASE WHEN ? THEN COALESCE(result_backup,result) ELSE result END,result_backup=NULL,result_backup_timestamp=NULL WHERE meeting_id=? AND start_time=? AND LOWER(status)='pending'")
            .bind(status).bind(message).bind(now).bind(now).bind(restore).bind(meeting_id).bind(started_at).execute(&mut *conn).await?;
        if let Some((id, _)) = job {
            sqlx::query("UPDATE enhancement_jobs SET status=?,error_code=?,updated_at=? WHERE id=? AND status IN ('queued','running')")
                .bind(status).bind(code).bind(now).bind(&id).execute(&mut *conn).await?;
            if status == "failed" && update.rows_affected() == 1 {
                let revision =
                    crate::database::repositories::content_version::read(conn, meeting_id)
                        .await?
                        .revision;
                crate::automation::events::append(
                    conn,
                    &crate::automation::events::DomainEvent::new(
                        meeting_id,
                        "summary.failed",
                        revision,
                        Some(&id),
                    ),
                )
                .await?;
            }
        }
        Ok(update.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn test_pool() -> SqlitePool {
        let pool = crate::meeting::tests::pool().await;
        pool
    }

    async fn seed_pending(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
        result: Option<&str>,
        result_backup: Option<&str>,
    ) {
        crate::meeting::tests::seed(pool, meeting_id).await;
        sqlx::query(
            "INSERT INTO summary_processes (
                meeting_id, status, created_at, updated_at, start_time, result, result_backup
            ) VALUES (?, 'PENDING', ?, ?, ?, ?, ?)",
        )
        .bind(meeting_id)
        .bind(started_at)
        .bind(started_at)
        .bind(started_at)
        .bind(result)
        .bind(result_backup)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_running_summary_job(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
        job_id: &str,
    ) {
        seed_pending(pool, meeting_id, started_at, None, None).await;
        let mut tx = pool.begin().await.unwrap();
        crate::database::repositories::content_version::initialize(&mut tx, meeting_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("INSERT INTO enhancement_jobs(id,meeting_id,kind,status,source_revision,params_json,created_at,updated_at) VALUES(?,?,'summary','running',0,'{}',?,?)")
            .bind(job_id).bind(meeting_id).bind(started_at).bind(started_at)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO summary_job_context(meeting_id,started_at,job_id) VALUES(?,?,?)")
            .bind(meeting_id)
            .bind(started_at)
            .bind(job_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn summary_result_and_event_commit_or_rollback_together() {
        let pool = test_pool().await;
        let started = Utc::now();
        seed_running_summary_job(&pool, "event-completion", started, "event-job").await;
        sqlx::query("INSERT INTO automation_webhooks(id,url,secret_ref,allowed_hosts_json,kinds_json,enabled,created_at) VALUES('broken-filter','https://example.test/hook','secret','[]','{',1,?)")
            .bind(started.timestamp()).execute(&pool).await.unwrap();

        let result = SummaryProcessesRepository::update_process_completed(
            &pool,
            "event-completion",
            started,
            json!({"markdown":"complete"}),
            1,
            1.0,
        )
        .await;
        assert!(result.is_err());
        let status: String = sqlx::query_scalar(
            "SELECT status FROM summary_processes WHERE meeting_id='event-completion'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let job_status: String =
            sqlx::query_scalar("SELECT status FROM enhancement_jobs WHERE id='event-job'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM automation_events WHERE meeting_id='event-completion'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "PENDING");
        assert_eq!(job_status, "running");
        assert_eq!(events, 0);

        sqlx::query("DELETE FROM automation_webhooks WHERE id='broken-filter'")
            .execute(&pool)
            .await
            .unwrap();
        let before_retry: (String, String, i64) = sqlx::query_as("SELECT s.status,j.status,j.source_revision FROM summary_processes s JOIN summary_job_context c ON c.meeting_id=s.meeting_id JOIN enhancement_jobs j ON j.id=c.job_id WHERE s.meeting_id='event-completion'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(before_retry.0, "PENDING");
        assert_eq!(before_retry.1, "running");
        assert_eq!(before_retry.2, 0);
        assert!(SummaryProcessesRepository::update_process_completed(
            &pool,
            "event-completion",
            started,
            json!({"markdown":"complete"}),
            1,
            1.0,
        )
        .await
        .unwrap());
        let event: (String, String) = sqlx::query_as(
            "SELECT kind,job_id FROM automation_events WHERE meeting_id='event-completion'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event, ("summary.completed".into(), "event-job".into()));
    }

    #[tokio::test]
    async fn terminal_compare_and_set_preserves_first_writer() {
        let pool = test_pool().await;
        let completed_start = Utc::now();
        seed_pending(&pool, "completed-first", completed_start, None, None).await;
        assert!(SummaryProcessesRepository::update_process_completed(
            &pool,
            "completed-first",
            completed_start,
            json!({"markdown": "completed"}),
            1,
            1.0,
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::update_process_cancelled(
            &pool,
            "completed-first",
            completed_start,
        )
        .await
        .unwrap());
        let completed_status: String = sqlx::query_scalar(
            "SELECT status FROM summary_processes WHERE meeting_id = 'completed-first'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(completed_status, "completed");

        let cancelled_start = completed_start + chrono::Duration::nanoseconds(1);
        let previous = r#"{"markdown":"previous"}"#;
        seed_pending(
            &pool,
            "cancelled-first",
            cancelled_start,
            Some(previous),
            Some(previous),
        )
        .await;
        assert!(SummaryProcessesRepository::update_process_cancelled(
            &pool,
            "cancelled-first",
            cancelled_start,
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::update_process_completed(
            &pool,
            "cancelled-first",
            cancelled_start,
            json!({"markdown": "completed"}),
            1,
            1.0,
        )
        .await
        .unwrap());
        let cancelled: (String, String) = sqlx::query_as(
            "SELECT status, result FROM summary_processes WHERE meeting_id = 'cancelled-first'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cancelled, ("cancelled".to_string(), previous.to_string()));
    }

    #[tokio::test]
    async fn cancelled_compare_and_set_rejects_stale_start_time() {
        let pool = test_pool().await;
        let current_start = Utc::now();
        seed_pending(&pool, "stale-cancel", current_start, None, None).await;
        assert!(!SummaryProcessesRepository::update_process_cancelled(
            &pool,
            "stale-cancel",
            current_start - chrono::Duration::nanoseconds(1),
        )
        .await
        .unwrap());
        let status: String = sqlx::query_scalar(
            "SELECT status FROM summary_processes WHERE meeting_id = 'stale-cancel'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "PENDING");
    }
}
