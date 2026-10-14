import { z } from 'zod';

const Revision = z.number().int().nonnegative();
export const SourceVersionsSchema = z.object({
  revision: Revision,
  transcript_revision: Revision,
  speaker_revision: Revision,
  summary_revision: Revision,
  notes_revision: Revision,
}).strict();
export type SourceVersions = z.infer<typeof SourceVersionsSchema>;

export const DocumentSegmentSchema = z.object({
  id: z.string(),
  text: z.string(),
  start_ms: Revision.nullable(),
  end_ms: Revision.nullable(),
  speaker_id: z.string().nullable(),
  speaker_name: z.string().nullable(),
  assignment: z.enum(['unknown', 'model', 'manual', 'overlap']),
}).strict().refine(segment => segment.start_ms === null || segment.end_ms === null || segment.end_ms >= segment.start_ms, 'end_ms must not precede start_ms');
export type DocumentSegment = z.infer<typeof DocumentSegmentSchema>;

export const MeetingDocumentSchema = z.object({
  schema_version: z.literal(1),
  meeting_id: z.string(),
  title: z.string(),
  created_at: z.string(),
  versions: SourceVersionsSchema,
  summary_markdown: z.string().nullable(),
  notes_markdown: z.string().nullable(),
  transcript: z.array(DocumentSegmentSchema),
  summary_stale: z.boolean(),
}).strict();
export type MeetingDocument = z.infer<typeof MeetingDocumentSchema>;
export interface EnhancementError { code: string; message: string }
