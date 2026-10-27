import { describe, expect, it } from 'vite-plus/test';
import { ChartSpecError, compileSpec, kindForChartType, pivotRows } from '../src/compile';
import { readChartTheme } from '../src/theme';
import type { ChartSpec } from '../src/types';
import { palette, token } from './setup';

type Dict = Record<string, unknown>;
const theme = readChartTheme();
const series = (option: Dict) => option.series as Dict[];
const axis = (option: Dict, key: 'xAxis' | 'yAxis') => (option[key] as Dict[])[0] as Dict;

describe('kindForChartType', () => {
  it('maps chart-specs names onto the core compiler kinds', () => {
    expect(kindForChartType('Line Chart')).toBe('line');
    expect(kindForChartType('line')).toBe('line');
    expect(kindForChartType('Stacked Bar Chart')).toBe('bar');
    expect(kindForChartType('Area Chart')).toBe('area');
    expect(kindForChartType('Scatter Plot')).toBe('scatter');
    expect(kindForChartType('Donut Chart')).toBe('pie');
    expect(kindForChartType('Heatmap')).toBe('heatmap');
    expect(kindForChartType('Sankey')).toBeUndefined();
  });
});

describe('compileSpec', () => {
  const daily: ChartSpec = {
    chartType: 'Line Chart',
    encodings: { x: 'day', y: { field: 'hours' } },
  };
  const dailyRows = [
    { day: 'Mon', hours: 3 },
    { day: 'Tue', hours: 5 },
  ];

  it('compiles through the core line kind and lays token ink on top', () => {
    const { option, kind, yFields, temporal } = compileSpec(daily, dailyRows, { theme });
    expect(kind).toBe('line');
    expect(temporal).toBe(false);
    expect(yFields).toEqual(['hours']);
    expect((option.dataset as Dict).source).toEqual(dailyRows);
    const [line] = series(option);
    if (!line) throw new Error('no line series');
    expect(line).toMatchObject({ type: 'line', name: 'hours', encode: { x: 'day', y: 'hours' } });
    expect((line?.lineStyle as Dict | undefined)?.width).toBe(2);
    expect(axis(option, 'xAxis').type).toBe('category');
    expect(option.color).toEqual(palette(':root'));
    expect(option.backgroundColor).toBe('transparent');
    expect((option.textStyle as Dict).color).toBe(theme.muted);
    expect(option.animation).toBe(true);
  });

  it('turns animation off under reduced motion', () => {
    const { option } = compileSpec(daily, dailyRows, { theme, animate: false });
    expect(option.animation).toBe(false);
    expect(option.animationDurationUpdate).toBe(0);
  });

  it('uses the dark steps when <html> is dark', () => {
    const root = document.createElement('div');
    root.classList.add('dark');
    document.body.append(root);
    const dark = readChartTheme(root);
    root.remove();
    const { option } = compileSpec(daily, dailyRows, { theme: dark });
    expect(option.color).toEqual(palette('.dark'));
    expect((option.textStyle as Dict).color).toBe(token('.dark', '--op-muted'));
  });

  it('pins a temporal x axis to the live window and normalises times to numbers', () => {
    const spec: ChartSpec = {
      chartType: 'Line Chart',
      encodings: { x: { field: 't', type: 'temporal' }, y: 'v' },
    };
    const rows = [
      { t: '1970-01-01T00:00:01.000Z', v: 1 },
      { t: 2_000, v: 2 },
    ];
    const { option, temporal } = compileSpec(spec, rows, {
      theme,
      window: { start: 0, end: 2_000 },
    });
    expect(temporal).toBe(true);
    expect(axis(option, 'xAxis')).toMatchObject({ type: 'time', min: 0, max: 2_000 });
    expect((option.dataset as Dict).source).toEqual([
      { t: 1_000, v: 1 },
      { t: 2_000, v: 2 },
    ]);
    expect(rows[0]?.t).toBe('1970-01-01T00:00:01.000Z');
  });

  it('pivots a series encoding to one line per series, keeping the carried order', () => {
    const spec: ChartSpec = {
      chartType: 'Line Chart',
      encodings: { x: 't', y: 'v', color: 'series' },
    };
    const rows = [
      { t: 1, v: 10, series: 'a' },
      { t: 1, v: 20, series: 'b' },
      { t: 2, v: 11, series: 'a' },
    ];
    const out = compileSpec(spec, rows, { theme, seriesOrder: ['b'] });
    expect(out.yFields).toEqual(['b', 'a']);
    expect(out.seriesOrder).toEqual(['b', 'a']);
    expect(series(out.option).map((s) => s.name)).toEqual(['b', 'a']);
    expect(out.option.legend).toBeDefined();
    expect((out.option.dataset as Dict).source).toEqual([
      { t: 1, b: 20, a: 10 },
      { t: 2, a: 11 },
    ]);
  });

  it('stacks a stacked bar and rounds its data ends', () => {
    const spec: ChartSpec = {
      chartType: 'Stacked Bar Chart',
      encodings: { x: 'day', y: 'h', color: 'r' },
    };
    const rows = [
      { day: 'Mon', r: 'eu', h: 1 },
      { day: 'Mon', r: 'us', h: 2 },
    ];
    const { option } = compileSpec(spec, rows, { theme });
    for (const s of series(option)) {
      expect(s.stack).toBe('total');
      expect((s.itemStyle as Dict).borderRadius).toEqual([4, 4, 0, 0]);
    }
  });

  it('compiles a pie with a surface gap between slices', () => {
    const spec: ChartSpec = { chartType: 'Pie Chart', encodings: { x: 'name', y: 'n' } };
    const { option } = compileSpec(spec, [{ name: 'a', n: 1 }], { theme });
    const [pie] = series(option);
    if (!pie) throw new Error('no pie series');
    expect(pie).toMatchObject({ type: 'pie', encode: { itemName: 'name', value: 'n' } });
    expect((pie?.itemStyle as Dict | undefined)?.borderColor).toBe(theme.bg);
  });

  it('scales a heatmap to its data on a single-hue ramp', () => {
    const spec: ChartSpec = { chartType: 'Heatmap', encodings: { x: 'h', y: 'd', color: 'n' } };
    const rows = [
      { h: 1, d: 'Mon', n: 4 },
      { h: 2, d: 'Mon', n: 9 },
    ];
    const { option } = compileSpec(spec, rows, { theme });
    expect(option.visualMap).toMatchObject({
      min: 4,
      max: 9,
      inRange: { color: [...theme.sequential] },
    });
  });

  it('declines a graph spec and names its renderer', () => {
    const spec: ChartSpec = { chartType: 'Network Graph', encodings: {} };
    expect(() => compileSpec(spec, [], { theme })).toThrow(ChartSpecError);
    expect(() => compileSpec(spec, [], { theme })).toThrow(/@loams-core\/flows/);
  });

  it('refuses an unknown chart type', () => {
    expect(() => compileSpec({ chartType: 'Sankey', encodings: {} }, [], { theme })).toThrow(
      /not supported/,
    );
  });
});

describe('pivotRows', () => {
  it('groups by x and fills one column per series', () => {
    const out = pivotRows(
      [
        { x: new Date(0), y: '3', s: 'a' },
        { x: new Date(0), y: null, s: 'b' },
      ],
      'x',
      'y',
      's',
    );
    expect(out.series).toEqual(['a', 'b']);
    expect(out.rows).toHaveLength(1);
    expect(out.rows[0]).toMatchObject({ a: 3, b: null });
  });
});
