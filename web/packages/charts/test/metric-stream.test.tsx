import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { mockMetricStream, seedMetricPoints } from '../src/mock';
import { useMetricStream } from '../src/use-metric-stream';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('mock metric stream', () => {
  it('emits one point per series every interval, deterministically', async () => {
    vi.useFakeTimers();
    let now = 1_000;
    const it1 = mockMetricStream({ series: ['a', 'b'], intervalMs: 500, seed: 1, now: () => now })[
      Symbol.asyncIterator
    ]();
    const pending = it1.next();
    now = 1_500;
    await vi.advanceTimersByTimeAsync(500);
    const first = await pending;
    expect(first.done).toBe(false);
    expect(first.value).toHaveLength(2);
    expect(first.value?.map((p: { series?: string; t: number }) => [p.series, p.t])).toEqual([
      ['a', 1_500],
      ['b', 1_500],
    ]);
    const again = mockMetricStream({
      series: ['a', 'b'],
      intervalMs: 500,
      seed: 1,
      now: () => 1_500,
    })[Symbol.asyncIterator]();
    const p2 = again.next();
    await vi.advanceTimersByTimeAsync(500);
    expect((await p2).value).toEqual(first.value);
  });

  it('settles a pending next and stops ticking on return', async () => {
    vi.useFakeTimers();
    const it1 = mockMetricStream({ intervalMs: 1_000 })[Symbol.asyncIterator]();
    const pending = it1.next();
    await it1.return?.();
    expect((await pending).done).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('stops after limit batches', async () => {
    vi.useFakeTimers();
    const it1 = mockMetricStream({ intervalMs: 10, limit: 1 })[Symbol.asyncIterator]();
    const a = it1.next();
    await vi.advanceTimersByTimeAsync(10);
    expect((await a).done).toBe(false);
    expect((await it1.next()).done).toBe(true);
  });

  it('seeds a window of history', () => {
    const points = seedMetricPoints(10_000, 1_000, { end: 20_000 });
    expect(points).toHaveLength(11);
    expect(points[0]?.t).toBe(10_000);
    expect(points.at(-1)?.t).toBe(20_000);
  });
});

describe('useMetricStream', () => {
  it('collects batches and windows them on the newest point', async () => {
    vi.useFakeTimers();
    let now = 0;
    const { result, unmount } = renderHook(() =>
      useMetricStream(
        () => mockMetricStream({ intervalMs: 1_000, now: () => (now += 1_000) }),
        2_500,
      ),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(result.current.map((p) => p.t)).toEqual([3_000, 4_000, 5_000]);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
