import { expect, test } from 'bun:test';
import { getTranscriptTiming } from '../../src/lib/transcript-timing';

test('an end time cannot invent a start and invalid ends cannot authorize seeking', () => {
  for (const bounds of [
    { timestamp: undefined, endTime: 20 },
    { timestamp: 12, endTime: 11 },
    { timestamp: 12, endTime: -1 },
    { timestamp: 12, endTime: NaN },
    { timestamp: 12, endTime: Infinity },
  ]) {
    const timing = getTranscriptTiming({ ...bounds, timingPrecision: 'exact' });
    expect(timing.label).toBe('[--:--]');
    expect(timing.seekSeconds).toBeUndefined();
    expect(timing.playLabel).toBeUndefined();
  }
});

test('valid legacy starts need no end and remain approximate, including true zero', () => {
  expect(getTranscriptTiming({ timestamp: 0 })).toMatchObject({
    label: '[约 00:00]', seekSeconds: 0, playLabel: 'Play from approximately [00:00]',
  });
  expect(getTranscriptTiming({ timestamp: 0, endTime: null })).toMatchObject({
    label: '[约 00:00]', seekSeconds: 0,
  });
  expect(getTranscriptTiming({ timestamp: 3690.9 })).toMatchObject({
    label: '[约 61:30]', seekSeconds: 3690.9, playLabel: 'Play from approximately [61:30]',
  });
});
