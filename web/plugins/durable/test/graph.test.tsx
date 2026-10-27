import type { SeriesPoint } from '@loams/charts';
import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import type { PromiseRecord } from '../src/envelope.js';
import { toBase64 } from '../src/envelope.js';
import { describeStep, StepList } from '../src/graph/ExecutionGraph.js';
import { GraphTab, useNow } from '../src/graph/GraphTab.js';
import { layoutRun, neighbour } from '../src/graph/layout.js';
import {
  applyEvent,
  edgesOf,
  fromPromises,
  type RunEvent,
  type RunGraph,
} from '../src/graph/model.js';
import {
  diff,
  finalTick,
  mockMetricStream,
  mockRunSource,
  SAMPLE_PLANS,
  simulate,
} from '../src/graph/sample.js';
import {
  MAX_POLL_FAILURES,
  merge,
  promiseRunSource,
  quiet,
  withSamples,
} from '../src/graph/source.js';

// @loams/charts renders ECharts; here only what the tab hands it matters.
vi.mock('@loams/charts', () => ({
  LiveChart: (p: { ariaLabel: string; data: readonly unknown[] }) => (
    <figure aria-label={p.ariaLabel} data-rows={p.data.length} />
  ),
  Sparkline: (p: { ariaLabel: string }) => <svg role="img" aria-label={p.ariaLabel} />,
  useMetricStream: () => [],
}));

beforeAll(() => {
  // What React Flow needs from a browser that jsdom lacks.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  (globalThis as { DOMMatrixReadOnly?: unknown }).DOMMatrixReadOnly ??= class {
    m22 = 1;
  };
  globalThis.CSS ??= { escape: (s: string) => s } as unknown as typeof CSS;
});

afterEach(cleanup);

/** `v`, failing the test when it is missing. */
function must<T>(v: T | undefined): T {
  if (v === undefined) throw new Error('expected a value');
  return v;
}

const json = (v: unknown) => ({ headers: {}, data: toBase64(JSON.stringify(v)) });

const p = (id: string, over: Partial<PromiseRecord> = {}): PromiseRecord => ({
  id,
  state: 'resolved',
  param: {},
  value: {},
  tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf' },
  timeoutAt: 9_000,
  createdAt: 1_000,
  ...over,
});

const ORDER = must(SAMPLE_PLANS[0]);
const REPORT = must(SAMPLE_PLANS[1]);

function play(events: RunEvent[]): RunGraph | undefined {
  return events.reduce<RunGraph | undefined>(applyEvent, undefined);
}

