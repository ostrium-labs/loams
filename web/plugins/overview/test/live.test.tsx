import { compileSpec, readChartTheme } from '@loams/charts';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { UsageChart } from '../src/live/charts.js';
import { formatKpi, KpiCard, kpiDelta } from '../src/live/kpis.js';
import { LiveDashboard } from '../src/live/live-dashboard.js';
import { MiniCanvas, nextService, toFlow } from '../src/live/mini-canvas.js';
import { KPIS, mockOverviewMetrics, PRODUCTS, TOPOLOGY } from '../src/live/mock.js';
import { trafficSpec, usageSpec } from '../src/live/specs.js';
import { startWith, ticker } from '../src/live/ticker.js';
import type { OverviewMetricsClient } from '../src/live/types.js';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const NOW = Date.UTC(2026, 9, 11, 12, 0, 0);
const panel = (id: string) => document.querySelector(`[data-panel="${id}"]`) as HTMLElement;

describe('ticker', () => {
  it('emits once per interval and stops at once on return', async () => {
    vi.useFakeTimers();
    const it = ticker(1000, (i) => i)[Symbol.asyncIterator]();
    const first = it.next();
    await vi.advanceTimersByTimeAsync(1000);
    expect(await first).toEqual({ done: false, value: 0 });
    const second = it.next();
    await it.return?.();
    // Settled by `return()`, without the timer firing.
    expect(await second).toEqual({ done: true, value: undefined });
    expect(vi.getTimerCount()).toBe(0);
    expect(await it.next()).toEqual({ done: true, value: undefined });
  });

  it('startWith yields the snapshot first, then the stream, and forwards return', async () => {
    vi.useFakeTimers();
    const it = startWith(
      async () => 'snap',
      ticker(500, () => 'tick'),
    )[Symbol.asyncIterator]();
    expect(await it.next()).toEqual({ done: false, value: 'snap' });
    const n = it.next();
    await vi.advanceTimersByTimeAsync(500);
    expect(await n).toEqual({ done: false, value: 'tick' });
    it.next();
    await it.return?.();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('mock metrics', () => {
  it('serves deterministic history and a stream that emits on an interval', async () => {
    vi.useFakeTimers();
    let t = NOW;
    const client = mockOverviewMetrics({ intervalMs: 1000, historyPoints: 10, now: () => t });
    const hist = await client.kpiHistory('requests');
    expect(hist).toHaveLength(10);
    expect(hist[9]?.t).toBe(NOW);
    expect(hist[0]?.t).toBe(NOW - 9000);
    expect(hist.every((p) => p.v >= 0)).toBe(true);

    const usage = await client.usageHistory('cpu');
    // One sample per product per point.
    expect(usage).toHaveLength(50);
    expect(new Set(usage.map((u) => u.product)).size).toBe(5);

    const it = client.trafficStream()[Symbol.asyncIterator]();
    const next = it.next();
    t += 1000;
    await vi.advanceTimersByTimeAsync(1000);
    const r = await next;
    if (r.done) throw new Error('stream ended early');
    expect(r.value.map((s) => s.series)).toEqual(['Requests/s', 'Errors/s']);
    expect(r.value[0]?.t).toBe(NOW + 1000);
    await it.return?.();
  });

  it('fixtures name no environment URLs or secrets', async () => {
    const client = mockOverviewMetrics({ now: () => NOW });
    const text = JSON.stringify([
      await client.deployments(10),
      await client.alerts(),
      await client.topology(),
    ]);
    expect(text).not.toMatch(/https?:\/\//);
    expect(text).not.toMatch(/token|secret|password/i);
  });
});

describe('kpis', () => {
  it('formats each unit and colours the delta by which way is better', () => {
    expect(formatKpi({ unit: 'rate' }, 420.4)).toBe('420/s');
    expect(formatKpi({ unit: 'percent' }, 0.5)).toBe('0.50%');
    expect(formatKpi({ unit: 'ms' }, 47.6)).toBe('48 ms');
    expect(formatKpi({ unit: 'bytes' }, 38e9)).toBe('38.0 GB');
    expect(kpiDelta([{ t: 0, v: 100 }])).toBeUndefined();
    expect(
      kpiDelta([
        { t: 0, v: 100 },
        { t: 1, v: 110 },
      ]),
    ).toBeCloseTo(10);
  });

  it('a KPI card shows the latest value and a labelled sparkline', async () => {
    const client = mockOverviewMetrics({ historyPoints: 5, now: () => NOW });
    const kpi = KPIS[0];
    if (!kpi) throw new Error('no KPIs');
    render(<KpiCard kpi={kpi} client={client} />);
    // The real Sparkline: its name carries the latest/min/max, its path one
    // segment per point after the first.
    const spark = await screen.findByRole('img', { name: /^Requests, last 5 minutes: latest / });
    await waitFor(() =>
      expect(spark.querySelector('path')?.getAttribute('d')?.match(/L/g)).toHaveLength(4),
    );
    expect(screen.getByText(/\/s$/)).toBeTruthy();
    expect(screen.queryByText('Loading')).toBeNull();
  });
});

describe('live dashboard', () => {
  it('renders every panel, says the data is a sample, and lists deployments and alerts', async () => {
    const client = mockOverviewMetrics({ now: () => NOW });
    render(<LiveDashboard client={client} now={() => NOW} />);
    // UX-5: the shared notice only, no second badge beside the heading.
    expect(screen.getAllByText('Sample data').length).toBe(1);
    await screen.findByRole('list', { name: 'Key metrics' });
    expect(screen.getAllByRole('listitem').length).toBeGreaterThanOrEqual(4);

    const traffic = within(panel('traffic')).getByLabelText(/Requests and errors per second/);
    // 60 points, two series each; the hidden table lists the latest 50.
    await waitFor(() =>
      expect(traffic.querySelector('caption')?.textContent).toMatch(/latest 50 of 120 rows/),
    );

    await within(panel('deployments')).findByText('v2.14.0');
    expect(within(panel('deployments')).getByText('Rolled back')).toBeTruthy();
    expect(within(panel('deployments')).getByText('2 min. ago')).toBeTruthy();

    await within(panel('alerts')).findByText('Error rate above 2% for 5 minutes');
    expect(within(panel('alerts')).getByText('1 firing')).toBeTruthy();
  });

  it('switches the usage chart between resources with pressed-state buttons', async () => {
    const client = mockOverviewMetrics({ now: () => NOW });
    render(<LiveDashboard client={client} now={() => NOW} />);
    const usage = panel('usage');
    const cpu = within(usage).getByRole('button', { name: 'CPU' });
    const mem = within(usage).getByRole('button', { name: 'Memory' });
    expect(cpu.getAttribute('aria-pressed')).toBe('true');
    expect(mem.getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(mem);
    expect(mem.getAttribute('aria-pressed')).toBe('true');
    const chart = await within(usage).findByLabelText(/Memory usage per product, stacked/);
    await waitFor(() =>
      expect(
        within(chart)
          .getAllByRole('columnheader')
          .map((h) => h.textContent),
      ).toContain('product'),
    );
    expect(within(chart).getByText(/Memory usage per product, stacked.*: data/)).toBeTruthy();
  });

  it('shows a failed read as Not reachable, not as numbers', async () => {
    const base = mockOverviewMetrics({ now: () => NOW });
    const client: OverviewMetricsClient = {
      ...base,
      alerts: async () => {
        throw new Error('alerts service down');
      },
    };
    render(<LiveDashboard client={client} now={() => NOW} />);
    await within(panel('alerts')).findByText('alerts service down');
    expect(within(panel('alerts')).getByText('Not reachable')).toBeTruthy();
  });

  it('says so when the KPI list cannot be read, instead of dropping the row', async () => {
    const base = mockOverviewMetrics({ now: () => NOW });
    const client: OverviewMetricsClient = {
      ...base,
      kpis: async () => {
        throw new Error('metrics service down');
      },
    };
    render(<LiveDashboard client={client} now={() => NOW} />);
    await screen.findByText('metrics service down');
    expect(screen.queryByRole('list', { name: 'Key metrics' })).toBeNull();
  });

  it('reloads usage history when the client changes, not only the resource', async () => {
    const a = mockOverviewMetrics({ now: () => NOW, historyPoints: 4 });
    const b = mockOverviewMetrics({ now: () => NOW, historyPoints: 2 });
    const { rerender } = render(<UsageChart client={a} />);
    const rows = () =>
      within(panel('usage'))
        .getByLabelText(/CPU usage per product/)
        .querySelectorAll('tbody tr').length;
    // Five products per point.
    await waitFor(() => expect(rows()).toBe(20));
    rerender(<UsageChart client={b} />);
    await waitFor(() => expect(rows()).toBe(10));
  });
});

describe('chart specs', () => {
  // Through the real compiler: a spec it would draw differently fails here, not on screen.
  const theme = readChartTheme();
  it('traffic is one line per series over a time axis', async () => {
    const rows = await mockOverviewMetrics({ now: () => NOW }).trafficHistory();
    const out = compileSpec(trafficSpec, rows, { theme });
    expect(out.kind).toBe('line');
    expect(out.temporal).toBe(true);
    expect(out.yFields).toEqual(['Requests/s', 'Errors/s']);
  });

  it('usage stacks one area per product', async () => {
    const rows = await mockOverviewMetrics({ now: () => NOW }).usageHistory('memory');
    const out = compileSpec(usageSpec('memory'), rows, { theme });
    expect(out.kind).toBe('area');
    expect(out.yFields).toEqual(PRODUCTS);
    const series = out.option.series as { stack?: string }[];
    expect(series).toHaveLength(PRODUCTS.length);
    expect(series.every((x) => x.stack === 'total')).toBe(true);
  });
});

describe('mini canvas', () => {
  it('builds read-only, labelled nodes and drops links to unknown services', () => {
    const { nodes, edges } = toFlow(
      { ...TOPOLOGY, links: [...TOPOLOGY.links, { from: 'gw', to: 'ghost' }] },
      false,
    );
    expect(nodes).toHaveLength(TOPOLOGY.services.length);
    expect(nodes.every((n) => n.draggable === false && n.connectable === false)).toBe(true);
    expect(nodes.find((n) => n.id === 'events')?.ariaLabel).toBe('events, stream, Degraded');
    expect(edges).toHaveLength(TOPOLOGY.links.length);
    expect(edges.every((e) => e.animated === false)).toBe(true);
    expect(toFlow(TOPOLOGY, true).edges.every((e) => e.animated)).toBe(true);
  });

  it('arrow keys move by position; Home and End jump to the ends', () => {
    const s = TOPOLOGY.services;
    expect(nextService(s, 'gw', 'ArrowRight')).toBe('sessions');
    expect(nextService(s, 'worker', 'ArrowRight')).toBe('orders');
    expect(nextService(s, 'worker', 'ArrowDown')).toBe('events');
    expect(nextService(s, 'orders', 'ArrowDown')).toBe('sessions');
    expect(nextService(s, 'gw', 'ArrowLeft')).toBeUndefined();
    expect(nextService(s, 'store', 'Home')).toBe('gw');
    expect(nextService(s, 'gw', 'End')).toBe('store');
    expect(nextService(s, 'gw', 'Enter')).toBeUndefined();
  });

  it('gives screen readers the services and their connections as text', async () => {
    const client = mockOverviewMetrics({ now: () => NOW });
    render(<MiniCanvas client={client} />);
    const list = await screen.findByRole('list', { name: 'Services and their connections' });
    expect(
      within(list).getByText(/api-gateway, gateway, Healthy; calls worker, events, sessions/),
    ).toBeTruthy();
    expect(screen.getByRole('group', { name: /Service map\. Use the arrow keys/ })).toBeTruthy();
  });

  it('moves focus between nodes with the arrow keys', async () => {
    const client = mockOverviewMetrics({ now: () => NOW });
    render(<MiniCanvas client={client} />);
    const map = await screen.findByRole('group', { name: /Service map/ });
    const node = (id: string) =>
      map.querySelector<HTMLElement>(`.react-flow__node[data-id="${id}"]`);
    await waitFor(() => expect(node('gw')).toBeTruthy());
    const gw = node('gw') as HTMLElement;
    act(() => gw.focus());
    fireEvent.keyDown(gw, { key: 'ArrowRight' });
    expect(document.activeElement).toBe(node('sessions'));
  });
});
