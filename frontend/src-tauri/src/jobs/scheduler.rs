use super::{cancellation::JobContext, repository, JobKind, JobSnapshot};
use crate::{database::repositories::content_version, meeting::EnhancementError};
use dashmap::DashMap;
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct JobService {
    pool: SqlitePool,
    slot: Arc<Semaphore>,
    resources: crate::resources::ResourceScheduler,
    cancellations: Arc<DashMap<String, CancellationToken>>,
}
pub struct JobLease {
    pub context: JobContext,
    _permit: OwnedSemaphorePermit,
    pub(crate) local: Option<crate::resources::ResourceLease>,
}
impl JobService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            slot: Arc::new(Semaphore::new(1)),
            resources: crate::resources::ResourceScheduler::new(),
            cancellations: Arc::new(DashMap::new()),
        }
    }
    pub fn resources(&self) -> crate::resources::ResourceScheduler {
        self.resources.clone()
    }
    pub(crate) fn try_audio_recovery_slot(
        &self,
    ) -> Result<(OwnedSemaphorePermit, crate::resources::ResourceLease), EnhancementError> {
        let permit = self
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| EnhancementError::new("BUSY", "Another heavy job is running."))?;
        let local = self
            .resources
            .try_local()
            .map_err(|e| EnhancementError::new("BUSY", &e))?;
        Ok((permit, local))
    }
    pub async fn enqueue(
        &self,
        kind: JobKind,
        meeting_id: &str,
        source_revision: i64,
        params: Value,
    ) -> Result<JobSnapshot, EnhancementError> {
        if meeting_id.trim().is_empty() || source_revision < 0 {
            return Err(EnhancementError::invalid(
                "meeting_id and a nonnegative source_revision are required.",
            ));
        }
        let params = params.to_string();
        if params.len() > 1024 * 1024 {
            return Err(EnhancementError::invalid("Job parameters exceed 1 MiB."));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        content_version::initialize(&mut tx, meeting_id).await?;
        if content_version::read(&mut tx, meeting_id).await?.revision != source_revision {
            return Err(EnhancementError::conflict());
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM enhancement_jobs WHERE status='queued'")
                .fetch_one(&mut *tx)
                .await?;
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM enhancement_jobs WHERE meeting_id=? AND kind=? AND status IN ('queued','running')").bind(meeting_id).bind(kind.as_str()).fetch_one(&mut *tx).await?;
        if count >= 16 || active > 0 {
            return Err(EnhancementError::new(
                "BUSY",
                "The job queue is full or this meeting already has an active job.",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO enhancement_jobs(id,meeting_id,kind,status,source_revision,params_json,created_at,updated_at) VALUES(?,?,?,'queued',?,?,?,?)")
            .bind(&id).bind(meeting_id).bind(kind.as_str()).bind(source_revision).bind(params).bind(now).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        self.cancellations
            .insert(id.clone(), CancellationToken::new());
        self.get(&id).await
    }
    /// Freeze the legacy projection and attempt identity atomically, before a
    /// queued task can start. No inference or provider request happens here.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach_summary(
        &self,
        id: &str,
        started_at: chrono::DateTime<chrono::Utc>,
        text: &str,
        provider: &str,
        model: &str,
        chunk_size: i32,
        overlap: i32,
    ) -> Result<(), EnhancementError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let (meeting_id, source_revision, status): (String,i64,String) = sqlx::query_as("SELECT meeting_id,source_revision,status FROM enhancement_jobs WHERE id=? AND kind='summary'").bind(id).fetch_one(&mut *tx).await?;
        if status != "queued" {
            return Err(EnhancementError::new(
                "NOT_READY",
                "Job is no longer queued.",
            ));
        }
        if content_version::read(&mut tx, &meeting_id).await?.revision != source_revision {
            return Err(EnhancementError::conflict());
        }
        sqlx::query("INSERT INTO summary_job_context(meeting_id,started_at,job_id) VALUES(?,?,?)")
            .bind(&meeting_id)
            .bind(started_at)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO summary_processes(meeting_id,status,created_at,updated_at,start_time) VALUES(?,'PENDING',?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET status='PENDING',updated_at=excluded.updated_at,start_time=excluded.start_time,result_backup=summary_processes.result,result_backup_timestamp=excluded.updated_at,error=NULL,end_time=NULL")
            .bind(&meeting_id).bind(started_at).bind(started_at).bind(started_at).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO transcript_chunks(meeting_id,transcript_text,model,model_name,chunk_size,overlap,created_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET transcript_text=excluded.transcript_text,model=excluded.model,model_name=excluded.model_name,chunk_size=excluded.chunk_size,overlap=excluded.overlap,created_at=excluded.created_at")
            .bind(&meeting_id).bind(text).bind(provider).bind(model).bind(chunk_size).bind(overlap).bind(started_at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub fn cancellation(&self, id: &str) -> CancellationToken {
        self.cancellations
            .entry(id.to_owned())
            .or_insert_with(CancellationToken::new)
            .clone()
    }
    pub async fn start(&self, id: &str) -> Result<JobLease, EnhancementError> {
        let token = self.cancellation(id);
        let permit = tokio::select! {
            _=token.cancelled()=>return Err(EnhancementError::new("CANCELLED","Job was cancelled.")),
            permit=self.slot.clone().acquire_owned()=>permit.map_err(|_|EnhancementError::new("NOT_READY","Scheduler is closed."))?,
        };
        let params: String =
            sqlx::query_scalar("SELECT params_json FROM enhancement_jobs WHERE id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let params: Value = serde_json::from_str(&params).unwrap_or(Value::Null);
        let local = if crate::resources::summary_uses_local_resources(
            params["provider"].as_str().unwrap_or(""),
        ) {
            Some(tokio::select! {
                biased;
                _=token.cancelled()=>return Err(EnhancementError::new("CANCELLED","Job was cancelled.")),
                local=self.resources.local()=>local.map_err(|e|EnhancementError::new("BUSY",&e))?,
            })
        } else {
            None
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let status: String = sqlx::query_scalar("SELECT status FROM enhancement_jobs WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        if status == "cancelled" || token.is_cancelled() {
            return Err(EnhancementError::new("CANCELLED", "Job was cancelled."));
        }
        if status != "queued" {
            return Err(EnhancementError::new(
                "NOT_READY",
                "Job is no longer queued.",
            ));
        }
        let (meeting_id, source_revision): (String, i64) =
            sqlx::query_as("SELECT meeting_id,source_revision FROM enhancement_jobs WHERE id=?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if content_version::read(&mut tx, &meeting_id).await?.revision != source_revision {
            let attempt: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT started_at FROM summary_job_context WHERE job_id=?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if let Some(started) = attempt {
                crate::database::repositories::summary::SummaryProcessesRepository::terminate_in_transaction(&mut tx,&meeting_id,started,"failed","CONFLICT","Meeting changed while this task was queued. Generate again from current content.").await?;
            }
            sqlx::query("UPDATE enhancement_jobs SET status='failed',error_code='CONFLICT',updated_at=? WHERE id=? AND status='queued'").bind(chrono::Utc::now()).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(EnhancementError::conflict());
        }
        let running: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM enhancement_jobs WHERE status='running'")
                .fetch_one(&mut *tx)
                .await?;
        if running > 0 {
            return Err(EnhancementError::new(
                "BUSY",
                "Another heavy job is running.",
            ));
        }
        sqlx::query("UPDATE enhancement_jobs SET status='running',updated_at=? WHERE id=? AND status='queued'").bind(chrono::Utc::now()).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(JobLease {
            context: JobContext {
                job_id: id.into(),
                cancellation: token,
            },
            _permit: permit,
            local,
        })
    }
    pub async fn get(&self, id: &str) -> Result<JobSnapshot, EnhancementError> {
        repository::get(&self.pool, id).await
    }
    pub async fn cancel(&self, id: &str) -> Result<JobSnapshot, EnhancementError> {
        self.get(id).await?;
        // Cancel the future as soon as possible; queued/running are the only
        // mutable states. Terminal jobs retain their result.
        if let Some(token) = self.cancellations.get(id) {
            token.cancel();
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE enhancement_jobs SET status='cancelled',error_code='CANCELLED',updated_at=? WHERE id=? AND status IN ('queued','running')")
            .bind(chrono::Utc::now()).bind(id).execute(&mut *tx).await?;
        let attempt: Option<(String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT meeting_id,started_at FROM summary_job_context WHERE job_id=?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((meeting_id, started_at)) = attempt {
            crate::database::repositories::summary::SummaryProcessesRepository::terminate_in_transaction(&mut tx, &meeting_id, started_at, "cancelled", "CANCELLED", "Generation was cancelled by user").await?;
        }
        tx.commit().await?;
        self.get(id).await
    }
    pub async fn fail(&self, id: &str, code: &str) -> Result<(), EnhancementError> {
        sqlx::query("UPDATE enhancement_jobs SET status='failed',error_code=?,updated_at=? WHERE id=? AND status IN ('queued','running')")
            .bind(code).bind(chrono::Utc::now()).bind(id).execute(&self.pool).await?;
        self.cancellations.remove(id);
        Ok(())
    }
    pub fn release_context(&self, id: &str) {
        self.cancellations.remove(id);
    }
    pub async fn recover_interrupted(&self) -> Result<u64, EnhancementError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let count=sqlx::query("UPDATE enhancement_jobs SET status='interrupted',error_code='RESTARTED',updated_at=? WHERE status IN ('queued','running')")
            .bind(chrono::Utc::now()).execute(&mut *tx).await?.rows_affected();
        sqlx::query("UPDATE summary_processes SET status='failed',error='Application restarted; the previous summary is preserved. Resubmit this job explicitly.' WHERE LOWER(status)='pending'")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(count)
    }
}
