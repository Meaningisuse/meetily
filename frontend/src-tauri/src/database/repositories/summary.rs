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
        let mut transaction = pool.begin().await?;
        let now = Utc::now();
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

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
        if update.rows_affected() == 1 {
            let versions = crate::database::repositories::content_version::bump(
                &mut transaction,
                meeting_id,
                crate::database::repositories::content_version::ContentKind::Summary,
            )
            .await?;
            let markdown = crate::meeting::normalize::summary_markdown(&result_str);
            sqlx::query("INSERT INTO meeting_document_content(meeting_id,summary_markdown,summary_source_transcript_revision,summary_source_speaker_revision) VALUES(?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET summary_markdown=excluded.summary_markdown,summary_source_transcript_revision=excluded.summary_source_transcript_revision,summary_source_speaker_revision=excluded.summary_source_speaker_revision")
                .bind(meeting_id).bind(markdown).bind(versions.transcript_revision).bind(versions.speaker_revision).execute(&mut *transaction).await?;
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
        let now = Utc::now();

        let update = sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'failed',
                error = ?,
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ? AND start_time = ? AND LOWER(status) = 'pending'
            "#,
        )
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .bind(started_at)
        .execute(pool)
        .await?;
        Ok(update.rows_affected() == 1)
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let now = Utc::now();

        let update = sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'cancelled',
                updated_at = ?,
                end_time = ?,
                error = 'Generation was cancelled by user',
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ? AND start_time = ? AND LOWER(status) = 'pending'
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .bind(started_at)
        .execute(pool)
        .await?;
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
