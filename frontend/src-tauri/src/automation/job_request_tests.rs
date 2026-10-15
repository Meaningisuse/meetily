//! HTTP-waiter ownership regressions. These exercise the route's orchestration
//! with real SQLite jobs and a controlled worker, never an inference provider.
use super::server::{cancel_job_with_action, create_summary_job_with_action};
use crate::{
    jobs::{JobKind, JobService},
    meeting::tests::{pool, seed},
    summary::commands::ProcessTranscriptResponse,
};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

async fn client(pool: &SqlitePool) {
    sqlx::query("INSERT INTO automation_clients(id,name,token_hash,scopes_json,secret_ref,expires_at,created_at) VALUES('job-request-client','job lifecycle test',zeroblob(32),'[\"write\"]','test-job-secret',?,?)")
        .bind(chrono::Utc::now().timestamp() + 3600)
        .bind(chrono::Utc::now().timestamp())
        .execute(pool).await.unwrap();
}

fn headers(key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-key", key.parse().unwrap());
    headers
}

async fn saved_response(pool: &SqlitePool, key: &str) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let saved: Option<String> = sqlx::query_scalar(
                "SELECT response_json FROM automation_requests WHERE client_id='job-request-client' AND key=?",
            )
            .bind(key).fetch_one(pool).await.unwrap();
            if let Some(saved) = saved {
                return serde_json::from_str(&saved).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.ok()
}

async fn assert_reserved(pool: &SqlitePool, key: &str) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_requests WHERE client_id='job-request-client' AND key=? AND response_json IS NULL",
    ).bind(key).fetch_one(pool).await.unwrap();
    assert_eq!(
        count, 1,
        "the accepted action must hold a persisted reservation"
    );
}

#[tokio::test]
async fn accepted_summary_keeps_attach_worker_and_response_after_http_waiter_is_aborted() {
    let pool = pool().await;
    seed(&pool, "accepted-summary").await;
    client(&pool).await;
    let jobs = JobService::new(pool.clone());
    let payload = json!({
        "meeting_id":"accepted-summary", "expected_revision":0,
        "model":"test-provider", "model_name":"controlled-worker",
        "custom_prompt":null, "template_id":null, "summary_language":null,
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let task_calls = calls.clone();
    let task_pool = pool.clone();
    let task_jobs = jobs.clone();
    let task_payload = payload.clone();
    let (enqueued_tx, enqueued_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (worker_tx, worker_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        create_summary_job_with_action(
            task_pool,
            "job-request-client".into(),
            headers("accepted-summary"),
            task_payload,
            move || async move {
                task_calls.fetch_add(1, Ordering::SeqCst);
                let job = task_jobs
                    .enqueue(
                        JobKind::Summary,
                        "accepted-summary",
                        0,
                        json!({"provider":"test-provider","model":"controlled-worker"}),
                    )
                    .await?;
                let started_at = chrono::Utc::now();
                let process_id = started_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
                enqueued_tx
                    .send((job.id.clone(), process_id.clone()))
                    .unwrap();
                release_rx.await.unwrap();
                task_jobs
                    .attach_summary(
                        &job.id,
                        started_at,
                        "Controlled transcript",
                        "test-provider",
                        "controlled-worker",
                        40000,
                        1000,
                    )
                    .await?;
                let worker_jobs = task_jobs.clone();
                let worker_id = job.id.clone();
                tokio::spawn(async move {
                    let lease = worker_jobs.start(&worker_id).await.unwrap();
                    // A synthetic provider failure is sufficient to prove that
                    // the admitted job has a running owner and reaches a terminal state.
                    worker_jobs
                        .fail(&worker_id, "TEST_PROVIDER_FAILURE")
                        .await
                        .unwrap();
                    drop(lease);
                    worker_jobs.release_context(&worker_id);
                    let _ = worker_tx.send(());
                });
                Ok(ProcessTranscriptResponse {
                    message: "Summary job queued".into(),
                    process_id,
                    job,
                })
            },
        )
        .await
    });
    let (job_id, process_id) = tokio::time::timeout(Duration::from_secs(5), enqueued_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(jobs.get(&job_id).await.unwrap().status, "queued");
    assert_reserved(&pool, "accepted-summary").await;
    let duplicate = create_summary_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        headers("accepted-summary"),
        payload.clone(),
        || async {
            panic!("a pending duplicate must not start another summary");
        },
    )
    .await
    .err()
    .expect("a pending duplicate must return BUSY");
    assert_eq!(duplicate.0.code, "BUSY");
    let mut changed_payload = payload.clone();
    changed_payload["model_name"] = json!("different-model");
    let conflict = create_summary_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        headers("accepted-summary"),
        changed_payload,
        || async {
            panic!("a conflicting body must not start another summary");
        },
    )
    .await
    .err()
    .expect("a changed summary request must conflict");
    assert_eq!(conflict.0.code, "CONFLICT");
    waiter.abort();
    assert!(waiter.await.is_err());
    let _ = release_tx.send(());
    let finished = tokio::time::timeout(Duration::from_secs(5), worker_rx).await;
    assert!(matches!(finished, Ok(Ok(()))), "an accepted summary must finish attachment and spawn its worker after the HTTP waiter is dropped");
    let job = jobs.get(&job_id).await.unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.error_code.as_deref(), Some("TEST_PROVIDER_FAILURE"));
    let attached = crate::jobs::repository::attempt(
        &pool,
        "accepted-summary",
        chrono::DateTime::parse_from_rfc3339(&process_id)
            .unwrap()
            .with_timezone(&chrono::Utc),
    )
    .await
    .unwrap()
    .expect("the actual queued job must retain its attempt identity");
    assert_eq!(attached.id, job_id);
    let saved = saved_response(&pool, "accepted-summary")
        .await
        .expect("the admitted summary response must be persisted after the waiter is dropped");
    assert_eq!(saved["process_id"], process_id);
    assert_eq!(saved["job"]["id"], job_id);
    let replay_calls = calls.clone();
    let replay = create_summary_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        headers("accepted-summary"),
        payload,
        move || async move {
            replay_calls.fetch_add(1, Ordering::SeqCst);
            panic!("replay must not enqueue a second summary job");
        },
    )
    .await
    .unwrap_or_else(|error| panic!("summary replay failed: {}", error.0));
    assert_eq!(replay.0, saved);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM enhancement_jobs WHERE meeting_id='accepted-summary'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, 1);
}

