import React from 'react';
import { afterAll, afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { MeetingSummary } from '../../src/types';

const originals = {
  core: { ...await import('@tauri-apps/api/core') },
  sidebar: { ...await import('../../src/components/Sidebar/SidebarProvider') },
  blocknoteReact: { ...await import('@blocknote/react') },
  blocknoteView: { ...await import('@blocknote/shadcn') },
  sonner: { ...await import('sonner') },
};
const blocks = (text: string) => [{
  id: 'paragraph', type: 'paragraph', props: {}, children: [],
  content: [{ type: 'text', text, styles: {} }],
}];
const markdown = (value: ReturnType<typeof blocks>) => value[0]?.content[0]?.text ?? '';
const editor = {
  document: blocks('A'),
  tryParseMarkdownToBlocks: async (value: string) => blocks(value),
  replaceBlocks: (_previous: unknown, next: ReturnType<typeof blocks>) => { editor.document = next; },
  blocksToMarkdownLossy: async (value: ReturnType<typeof blocks>) => markdown(value),
};
mock.module('@blocknote/react', () => ({ ...originals.blocknoteReact, useCreateBlockNote: () => editor }));
mock.module('@blocknote/shadcn', () => ({
  ...originals.blocknoteView,
  BlockNoteView: ({ onChange }: { onChange: () => void }) => <textarea aria-label="Summary editor" onChange={onChange} />,
}));
mock.module('../../src/components/Sidebar/SidebarProvider', () => ({
  ...originals.sidebar,
  useSidebar: () => ({ meetings: [], setMeetings: () => {}, setCurrentMeeting: () => {} }),
}));
const success = mock(() => {});
const failure = mock(() => {});
mock.module('sonner', () => ({ ...originals.sonner, toast: { success, error: failure, info: mock(() => {}) } }));
let stored: { revision: number; summary: MeetingSummary };
let waitForSave: () => Promise<void> = async () => {};
const invoke = mock(async (command: string, args?: Record<string, unknown>) => {
  if (command !== 'api_save_meeting_summary') throw new Error(`Unexpected native call: ${command}`);
  await waitForSave();
  if (args!.meetingId !== 'meeting-a') throw new Error('Wrong meeting');
  if (args!.expectedRevision !== stored.revision) throw new Error('CONFLICT');
  stored = { revision: stored.revision + 1, summary: args!.summary as MeetingSummary };
  return { revision: stored.revision };
});
mock.module('@tauri-apps/api/core', () => ({ ...originals.core, invoke }));
const { useMeetingData } = await import('../../src/hooks/meeting-details/useMeetingData');
const { BlockNoteSummaryView } = await import('../../src/components/AISummary/BlockNoteSummaryView');
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originals.core);
  mock.module('../../src/components/Sidebar/SidebarProvider', () => originals.sidebar);
  mock.module('@blocknote/react', () => originals.blocknoteReact);
  mock.module('@blocknote/shadcn', () => originals.blocknoteView);
  mock.module('sonner', () => originals.sonner);
});

let state!: ReturnType<typeof useMeetingData>;
let renderer: ReactTestRenderer | undefined;
const timers = new Map<number, () => void>();
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
beforeEach(() => {
  invoke.mockClear(); success.mockClear(); failure.mockClear(); timers.clear();
  stored = { revision: 0, summary: { markdown: 'A' } };
  editor.document = blocks('A');
  waitForSave = async () => {};
  editor.blocksToMarkdownLossy = async (value: ReturnType<typeof blocks>) => markdown(value);
  globalThis.setTimeout = ((callback: () => void) => { const id = timers.size + 1; timers.set(id, callback); return id; }) as typeof setTimeout;
  globalThis.clearTimeout = ((id: number) => { timers.delete(id); }) as typeof clearTimeout;
});
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
  globalThis.setTimeout = realSetTimeout;
  globalThis.clearTimeout = realClearTimeout;
});
function MeetingEditor({ initial, legacy = false }: { initial: MeetingSummary; legacy?: boolean }) {
  state = useMeetingData({
    meeting: { id: 'meeting-a', title: 'Meeting A', transcripts: [] },
    summaryData: initial, summaryRevision: 0,
  });
  if (legacy) return null;
  return <BlockNoteSummaryView ref={state.blockNoteSummaryRef} summaryData={state.aiSummary}
    onSave={state.handleSaveSummary} onDirtyChange={state.setIsSummaryDirty} />;
}
async function show(initial: MeetingSummary = stored.summary, legacy = false) {
  await act(async () => { renderer = create(<MeetingEditor initial={initial} legacy={legacy} />); });
  await act(async () => { for (const callback of timers.values()) callback(); timers.clear(); });
}
async function edit(value: string) {
  await act(async () => {
    editor.document = blocks(value);
    renderer!.root.findByProps({ 'aria-label': 'Summary editor' }).props.onChange();
  });
}

