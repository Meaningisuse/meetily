import type { ModelConfig } from '@/components/ModelSettingsModal';
import type { TranscriptModelProps } from '@/components/TranscriptSettings';
export type NativeInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export async function fetchNativeMeetings(invoke: NativeInvoke) {
  const ready = await invoke<boolean>('api_native_ready');
  if (!ready) return { ready: false, meetings: null };
  const meetings = await invoke<Array<{ id: string; title: string; revision?: number }>>('api_get_meetings');
  return { ready: true, meetings };
}
export async function fetchNativeConfiguration(invoke: NativeInvoke) {
  const [model, transcript] = await Promise.all([
    invoke<ModelConfig>('api_get_model_config'),
    invoke<TranscriptModelProps>('api_get_transcript_config'),
  ]);
  return { model, transcript };
}
