// jsdom has no layout, canvas or ResizeObserver. React Flow needs the
// observer and DOMMatrixReadOnly to mount.

import { vi } from 'vite-plus/test';

class RO {
  observe() {}
  unobserve() {}
  disconnect() {}
}
const g = globalThis as Record<string, unknown>;
g.ResizeObserver ??= RO;
g.DOMMatrixReadOnly ??= class {
  m22: number;
  constructor(t?: string) {
    const scale = t?.match(/scale\(([1-9.])\)/)?.[1];
    this.m22 = scale ? Number(scale) : 1;
  }
};
const css = g.CSS as { escape?: (s: string) => string } | undefined;
if (!css?.escape) g.CSS = { ...(css ?? {}), escape: (s: string) => s.replace(/["\\]/g, '\\$&') };

// ECharts needs a real canvas (a devDependency here only so this mock can
// name it). Only ECharts is replaced, the way
// @loams/charts' own tests do it: LiveChart, Sparkline and useMetricStream run
// for real, so these tests read what a screen reader would (the figure's label
// and the hidden data table), not ECharts' paint.
vi.mock('echarts', () => ({
  init: () => ({
    setOption() {},
    clear() {},
    resize() {},
    dispose() {},
    isDisposed: () => false,
  }),
}));
