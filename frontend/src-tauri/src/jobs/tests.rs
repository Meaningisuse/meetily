use super::{JobKind, JobService};
use crate::meeting::tests::{pool, seed};
use serde_json::json;

#[tokio::test]
async fn one_heavy_job_and_bounded_queue() {
    let pool = pool().await;
    for i in 0..19 {
        seed(&pool, &format!("m{i}")).await;
    }
    let jobs = JobService::new(pool);
    let first = jobs
        .enqueue(JobKind::Summary, "m0", 0, json!({}))
        .await
        .unwrap();
    let first_lease = jobs.start(&first.id).await.unwrap();
    let mut queued = Vec::new();
    for i in 1..17 {
        queued.push(
            jobs.enqueue(JobKind::Summary, &format!("m{i}"), 0, json!({}))
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        jobs.enqueue(JobKind::Summary, "m17", 0, json!({}))
            .await
            .unwrap_err()
            .code,
        "BUSY"
    );
    let next = queued[0].id.clone();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), jobs.start(&next))
            .await
            .is_err()
    );
    jobs.cancel(&first.id).await.unwrap();
    assert!(first_lease.context.cancellation.is_cancelled());
    drop(first_lease);
    let next_lease = jobs.start(&next).await.unwrap();
    assert_eq!(jobs.get(&next).await.unwrap().status, "running");
    drop(next_lease);
}

#[tokio::test]
async fn concurrent_enqueue_cannot_exceed_queue_limit() {
    let directory = tempfile::tempdir().unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(directory.path().join("concurrent.sqlite"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    for i in 0..24 {
        seed(&pool, &format!("c{i}")).await;
    }
    let jobs = JobService::new(pool);
    let mut requests = Vec::new();
    for i in 0..24 {
        let jobs = jobs.clone();
        requests.push(tokio::spawn(async move {
            jobs.enqueue(JobKind::Diarization, &format!("c{i}"), 0, json!({}))
                .await
        }));
    }
    let mut admitted = 0;
    for request in requests {
        match request.await.unwrap() {
            Ok(_) => admitted += 1,
            Err(error) => assert_eq!(error.code, "BUSY"),
        }
    }
    assert_eq!(admitted, 16);
}

#[tokio::test]
async fn restart_interrupts_running_without_auto_retry() {
    let pool = pool().await;
    seed(&pool, "restart").await;
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(
            JobKind::Summary,
            "restart",
            0,
            json!({"provider":"test-must-not-call"}),
        )
        .await
        .unwrap();
    let lease = jobs.start(&job.id).await.unwrap();
    drop(lease);
    let restarted = JobService::new(pool);
    assert_eq!(restarted.recover_interrupted().await.unwrap(), 1);
    assert_eq!(restarted.get(&job.id).await.unwrap().status, "interrupted");
    assert_eq!(
        restarted.get(&job.id).await.unwrap().error_code.as_deref(),
        Some("RESTARTED")
    );
    assert!(restarted.start(&job.id).await.is_err());
}

#[tokio::test]
async fn queued_cancel_does_not_run_and_retains_other_jobs() {
    let pool = pool().await;
    seed(&pool, "cancel-a").await;
    seed(&pool, "cancel-b").await;
    let jobs = JobService::new(pool);
    let a = jobs
        .enqueue(JobKind::Summary, "cancel-a", 0, json!({}))
        .await
        .unwrap();
    let b = jobs
        .enqueue(JobKind::Summary, "cancel-b", 0, json!({}))
        .await
        .unwrap();
    jobs.cancel(&a.id).await.unwrap();
    assert_eq!(jobs.start(&a.id).await.err().unwrap().code, "CANCELLED");
    assert_eq!(jobs.get(&b.id).await.unwrap().status, "queued");
}

#[tokio::test]
async fn source_change_does_not_overwrite_new_summary() {
    let pool = pool().await;
    seed(&pool, "source-race").await;
    let writes = crate::meeting::MeetingWriteService::new(pool.clone());
    writes
        .save_summary_markdown("source-race", "Previous summary", 0)
        .await
        .unwrap();
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "source-race", 1, json!({}))
        .await
        .unwrap();
    let started = chrono::Utc::now();
    jobs.attach_summary(
        &job.id,
        started,
        "Full transcript",
        "ollama",
        "model",
        4000,
        0,
    )
    .await
    .unwrap();
    let _lease = jobs.start(&job.id).await.unwrap();
    writes
        .save_summary_markdown("source-race", "New human edit", 1)
        .await
        .unwrap();
    assert!(!crate::database::repositories::summary::SummaryProcessesRepository::update_process_completed(&pool, "source-race", started, json!({"markdown":"Old generated answer"}), 1, 0.1).await.unwrap());
    let document = crate::meeting::MeetingDocumentService::new(pool.clone())
        .snapshot("source-race")
        .await
        .unwrap();
    assert_eq!(document.summary_markdown.as_deref(), Some("New human edit"));
    let job = jobs.get(&job.id).await.unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.error_code.as_deref(), Some("CONFLICT"));
    let archived: (String, i64) =
        sqlx::query_as("SELECT result_json,stale FROM enhancement_job_results WHERE job_id=?")
            .bind(&job.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(archived.0.contains("Old generated answer"));
    assert_eq!(archived.1, 1);
}

#[tokio::test]
async fn job_result_and_success_commit_together() {
    let pool = pool().await;
    seed(&pool, "success").await;
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "success", 0, json!({}))
        .await
        .unwrap();
    let started = chrono::Utc::now();
    jobs.attach_summary(&job.id, started, "Transcript", "ollama", "model", 4000, 0)
        .await
        .unwrap();
    let _lease = jobs.start(&job.id).await.unwrap();
    assert!(crate::database::repositories::summary::SummaryProcessesRepository::update_process_completed(&pool, "success", started, json!({"markdown":"Generated answer"}), 1, 0.1).await.unwrap());
    assert_eq!(jobs.get(&job.id).await.unwrap().status, "succeeded");
    let stale: i64 = sqlx::query_scalar("SELECT stale FROM enhancement_job_results WHERE job_id=?")
        .bind(&job.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stale, 0);
}

