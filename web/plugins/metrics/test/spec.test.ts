// The dashboard model: panels as @loams-core/dashboards widgets, layout edits that stay on
// the 12-column grid, and the schema the dashboards service validates with.

import { validateManifest } from '@loams/console-host';
import type { DashboardSpec } from '@loams-core/types';
import { describe, expect, it } from 'vite-plus/test';
import pkg from '../package.json';
import { formatValue, METRICS, metricDef } from '../src/catalog.js';
import { DEFAULT_DASHBOARDS } from '../src/defaults.js';
import {
  addPanel,
  applyGrid,
  GRID_COLUMNS,
  isDirty,
  MIN_W,
  newPanelId,
  nudgePanel,
  panelsOf,
  panelWidget,
  rangeOf,
  readPanel,
  refreshOf,
  removePanel,
  setParam,
  stepFor,
  updatePanel,
  validateDashboard,
} from '../src/spec.js';

const empty = (): DashboardSpec => ({
  id: 'd',
  version: 3,
  title: 'D',
  params: [],
  layout: [],
  widgets: {},
});

const usage = () => structuredClone(DEFAULT_DASHBOARDS[0] as DashboardSpec);

describe('defaults', () => {
  it('ships the five dashboards, each valid under the dashboards schema', () => {
    expect(DEFAULT_DASHBOARDS.map((d) => d.title)).toEqual([
      'Usage',
      'Postgres',
      'Loams SQL',
      'Live',
      'House',
    ]);
    for (const d of DEFAULT_DASHBOARDS) {
      const v = validateDashboard(d);
      expect(v.errors).toEqual([]);
      expect(v.valid).toBe(true);
    }
  });

  it('charts only catalog metrics, and Usage reads the loams_function_* family', () => {
    for (const d of DEFAULT_DASHBOARDS) {
      for (const p of panelsOf(d)) expect(metricDef(p.metric)).toBeDefined();
    }
    const metrics = panelsOf(usage()).map((p) => p.metric);
    expect(metrics.every((m) => m.startsWith('loams_function_'))).toBe(true);
    const splits = new Set(metrics.map((m) => metricDef(m)?.splitBy));
    expect(splits).toEqual(new Set(['function', 'namespace']));
  });

  it('names Postgres, Loams SQL, Live and House metrics the task asks for', () => {
    const ids = new Set(METRICS.map((m) => m.id));
    for (const id of [
      'loams_postgres_computes',
      'loams_postgres_wal_lag_bytes',
      'loams_postgres_connections',
      'loams_sql_wake_latency_ms',
      'loams_sql_active_keyspaces',
      'loams_sql_queries_per_second',
      'loams_live_invocations_total',
      'loams_live_errors_total',
      'loams_live_worker_restarts_total',
      'loams_house_queries_total',
      'loams_house_bytes_scanned_total',
    ])
      expect(ids.has(id)).toBe(true);
  });
});

describe('panels', () => {
  it('a chart panel is a chart widget with a chartSpec; a stat is a kpi widget', () => {
    const line = panelWidget('a', 'loams_sql_queries_per_second', 'line');
    expect(line.type).toBe('chart');
    expect(line.chartSpec?.chartType).toBe('Line Chart');
    expect(line.chartSpec?.title).toBe('QPS');
    expect(line.chart).toBeUndefined();
    const stat = panelWidget('b', 'loams_sql_active_keyspaces', 'stat', '  ');
    expect(stat.type).toBe('kpi');
    expect(readPanel(stat)).toEqual({
      id: 'b',
      metric: 'loams_sql_active_keyspaces',
      type: 'stat',
      title: 'Active keyspaces',
    });
  });

  it('readPanel ignores widgets it does not draw', () => {
    expect(readPanel({ id: 'x', type: 'text' })).toBeUndefined();
    expect(readPanel(undefined)).toBeUndefined();
  });

  it('panelsOf reads in layout order', () => {
    const s = addPanel(
      addPanel(empty(), panelWidget('b', 'loams_house_queries_total', 'bar'), { x: 6, y: 0 }),
      panelWidget('a', 'loams_house_queries_total', 'line'),
      { x: 0, y: 0 },
    );
    expect(panelsOf(s).map((p) => p.id)).toEqual(['a', 'b']);
  });

  it('newPanelId is unique on the dashboard', () => {
    let s = empty();
    const a = newPanelId(s, 'loams_house_queries_total');
    s = addPanel(s, panelWidget(a, 'loams_house_queries_total', 'line'));
    const b = newPanelId(s, 'loams_house_queries_total');
    expect(a).toBe('house-queries-total-1');
    expect(b).toBe('house-queries-total-2');
  });
});

