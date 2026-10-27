// The fixture-backed client: deterministic series, an interval stream that stops when asked,
// and the server's version check on save.

import type { DashboardSpec } from '@loams-core/types';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { VersionConflict } from '../src/client.js';
import { DEFAULT_DASHBOARDS } from '../src/defaults.js';
import { createMockMetricsClient, mockPointsAt, mockValue, seriesOf } from '../src/mock.js';
import { abortable, backfillThenLive, latestBySeries } from '../src/stream.js';

const NOW = 1_760_000_000_000;

afterEach(() => {
  vi.useRealTimers();
});

describe('mock series', () => {
  it('splits by the metric label and is deterministic', () => {
    expect(seriesOf('loams_function_invocations_total')).toEqual([
      'checkout',
      'resize-image',
      'send-email',
    ]);
    expect(seriesOf('loams_postgres_computes')).toEqual(['up', 'suspended']);
    expect(seriesOf('loams_sql_wake_latency_ms')).toEqual(['p50', 'p95']);
    expect(seriesOf('loams_sql_queries_per_second')).toEqual([undefined]);
    expect(mockPointsAt('loams_sql_wake_latency_ms', NOW)).toEqual(
      mockPointsAt('loams_sql_wake_latency_ms', NOW),
    );
  });

  it('never goes negative, and counts are whole', () => {
    for (let t = NOW; t < NOW + 3_600_000; t += 37_000) {
      const v = mockValue('loams_live_worker_restarts_total', undefined, 0, t);
      expect(v).toBeGreaterThanOrEqual(0);
      expect(Number.isInteger(v)).toBe(true);
    }
  });

  it('p95 wake latency sits above p50', () => {
    const p50 = mockValue('loams_sql_wake_latency_ms', 'p50', 0, NOW);
    const p95 = mockValue('loams_sql_wake_latency_ms', 'p95', 1, NOW);
    expect(p95).toBeGreaterThan(p50);
  });

  it('query backfills the range at the step, oldest first', async () => {
    const c = createMockMetricsClient({ now: () => NOW });
    const pts = await c.query({
      metric: 'loams_postgres_connections',
      rangeMs: 60_000,
      stepMs: 10_000,
    });
    // 7 steps (both ends included) x 2 computes.
    expect(pts).toHaveLength(14);
    expect(pts[0]?.t).toBeLessThan(pts.at(-1)?.t as number);
    expect(new Set(pts.map((p) => p.series))).toEqual(new Set(['main', 'reporting']));
  });
});

describe('mock stream', () => {
  it('emits the newest points on the interval and stops on abort', async () => {
    vi.useFakeTimers();
    let now = NOW;
    const c = createMockMetricsClient({ now: () => now });
    const ctl = new AbortController();
    const it = c
      .stream({
        metric: 'loams_sql_queries_per_second',
        rangeMs: 60_000,
        stepMs: 1000,
        intervalMs: 5000,
        signal: ctl.signal,
      })
      [Symbol.asyncIterator]();
    const first = it.next();
    now += 5000;
    await vi.advanceTimersByTimeAsync(5000);
    const r = await first;
    expect(r.done).toBe(false);
    expect(r.value).toEqual([{ t: NOW + 5000, v: expect.any(Number) }]);
    const second = it.next();
    ctl.abort();
    await vi.advanceTimersByTimeAsync(0);
    expect((await second).done).toBe(true);
  });

  it('abortable aborts the underlying request when the consumer stops', async () => {
    let seen: AbortSignal | undefined;
    const src = abortable(async function* (signal) {
      seen = signal;
      yield 1;
      yield 2;
    });
    for await (const v of src) {
      expect(v).toBe(1);
      break;
    }
    expect(seen?.aborted).toBe(true);
  });

  it('backfillThenLive yields the backfill first, and only that when refresh is off', async () => {
    const c = createMockMetricsClient({ now: () => NOW });
    const batches: number[] = [];
    for await (const pts of backfillThenLive(c, {
      metric: 'loams_house_queries_total',
      rangeMs: 30_000,
      stepMs: 10_000,
      intervalMs: 0,
    }))
      batches.push(pts.length);
    expect(batches).toEqual([4]);
  });

  it('latestBySeries keeps the newest value of each series', () => {
    expect(
      latestBySeries([
        { t: 1, v: 1, series: 'a' },
        { t: 1, v: 5, series: 'b' },
        { t: 2, v: 2, series: 'a' },
        { t: 0, v: 9, series: 'a' },
      ]),
    ).toEqual([
      { series: 'a', v: 2 },
      { series: 'b', v: 5 },
    ]);
    expect(latestBySeries([{ t: 1, v: 3 }])).toEqual([{ v: 3 }]);
  });
});

describe('mock save', () => {
  it('serves the defaults, bumps the version on save, and refuses a stale base', async () => {
    const c = createMockMetricsClient();
    const list = await c.listDashboards();
    expect(list.map((d) => d.id)).toEqual(DEFAULT_DASHBOARDS.map((d) => d.id));
    const usage = list[0] as DashboardSpec;
    const saved = await c.saveDashboard({ ...usage, title: 'My usage' }, usage.version);
    expect(saved.version).toBe(usage.version + 1);
    expect((await c.listDashboards())[0]?.title).toBe('My usage');
    await expect(c.saveDashboard(usage, usage.version)).rejects.toBeInstanceOf(VersionConflict);
    expect(c.saves).toHaveLength(1);
  });

  it('a saved copy replaces its default', async () => {
    const mine = { ...(DEFAULT_DASHBOARDS[1] as DashboardSpec), version: 4, title: 'PG' };
    const c = createMockMetricsClient({ saved: [mine] });
    const list = await c.listDashboards();
    expect(list.find((d) => d.id === mine.id)).toMatchObject({ version: 4, title: 'PG' });
    expect(list).toHaveLength(DEFAULT_DASHBOARDS.length);
  });

  it('honours an aborted list', async () => {
    const c = createMockMetricsClient({ latencyMs: 50 });
    const ctl = new AbortController();
    const p = c.listDashboards(ctl.signal);
    ctl.abort();
    await expect(p).rejects.toBeDefined();
  });
});
