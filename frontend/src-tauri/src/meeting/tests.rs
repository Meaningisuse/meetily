use super::{MeetingDocumentService, MeetingWriteService};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

pub(crate) async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

pub(crate) async fn seed(pool: &SqlitePool, id: &str) {
    sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?,'中文会议','2026-10-14T00:00:00Z','2026-10-14T00:00:00Z')")
        .bind(id).execute(pool).await.unwrap();
}

#[tokio::test]
async fn snapshot_reads_all_pages_and_legacy_content() {
    let pool = pool().await;
    seed(&pool, "legacy").await;
    for i in 0..251 {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,speaker) VALUES (?,'legacy',?,'2026-10-14T00:00:00Z','mic')")
            .bind(format!("s{i:03}")).bind(format!("第{i}句")).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO summary_processes(meeting_id,status,created_at,updated_at,result) VALUES ('legacy','completed','2026-10-14T00:00:00Z','2026-10-14T00:00:00Z',?)")
        .bind(r#"{"_section_order":["actions"],"actions":{"title":"行动项","blocks":[{"content":"检查尾段"}]}}"#).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO meeting_notes VALUES ('legacy','原笔记','[{\"type\":\"paragraph\"}]','2026-10-14T00:00:00Z','2026-10-14T00:00:00Z')").execute(&pool).await.unwrap();
    let service = MeetingDocumentService::new(pool.clone());
    let document = service.snapshot("legacy").await.unwrap();
    assert_eq!(document.transcript.len(), 251);
    assert_eq!(document.transcript.last().unwrap().text, "第250句");
    assert_eq!(
        document.summary_markdown.as_deref(),
        Some("## 行动项\n\n- 检查尾段")
    );
    assert_eq!(document.notes_markdown.as_deref(), Some("原笔记"));
    for segment in &document.transcript {
        assert_eq!(segment.start_ms, None);
        assert_eq!(segment.end_ms, None);
        assert_eq!(segment.speaker_id, None);
        assert_eq!(segment.speaker_name, None);
        assert_eq!(segment.assignment, "unknown");
    }
    sqlx::query("UPDATE summary_processes SET result=? WHERE meeting_id='legacy'")
        .bind(serde_json::json!({"markdown":"## 行动项\n\n- 检查尾段"}).to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        service.snapshot("legacy").await.unwrap().summary_markdown,
        document.summary_markdown
    );
}

#[tokio::test]
async fn write_rejects_stale_revision() {
    let pool = pool().await;
    seed(&pool, "edit").await;
    let service = MeetingWriteService::new(pool.clone());
    let versions = service
        .save_summary_markdown("edit", "首写", 0)
        .await
        .unwrap();
    assert_eq!(versions.revision, 1);
    assert_eq!(versions.summary_revision, 1);
    let error = service
        .save_summary_markdown("edit", "旧编辑者覆盖", 0)
        .await
        .unwrap_err();
    assert_eq!(error.code, "CONFLICT");
    assert_eq!(
        MeetingDocumentService::new(pool.clone())
            .snapshot("edit")
            .await
            .unwrap()
            .summary_markdown
            .as_deref(),
        Some("首写")
    );
    assert_eq!(
        service
            .save_notes_markdown("missing", "x", 0)
            .await
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
}

#[tokio::test]
async fn enhancement_migrations_preserve_existing_rows() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        include_str!("../../migrations/20250916100000_initial_schema.sql"),
        include_str!("../../migrations/20250920155811_add_openrouter_api_key.sql"),
        include_str!("../../migrations/20251006000000_add_audio_sync_fields.sql"),
        include_str!("../../migrations/20251010153942_add_ollama_endpoint.sql"),
        include_str!("../../migrations/20251101000000_add_summary_backup.sql"),
        include_str!("../../migrations/20251105120000_add_pro_license_custom_openai.sql"),
        include_str!("../../migrations/20251110000000_add_grace_period_to_licensing.sql"),
        include_str!("../../migrations/20251110000001_add_speaker_field.sql"),
        include_str!("../../migrations/20251223000000_add_meeting_notes.sql"),
        include_str!("../../migrations/20251229000000_add_gemini_api_key.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    seed(&pool, "upgrade").await;
    sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time,audio_end_time) VALUES('tail','upgrade','尾段','old',1.25,2.5)").execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO meeting_notes VALUES('upgrade','笔记','{\"private\":true}','old','old')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/20261014000100_add_content_versions.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let document = MeetingDocumentService::new(pool.clone())
        .snapshot("upgrade")
        .await
        .unwrap();
    assert_eq!(document.versions.revision, 0);
    assert_eq!(document.transcript[0].text, "尾段");
    assert_eq!(document.transcript[0].start_ms, Some(1250));
    assert_eq!(document.transcript[0].end_ms, Some(2500));
    assert_eq!(document.notes_markdown.as_deref(), Some("笔记"));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT notes_json FROM meeting_notes")
            .fetch_one(&pool)
            .await
            .unwrap(),
        r#"{"private":true}"#
    );
    MeetingWriteService::new(pool.clone())
        .save_notes_markdown("upgrade", "新笔记", 0)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT notes_json FROM meeting_notes")
            .fetch_one(&pool)
            .await
            .unwrap(),
        r#"{"private":true}"#
    );
    sqlx::query("DELETE FROM meetings WHERE id='upgrade'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM meeting_content_versions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn legacy_writes_advance_versions_and_invalidate_summary_source() {
    use crate::database::repositories::{
        meeting::MeetingsRepository, summary::SummaryProcessesRepository,
        transcript::TranscriptsRepository,
    };
    let pool = pool().await;
    let id = TranscriptsRepository::save_transcript(
        &pool,
        "recorded",
        &[crate::api::TranscriptSegment {
            id: "one".into(),
            text: "你好".into(),
            timestamp: "2026-10-14T00:00:00Z".into(),
            audio_start_time: Some(0.0),
            audio_end_time: Some(1.0),
            duration: Some(1.0),
        }],
        None,
    )
    .await
    .unwrap();
    let service = MeetingDocumentService::new(pool.clone());
    let first = service.snapshot(&id).await.unwrap();
    assert_eq!(first.versions.transcript_revision, 1);
    assert!(MeetingsRepository::update_meeting_title(&pool, &id, "改名")
        .await
        .unwrap());
    assert_eq!(service.snapshot(&id).await.unwrap().versions.revision, 2);
    assert!(SummaryProcessesRepository::update_meeting_summary(&pool,&id,&serde_json::json!({"markdown":"摘要","summary_json":[{"type":"paragraph","content":[{"text":"摘要"}]}]})).await.unwrap());
    let document = service.snapshot(&id).await.unwrap();
    assert_eq!(document.versions.summary_revision, 1);
    assert_eq!(document.versions.revision, 3);
    assert_eq!(document.summary_markdown.as_deref(), Some("摘要"));
    let original: String =
        sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id=?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(original.contains("summary_json"));
    let mut tx = pool.begin().await.unwrap();
    crate::database::repositories::content_version::bump(
        &mut tx,
        &id,
        crate::database::repositories::content_version::ContentKind::Transcript,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(service.snapshot(&id).await.unwrap().summary_stale);
}

#[tokio::test]
async fn rust_snapshot_matches_public_json_contract() {
    let pool = pool().await;
    seed(&pool, "golden").await;
    sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES('segment','golden','没有时间','old')").execute(&pool).await.unwrap();
    let document = MeetingDocumentService::new(pool)
        .snapshot("golden")
        .await
        .unwrap();
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/meeting-document-v1.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(document).unwrap(), golden);
}

