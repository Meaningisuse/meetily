import { afterAll, describe, expect, mock, test } from 'bun:test';
import React from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const originalMotion = await import('framer-motion');
const originalTooltip = await import('../../src/components/ui/tooltip');
const passthrough = ({ children }: { children?: React.ReactNode }) => <>{children}</>;

mock.module('framer-motion', () => ({
  ...originalMotion,
  motion: { div: passthrough },
  AnimatePresence: passthrough,
}));
mock.module('../../src/components/ui/tooltip', () => ({
  Tooltip: passthrough,
  TooltipTrigger: passthrough,
  TooltipContent: passthrough,
}));

const { VirtualizedTranscriptView } = await import('../../src/components/VirtualizedTranscriptView');
afterAll(() => {
  mock.module('framer-motion', () => originalMotion);
  mock.module('../../src/components/ui/tooltip', () => originalTooltip);
});

describe('transcript playback seeking', () => {
  test('clicking a transcript timestamp seeks to its segment start', async () => {
    const seekTo = mock((_seconds: number) => {});
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(
        <VirtualizedTranscriptView
          segments={[{ id: 'segment-1', timestamp: 12.5, text: 'A sample sentence' }]}
          disableAutoScroll
          showConfidence={false}
          onSeekTo={seekTo}
        />,
      );
    });

    const timestamp = renderer!.root.findByProps({ 'aria-label': 'Play from [00:12]' });
    await act(async () => timestamp.props.onClick());
    expect(seekTo).toHaveBeenCalledWith(12.5);
    await act(async () => renderer!.unmount());
  });

  test('timestamps remain read-only when no playback handler is supplied', async () => {
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(
        <VirtualizedTranscriptView
          segments={[{ id: 'segment-1', timestamp: 12.5, text: 'A sample sentence' }]}
          disableAutoScroll
          showConfidence={false}
        />,
      );
    });

    expect(renderer!.root.findAllByType('button')).toHaveLength(0);
    expect(renderer!.root.findByType('span').children).toContain('[00:12]');
    await act(async () => renderer!.unmount());
  });
});
