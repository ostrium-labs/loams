import { describe, expect, it } from 'vite-plus/test';
import type { Row } from '../src/types';
import { appendWindow, latestTime, timeOf, windowBounds, windowPoints } from '../src/window';

const row = (t: unknown, v = 0) => ({ t, v });

describe('timeOf', () => {
  it('reads numbers, Dates and ISO strings, and nothing else', () => {
    expect(timeOf(1_000)).toBe(1_000);
    expect(timeOf(new Date(2_000))).toBe(2_000);
    expect(timeOf('1970-01-01T00:00:03.000Z')).toBe(3_000);
    expect(timeOf(Number.NaN)).toBeUndefined();
    expect(timeOf('soon')).toBeUndefined();
    expect(timeOf(null)).toBeUndefined();
    expect(timeOf('')).toBeUndefined();
  });
});

describe('appendWindow', () => {
  const opts = { windowMs: 10_000, timeField: 't' };

  it('appends and trims to the window anchored on the newest row', () => {
    const rows = [row(0), row(5_000)];
    const next = appendWindow(rows, [row(12_000)], opts);
    expect(next.map((r) => r.t)).toEqual([5_000, 12_000]);
    // Inputs are untouched.
    expect(rows).toHaveLength(2);
  });

  it('keeps a row exactly on the window edge', () => {
    const next = appendWindow([row(0)], [row(10_000)], opts);
    expect(next.map((r) => r.t)).toEqual([0, 10_000]);
  });

  it('anchors on an explicit now, dropping rows from the future', () => {
    const next = appendWindow([row(1_000), row(9_000)], [row(20_000)], { ...opts, now: 12_000 });
    expect(next.map((r) => r.t)).toEqual([9_000]);
  });

  it('merges an out-of-order batch by time, keeping arrival order for ties', () => {
    const rows = [row(1_000, 1), row(3_000, 3)];
    const next = appendWindow(rows, [row(2_000, 2), row(3_000, 4)], opts);
    expect(next.map((r) => r.v)).toEqual([1, 2, 3, 4]);
  });

  it('keeps rows without a time and sorts them last', () => {
    const next = appendWindow([row(1_000)], [row('n/a', 9), row(500)], opts);
    expect(next.map((r) => r.t)).toEqual([500, 1_000, 'n/a']);
  });

  it('accepts Date and ISO times', () => {
    const next = appendWindow([{ t: new Date(0) }], [{ t: '1970-01-01T00:00:20.000Z' }], opts);
    expect(next).toHaveLength(1);
  });

  it('keeps timed rows ahead of untimed ones under maxRows', () => {
    const timed: Row[] = [{ t: 1_000 }, { t: 2_000 }, { t: 3_000 }];
    const untimed: Row[] = [{ t: 'x' }, { t: 'y' }, { t: 'z' }];
    expect(appendWindow(timed, untimed, { ...opts, maxRows: 2 })).toEqual([
      { t: 2_000 },
      { t: 3_000 },
    ]);
    expect(appendWindow(timed, untimed, { ...opts, maxRows: 4 })).toEqual([
      { t: 1_000 },
      { t: 2_000 },
      { t: 3_000 },
      { t: 'z' },
    ]);
  });

  it('caps rows at maxRows, newest kept', () => {
    const many = Array.from({ length: 10 }, (_, i) => row(i));
    const next = appendWindow([], many, { ...opts, maxRows: 3 });
    expect(next.map((r) => r.t)).toEqual([7, 8, 9]);
  });

  it('keeps everything when windowMs is zero', () => {
    const next = appendWindow([row(0)], [row(1_000_000)], { ...opts, windowMs: 0 });
    expect(next).toHaveLength(2);
  });

  it('trims with an empty batch when now moves on', () => {
    const next = appendWindow([row(0), row(8_000)], [], { ...opts, now: 15_000 });
    expect(next.map((r) => r.t)).toEqual([8_000]);
  });
});

describe('windowPoints and bounds', () => {
  it('windows metric points on t', () => {
    const next = windowPoints(
      [
        { t: 0, v: 1 },
        { t: 4_000, v: 2 },
      ],
      [{ t: 6_000, v: 3, series: 'a' }],
      5_000,
    );
    expect(next).toEqual([
      { t: 4_000, v: 2 },
      { t: 6_000, v: 3, series: 'a' },
    ]);
  });

  it('reports the axis range for the window', () => {
    const rows = [row(2_000), row(9_000)];
    expect(latestTime(rows, 't')).toBe(9_000);
    expect(windowBounds(rows, 't', 5_000)).toEqual({ start: 4_000, end: 9_000 });
    expect(windowBounds([], 't', 5_000)).toBeUndefined();
  });
});
