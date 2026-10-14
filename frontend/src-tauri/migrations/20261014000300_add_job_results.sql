-- Each attempt keeps its own frozen source and result, independent of the
-- editable summary and its existing regeneration backup.
CREATE TABLE enhancement_job_results (
    job_id TEXT PRIMARY KEY REFERENCES enhancement_jobs(id) ON DELETE CASCADE,
    source_revision INTEGER NOT NULL,
    result_json TEXT NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0 CHECK(stale IN (0,1)),
    created_at TEXT NOT NULL
);
CREATE TABLE summary_job_context (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    job_id TEXT NOT NULL UNIQUE REFERENCES enhancement_jobs(id) ON DELETE CASCADE,
    PRIMARY KEY(meeting_id,started_at)
);
CREATE UNIQUE INDEX idx_enhancement_job_active_meeting_kind
ON enhancement_jobs(meeting_id,kind) WHERE status IN ('queued','running');
