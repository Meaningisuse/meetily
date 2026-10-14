CREATE TABLE meeting_content_versions (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
    transcript_revision INTEGER NOT NULL DEFAULT 0,
    speaker_revision INTEGER NOT NULL DEFAULT 0,
    summary_revision INTEGER NOT NULL DEFAULT 0,
    notes_revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO meeting_content_versions(meeting_id) SELECT id FROM meetings;
CREATE TABLE meeting_document_content (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    summary_markdown TEXT,
    notes_markdown TEXT,
    summary_source_transcript_revision INTEGER,
    summary_source_speaker_revision INTEGER
);
