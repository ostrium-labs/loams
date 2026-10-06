/**
 * The cordis service that renders a `graph` widget.
 *
 * Structurally this is `plugin-echarts-render`'s `RenderService` with a
 * different terminal value, and it is a deliberate mirror rather than a
 * near-copy: both declare `static inject = ["data", "flint"]`, both fetch
 * through `ctx.data.fetchWidgetData`, and both resolve their theme through
 * `ctx.flint.resolveWidgetTheme` instead of reading `widget.flint.theme_spec` or
 * `dashboardSpec.theme` themselves. That precedence is documented at length in
 * `dashboard-spec.ts` and a per-widget override WINS over the dashboard default,
 * so re-implementing it here would be a second copy free to drift.
 *
 * What differs is everything downstream of the theme: ECharts ends in an options
 * object, this ends in a `{ nodes, edges }` pair. That is why `graph` is a
 * sibling `Widget.type` and not a `ChartSchema.kind`, and why
 * `plugin-echarts-render` declines a graph widget rather than guessing a chart
 * kind for it.
 *
 * The theme path degrades at every step. A graph tile that renders in the
 * dashboard's own colours is strictly better than a tile that throws and shows an
 * error, so an unresolvable theme, a throwing resolver and a missing flint
 * service all land on `DEFAULT_FLOW_THEME` with a log line, exactly as the
 * echarts service lands on unthemed options.
 */
import { Context, Service } from "cordis";
import { DASHBOARD_THEME_PARAM, splitRenderParams } from "@loams-plugins/plugin-echarts-render";
import { compileGraph, type CompiledGraph } from "./compiler.js";
import { flowThemeFromDecisions, DEFAULT_FLOW_THEME, type FlowTheme } from "./theme.js";
import type { DesignDecisions, ThemeReport } from "flint-chart/core";
// Side-effect imports: augment cordis Context with the `data` and `flint` keys
// this service injects. Re-declaring them locally would conflict with the
// packages that own them.
import "@loams-plugins/plugin-data";
import "@loams-plugins/plugin-flint";

/**
 * The reserved `params` key carrying the dashboard theme, and the splitter that
 * removes it.
 *
 * Imported rather than redeclared: it is the render protocol shared with
 * `plugin-echarts-render` -- the browser card posts one `{ widget, params }` body
 * to a preview route for either kind of widget -- and two copies of the same
 * string literal would be free to drift into two different protocols.
 */
export { DASHBOARD_THEME_PARAM };

/** Cap on remembered warnings, so one bad theme logs once and not once a render. */
const MAX_REMEMBERED_WARNINGS = 32;

/**
 * The slice of the flint service this package depends on.
 *
 * Declared structurally, for the same reason `plugin-echarts-render` does it: a
 * missing or differently shaped resolver degrades to "unthemed" instead of
 * failing to compile, and so the theme path can be exercised against a stub.
 */
interface FlintThemeBridge {
  resolveWidgetTheme?(widget: unknown, dashboardTheme?: unknown): ThemeResolutionLike | undefined;
  groundTheme?(spec: unknown, chartType: string, facts?: unknown): ThemeGroundingLike | undefined;
}

/** The shape of `FlintService.resolveWidgetTheme`'s return, read defensively. */
interface ThemeResolutionLike {
  valid?: boolean;
  source?: string;
  spec?: unknown;
  report?: ThemeReport[];
}

/** The shape of `FlintService.groundTheme`'s return, read defensively. */
interface ThemeGroundingLike {
  valid?: boolean;
  report?: ThemeReport[];
  decisions?: DesignDecisions;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Thrown when a widget this service cannot draw reaches it.
 *
 * The graph mirror of `NotAnEChartsWidgetError`, and for the same reason: a
 * well-formed chart widget reaching the flow renderer should not report
 * "graph needs at least one node", which is true and useless. It should say the
 * widget is not a graph and name the package that does own it.
 */
export class NotAGraphWidgetError extends Error {
  readonly widgetType: unknown;

  constructor(message: string, widgetType?: unknown) {
    super(message);
    this.name = "NotAGraphWidgetError";
    this.widgetType = widgetType;
  }
}

/** What the browser tile receives, and what a render test can assert on. */
export interface CompiledGraphWidget extends CompiledGraph {
  theme: FlowTheme;
  fitView: boolean;
  pannable: boolean;
  zoomable: boolean;
}

export class FlowRenderService extends Service {
  static inject = ["data", "flint"];

  /** Warnings already emitted, so one bad theme logs once and not once a render. */
  private readonly _warned = new Set<string>();

  constructor(ctx: Context) {
    super(ctx, "flow");
  }

