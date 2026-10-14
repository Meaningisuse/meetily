use super::{document::*, normalize, EnhancementError};
use crate::database::repositories::content_version::{self, ContentKind};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct MeetingDocumentService {
    pool: SqlitePool,
}
impl MeetingDocumentService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    pub async fn snapshot(&self, id: &str) -> Result<MeetingDocument, EnhancementError> {
        if id.trim().is_empty() {
            return Err(EnhancementError::invalid("meeting_id is required."));
        }
        let mut tx = self.pool.begin().await?;
        let meeting = sqlx::query("SELECT title,created_at FROM meetings WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let versions = sqlx::query_as::<_,SourceVersions>("SELECT revision,transcript_revision,speaker_revision,summary_revision,notes_revision FROM meeting_content_versions WHERE meeting_id=?").bind(id).fetch_optional(&mut *tx).await?.unwrap_or_default();
        let canonical = sqlx::query("SELECT * FROM meeting_document_content WHERE meeting_id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let summary_raw: Option<String> = sqlx::query_scalar(
            "SELECT COALESCE(result,result_backup) FROM summary_processes WHERE meeting_id=?",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let summary_markdown = canonical
            .as_ref()
            .and_then(|row| row.get::<Option<String>, _>("summary_markdown"))
            .or_else(|| summary_raw.as_deref().and_then(normalize::summary_markdown))
            .filter(|text| !crate::summary::processor::contains_reasoning_marker(text));
        let notes_raw = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT notes_markdown,notes_json FROM meeting_notes WHERE meeting_id=?",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .and_then(|(markdown, json)| {
            markdown.or_else(|| json.as_deref().and_then(normalize::summary_markdown))
        });
        let notes_markdown = canonical
            .as_ref()
            .and_then(|row| row.get::<Option<String>, _>("notes_markdown"))
            .or(notes_raw);
        let summary_stale = canonical.as_ref().is_some_and(|row| {
            row.get::<Option<i64>, _>("summary_source_transcript_revision")
                .is_some_and(|revision| revision != versions.transcript_revision)
                || row
                    .get::<Option<i64>, _>("summary_source_speaker_revision")
                    .is_some_and(|revision| revision != versions.speaker_revision)
        });
        let rows = sqlx::query("SELECT id,transcript,audio_start_time,audio_end_time,timing_precision FROM transcripts WHERE meeting_id=? ORDER BY audio_start_time IS NULL, audio_start_time, timestamp, id").bind(id).fetch_all(&mut *tx).await?;
        let transcript = rows
            .into_iter()
            .map(|row| {
                let mut start = normalize::seconds_to_ms(row.get("audio_start_time"));
                let mut end = normalize::seconds_to_ms(row.get("audio_end_time"));
                if matches!((start,end),(Some(s),Some(e)) if e < s) {
                    start = None;
                    end = None;
                }
                DocumentSegment {
                    id: row.get("id"),
                    text: row.get("transcript"),
                    start_ms: start,
                    end_ms: end,
                    timing_precision: row.get("timing_precision"),
                    speaker_id: None,
                    speaker_name: None,
                    assignment: "unknown".into(),
                }
            })
            .collect();
        tx.commit().await?;
        let created_at: String = meeting.get("created_at");
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&chrono::Utc).to_rfc3339())
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&created_at, "%Y-%m-%d %H:%M:%S%.f")
                    .map(|d| d.and_utc().to_rfc3339())
            })
            .unwrap_or(created_at);
        Ok(MeetingDocument {
            schema_version: 2,
            meeting_id: id.into(),
            title: meeting.get("title"),
            created_at,
            versions,
            summary_markdown,
            notes_markdown,
            transcript,
            summary_stale,
        })
    }
}
#[derive(Clone)]
pub struct MeetingWriteService {
    pool: SqlitePool,
}
impl MeetingWriteService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    pub async fn save_summary_markdown(
        &self,
        id: &str,
        markdown: &str,
        expected: i64,
    ) -> Result<SourceVersions, EnhancementError> {
        self.save_summary_value(id, &serde_json::json!({"markdown":markdown}), expected)
            .await
    }
    pub async fn save_summary_value(
        &self,
        id: &str,
        value: &Value,
        expected: i64,
    ) -> Result<SourceVersions, EnhancementError> {
        if !crate::summary::content::summary_is_renderable(value) {
            return Err(EnhancementError::invalid(
                "Summary contains no visible content or model reasoning markers.",
            ));
        }
        let encoded = value.to_string();
        if encoded.len() > 1024 * 1024 {
            return Err(EnhancementError::invalid("Summary exceeds 1 MiB."));
        }
        let markdown = normalize::summary_markdown(&encoded)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| EnhancementError::invalid("Summary has no visible content."))?;
        if crate::summary::processor::contains_reasoning_marker(&markdown) {
            return Err(EnhancementError::invalid(
                "Summary contains model reasoning markers.",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let versions =
            content_version::compare_and_bump(&mut tx, id, ContentKind::Summary, expected).await?;
        let now = chrono::Utc::now();
        // Preserve the original BlockNote JSON; canonical markdown is a separate
        // representation and never masquerades as the editor's block content.
        sqlx::query("INSERT INTO summary_processes(meeting_id,status,created_at,updated_at,result) VALUES (?,'completed',?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET result=excluded.result,updated_at=excluded.updated_at")
            .bind(id).bind(now).bind(now).bind(encoded).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO meeting_document_content(meeting_id,summary_markdown,summary_source_transcript_revision,summary_source_speaker_revision) VALUES(?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET summary_markdown=excluded.summary_markdown,summary_source_transcript_revision=excluded.summary_source_transcript_revision,summary_source_speaker_revision=excluded.summary_source_speaker_revision")
            .bind(id).bind(markdown).bind(versions.transcript_revision).bind(versions.speaker_revision).execute(&mut *tx).await?;
        sqlx::query("UPDATE meetings SET updated_at=? WHERE id=?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(versions)
    }
    pub async fn save_notes_markdown(
        &self,
        id: &str,
        markdown: &str,
        expected: i64,
    ) -> Result<SourceVersions, EnhancementError> {
        if markdown.len() > 1024 * 1024 {
            return Err(EnhancementError::invalid("Notes exceed 1 MiB."));
        }
        let mut tx = self.pool.begin().await?;
        let versions =
            content_version::compare_and_bump(&mut tx, id, ContentKind::Notes, expected).await?;
        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO meeting_notes(meeting_id,notes_markdown,created_at,updated_at) VALUES (?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET notes_markdown=excluded.notes_markdown,updated_at=excluded.updated_at")
            .bind(id).bind(markdown).bind(now).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO meeting_document_content(meeting_id,notes_markdown) VALUES (?,?) ON CONFLICT(meeting_id) DO UPDATE SET notes_markdown=excluded.notes_markdown")
            .bind(id).bind(markdown).execute(&mut *tx).await?;
        sqlx::query("UPDATE meetings SET updated_at=? WHERE id=?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(versions)
    }
    pub async fn save_title(
        &self,
        id: &str,
        title: &str,
        expected: i64,
    ) -> Result<SourceVersions, EnhancementError> {
        if title.trim().is_empty() || title.chars().count() > 256 {
            return Err(EnhancementError::invalid(
                "Title must contain 1–256 characters.",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let versions =
            content_version::compare_and_bump(&mut tx, id, ContentKind::Title, expected).await?;
        sqlx::query("UPDATE meetings SET title=?,updated_at=? WHERE id=?")
            .bind(title)
            .bind(chrono::Utc::now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE transcript_chunks SET meeting_name=? WHERE meeting_id=?")
            .bind(title)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(versions)
    }
}
