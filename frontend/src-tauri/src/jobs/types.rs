use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Summary,
    Export,
    Diarization,
}
impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Export => "export",
            Self::Diarization => "diarization",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct JobSnapshot {
    pub id: String,
    pub meeting_id: String,
    pub kind: String,
    pub status: String,
    pub source_revision: i64,
    pub progress: f64,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
