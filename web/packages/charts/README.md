# @loams/charts

The console's one chart wrapper. Plugins import these and do not wrap ECharts
themselves.

```ts
import {
  LiveChart,
  Sparkline,
  useMetricStream,
  type ChartSpec,
  type SeriesPoint,
} from '@loams/charts';
```

| Export                                                              | What it does                                                                                                                                                                                   |
| ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `LiveChart({ spec, data, stream?, windowMs?, height?, ariaLabel })` | Compiles a `ChartSpec` through `@loams-core/charts` and draws it with ECharts. `stream` batches are appended, trimmed to `windowMs` of the spec's x field, and repainted at most every 250 ms. |
| `Sparkline({ points, ariaLabel, height? })`                         | Inline SVG trend, one path per `series`.                                                                                                                                                       |
| `useMetricStream(source, windowMs)`                                 | Subscribes to `source()` and returns the windowed points.                                                                                                                                      |
| `mockMetricStream`, `mockRowStream`, `seedMetricPoints`             | Deterministic fixture streams that emit on an interval, for pages with no backend yet.                                                                                                         |

`ChartSpec` is the type from `@loams-core/types` (the schema `@loams-core/chart-specs`
validates), re-exported here.

## Behaviour

- **Lifecycle.** One ECharts instance per chart, resized by a ResizeObserver and
  disposed on unmount. Stream subscriptions call `return()` on unmount.
- **Window.** Anchored on the newest row's time, not the wall clock, so a stalled
  stream holds its picture. Out-of-order batches merge by time; at most 5,000 rows are kept.
- **Props.** `spec` and `data` are compared by content, so inline literals are fine.
  Memoise `stream`: a new iterable restarts the subscription.
- **Theme.** Ink comes from the `--op-*` tokens in `@loams/ui/tokens.css` and follows
  the `.dark` class on `<html>`. Series colours are a validated categorical order with
  separate light and dark steps; override with `--op-chart-1` .. `--op-chart-8`.
- **Motion.** `prefers-reduced-motion: reduce` turns every chart animation off.
- **Accessibility.** The canvas is `aria-hidden`; the `<figure>` carries `ariaLabel`, and
  a visually hidden table lists the latest 50 rows. A spec that cannot be drawn shows
  a message in place of the chart and keeps the table.
- **Kinds.** Line, area, bar (stacked and grouped), pie/donut, scatter, heatmap, funnel and
  waterfall. A `color`/`series` encoding on line, area or bar pivots long rows to one series
  per value, and colours stay with the series as the window moves. Graph specs are declined
  to `@loams-core/flows`.

## Gallery

`import { ChartGallery } from '@loams/charts/gallery'` renders selected chart kinds (line, area, stacked bar, donut) on mock data,
with a dark-mode toggle.
