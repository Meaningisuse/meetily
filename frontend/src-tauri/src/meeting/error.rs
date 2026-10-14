use serde::Serialize;
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct EnhancementError {
    pub code: String,
    pub message: String,
}
impl EnhancementError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new("INVALID_INPUT", message)
    }
    pub fn conflict() -> Self {
        Self::new("CONFLICT", "Meeting content changed; reload before saving.")
    }
}
impl From<sqlx::Error> for EnhancementError {
    fn from(error: sqlx::Error) -> Self {
        log::error!("Meeting database operation failed: {error}");
        match error {
            sqlx::Error::RowNotFound => Self::new("NOT_FOUND", "Meeting not found."),
            sqlx::Error::Database(ref db) if db.is_unique_violation() => Self::conflict(),
            sqlx::Error::Database(ref db)
                if matches!(db.code().as_deref(), Some("5" | "6" | "517")) =>
            {
                Self::new("BUSY", "Meeting database is busy; retry later.")
            }
            _ => Self::new("IO_ERROR", "Meeting data could not be accessed."),
        }
    }
}
