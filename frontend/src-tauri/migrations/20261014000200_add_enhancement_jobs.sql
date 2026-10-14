CREATE TABLE enhancement_jobs (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('summary','export','diarization')),
    status TEXT NOT NULL CHECK(status IN ('queued','running','succeeded','failed','cancelled','interrupted')),
    source_revision INTEGER NOT NULL,
    params_json TEXT NOT NULL,
    progress REAL NOT NULL DEFAULT 0 CHECK(progress >= 0 AND progress <= 1),
    error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_enhancement_jobs_status_created ON enhancement_jobs(status,created_at,id);
