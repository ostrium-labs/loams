// A stand-in for @loams/charts' LiveChart in jsdom (no canvas, no ECharts): the same props,
// drawn as a plain figure a test can find by its accessible name and inspect through data
// attributes. Sparkline and useMetricStream are the real ones (SVG and a hook, no canvas).

import type { LiveChartProps } from '@loams/charts';

export function LiveChart(p: LiveChartProps) {
  return (
    <figure
      aria-label={p.ariaLabel}
      data-chart-type={p.spec.chartType}
      data-rows={p.data.length}
      data-live={p.stream ? 'yes' : 'no'}
      data-window={p.windowMs}
    />
  );
}
