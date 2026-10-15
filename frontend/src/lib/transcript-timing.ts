import type { Transcript, TranscriptSegmentData } from '@/types';

/** Preserve missing recording times and precision across every display entry. */
export function transcriptsToSegments(transcripts: Transcript[]): TranscriptSegmentData[] {
  return transcripts.map(transcript => ({
    id: transcript.id,
    timestamp: transcript.audio_start_time,
    endTime: transcript.audio_end_time,
    text: transcript.text,
    confidence: transcript.confidence,
    timingPrecision: transcript.timing_precision,
  }));
}

type TimingInput = Pick<TranscriptSegmentData, 'timestamp' | 'endTime' | 'timingPrecision'>;
interface TranscriptTiming {
  label: string;
  description: string;
  playLabel?: string;
  seekSeconds?: number;
}

/** A numeric zero is a real seek target; absent/invalid times are never coerced. */
export function getTranscriptTiming({ timestamp, endTime, timingPrecision }: TimingInput): TranscriptTiming {
  const validStart = typeof timestamp === 'number' && Number.isFinite(timestamp) && timestamp >= 0;
  const validEnd = endTime == null || (
    typeof endTime === 'number' && Number.isFinite(endTime) && validStart && endTime >= timestamp
  );
  if (!validStart || !validEnd || timingPrecision === 'unavailable') {
    return { label: '[--:--]', description: 'Recording time unavailable' };
  }

  const seconds = Math.floor(timestamp);
  const minutes = Math.floor(seconds / 60).toString().padStart(2, '0');
  const remainder = (seconds % 60).toString().padStart(2, '0');
  const time = `${minutes}:${remainder}`;
  // Older recordings can have usable bounds without a precision field. Treat
  // them conservatively as approximate rather than silently claiming exactness.
  const exact = timingPrecision === 'exact';
  return {
    label: exact ? `[${time}]` : `[约 ${time}]`,
    description: exact ? 'Recording-relative time' : 'Approximate time; playback may start before or after the spoken words.',
    playLabel: exact ? `Play from [${time}]` : `Play from approximately [${time}]`,
    seekSeconds: timestamp,
  };
}