test('a second save after a successful manual edit never writes the original summary back', async () => {
  await show();
  await edit('B');
  await act(async () => { await state.saveAllChanges(); });
  expect(stored.summary).toMatchObject({ markdown: 'B' });
  await act(async () => { await state.saveAllChanges(); });
  expect(stored.summary).toMatchObject({ markdown: 'B' });
  expect(invoke).toHaveBeenCalledTimes(1);
});

test('editing while a manual save is pending stays dirty and the next save persists the newer draft with the new revision', async () => {
  await show();
  await edit('B');
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  waitForSave = () => gate;
  let saving!: Promise<void>;
  await act(async () => { saving = state.saveAllChanges(); });
  expect(invoke).toHaveBeenCalledTimes(1);
  await edit('C');
  await act(async () => { release(); await saving; });
  expect(stored.summary).toMatchObject({ markdown: 'B' });
  expect(state.isSummaryDirty).toBe(true);
  expect(state.blockNoteSummaryRef.current?.isDirty).toBe(true);
  waitForSave = async () => {};
  await act(async () => { await state.saveAllChanges(); });
  expect(stored.summary).toMatchObject({ markdown: 'C' });
  expect(invoke.mock.calls[1][1]?.expectedRevision).toBe(1);
  expect(state.isSummaryDirty).toBe(false);
  expect(failure).not.toHaveBeenCalled();
});

test.each(['CONFLICT', 'IO_ERROR'])('a failed native save (%s) preserves the draft and revision for a successful retry', async (code) => {
  await show();
  await edit('B');
  waitForSave = async () => { throw new Error(code); };
  await act(async () => { await state.saveAllChanges(); });
  expect(stored).toMatchObject({ revision: 0, summary: { markdown: 'A' } });
  expect(state.isSummaryDirty).toBe(true);
  expect(state.blockNoteSummaryRef.current?.isDirty).toBe(true);
  expect(await state.blockNoteSummaryRef.current!.getMarkdown()).toBe('B');
  expect(success).not.toHaveBeenCalled();
  expect(failure).toHaveBeenCalledTimes(1);

  waitForSave = async () => {};
  await act(async () => { await state.saveAllChanges(); });
  expect(stored).toMatchObject({ revision: 1, summary: { markdown: 'B' } });
  expect(invoke.mock.calls.map(call => call[1]?.expectedRevision)).toEqual([0, 0]);
  expect(state.isSummaryDirty).toBe(false);
  expect(success).toHaveBeenCalledTimes(1);
});

test('editing during markdown conversion preserves the newer draft until it is saved with the next revision', async () => {
  await show();
  await edit('B');
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  editor.blocksToMarkdownLossy = async (value: ReturnType<typeof blocks>) => {
    await gate;
    return markdown(value);
  };
  let saving!: Promise<void>;
  await act(async () => { saving = state.saveAllChanges(); });
  expect(invoke).not.toHaveBeenCalled();
  await edit('C');
  await act(async () => { release(); await saving; });
  expect(stored.summary).toMatchObject({ markdown: 'B' });
  expect(state.isSummaryDirty).toBe(true);
  expect(state.blockNoteSummaryRef.current?.isDirty).toBe(true);

  editor.blocksToMarkdownLossy = async (value: ReturnType<typeof blocks>) => markdown(value);
  await act(async () => { await state.saveAllChanges(); });
  expect(stored).toMatchObject({ revision: 2, summary: { markdown: 'C' } });
  expect(invoke.mock.calls[1][1]?.expectedRevision).toBe(1);
  expect(state.isSummaryDirty).toBe(false);
  expect(failure).not.toHaveBeenCalled();
});

test('legacy summary manual edits still save through the existing revision checked path', async () => {
  const initial = { Agenda: { title: 'Agenda', blocks: [{ id: 'Agenda-1', type: 'text' as const, content: 'A', color: 'default' }] } };
  stored.summary = initial;
  await show(initial, true);
  await act(async () => {
    state.handleSummaryChange({ Agenda: { title: 'Agenda', blocks: [{ id: 'Agenda-1', type: 'text', content: 'Legacy edit', color: 'default' }] } });
  });
  await act(async () => { await state.saveAllChanges(); });
  expect(stored.summary).toMatchObject({ Agenda: { blocks: [{ content: 'Legacy edit' }] } });
  expect(invoke.mock.calls[0][1]?.expectedRevision).toBe(0);
  expect(failure).not.toHaveBeenCalled();
});