#[tokio::test]
async fn cancellation_preserves_later_edit_and_blocks_late_completion() {
    let pool = pool().await;
    seed(&pool, "cancel-edit").await;
    let writes = crate::meeting::MeetingWriteService::new(pool.clone());
    writes
        .save_summary_markdown("cancel-edit", "Before", 0)
        .await
        .unwrap();
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "cancel-edit", 1, json!({}))
        .await
        .unwrap();
    let started = chrono::Utc::now();
    jobs.attach_summary(&job.id, started, "Transcript", "ollama", "model", 4000, 0)
        .await
        .unwrap();
    let _lease = jobs.start(&job.id).await.unwrap();
    writes
        .save_summary_markdown("cancel-edit", "Keep this edit", 1)
        .await
        .unwrap();
    jobs.cancel(&job.id).await.unwrap();
    assert!(!crate::database::repositories::summary::SummaryProcessesRepository::update_process_completed(&pool, "cancel-edit", started, json!({"markdown":"Late answer"}), 1, 0.1).await.unwrap());
    let result: String =
        sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id='cancel-edit'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(result.contains("Keep this edit"));
    assert_eq!(jobs.get(&job.id).await.unwrap().status, "cancelled");
}

#[tokio::test]
async fn queued_source_change_fails_before_inference() {
    let pool = pool().await;
    seed(&pool, "queued-change").await;
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "queued-change", 0, json!({}))
        .await
        .unwrap();
    let started = chrono::Utc::now();
    jobs.attach_summary(
        &job.id,
        started,
        "Original text",
        "ollama",
        "model",
        4000,
        0,
    )
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    crate::database::repositories::content_version::bump(
        &mut conn,
        "queued-change",
        crate::database::repositories::content_version::ContentKind::Speaker,
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(jobs.start(&job.id).await.err().unwrap().code, "CONFLICT");
    assert_eq!(jobs.get(&job.id).await.unwrap().status, "failed");
}

#[tokio::test]
async fn restart_projects_pending_summary_and_preserves_result() {
    let pool = pool().await;
    seed(&pool, "restart-result").await;
    crate::meeting::MeetingWriteService::new(pool.clone())
        .save_summary_markdown("restart-result", "Saved summary", 0)
        .await
        .unwrap();
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "restart-result", 1, json!({}))
        .await
        .unwrap();
    let started = chrono::Utc::now();
    jobs.attach_summary(&job.id, started, "Transcript", "ollama", "model", 4000, 0)
        .await
        .unwrap();
    let restarted = JobService::new(pool.clone());
    assert_eq!(restarted.recover_interrupted().await.unwrap(), 1);
    let row: (String, String) = sqlx::query_as(
        "SELECT status,result FROM summary_processes WHERE meeting_id='restart-result'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "failed");
    assert!(row.1.contains("Saved summary"));
    assert_eq!(restarted.get(&job.id).await.unwrap().status, "interrupted");
}

#[tokio::test]
async fn saved_audio_recovery_uses_the_shared_heavy_slot() {
    let pool = crate::meeting::tests::pool().await;
    crate::meeting::tests::seed(&pool, "recovery").await;
    let jobs = super::JobService::new(pool);
    let permit = jobs.try_audio_recovery_slot().unwrap();
    assert!(jobs.try_audio_recovery_slot().is_err());
    let queued = jobs
        .enqueue(
            super::JobKind::Summary,
            "recovery",
            0,
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), jobs.start(&queued.id))
            .await
            .is_err()
    );
    drop(permit);
    let lease = jobs.start(&queued.id).await.unwrap();
    assert!(jobs.try_audio_recovery_slot().is_err());
    drop(lease);
}