  /**
   * Compile a graph widget into React Flow's `{ nodes, edges }`.
   *
   * `dashboardTheme` is passed explicitly rather than read from ambient state: a
   * render call carries what it needs. It may also arrive as
   * `params[DASHBOARD_THEME_PARAM]`, which is stripped before the data query
   * because every params entry becomes a SQL filter column; an explicit argument
   * outranks the reserved key.
   */
  async compileGraphWidget(
    widget: any,
    params?: Record<string, unknown>,
    dashboardTheme?: unknown,
  ): Promise<CompiledGraphWidget> {
    if (!isPlainObject(widget) || !isPlainObject(widget.graph)) {
      throw new NotAGraphWidgetError(
        `Not a graph widget (type: ${JSON.stringify(isPlainObject(widget) ? widget.type : widget)}); ` +
          "graphs are rendered by @loams-plugins/plugin-flow-render.",
        isPlainObject(widget) ? widget.type : widget,
      );
    }

    const split = splitRenderParams(params);
    const data = await this.ctx.data.fetchWidgetData(widget, split.dataParams);
    // A data service that answers with a bare array, a `{data}` envelope, or
    // nothing at all are all states that have to render something.
    const rows = Array.isArray(data?.data) ? data.data : Array.isArray(data) ? data : [];

    const theme = this._resolveTheme(widget, dashboardTheme ?? split.dashboardTheme);
    const graph = compileGraph(widget.graph, rows, { palette: theme.palette });

    if (graph.diagnostics.droppedNodes.length > 0 || graph.diagnostics.droppedEdges.length > 0) {
      this.ctx.logger.warn(
        "graph widget %s: dropped %d node(s) [%s] and %d edge(s) [%s] as malformed or dangling",
        typeof widget.id === "string" && widget.id.length > 0 ? widget.id : "(unnamed)",
        graph.diagnostics.droppedNodes.length,
        graph.diagnostics.droppedNodes.join(", "),
        graph.diagnostics.droppedEdges.length,
        graph.diagnostics.droppedEdges.join(", "),
      );
    }

    return {
      ...graph,
      theme,
      fitView: widget.graph.fitView !== false,
      pannable: widget.graph.pannable !== false,
      zoomable: widget.graph.zoomable !== false,
    };
  }

  /**
   * The non-throwing form, for a caller routing a mixed set of widgets.
   *
   * A preview endpoint that accepts any widget needs to answer "I do not render
   * this" without the throw becoming a 500 for the caller's mistake.
   */
  async tryCompileGraphWidget(
    widget: any,
    params?: Record<string, unknown>,
    dashboardTheme?: unknown,
  ): Promise<{ ok: true; graph: CompiledGraphWidget } | { ok: false; reason: string }> {
    try {
      return { ok: true, graph: await this.compileGraphWidget(widget, params, dashboardTheme) };
    } catch (e) {
      return { ok: false, reason: (e as Error).message };
    }
  }

  /** Flatten flint's `ThemeReport[]` into one loggable line. */
  private _describeReport(report: ThemeReport[] | undefined): string {
    if (!Array.isArray(report) || report.length === 0) return "no report attached";
    return report.map((entry) => `${entry.path}: ${entry.message}`).join("; ");
  }

  /** Log a theme failure once. Repeating it every render is noise, not signal. */
  private _warnOnce(key: string, message: string, ...detail: unknown[]): void {
    if (this._warned.has(key) || this._warned.size >= MAX_REMEMBERED_WARNINGS) return;
    this._warned.add(key);
    this.ctx.logger.warn(message, ...detail);
  }

  /**
   * Resolve the theme for a graph widget, degrading to the dashboard's own tokens.
   *
   * Four steps, each able to decline without taking the tile down: resolve the
   * selection (`resolveWidgetTheme`), ground it (`groundTheme`), map the grounded
   * ink onto graph roles, and log why if any of that said nothing. Precedence
   * between a per-widget override and the dashboard default is NOT decided here;
   * it belongs to `resolveWidgetTheme`.
   */
  private _resolveTheme(widget: unknown, dashboardTheme: unknown): FlowTheme {
    const bridge = this.ctx.flint as unknown as FlintThemeBridge | undefined;
    if (!bridge || typeof bridge.resolveWidgetTheme !== "function") {
      this._warnOnce(
        "no-bridge",
        "No flint theme service; rendering the graph with the dashboard's own tokens",
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    let resolution: ThemeResolutionLike | undefined;
    try {
      resolution = bridge.resolveWidgetTheme(widget, dashboardTheme ?? null);
    } catch (e) {
      this._warnOnce(
        `resolve-threw:${(e as Error).message}`,
        "Graph theme resolution threw; rendering unthemed:",
        (e as Error).message,
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    if (!resolution || resolution.valid !== true) {
      this._warnOnce(
        `invalid:${this._describeReport(resolution?.report)}`,
        "Graph theme could not be resolved; rendering unthemed:",
        this._describeReport(resolution?.report),
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    // `source: 'none'` is a real answer, not a failure: neither the dashboard
    // nor the widget named a house, so the graph wears the dashboard's tokens
    // exactly as it did before themes existed. `spec` is absent in that state by
    // construction.
    if (resolution.source === "none" || resolution.spec === undefined) {
      return { ...DEFAULT_FLOW_THEME };
    }

    if (typeof bridge.groundTheme !== "function") {
      this._warnOnce(
        "no-ground",
        "No groundTheme() on the flint service; rendering the graph unthemed",
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    // `graph` is not a chart type flint knows, so grounding may report that it
    // could not honour part of the spec. That is fine: `flowThemeFromDecisions`
    // maps whatever it did bind and falls back per role for the rest.
    let grounding: ThemeGroundingLike | undefined;
    try {
      grounding = bridge.groundTheme(resolution.spec, "graph", {
        markTypes: ["node", "edge"],
        titled: false,
        partToWhole: false,
      });
    } catch (e) {
      this._warnOnce(
        `ground-threw:${(e as Error).message}`,
        "Graph theme grounding threw; rendering unthemed:",
        (e as Error).message,
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    if (!grounding || grounding.valid !== true || !grounding.decisions) {
      this._warnOnce(
        `ground-invalid:${this._describeReport(grounding?.report)}`,
        "Graph theme could not be grounded; rendering unthemed:",
        this._describeReport(grounding?.report),
      );
      return { ...DEFAULT_FLOW_THEME };
    }

    try {
      return flowThemeFromDecisions(grounding.decisions);
    } catch (e) {
      this._warnOnce(
        `map-threw:${(e as Error).message}`,
        "Mapping the graph theme failed; rendering unthemed:",
        (e as Error).message,
      );
      return { ...DEFAULT_FLOW_THEME };
    }
  }
}

declare module "cordis" {
  interface Context {
    flow: FlowRenderService;
  }
}
