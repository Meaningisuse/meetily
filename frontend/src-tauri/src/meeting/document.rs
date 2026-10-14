use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SourceVersions {
    pub revision: i64,
    pub transcript_revision: i64,
    pub speaker_revision: i64,
    pub summary_revision: i64,
    pub notes_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSegment {
    pub id: String,
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub speaker_id: Option<String>,
    pub speaker_name: Option<String>,
    pub assignment: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDocument {
    pub schema_version: u32,
    pub meeting_id: String,
    pub title: String,
    pub created_at: String,
    pub versions: SourceVersions,
    pub summary_markdown: Option<String>,
    pub notes_markdown: Option<String>,
    pub transcript: Vec<DocumentSegment>,
    pub summary_stale: bool,
}
