import { Context, Service } from "cordis";
import { registerChartKind, compileNativeWidget, declineEChartsRender } from "./compiler.js";
import type { RenderDecline } from "./compiler.js";
import { applyGroundedInk, buildThemeFacts, collectUnmappedDecisions } from "./theme-decisions.js";
import type { ThemeGroundingFacts } from "./theme-decisions.js";
import type { DesignDecisions, ThemeReport } from "flint-chart/core";
// Side-effect imports: augment cordis Context with the `data` and `flint` keys
// this service injects. Re-declaring them locally would conflict with the
// packages that own them.
import "@loams-plugins/plugin-data";
import "@loams-plugins/plugin-flint";

/**
 * The reserved `params` key a caller may use to hand the dashboard theme down
 * with the rest of the render parameters.
 *
 * `compileWidget(widget, params, dashboardTheme)` is the explicit way to pass
 * it, and is what a server route should use. This key exists for callers that
 * cannot widen a call signature - the browser card posts `{ widget, params }` to
 * a route that forwards `params` verbatim.
 *
 * It is stripped before the data query (see `splitRenderParams`), because every
 * entry in `params` becomes a filter column in `SupersetAdapter.queryData`. A
 * caller that puts the theme there MUST strip it before its own direct
 * `fetchWidgetData` call, or the chart will be queried with a filter on a column
 * named after this key.
 */
export const DASHBOARD_THEME_PARAM = "__dashboardTheme";

/** Cap on remembered warnings, so a broken theme cannot grow the key set forever. */
const MAX_REMEMBERED_WARNINGS = 32;

/**
 * The slice of the flint service this package depends on.
 *
 * Declared structurally rather than imported from `@loams-plugins/plugin-flint` so that a
 * missing or differently shaped resolver degrades to "unthemed" instead of
 * failing to compile - and so the theme path can be exercised against a stub.
 */
interface FlintThemeBridge {
  resolveWidgetTheme?(widget: unknown, dashboardTheme?: unknown): ThemeResolutionLike | undefined;
  groundTheme?(
    spec: unknown,
    chartType: string,
    facts?: ThemeGroundingFacts,
  ): ThemeGroundingLike | undefined;
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

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

/**
 * Separate the reserved theme key out of a render `params` bag.
 *
 * Returns the params the data layer should see plus the theme selection, if one
 * was carried. The `dataParams` reference is the input itself when there was
 * nothing to strip, so the common path cannot perturb the data cache key.
 */
export function splitRenderParams(params?: Record<string, unknown>): {
  dataParams?: Record<string, unknown>;
  dashboardTheme?: unknown;
} {
  if (params === undefined) return {};
  if (!isPlainObject(params) || !(DASHBOARD_THEME_PARAM in params)) {
    return { dataParams: params };
  }
  const { [DASHBOARD_THEME_PARAM]: dashboardTheme, ...dataParams } = params;
  return { dataParams, dashboardTheme };
}

/** Flatten flint's `ThemeReport[]` into one loggable line. */
function describeThemeReport(report: ThemeReport[] | undefined): string {
  if (!Array.isArray(report) || report.length === 0) return "no report attached";
  return report.map((entry) => `${entry.path}: ${entry.message}`).join("; ");
}

/**
 * Thrown when a widget this package cannot render reaches it.
 *
 * `Unknown chart kind: undefined` was the old answer for a graph widget, and it
 * was wrong twice over: the widget was well-formed, and no chart kind could ever
 * have rendered it. This says what is actually true and names the package that
 * does render it.
 */
export class NotAnEChartsWidgetError extends Error {
  readonly widgetType: string;
  readonly reason: string;

  constructor(decline: RenderDecline) {
    super(decline.reason);
    this.name = "NotAnEChartsWidgetError";
    this.widgetType = decline.widgetType;
    this.reason = decline.reason;
  }
}

export class RenderService extends Service {
  static inject = ["data", "flint"];

  registerKind = registerChartKind;

  /** Warnings already emitted, so one bad theme logs once and not once a render. */
  private readonly _warned = new Set<string>();

  constructor(ctx: Context) {
    super(ctx, "render");
  }

