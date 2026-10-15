import { afterAll, describe, expect, mock, test } from 'bun:test';
import React from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const originalMotion = await import('framer-motion');
const originalTooltip = await import('../../src/components/ui/tooltip');
const originalVirtual = await import('@tanstack/react-virtual');
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

// Deterministic viewport replaces DOM measurement only. Both real rendering
// branches and the real timestamp click handler remain under test.
mock.module('@tanstack/react-virtual', () => ({
  ...originalVirtual,
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, start: index * 60, key: index })),
    getTotalSize: () => count * 60,
    measureElement: () => {},
  }),
}));

const { VirtualizedTranscriptView } = await import('../../src/components/VirtualizedTranscriptView');
afterAll(() => {
  mock.module('framer-motion', () => originalMotion);
  mock.module('../../src/components/ui/tooltip', () => originalTooltip);
  mock.module('@tanstack/react-virtual', () => originalVirtual);
});

describe('transcript playback seeking', () => {
  test('clicking a transcript timestamp seeks to its segment start', async () => {
    const seekTo = mock((_seconds: number) => {});
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(
        <VirtualizedTranscriptView
          segments={[{ id: 'segment-1', timestamp: 12.5, timingPrecision: 'exact', text: 'A sample sentence' }]}
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
          segments={[{ id: 'segment-1', timestamp: 12.5, timingPrecision: 'exact', text: 'A sample sentence' }]}
          disableAutoScroll
          showConfidence={false}
        />,
      );
    });

    expect(renderer!.root.findAllByType('button')).toHaveLength(0);
    expect(renderer!.root.findByType('span').children).toContain('[00:12]');
    await act(async () => renderer!.unmount());
  });
  for (const count of [1, 10]) {
    test(`unknown and invalid times cannot seek in a ${count === 1 ? 'small' : 'virtualized'} list`, async () => {
      for (const timestamp of [undefined, null, NaN, Infinity, -1]) {
        const seekTo = mock((_seconds: number) => {});
        let renderer: ReactTestRenderer | undefined;
        await act(async () => {
          renderer = create(<VirtualizedTranscriptView
            segments={Array.from({ length: count }, (_, index) => ({ id: `unknown-${index}`, timestamp, text: 'Unknown recording time' }))}
            disableAutoScroll showConfidence={false} onSeekTo={seekTo}
          />);
        });
        try {
          expect(renderer!.root.findAllByType('button')).toHaveLength(0);
          expect(renderer!.root.findAllByType('span').filter(node => node.children.includes('[--:--]'))).toHaveLength(count);
          expect(seekTo).not.toHaveBeenCalled();
        } finally { await act(async () => renderer!.unmount()); }
      }
    });

    test(`approximate times are labeled and seek without rounding in a ${count === 1 ? 'small' : 'virtualized'} list`, async () => {
      const seekTo = mock((_seconds: number) => {});
      let renderer: ReactTestRenderer | undefined;
      await act(async () => {
        renderer = create(<VirtualizedTranscriptView
          segments={Array.from({ length: count }, (_, index) => ({ id: `approximate-${index}`, timestamp: 12.5, timingPrecision: 'approximate', text: 'Approximate recording time' }))}
          disableAutoScroll showConfidence={false} onSeekTo={seekTo}
        />);
      });
      try {
        const buttons = renderer!.root.findAllByType('button');
        expect(buttons).toHaveLength(count);
        expect(buttons[0].children).toContain('[约 00:12]');
        expect(buttons[0].props['aria-label']).toBe('Play from approximately [00:12]');
        await act(async () => buttons[0].props.onClick());
        expect(seekTo).toHaveBeenCalledWith(12.5);
      } finally { await act(async () => renderer!.unmount()); }
    });
  }

  test('invalid bounds and unavailable precision stay noninteractive in a virtualized list', async () => {
    const invalid = [
      { id: 'reversed', timestamp: 12, endTime: 11, timingPrecision: 'exact' as const },
      { id: 'nonfinite-end', timestamp: 12, endTime: Infinity, timingPrecision: 'approximate' as const },
      { id: 'unavailable', timestamp: 12, timingPrecision: 'unavailable' as const },
    ];
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(<VirtualizedTranscriptView
        segments={Array.from({ length: 10 }, (_, index) => ({ ...invalid[index % 3], id: `invalid-${index}`, text: 'Invalid bounds' }))}
        disableAutoScroll showConfidence={false} onSeekTo={() => { throw new Error('invalid bounds must not seek'); }}
      />);
    });
    try {
      expect(renderer!.root.findAllByType('button')).toHaveLength(0);
      expect(renderer!.root.findAllByType('span').filter(node => node.children.includes('[--:--]'))).toHaveLength(10);
    } finally { await act(async () => renderer!.unmount()); }
  });

  test('virtualized exact and approximate zero remain distinguishable and seekable', async () => {
    const seekTo = mock((_seconds: number) => {});
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(<VirtualizedTranscriptView
        segments={Array.from({ length: 10 }, (_, index) => ({
          id: `zero-${index}`, timestamp: 0, endTime: 1,
          timingPrecision: index % 2 === 0 ? 'exact' : 'approximate', text: 'A real zero-second time',
        }))}
        disableAutoScroll showConfidence={false} onSeekTo={seekTo}
      />);
    });
    try {
      const buttons = renderer!.root.findAllByType('button');
      expect(buttons).toHaveLength(10);
      expect(buttons[0].children).toContain('[00:00]');
      expect(buttons[1].children).toContain('[约 00:00]');
      await act(async () => { buttons[0].props.onClick(); buttons[1].props.onClick(); });
      expect(seekTo.mock.calls).toEqual([[0], [0]]);
    } finally { await act(async () => renderer!.unmount()); }
  });

  test('explicitly unavailable precision cannot seek even with a numeric start', async () => {
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(<VirtualizedTranscriptView
        segments={[{ id: 'unavailable', timestamp: 0, timingPrecision: 'unavailable', text: 'No verified time' }]}
        disableAutoScroll showConfidence={false} onSeekTo={() => { throw new Error('unavailable time must not seek'); }}
      />);
    });
    try {
      expect(renderer!.root.findAllByType('button')).toHaveLength(0);
      expect(renderer!.root.findByType('span').children).toContain('[--:--]');
    } finally { await act(async () => renderer!.unmount()); }
  });

  test('a genuine zero-second start remains seekable', async () => {
    const seekTo = mock((_seconds: number) => {});
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(<VirtualizedTranscriptView
        segments={[{ id: 'zero', timestamp: 0, endTime: 1, timingPrecision: 'exact', text: 'Recording starts here' }]}
        disableAutoScroll showConfidence={false} onSeekTo={seekTo}
      />);
    });
    try {
      const button = renderer!.root.findByType('button');
      expect(button.children).toContain('[00:00]');
      await act(async () => button.props.onClick());
      expect(seekTo).toHaveBeenCalledWith(0);
    } finally { await act(async () => renderer!.unmount()); }
  });

  test('legacy numeric times without precision are approximate even without playback', async () => {
    let renderer: ReactTestRenderer | undefined;
    await act(async () => {
      renderer = create(<VirtualizedTranscriptView
        segments={[{ id: 'legacy', timestamp: 12.5, text: 'Old recording with bounds' }]}
        disableAutoScroll showConfidence={false}
      />);
    });
    try {
      expect(renderer!.root.findAllByType('button')).toHaveLength(0);
      expect(renderer!.root.findByType('span').children).toContain('[约 00:12]');
    } finally { await act(async () => renderer!.unmount()); }
  });

});
