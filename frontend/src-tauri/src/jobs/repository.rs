use super::JobSnapshot;
use crate::meeting::EnhancementError;
use sqlx::SqlitePool;

pub async fn get(pool: &SqlitePool, id: &str) -> Result<JobSnapshot, EnhancementError> {
    Ok(sqlx::query_as("SELECT id,meeting_id,kind,status,source_revision,progress,error_code,created_at,updated_at FROM enhancement_jobs WHERE id=?").bind(id).fetch_one(pool).await?)
}
pub async fn attempt(
    pool: &SqlitePool,
    meeting_id: &str,
    started_at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<JobSnapshot>, sqlx::Error> {
    sqlx::query_as("SELECT j.id,j.meeting_id,j.kind,j.status,j.source_revision,j.progress,j.error_code,j.created_at,j.updated_at FROM enhancement_jobs j JOIN summary_job_context c ON c.job_id=j.id WHERE c.meeting_id=? AND c.started_at=?")
        .bind(meeting_id).bind(started_at).fetch_optional(pool).await
}