  /**
   * Compile a widget to ECharts options, with its theme applied.
   *
   * `dashboardTheme` is the dashboard-level selection
   * ({ preset, custom } or a bare preset name) and is passed explicitly rather
   * than read from ambient state: a render call carries what it needs. It may
   * also arrive as `params[DASHBOARD_THEME_PARAM]`, which this method strips
   * before the data query; an explicit argument outranks the reserved key.
   *
   * Precedence over the per-widget `flint.theme_spec` is NOT decided here: it
   * belongs to `resolveWidgetTheme`, which is also where a bare preset name, a
   * `{ preset, custom }` pair and a malformed widget override are all resolved.
   *
   * @throws {NotAnEChartsWidgetError} when the widget is not an ECharts widget.
   * A `graph` widget reaches here when a caller routed it to the wrong renderer;
   * the error names the package that owns it. Use `tryCompileWidget` where the
   * widget's type is not known in advance.
   */
  async compileWidget(widget: any, params?: Record<string, unknown>, dashboardTheme?: unknown) {
    const decline = declineEChartsRender(widget);
    if (decline) throw new NotAnEChartsWidgetError(decline);

    const { dataParams, dashboardTheme: themeFromParams } = splitRenderParams(params);
    const data = await this.ctx.data.fetchWidgetData(widget, dataParams);
    const rows = Array.isArray(data?.data) ? data.data : Array.isArray(data) ? data : [];

    let options: Record<string, unknown> | undefined;

    if (widget.flint) {
      try {
        if (this.ctx.flint) {
          options = await this.ctx.flint.compile(widget, data);
        }
      } catch (e) {
        this.ctx.logger.warn(
          "Flint compile failed, falling back to basic mapper:",
          (e as Error).message,
        );
      }
    }

    if (!options) options = compileNativeWidget(widget, rows);

    // The theme is applied AFTER compilation, deliberately. `FlintService.compile`
    // falls back to a hardcoded Power BI theme and rewrites `res.color`, so an
    // overlay applied before it would be silently outranked for every widget
    // without its own `theme_spec`. A theme that loses to a fallback is not a
    // theme.
    return this._applyTheme(widget, options, dashboardTheme ?? themeFromParams, rows);
  }

  /**
   * Compile, or decline -- for a caller that does not know the widget's type.
   *
   * The decline happens BEFORE the data query, so a widget routed to the wrong
   * renderer costs no query. A chart widget with an unregistered kind still
   * throws, because that is a real authoring error about a widget this package
   * does own.
   */
  async tryCompileWidget(
    widget: any,
    params?: Record<string, unknown>,
    dashboardTheme?: unknown,
  ): Promise<{ rendered: true; options: Record<string, unknown> } | RenderDecline> {
    const decline = declineEChartsRender(widget);
    if (decline) return decline;
    return { rendered: true, options: await this.compileWidget(widget, params, dashboardTheme) };
  }

  async previewWidget(widget: any, params?: Record<string, unknown>, dashboardTheme?: unknown) {
    const options = await this.compileWidget(widget, params, dashboardTheme);
    return { widget, options };
  }

  /**
   * The chart type grounding is told about.
   *
   * The widget's own flint declaration when it has one - that is the semantic
   * chart the author asked for - otherwise the realized mark family, which is
   * the coarsest true statement available about a compiled option.
   */
  private _chartType(widget: unknown, options: Record<string, unknown>): string {
    const flint = isPlainObject(widget) ? widget.flint : undefined;
    const declared = asString(isPlainObject(flint) ? flint.chartType : undefined);
    if (declared !== undefined) return declared;
    const first = Array.isArray(options.series) ? options.series.find(isPlainObject) : undefined;
    return asString(first?.type) ?? "";
  }

  /** Log a theme failure once. Repeating it every render is noise, not signal. */
  private _warnOnce(key: string, message: string, ...detail: unknown[]): void {
    if (this._warned.has(key) || this._warned.size >= MAX_REMEMBERED_WARNINGS) return;
    this._warned.add(key);
    this.ctx.logger.warn(message, ...detail);
  }