#[tokio::test]
async fn committed_job_cancel_persists_and_replays_after_http_waiter_is_aborted() {
    let pool = pool().await;
    seed(&pool, "cancelled-job").await;
    client(&pool).await;
    let jobs = JobService::new(pool.clone());
    let job = jobs
        .enqueue(JobKind::Summary, "cancelled-job", 0, json!({}))
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let task_calls = calls.clone();
    let task_pool = pool.clone();
    let task_jobs = jobs.clone();
    let job_id = job.id.clone();
    let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        cancel_job_with_action(
            task_pool,
            "job-request-client".into(),
            job_id,
            headers("committed-cancel"),
            move |id| async move {
                task_calls.fetch_add(1, Ordering::SeqCst);
                let cancelled = task_jobs.cancel(&id).await?;
                cancelled_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(cancelled)
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), cancelled_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(jobs.get(&job.id).await.unwrap().status, "cancelled");
    assert_reserved(&pool, "committed-cancel").await;
    let duplicate = cancel_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        job.id.clone(),
        headers("committed-cancel"),
        |_| async {
            panic!("a pending duplicate must not cancel another job");
        },
    )
    .await
    .err()
    .expect("a pending duplicate cancellation must return BUSY");
    assert_eq!(duplicate.0.code, "BUSY");
    let conflict = cancel_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        "different-job".into(),
        headers("committed-cancel"),
        |_| async {
            panic!("a conflicting job id must not reach cancellation");
        },
    )
    .await
    .err()
    .expect("a changed cancellation target must conflict");
    assert_eq!(conflict.0.code, "CONFLICT");
    waiter.abort();
    assert!(waiter.await.is_err());
    let _ = release_tx.send(());
    let saved = saved_response(&pool, "committed-cancel")
        .await
        .expect("the completed cancellation must persist its response after the waiter is dropped");
    assert_eq!(saved["id"], job.id);
    assert_eq!(saved["status"], "cancelled");
    assert_eq!(saved["error_code"], "CANCELLED");
    let replay_calls = calls.clone();
    let replay = cancel_job_with_action(
        pool.clone(),
        "job-request-client".into(),
        job.id.clone(),
        headers("committed-cancel"),
        move |_| async move {
            replay_calls.fetch_add(1, Ordering::SeqCst);
            panic!("replay must not execute another job cancellation");
        },
    )
    .await
    .unwrap_or_else(|error| panic!("cancellation replay failed: {}", error.0));
    assert_eq!(replay.0, saved);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let final_job = jobs.get(&job.id).await.unwrap();
    assert_eq!(final_job.status, "cancelled");
    assert_eq!(final_job.error_code.as_deref(), Some("CANCELLED"));
}

#[tokio::test]
async fn missing_job_cancel_cleans_reservation_and_same_key_retry_reenters_real_action() {
    let pool = pool().await;
    client(&pool).await;
    let jobs = JobService::new(pool.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    for expected_calls in 1..=2 {
        let action_jobs = jobs.clone();
        let action_calls = calls.clone();
        let result = cancel_job_with_action(
            pool.clone(),
            "job-request-client".into(),
            "missing-job".into(),
            headers("missing-cancel"),
            move |id| async move {
                action_calls.fetch_add(1, Ordering::SeqCst);
                action_jobs.cancel(&id).await
            },
        )
        .await;
        let error = result
            .err()
            .expect("a missing job must return its ordinary error");
        assert_eq!(error.0.code, "NOT_FOUND");
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        let reserved: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM automation_requests WHERE client_id='job-request-client' AND key='missing-cancel'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(
            reserved, 0,
            "an ordinary error must release the reservation for a retry"
        );
    }
    let jobs_created: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM enhancement_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs_created, 0);
}
