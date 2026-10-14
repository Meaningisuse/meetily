export interface Discovery {
  pid: number;
  instance_id: string;
  base_url: string;
  protocol_version: number;
}

export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

export interface MeetingBrief {
  id: string;
  title: string;
  created_at: string;
  revision: number;
}

export interface TranscriptItem {
  id: string;
  text: string;
  start_ms: number | null;
  end_ms: number | null;
  speaker: string | null;
}

export interface SearchHit {
  meeting_id: string;
  segment_id: string;
  start_ms: number | null;
  snippet: string;
  revision: number;
}

export interface MeetingSummary {
  meeting_id: string;
  title: string;
  created_at: string;
  versions: {
    revision: number;
    transcript_revision: number;
    speaker_revision: number;
    summary_revision: number;
    notes_revision: number;
  };
  summary_markdown: string | null;
  summary_stale: boolean;
}

export interface JobStatus {
  id: string;
  meeting_id: string;
  kind: string;
  status: string;
  source_revision: number;
  progress: number;
  error_code: string | null;
  created_at: string;
  updated_at: string;
}

export interface ApiPageOptions {
  limit?: number;
  cursor?: string;
}