  /**
   * Overlay the widget's effective flint theme onto compiled ECharts options.
   *
   * Four steps, each of which can decline without taking the chart down:
   * resolve the selection (`resolveWidgetTheme`), ground it against this chart
   * (`groundTheme`), map the grounded ink onto option keys, and log why if any
   * of that said nothing. A theme that cannot be applied leaves the options
   * exactly as they were.
   */
  private _applyTheme(
    widget: unknown,
    options: Record<string, unknown>,
    dashboardTheme: unknown,
    rows: unknown[],
  ): Record<string, unknown> {
    const bridge = this.ctx.flint as unknown as FlintThemeBridge | undefined;
    if (
      !bridge ||
      typeof bridge.resolveWidgetTheme !== "function" ||
      typeof bridge.groundTheme !== "function"
    ) {
      this._warnOnce(
        "no-bridge",
        "No flint theme service (neither resolveWidgetTheme() nor groundTheme()); rendering unthemed",
      );
      return options;
    }

    let resolution: ThemeResolutionLike | undefined;
    try {
      resolution = bridge.resolveWidgetTheme(widget, dashboardTheme ?? null);
    } catch (e) {
      this._warnOnce(
        `resolve-threw:${(e as Error).message}`,
        "Theme resolution threw; rendering unthemed:",
        (e as Error).message,
      );
      return options;
    }

    if (!resolution || resolution.valid !== true) {
      const report = describeThemeReport(resolution?.report);
      this._warnOnce(
        `invalid:${report}`,
        `Theme "${this._themeLabel(widget, dashboardTheme)}" could not be resolved; rendering unthemed: ${report}`,
      );
      return options;
    }

    // `source: 'none'` is a real answer, not a failure: the dashboard and the
    // widget both named no house, so the chart renders exactly as it did before
    // themes existed. `spec` is absent in that state by construction.
    if (resolution.source === "none" || resolution.spec === undefined) return options;

    let grounding: ThemeGroundingLike | undefined;
    const chartType = this._chartType(widget, options);
    try {
      grounding = bridge.groundTheme(resolution.spec, chartType, buildThemeFacts(options, rows));
    } catch (e) {
      this._warnOnce(
        `ground-threw:${chartType}:${(e as Error).message}`,
        "Theme grounding threw; rendering unthemed:",
        (e as Error).message,
      );
      return options;
    }

    if (!grounding || grounding.valid !== true || !grounding.decisions) {
      const report = describeThemeReport(grounding?.report);
      this._warnOnce(
        `ground-invalid:${chartType}:${report}`,
        `Theme "${this._themeLabel(widget, dashboardTheme)}" could not be grounded for a ${chartType || "chart"}; rendering unthemed: ${report}`,
      );
      return options;
    }

    const unmapped = collectUnmappedDecisions(grounding.decisions);
    if (unmapped.length > 0) {
      this.ctx.logger.debug(
        "Grounded theme decisions with no ECharts mapping:",
        unmapped.join(", "),
      );
    }

    try {
      return applyGroundedInk(options, grounding.decisions, rows);
    } catch (e) {
      this._warnOnce(
        `apply-threw:${(e as Error).message}`,
        "Applying the theme to the ECharts options failed; rendering unthemed:",
        (e as Error).message,
      );
      return options;
    }
  }

  /**
   * A short label for a failed theme, for logs only.
   *
   * Reads the widget id and the id of a spec that resolved far enough to have
   * one. Nothing here influences resolution: an unresolved selection has no id
   * to read, which is exactly why the report it came with is logged beside it.
   */
  private _themeLabel(widget: unknown, dashboardTheme: unknown): string {
    const id = asString(isPlainObject(widget) ? widget.id : undefined);
    if (id !== undefined) return `widget ${id}`;
    return asString(asThemeSelectionId(dashboardTheme)) ?? "dashboard";
  }
}

/**
 * The preset id of a theme selection, for log lines.
 *
 * Diagnostic only. Reading a selection's id is not resolving it: the resolution
 * that decides what to render has already run and already failed.
 */
function asThemeSelectionId(theme: unknown): string | undefined {
  if (typeof theme === "string") return theme;
  if (!isPlainObject(theme)) return undefined;
  const preset = asString(theme.preset);
  if (preset !== undefined) return preset;
  const custom = isPlainObject(theme.custom) ? theme.custom : undefined;
  return custom ? (asString(custom.id) ?? asString(custom.extends) ?? "custom") : undefined;
}

declare module "cordis" {
  interface Context {
    render: RenderService;
  }
}
