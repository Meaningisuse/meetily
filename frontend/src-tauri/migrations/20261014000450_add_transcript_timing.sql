ALTER TABLE transcripts
    ADD COLUMN timing_precision TEXT NOT NULL DEFAULT 'unavailable'
    CHECK (timing_precision IN ('exact', 'approximate', 'unavailable'));

-- Existing audio_start_time/audio_end_time values are chunk/VAD bounds, not word timings.
UPDATE transcripts
SET timing_precision = 'approximate'
WHERE audio_start_time IS NOT NULL AND audio_end_time IS NOT NULL;