describe('layout edits', () => {
  it('addPanel puts a panel at the left edge below everything, as DashboardSpecService does', () => {
    const s = usage();
    const bottom = s.layout.reduce((m, it) => Math.max(m, it.y + it.h), 0);
    const next = addPanel(s, panelWidget('new', 'loams_live_errors_total', 'bar'));
    expect(next.layout.at(-1)).toEqual({ id: 'new', x: 0, y: bottom, w: 6, h: 3 });
    expect(next.widgets.new?.chartSpec?.chartType).toBe('Bar Chart');
    expect(next.version).toBe(s.version);
    expect(s.widgets.new).toBeUndefined();
  });

  it('addPanel clamps a position onto the grid', () => {
    const s = addPanel(empty(), panelWidget('w', 'loams_live_errors_total', 'line'), {
      x: 11,
      y: -4,
      w: 40,
      h: 0,
    });
    expect(s.layout[0]).toEqual({ id: 'w', x: 0, y: 0, w: GRID_COLUMNS, h: 2 });
  });

  it('removePanel drops the widget and its layout row', () => {
    const s = usage();
    const id = s.layout[0]?.id as string;
    const next = removePanel(s, id);
    expect(next.widgets[id]).toBeUndefined();
    expect(next.layout.some((l) => l.id === id)).toBe(false);
    expect(removePanel(next, id)).toBe(next);
  });

  it('nudgePanel moves and resizes by a cell, never off the grid', () => {
    let s = addPanel(empty(), panelWidget('w', 'loams_live_errors_total', 'line'), {
      x: 0,
      y: 0,
      w: 6,
      h: 3,
    });
    s = nudgePanel(s, 'w', { dx: -1, dy: -1 });
    expect(s.layout[0]).toMatchObject({ x: 0, y: 0 });
    s = nudgePanel(s, 'w', { dx: 1 });
    expect(s.layout[0]).toMatchObject({ x: 1 });
    s = nudgePanel(s, 'w', { dw: 10 });
    // Wider than the space to its right: it shifts left to stay on the grid.
    expect(s.layout[0]).toMatchObject({ x: 0, w: GRID_COLUMNS });
    for (let i = 0; i < 20; i += 1) s = nudgePanel(s, 'w', { dw: -1 });
    expect(s.layout[0]?.w).toBe(MIN_W);
  });

  it('applyGrid writes a drag back, and returns the same spec when nothing moved', () => {
    const s = usage();
    const same = applyGrid(
      s,
      s.layout.map((l) => ({ i: l.id, x: l.x, y: l.y, w: l.w, h: l.h })),
    );
    expect(same).toBe(s);
    const first = s.layout[0];
    if (!first) throw new Error('no layout');
    const moved = applyGrid(s, [{ i: first.id, x: 4, y: 9, w: 5, h: 4 }]);
    expect(moved.layout[0]).toEqual({ id: first.id, x: 4, y: 9, w: 5, h: 4 });
  });

  it('updatePanel changes the chart type and keeps the title; a new metric takes its label', () => {
    let s = addPanel(empty(), panelWidget('w', 'loams_live_errors_total', 'line', 'Mine'));
    s = updatePanel(s, 'w', { type: 'area' });
    expect(readPanel(s.widgets.w)).toMatchObject({ type: 'area', title: 'Mine' });
    s = updatePanel(s, 'w', { metric: 'loams_house_queries_total' });
    expect(readPanel(s.widgets.w)).toMatchObject({
      metric: 'loams_house_queries_total',
      title: 'Queries',
    });
    expect(s.layout).toHaveLength(1);
  });
});

describe('params', () => {
  it('time range and refresh are dashboard params with defaults', () => {
    const s = empty();
    expect(rangeOf(s).id).toBe('1h');
    expect(refreshOf(s).id).toBe('15s');
    const t = setParam(setParam(s, 'range', '6h'), 'refresh', 'off');
    expect(rangeOf(t).ms).toBe(6 * 60 * 60_000);
    expect(refreshOf(t).ms).toBe(0);
    expect(t.params.filter((p) => p.name === 'range')).toHaveLength(1);
    expect(rangeOf(setParam(s, 'range', 'nonsense')).id).toBe('1h');
    expect(isDirty(s, t)).toBe(true);
    expect(isDirty(s, { ...s, version: 9 })).toBe(false);
  });

  it('stepFor gives about 120 points, never under a second', () => {
    expect(stepFor(60 * 60_000)).toBe(30_000);
    expect(stepFor(10_000)).toBe(1000);
  });
});

describe('validation', () => {
  it('reports the schema errors the dashboards service would', () => {
    const bad = { ...empty(), widgets: { w: { id: 'w', type: 'chart' } } };
    const v = validateDashboard(bad);
    expect(v.valid).toBe(false);
    expect(v.errors.join(' ')).toMatch(/exactly one of 'chartSpec' or 'chart'/);
    expect(validateDashboard({ ...empty(), title: '' }).valid).toBe(false);
  });
});

describe('format', () => {
  it('formats values in their unit', () => {
    expect(formatValue(1536, 'bytes')).toBe('1.5 KiB');
    expect(formatValue(3 * 1024 * 1024, 'bytes_per_second')).toBe('3.0 MiB/s');
    expect(formatValue(42.5, 'per_second')).toBe('42.5/s');
    expect(formatValue(12_345, 'count')).toBe('12.3k');
    expect(formatValue(180, 'ms')).toBe('180 ms');
    expect(formatValue(Number.NaN, 'ms')).toBe('-');
  });
});

describe('manifest', () => {
  it('is a valid desktop console plugin manifest', () => {
    const m = validateManifest(pkg);
    expect(m.editions).toEqual(['desktop']);
    expect(m.inject).toEqual(['router', 'slots']);
  });
});