#[test]
fn legacy_timing_rejects_nonfinite_negative_and_overflow() {
    use super::normalize::seconds_to_ms;
    for seconds in [
        None,
        Some(f64::NAN),
        Some(f64::INFINITY),
        Some(-1.0),
        Some(f64::MAX),
    ] {
        assert_eq!(seconds_to_ms(seconds), None);
    }
    assert_eq!(seconds_to_ms(Some(0.125)), Some(125));
}

#[tokio::test]
async fn snapshot_recovers_json_only_notes_without_exposing_reasoning() {
    let pool = pool().await;
    seed(&pool, "json-notes").await;
    sqlx::query("INSERT INTO meeting_notes(meeting_id,notes_json,created_at,updated_at) VALUES('json-notes',?,'old','old')")
        .bind(r#"[{"type":"paragraph","content":[{"type":"text","text":"只存JSON的笔记"}]}]"#).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO summary_processes(meeting_id,status,created_at,updated_at,result) VALUES('json-notes','completed','old','old',?)")
        .bind(serde_json::json!({"markdown":"<think>internal reasoning</think>\nresult"}).to_string()).execute(&pool).await.unwrap();
    let document = MeetingDocumentService::new(pool)
        .snapshot("json-notes")
        .await
        .unwrap();
    assert_eq!(document.notes_markdown.as_deref(), Some("只存JSON的笔记"));
    assert_eq!(document.summary_markdown, None);
}

#[tokio::test]
async fn shared_summary_writer_rejects_invisible_or_reasoning_content() {
    let pool = pool().await;
    seed(&pool, "invalid-content").await;
    let writer = MeetingWriteService::new(pool.clone());
    for value in [
        serde_json::json!({"markdown":"```\n```"}),
        serde_json::json!({"s":{"title":"Only heading","blocks":[]}}),
        serde_json::json!({"summary_json":[{"type":"paragraph","content":[{"text":"<think>private</think>"}]}]}),
    ] {
        assert_eq!(
            writer
                .save_summary_value("invalid-content", &value, 0)
                .await
                .unwrap_err()
                .code,
            "INVALID_INPUT"
        );
    }
    assert_eq!(
        MeetingDocumentService::new(pool)
            .snapshot("invalid-content")
            .await
            .unwrap()
            .versions
            .revision,
        0
    );
}
