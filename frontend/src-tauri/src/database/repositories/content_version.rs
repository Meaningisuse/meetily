use crate::meeting::{EnhancementError, SourceVersions};
use sqlx::SqliteConnection;

#[derive(Clone, Copy)]
pub enum ContentKind {
    Title,
    Transcript,
    Speaker,
    Summary,
    Notes,
}

pub async fn initialize(conn: &mut SqliteConnection, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO meeting_content_versions(meeting_id) SELECT id FROM meetings WHERE id=?").bind(id).execute(conn).await?;
    Ok(())
}
pub async fn read(conn: &mut SqliteConnection, id: &str) -> Result<SourceVersions, sqlx::Error> {
    sqlx::query_as("SELECT revision,transcript_revision,speaker_revision,summary_revision,notes_revision FROM meeting_content_versions WHERE meeting_id=?").bind(id).fetch_one(conn).await
}
pub async fn bump(
    conn: &mut SqliteConnection,
    id: &str,
    kind: ContentKind,
) -> Result<SourceVersions, sqlx::Error> {
    initialize(conn, id).await?;
    let field = match kind {
        ContentKind::Title => None,
        ContentKind::Transcript => Some("transcript_revision"),
        ContentKind::Speaker => Some("speaker_revision"),
        ContentKind::Summary => Some("summary_revision"),
        ContentKind::Notes => Some("notes_revision"),
    };
    let sql = match field { Some(field) => format!("UPDATE meeting_content_versions SET revision=revision+1,{field}={field}+1 WHERE meeting_id=?"), None => "UPDATE meeting_content_versions SET revision=revision+1 WHERE meeting_id=?".to_owned() };
    sqlx::query(&sql).bind(id).execute(&mut *conn).await?;
    read(conn, id).await
}
pub async fn compare_and_bump(
    conn: &mut SqliteConnection,
    id: &str,
    kind: ContentKind,
    expected: i64,
) -> Result<SourceVersions, EnhancementError> {
    if expected < 0 {
        return Err(EnhancementError::invalid(
            "expected_revision must be nonnegative.",
        ));
    }
    initialize(conn, id).await?;
    // First statement upgrades SQLite's transaction to a writer. The comparison
    // and all content writes stay in this transaction; no read-then-write race.
    let update = sqlx::query(
        "UPDATE meeting_content_versions SET revision=revision WHERE meeting_id=? AND revision=?",
    )
    .bind(id)
    .bind(expected)
    .execute(&mut *conn)
    .await?;
    if update.rows_affected() == 0 {
        if sqlx::query("SELECT id FROM meetings WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .is_none()
        {
            return Err(EnhancementError::new("NOT_FOUND", "Meeting not found."));
        }
        return Err(EnhancementError::conflict());
    }
    Ok(bump(conn, id, kind).await?)
}
