![Loams — Your data. Your bucket.](../../../docs/assets/loams-banner.svg)

# `@loams-core/flows`

Renders `graph` dashboard widgets into the `{ nodes, edges }` pair
[React Flow](https://reactflow.dev) mounts.

## Why this is a sibling of `charts`, not a chart kind

`charts` has a registry, `registerChartKind(kind, { compile })`,
and every entry's `compile` returns an ECharts **option object**. That is the
terminal value of that pipeline: `RenderService.compileWidget` hands it to
`EChartsInstance.setOption`. The native kinds and the chart-specs assembler both end
there.

React Flow has no such option object. Its input is two arrays -- `nodes[]` and
`edges[]` -- which it renders through a React component tree with its own
custom-node components, its own handles, and its own drag, zoom and pan state.
A function that returns an options blob cannot express any of that, so there was
no integration point to register into.

So `graph` is a **sibling `Widget.type`** rather than a new `ChartSchema.kind`.
That keeps the ECharts contract intact -- every registered kind still terminates
in an option object -- and gives the graph its own terminal type. Correspondingly,
`charts` **declines** a graph widget
(`declineChartRender`) instead of throwing `Unknown chart kind: undefined`,
which is what it used to do for anything it did not recognise.

A `ChartSchema.kind` is a legitimate thing to add for the _next_ graph-shaped
visualisation that ECharts genuinely can draw -- a network chart on its `graph`
series, say. This is not that.

## Shape

Mirrors `charts` exactly:

|                 | `charts`                                | this package                     |
| --------------- | ------------------------------------------------------ | -------------------------------- |
| service         | `RenderService` (`ctx.render`)                         | `FlowRenderService` (`ctx.flow`) |
| `static inject` | `["data", "chartSpecs"]`                                    | `["data", "chartSpecs"]`              |
| data            | `ctx.data.fetchWidgetData`                             | same                             |
| theme           | `ctx.chartSpecs.resolveWidgetTheme(widget, dashboardTheme)` | same                             |
| terminal value  | ECharts option object                                  | `{ nodes, edges }`               |
| refuses         | `NotAChartWidgetError`                              | `NotAGraphWidgetError`           |

`compileGraphWidget(widget, params?, dashboardTheme?)` returns
`{ nodes, edges, diagnostics, theme, fitView, pannable, zoomable }`.
`tryCompileGraphWidget` is the non-throwing form, for a caller routing a mixed set
of widgets.

## Theme resolution

The effective theme comes from `ctx.chartSpecs.resolveWidgetTheme(widget, dashboardTheme)`,
**not** from reading `widget.chartSpec.theme_spec` or `dashboardSpec.theme` directly.
That precedence -- a per-widget override WINS over the dashboard default -- is
documented at length in `core/types/src/dashboard-spec.ts`, and a bare preset
name, a `{ preset, custom }` pair and a malformed per-widget override are all
resolved there. A second implementation would be free to drift.

`graph.ts` consumes chart-specs' **grounded** decisions, never an authored
`ThemeSpec`, for the reason `charts/src/theme-decisions.ts` sets
out: grounding is where chart-specs' ink borrowing chain, presence ordinals and
accessibility rules get applied.

Degradation is total and deliberate -- a missing chart-specs service, a throwing
resolver, an unresolvable theme, an invalid grounding and a mapper failure all
land on `DEFAULT_FLOW_THEME` with one log line (once per distinct cause). An
unthemed graph tile is strictly better than an error tile. `source: 'none'` is
treated as a real answer rather than a failure, matching the echarts service.

## Layout

`layoutGraph` ranks each node by its **longest** path from any root, then
`positionNodes` places ranks along the flow axis and siblings along the other.
Longest-path rather than shortest, so a node that is both a root and downstream
of something else cannot end up with an edge running backwards through the ranks.

Relaxation runs to a fixed point with a hard pass bound of one pass per node. A
cycle (`a -> b -> a`) never reaches a fixed point; without the bound, an
unbounded loop over author-supplied data is a hung request on the server.

## Diagnostics

`compileGraph` re-checks referential integrity that `GraphSpecSchema` already
enforces, because a preview route accepts whatever body it is posted and a
dangling endpoint's React Flow failure mode is to mount an edge that renders as
nothing -- a graph with a silently missing connection. Dropped ids are returned
in `diagnostics` rather than thrown, and the service logs them.

## Dependencies

| Package            | Licence      | Why                                          |
| ------------------ | ------------ | -------------------------------------------- |
| `cordis`           | MIT          | the plugin host                              |
| `flint-chart`      | see `NOTICE` | `DesignDecisions` / `ThemeReport` types only |
| `@loams-plugins/*` | Apache-2.0   | workspace siblings                           |

`@xyflow/react` (MIT) is a dependency of `@loams-core/dashboard-ui`, which is
where React Flow is actually imported -- this package emits the arrays and has no
opinion about the renderer, which is what keeps it testable in a node
environment with no DOM.