describe('fromPromises', () => {
  const promises = [
    p('wf', {
      state: 'pending',
      tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf' },
      param: json({ func: 'fulfil', args: [] }),
      settledAt: undefined,
    }),
    p('wf.0', { createdAt: 1_100, settledAt: 1_200, param: json({ func: 'validate' }) }),
    // Two siblings created after wf.0 settled and overlapping: parallel.
    p('wf.1', { createdAt: 1_300, settledAt: 1_600 }),
    p('wf.2', {
      createdAt: 1_310,
      settledAt: 1_500,
      state: 'rejected',
      value: json({ message: 'boom' }),
    }),
    p('wf.3', {
      createdAt: 1_700,
      state: 'pending',
      settledAt: undefined,
      tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf', 'resonate:timeout': 'true' },
    }),
    p('wf.4', {
      createdAt: 1_800,
      state: 'pending',
      settledAt: undefined,
      tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf', 'resonate:external': 'true' },
    }),
    p('other', { tags: { 'resonate:origin': 'other' } }),
  ];
  const run = must(fromPromises('wf', promises));
  const by = (id: string) => must(run.steps.find((s) => s.id === id));

  it('maps kinds, statuses and names', () => {
    expect(run.name).toBe('fulfil');
    expect(run.status).toBe('running');
    expect(run.steps.map((s) => s.id)).not.toContain('other');
    expect(by('wf').kind).toBe('workflow');
    expect(by('wf.0').name).toBe('validate');
    expect(by('wf.0').status).toBe('succeeded');
    expect(by('wf.2').status).toBe('failed');
    expect(by('wf.2').error).toContain('boom');
    expect(by('wf.3')).toMatchObject({ kind: 'timer', status: 'sleeping', wakeAt: 9_000 });
    expect(by('wf.4')).toMatchObject({ kind: 'await', status: 'pending' });
  });

  it('infers sequence and parallel branches from time', () => {
    expect(by('wf.0').after).toEqual(['wf']);
    expect(by('wf.1').after).toEqual(['wf.0']);
    expect(by('wf.2').after).toEqual(['wf.0']);
    expect(by('wf.3').after.sort()).toEqual(['wf.1', 'wf.2']);
  });

  it('treats a timer that timed out as woken, not failed', () => {
    const r = must(
      fromPromises('wf', [
        p('wf', { tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf' } }),
        p('wf.t', {
          state: 'rejected_timedout',
          tags: { 'resonate:origin': 'wf', 'resonate:parent': 'wf', 'resonate:timer': 'true' },
        }),
      ]),
    );
    expect(r.steps.find((s) => s.id === 'wf.t')?.status).toBe('succeeded');
  });

  it('is undefined without the root', () => {
    expect(fromPromises('nope', promises)).toBeUndefined();
  });
});

describe('layout', () => {
  const run = simulate(ORDER, finalTick(ORDER), 0, 1000);
  const layout = layoutRun(run);

  it('puts parallel branches in one column', () => {
    expect(layout.columns[0]).toEqual(['order-7f3a']);
    expect(layout.columns[1]).toEqual(['order-7f3a.0']);
    expect(layout.columns[2]).toEqual(['order-7f3a.1', 'order-7f3a.2', 'order-7f3a.3']);
    expect(layout.placed.get('order-7f3a.4')?.col).toBe(3);
    expect(edgesOf(run)).toContainEqual(['order-7f3a.2', 'order-7f3a.4']);
  });

  it('moves along the graph with the arrow keys', () => {
    expect(neighbour(layout, 'order-7f3a.0', 'right')).toBe('order-7f3a.2');
    expect(neighbour(layout, 'order-7f3a.2', 'up')).toBe('order-7f3a.1');
    expect(neighbour(layout, 'order-7f3a.1', 'up')).toBe('order-7f3a.1');
    expect(neighbour(layout, 'order-7f3a.3', 'right')).toBe('order-7f3a.4');
    expect(neighbour(layout, 'order-7f3a.4', 'left')).toBe('order-7f3a.2');
    expect(neighbour(layout, 'order-7f3a', 'left')).toBe('order-7f3a');
  });
});

describe('sample runs', () => {
  const step = (r: RunGraph, id: string) => must(r.steps.find((s) => s.id === id));

  it('retries a step as stacked attempts', () => {
    // validate takes ticks 0-2; charge-card attempt 1 runs 2-4, backs off, 2 runs 5-7, ...
    const t3 = simulate(ORDER, 3, 0, 1000);
    expect(step(t3, 'order-7f3a.2')).toMatchObject({ status: 'running' });
    const t4 = simulate(ORDER, 4, 0, 1000);
    expect(step(t4, 'order-7f3a.2').status).toBe('retrying');
    expect(step(t4, 'order-7f3a.2').attempts).toHaveLength(1);
    const done = simulate(ORDER, finalTick(ORDER), 0, 1000);
    const charge = step(done, 'order-7f3a.2');
    expect(charge.status).toBe('succeeded');
    expect(charge.attempts.map((a) => a.status)).toEqual(['failed', 'failed', 'succeeded']);
    expect(done.status).toBe('succeeded');
    expect(done.endedAt).toBe(finalTick(ORDER) * 1000);
  });

  it('sleeps on a timer and waits on an await', () => {
    const awaitStart = 2 + 2 * 3 + 2; // validate, then charge-card's three attempts
    const t = simulate(ORDER, awaitStart + 1, 0, 1000);
    expect(step(t, 'order-7f3a.4').status).toBe('pending');
    expect(step(t, 'order-7f3a.4').startedAt).toBe(awaitStart * 1000);
    const s = simulate(ORDER, awaitStart + 4, 0, 1000);
    expect(step(s, 'order-7f3a.5')).toMatchObject({ kind: 'timer', status: 'sleeping' });
  });

  it('fails the run when retries run out, and stops downstream', () => {
    const done = simulate(REPORT, finalTick(REPORT), 0, 1000);
    expect(done.status).toBe('failed');
    expect(step(done, 'report-2210.2').attempts).toHaveLength(3);
    expect(step(done, 'report-2210.2').error).toMatch(/division by zero/);
    expect(step(done, 'report-2210.3').status).toBe('pending');
    expect(step(done, 'report-2210.1').status).toBe('succeeded');
  });

  it('diff events replay one state into the next', () => {
    const a = simulate(ORDER, 3, 0, 1000);
    const b = simulate(ORDER, 9, 0, 1000);
    expect(diff(a, b).reduce<RunGraph | undefined>(applyEvent, a)).toEqual(b);
  });

  it('watchRun streams to the final state', async () => {
    const src = mockRunSource({ tickMs: 1, now: () => 0 });
    const events: RunEvent[] = [];
    for await (const e of src.watchRun('order-7f3a', new AbortController().signal)) events.push(e);
    expect(events[0]?.type).toBe('snapshot');
    expect(play(events)?.status).toBe('succeeded');
  });

  it('stops when aborted', async () => {
    const src = mockRunSource({ tickMs: 5 });
    const ac = new AbortController();
    const events: RunEvent[] = [];
    for await (const e of src.watchRun('report-2210', ac.signal)) {
      events.push(e);
      ac.abort();
    }
    expect(events).toHaveLength(1);
  });

  it('the metric stream backfills, then emits on an interval', async () => {
    const ac = new AbortController();
    const it = mockMetricStream({ intervalMs: 1, signal: ac.signal, backfill: 5, now: () => 100 })[
      Symbol.asyncIterator
    ]();
    const batch = async (): Promise<SeriesPoint[]> => {
      const r = await it.next();
      if (r.done) throw new Error('metric stream ended early');
      return r.value;
    };
    const first = await batch();
    expect(first).toHaveLength(10);
    expect(new Set(first.map((x) => x.series))).toEqual(
      new Set(['fulfil-order', 'nightly-report']),
    );
    expect(first.every((x) => x.t < 100 && x.v >= 200)).toBe(true);
    const second = await batch();
    expect(second.map((x) => x.t)).toEqual([100, 100]);
    ac.abort();
    await it.return?.(undefined);
  });
});

describe('sources', () => {
  it('merge interleaves and quiet swallows a failing source', async () => {
    async function* a() {
      yield 1;
      yield 2;
    }
    async function* bad(): AsyncGenerator<number> {
      yield 3;
      throw new Error('down');
    }
    const out: number[] = [];
    for await (const x of merge(a(), quiet(bad()))) out.push(x);
    expect(out.sort()).toEqual([1, 2, 3]);
  });

  it('the promise source polls one run by its origin tag until it settles', async () => {
    let calls = 0;
    const api = {
      getPromise: vi.fn(async () => p('wf', { state: calls++ < 1 ? 'pending' : 'resolved' })),
      searchPromises: vi.fn(async () => ({ items: [p('wf.0')] })),
    };
    const src = promiseRunSource(api as never, 1);
    const events: RunEvent[] = [];
    for await (const e of src.watchRun('wf', new AbortController().signal)) events.push(e);
    expect(api.searchPromises).toHaveBeenCalledWith({
      tags: { 'resonate:origin': 'wf' },
      limit: 500,
    });
    expect(events.map((e) => e.type)).toEqual(['snapshot', 'snapshot']);
    expect(play(events)?.status).toBe('succeeded');
  });

  it('the promise source follows the search cursor to the end of a run', async () => {
    const pages: Record<string, { items: PromiseRecord[]; cursor?: string }> = {
      '': { items: [p('wf.0')], cursor: 'c1' },
      c1: { items: [p('wf.1')], cursor: 'c2' },
      c2: { items: [p('wf.2')] },
    };
    const api = {
      getPromise: vi.fn(async () => p('wf', { state: 'resolved' })),
      searchPromises: vi.fn(async (q: { cursor?: string }) => must(pages[q.cursor ?? ''])),
    };
    const src = promiseRunSource(api as never, 1);
    const events: RunEvent[] = [];
    for await (const e of src.watchRun('wf', new AbortController().signal)) events.push(e);
    expect(api.searchPromises).toHaveBeenCalledTimes(3);
    expect(api.searchPromises).toHaveBeenLastCalledWith({
      tags: { 'resonate:origin': 'wf' },
      limit: 500,
      cursor: 'c2',
    });
    expect(
      play(events)
        ?.steps.map((s) => s.id)
        .sort(),
    ).toEqual(['wf', 'wf.0', 'wf.1', 'wf.2']);
  });

  it('the promise source rides out a failed poll, and gives up after several in a row', async () => {
    let calls = 0;
    const flaky = {
      getPromise: vi.fn(async () => {
        calls++;
        if (calls === 2) throw new Error('blip');
        return p('wf', { state: calls < 3 ? 'pending' : 'resolved' });
      }),
      searchPromises: vi.fn(async () => ({ items: [p('wf.0')] })),
    };
    const events: RunEvent[] = [];
    for await (const e of promiseRunSource(flaky as never, 1).watchRun(
      'wf',
      new AbortController().signal,
    ))
      events.push(e);
    expect(play(events)?.status).toBe('succeeded');

    const down = {
      getPromise: vi.fn(async () => {
        throw new Error('engine down');
      }),
      searchPromises: vi.fn(async () => ({ items: [] })),
    };
    const watch = async () => {
      for await (const _ of promiseRunSource(down as never, 1).watchRun(
        'wf',
        new AbortController().signal,
      )) {
        // no snapshot ever arrives
      }
    };
    await expect(watch()).rejects.toThrow('engine down');
    expect(down.getPromise).toHaveBeenCalledTimes(MAX_POLL_FAILURES);
  });

  it('withSamples lists samples after the engine runs, and survives a down engine', async () => {
    const down = {
      listRuns: async () => {
        throw new Error('no engine');
      },
      watchRun: () => {
        throw new Error('no engine');
      },
      metrics: () => quiet((async function* () {})()),
    };
    const src = withSamples(down as never, mockRunSource({ tickMs: 1 }));
    const runs = await src.listRuns();
    expect(runs.map((r) => r.runId)).toEqual(['sample:order-7f3a', 'sample:report-2210']);
    expect(runs.every((r) => r.sample)).toBe(true);
    const first = await src
      .watchRun('sample:report-2210', new AbortController().signal)
      [Symbol.asyncIterator]()
      .next();
    expect(first.value).toMatchObject({ type: 'snapshot', run: { runId: 'sample:report-2210' } });
  });
});

describe('graph ui', () => {
  it('describes a retried step for screen readers', () => {
    const done = simulate(ORDER, finalTick(ORDER), 0, 1000);
    const charge = must(done.steps.find((s) => s.id === 'order-7f3a.2'));
    expect(describeStep(charge)).toMatch(/^charge-card, step, succeeded, attempt 3, took/);
  });

  it('the step list selects with the keyboard-reachable buttons', () => {
    const run = simulate(ORDER, 3, 0, 1000);
    const onSelect = vi.fn();
    render(<StepList run={run} selectedId="order-7f3a.0" onSelect={onSelect} />);
    const list = screen.getByRole('list', { name: 'Steps of fulfil-order' });
    const buttons = within(list).getAllByRole('button');
    expect(buttons).toHaveLength(run.steps.length);
    expect(buttons[1]?.getAttribute('aria-current')).toBe('true');
    fireEvent.click(must(buttons[2]));
    expect(onSelect).toHaveBeenCalledWith('order-7f3a.1');
  });

  it('the tab streams a sample run, opens a step, and charts its latency', async () => {
    const nothing = () => (async function* () {})();
    const engine = { listRuns: async () => [], watchRun: nothing, metrics: nothing };
    const source = withSamples(engine, mockRunSource({ tickMs: 60_000 }));
    render(<GraphTab source={source} />);
    const run = await screen.findByRole('combobox', { name: 'Run' });
    expect(
      within(run)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual([
      'fulfil-order · sample:order-7f3a (sample)',
      'nightly-report · sample:report-2210 (sample)',
    ]);
    const list = await screen.findByRole('list', { name: 'Steps of fulfil-order' });
    expect(screen.getAllByText('Sample data').length).toBeGreaterThanOrEqual(2);
    expect(
      screen.getByRole('region', { name: /Execution graph of fulfil-order: 9 steps/ }),
    ).toBeTruthy();
    fireEvent.click(within(list).getByRole('button', { name: /^validate-order/ }));
    expect(await screen.findByLabelText('Detail of step validate-order')).toBeTruthy();
    expect(screen.getByText(/ord_7f3a/)).toBeTruthy();
    expect(
      screen
        .getByLabelText('Latency of each step of fulfil-order, in milliseconds')
        .getAttribute('data-rows'),
    ).toBe('1');
    fireEvent.click(screen.getByRole('button', { name: 'Close step detail' }));
    await waitFor(() =>
      expect(screen.queryByLabelText('Detail of step validate-order')).toBeNull(),
    );
  });
});

describe('useNow', () => {
  it('ticks while given an interval and freezes without one', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(1_000);
      const { result, rerender } = renderHook(({ ms }: { ms?: number }) => useNow(ms), {
        initialProps: { ms: 1_000 } as { ms?: number },
      });
      expect(result.current).toBe(1_000);
      act(() => vi.advanceTimersByTime(2_000));
      expect(result.current).toBe(3_000);
      rerender({ ms: undefined });
      act(() => vi.advanceTimersByTime(5_000));
      expect(result.current).toBe(3_000);
    } finally {
      vi.useRealTimers();
    }
  });
});
