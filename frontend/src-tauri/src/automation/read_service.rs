use crate::meeting::{document::SourceVersions, service::MeetingDocumentService, EnhancementError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MeetingBrief {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub revision: i64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptItem {
    pub id: String,
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub timing_precision: String,
    pub speaker: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub meeting_id: String,
    pub segment_id: String,
    pub start_ms: Option<i64>,
    pub snippet: String,
    pub revision: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Cursor {
    Meetings {
        created_at: String,
        id: String,
    },
    Transcript {
        revision: i64,
        start: Option<f64>,
        timestamp: String,
        id: String,
    },
    Search {
        created_at: String,
        meeting_id: String,
        segment_id: String,
    },
}

pub struct MeetingReadService {
    pool: SqlitePool,
}

impl MeetingReadService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn page_limit(request: &PageRequest) -> Result<usize, EnhancementError> {
        let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(EnhancementError::invalid("limit must be between 1 and 200"));
        }
        Ok(limit)
    }

    fn decode<T: DeserializeOwned>(raw: &Option<String>) -> Result<Option<T>, EnhancementError> {
        let Some(raw) = raw else { return Ok(None) };
        if raw.len() > 1024 {
            return Err(EnhancementError::invalid("cursor is too long"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| EnhancementError::invalid("invalid cursor"))?;
        let cursor = serde_json::from_slice(&bytes)
            .map_err(|_| EnhancementError::invalid("invalid cursor"))?;
        Ok(Some(cursor))
    }

    fn encode<T: Serialize>(cursor: &T) -> Option<String> {
        serde_json::to_vec(cursor)
            .ok()
            .map(|v| URL_SAFE_NO_PAD.encode(v))
    }

    pub async fn list(&self, request: PageRequest) -> Result<Page<MeetingBrief>, EnhancementError> {
        let limit = Self::page_limit(&request)?;
        let cursor: Option<Cursor> = Self::decode(&request.cursor)?;
        let rows = match cursor {
            Some(Cursor::Meetings { created_at, id }) => sqlx::query("SELECT m.id,m.title,m.created_at,COALESCE(v.revision,0) revision FROM meetings m LEFT JOIN meeting_content_versions v ON v.meeting_id=m.id WHERE m.created_at < ? OR (m.created_at = ? AND m.id > ?) ORDER BY m.created_at DESC,m.id ASC LIMIT ?")
                .bind(created_at.clone()).bind(created_at).bind(id).bind((limit + 1) as i64).fetch_all(&self.pool).await?,
            Some(_) => return Err(EnhancementError::invalid("cursor does not match meeting list")),
            None => sqlx::query("SELECT m.id,m.title,m.created_at,COALESCE(v.revision,0) revision FROM meetings m LEFT JOIN meeting_content_versions v ON v.meeting_id=m.id ORDER BY m.created_at DESC,m.id ASC LIMIT ?")
                .bind((limit + 1) as i64).fetch_all(&self.pool).await?,
        };
        let more = rows.len() > limit;
        let next_cursor = if more {
            rows.get(limit - 1).and_then(|r| {
                Self::encode(&Cursor::Meetings {
                    created_at: r.get("created_at"),
                    id: r.get("id"),
                })
            })
        } else {
            None
        };
        let items: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(|r| MeetingBrief {
                id: r.get("id"),
                title: r.get("title"),
                created_at: r.get("created_at"),
                revision: r.get("revision"),
            })
            .collect();
        Ok(Page { items, next_cursor })
    }

    pub async fn transcript(
        &self,
        meeting_id: &str,
        request: PageRequest,
    ) -> Result<Page<TranscriptItem>, EnhancementError> {
        let limit = Self::page_limit(&request)?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?)")
            .bind(meeting_id)
            .fetch_one(&self.pool)
            .await?;
        if !exists {
            return Err(EnhancementError::new("NOT_FOUND", "Meeting not found."));
        }
        // Legacy/imported rows can lack a version row until their first edit.
        let versions: SourceVersions=sqlx::query_as("SELECT revision,transcript_revision,speaker_revision,summary_revision,notes_revision FROM meeting_content_versions WHERE meeting_id=?").bind(meeting_id).fetch_optional(&self.pool).await?.unwrap_or_default();
        let cursor: Option<Cursor> = Self::decode(&request.cursor)?;
        let rows=match cursor {
            Some(Cursor::Transcript{revision,start,timestamp,id}) if revision==versions.transcript_revision=>sqlx::query("SELECT id,transcript,audio_start_time,audio_end_time,speaker,timestamp,timing_precision FROM transcripts WHERE meeting_id=? AND ((audio_start_time IS NULL) > ? OR ((audio_start_time IS NULL)=? AND (audio_start_time > ? OR (audio_start_time IS ? AND (timestamp > ? OR (timestamp = ? AND id > ?)))))) ORDER BY audio_start_time IS NULL,audio_start_time,timestamp,id LIMIT ?")
                .bind(meeting_id).bind(start.is_none()).bind(start.is_none()).bind(start).bind(start).bind(timestamp.clone()).bind(timestamp).bind(id).bind((limit+1) as i64).fetch_all(&self.pool).await?,
            Some(Cursor::Transcript{..})=>return Err(EnhancementError::conflict()),
            Some(_)=>return Err(EnhancementError::invalid("cursor does not match transcript")),
            None=>sqlx::query("SELECT id,transcript,audio_start_time,audio_end_time,speaker,timestamp,timing_precision FROM transcripts WHERE meeting_id=? ORDER BY audio_start_time IS NULL,audio_start_time,timestamp,id LIMIT ?").bind(meeting_id).bind((limit+1) as i64).fetch_all(&self.pool).await?,
        };
        let more = rows.len() > limit;
        let next_cursor = if more {
            if let Some(row) = rows.get(limit - 1) {
                Self::encode(&Cursor::Transcript {
                    revision: versions.transcript_revision,
                    start: row.get("audio_start_time"),
                    timestamp: row.get("timestamp"),
                    id: row.get("id"),
                })
            } else {
                None
            }
        } else {
            None
        };
        let items: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(|r| TranscriptItem {
                id: r.get("id"),
                text: r.get("transcript"),
                start_ms: r
                    .get::<Option<f64>, _>("audio_start_time")
                    .map(|v| (v * 1000.0).round() as i64),
                end_ms: r
                    .get::<Option<f64>, _>("audio_end_time")
                    .map(|v| (v * 1000.0).round() as i64),
                timing_precision: r.get("timing_precision"),
                speaker: r.get("speaker"),
            })
            .collect();
        Ok(Page { items, next_cursor })
    }

    pub async fn search(
        &self,
        query: &str,
        request: PageRequest,
    ) -> Result<Page<SearchHit>, EnhancementError> {
        if query.is_empty() || query.chars().count() > 256 {
            return Err(EnhancementError::invalid(
                "q must contain 1 to 256 characters",
            ));
        }
        let limit = Self::page_limit(&request)?;
        let cursor: Option<Cursor> = Self::decode(&request.cursor)?;
        let (after, created_at, meeting_id, segment_id) = match cursor {
            None => (false, String::new(), String::new(), String::new()),
            Some(Cursor::Search {
                created_at,
                meeting_id,
                segment_id,
            }) => (true, created_at, meeting_id, segment_id),
            Some(_) => return Err(EnhancementError::invalid("cursor does not match search")),
        };
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows=sqlx::query("SELECT t.id segment_id,t.meeting_id,m.created_at,t.audio_start_time,t.transcript,COALESCE(v.revision,0) revision FROM transcripts t JOIN meetings m ON m.id=t.meeting_id LEFT JOIN meeting_content_versions v ON v.meeting_id=m.id WHERE t.transcript LIKE ? ESCAPE '\\' AND (?=0 OR m.created_at < ? OR (m.created_at=? AND (m.id>? OR (m.id=? AND t.id>?)))) ORDER BY m.created_at DESC,m.id ASC,t.id ASC LIMIT ?")
            .bind(format!("%{escaped}%")).bind(after).bind(&created_at).bind(&created_at).bind(&meeting_id).bind(&meeting_id).bind(&segment_id).bind((limit+1) as i64).fetch_all(&self.pool).await?;
        let more = rows.len() > limit;
        let next_cursor = if more {
            rows.get(limit - 1).and_then(|r| {
                Self::encode(&Cursor::Search {
                    created_at: r.get("created_at"),
                    meeting_id: r.get("meeting_id"),
                    segment_id: r.get("segment_id"),
                })
            })
        } else {
            None
        };
        let items: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(|r| {
                let text: String = r.get("transcript");
                let snippet: String = text.chars().take(300).collect();
                SearchHit {
                    meeting_id: r.get("meeting_id"),
                    segment_id: r.get("segment_id"),
                    start_ms: r
                        .get::<Option<f64>, _>("audio_start_time")
                        .map(|v| (v * 1000.0).round() as i64),
                    snippet,
                    revision: r.get("revision"),
                }
            })
            .collect();
        Ok(Page { items, next_cursor })
    }

    pub async fn summary(
        &self,
        meeting_id: &str,
    ) -> Result<crate::meeting::document::MeetingDocument, EnhancementError> {
        MeetingDocumentService::new(self.pool.clone())
            .snapshot(meeting_id)
            .await
    }

    pub async fn job(&self, id: &str) -> Result<serde_json::Value, EnhancementError> {
        let row=sqlx::query("SELECT id,meeting_id,kind,status,source_revision,progress,error_code,created_at,updated_at FROM enhancement_jobs WHERE id=?").bind(id).fetch_optional(&self.pool).await?.ok_or_else(||EnhancementError::new("NOT_FOUND","Job not found."))?;
        Ok(
            serde_json::json!({"id":row.get::<String,_>("id"),"meeting_id":row.get::<String,_>("meeting_id"),"kind":row.get::<String,_>("kind"),"status":row.get::<String,_>("status"),"source_revision":row.get::<i64,_>("source_revision"),"progress":row.get::<f64,_>("progress"),"error_code":row.get::<Option<String>,_>("error_code"),"created_at":row.get::<String,_>("created_at"),"updated_at":row.get::<String,_>("updated_at")}),
        )
    }
}
