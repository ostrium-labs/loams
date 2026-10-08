import { Context, Service } from "cordis";
import {
  DEFAULT_THEME_ICON,
  THEME_PRESETS,
  groundTheme as flintGroundTheme,
  resolveThemeSpec,
} from "flint-chart/core";
import type {
  DesignDecisions,
  GroundingContext,
  ThemeReport,
  ThemeSpec as FlintThemeSpec,
} from "flint-chart/core";
import { mapColumnsToSemanticTypes } from "./type-mapper.js";
import type { DashboardTheme, SupersetQueryResult, Widget } from "@loams-core/types";
import { FlintSpecSchema, ThemeSpecSchema } from "@loams-core/types";
import type { ThemeCatalogueEntry } from "@loams-core/types";
// Side-effect import: augments cordis Context with the `controlPlane` key this service injects.
import "@loams-core/bi";

declare module "cordis" {
  interface Context {
    flint: FlintService;
  }
}

/**
 * The result of resolving a theme selection.
 *
 * Discriminated on `valid`, then on `source`, so a caller can narrow without
 * inspecting the optional fields. `report` is ALWAYS present — including on the
 * success path — because flint reports downgrades and approximations rather
 * than swallowing them, and a caller that only saw `spec` would treat a
 * downgraded theme as fully applied.
 */
export type ThemeResolution =
  | { valid: true; source: "none"; report: ThemeReport[]; spec?: undefined }
  | { valid: true; source: "preset" | "custom"; report: ThemeReport[]; spec: FlintThemeSpec }
  | { valid: false; source: "preset" | "custom"; report: ThemeReport[]; spec?: undefined };

/** The result of grounding a spec against a chart. */
export type ThemeGrounding =
  | { valid: true; report: ThemeReport[]; decisions: DesignDecisions }
  | { valid: false; report: ThemeReport[]; decisions?: undefined };

/**
 * Chart facts grounding is allowed to consult.
 *
 * `GroundingContext` requires these as non-optional, so a caller that knows
 * only a chart type cannot supply an honest value for all of them. Rather than
 * inventing them, this service leaves the unknown ones at the neutral "not
 * known" value and lets flint's `report` record whatever the ground could not
 * honour as a result. A caller that DOES know the real facts passes them here
 * and gets a better answer.
 */
export interface GroundingFacts {
  /** Pass `''` (the default) to say "unknown" rather than name a channel. */
  markChannel?: string;
  markTypes?: string[];
  channelSemantics?: Record<string, unknown>;
  table?: unknown[];
  canvasSize?: { width: number; height: number };
  subplotSize?: { width: number; height: number };
  titled?: boolean;
  headline?: string;
  stacked?: boolean | "normalize";
  partToWhole?: boolean;
  valueLabels?: "on" | "off";
  hostSurface?: string;
}

export class FlintService extends Service {
  static inject = ["controlPlane"];

  constructor(ctx: Context) {
    super(ctx, "flint");
  }

  /**
   * The house an unthemed chart falls back to.
   *
   * This is a real renderable default, not a theme override: it only applies
   * when {@link FlintService.resolveWidgetTheme} reports `source: 'none'`. As
   * long as a widget resolves to a real theme, that theme wins outright and
   * this never reaches the assembler.
   */
  private static readonly FALLBACK_THEME_ID = "powerbi-light";

  /**
   * Theme warnings already emitted, keyed by message fingerprint.
   *
   * `resolveTheme` runs once per widget per render, so a dashboard with a bad
   * theme id logged the identical warning once per chart on every repaint and
   * buried everything else. Deduped per service instance and capped, so a caller
   * feeding genuinely distinct bad values still gets distinct warnings without
   * the map growing without bound.
   */
  private readonly _warnedThemeIssues = new Set<string>();

  /** Log a theme problem once per distinct message. */
  private _warnOnce(key: string, message: string): void {
    if (this._warnedThemeIssues.has(key)) return;
    if (this._warnedThemeIssues.size >= 64) {
      // Bounded: stop tracking rather than grow forever. The warning below says
      // so explicitly so silence is not mistaken for "no problems".
      this.ctx.logger.debug(
        "flint: theme warning deduplication limit reached; further theme warnings suppressed",
      );
      return;
    }
    this._warnedThemeIssues.add(key);
    this.ctx.logger.warn(message);
  }

  /**
   * The Power BI categorical palette, used only when a chart has no theme.
   *
   * Named rather than inlined at each use so the fallback compiler and the
   * assembler post-processing cannot drift apart.
   */
  private static readonly FALLBACK_PALETTE = [
    "#118dff",
    "#12239e",
    "#e66c37",
    "#6b007b",
    "#e044a7",
    "#744ec2",
    "#d9b300",
    "#d64550",
    "#197278",
    "#5c2e91",
    "#ff9d3b",
    "#4a9c2d",
  ];

  /**
   * Compile a widget to a backend-neutral ECharts option.
   *
   * `dashboardTheme` is the dashboard-level selection. It is honoured only when
   * the widget carries no `flint.theme_spec` of its own — see
   * {@link FlintService.resolveWidgetTheme} for the precedence rule, which is
   * deliberately not re-implemented here.
   *
   * THEME OWNERSHIP: the theme resolved here is the `theme_spec` handed to
   * flint's assembler. Flint documents `theme_spec` as realized by its
   * Vega-Lite assembler, and this repo renders ECharts, so the returned option
   * is NOT reliably themed by this call alone. Callers that need themes to
   * actually paint (the ECharts renderer) apply the resolved spec to the option
   * after this returns. Passing `dashboardTheme` here at minimum stops the
   * hardcoded fallback from overriding it.
   */
  async compile(
    widget: Widget,
    data: SupersetQueryResult,
    dashboardTheme?: DashboardTheme | string | null,
  ): Promise<Record<string, unknown>> {
    const flintSpec = widget.flint || ({ chartType: "Bar Chart", encodings: {} } as any);
    const rows = Array.isArray(data?.data) ? data.data : Array.isArray(data) ? data : [];

    // Infer semantic types
    const semanticTypes = this._inferSemanticTypes(widget, data);

    // Resolve the effective theme ONCE, so the assembler and the fallback
    // compiler below cannot disagree about which house this widget wears.
    // `resolveWidgetTheme` never throws; an unresolvable theme reports and
    // degrades to the fallback rather than taking the chart down.
    const resolution = this.resolveWidgetTheme(widget, dashboardTheme);
    const resolvedSpec = resolution.valid ? resolution.spec : undefined;
    const isThemed = resolvedSpec !== undefined;
    const pbiColors = FlintService.FALLBACK_PALETTE;

    const input = {
      data: { values: rows },
      semantic_types: semanticTypes,
      chart_spec: {
        chartType: flintSpec.chartType,
        encodings: flintSpec.encodings,
        title: flintSpec.title,
        subtitle: flintSpec.subtitle,
        baseSize: flintSpec.baseSize || { width: 400, height: 320 },
        chartProperties: flintSpec.chartProperties,
      },
      theme_spec: resolvedSpec ?? { id: FlintService.FALLBACK_THEME_ID },
    };

    try {
      const flintEcharts = await import("flint-chart/echarts").catch(() => null);
      if (flintEcharts && typeof flintEcharts.assembleECharts === "function") {
        const res = flintEcharts.assembleECharts(input);
        const hasValidSeries =
          res &&
          Array.isArray(res.series) &&
          res.series.length > 0 &&
          !res.series.some((s: any) => s.type === "custom" && typeof s.renderItem !== "function");

        if (hasValidSeries) {
          // Do NOT stamp the Power BI palette over a themed chart: the whole
          // point of a theme is that the author chose the colours, and this
          // branch used to overwrite them for every widget that had a
          // dashboard theme but no per-widget one.
          if (!isThemed && (!res.color || res.color[0] === "#5470c6")) {
            res.color = pbiColors;
          }
          return res;
        }
      }
    } catch {
      // Fall through to semantic fallback compiler
    }

    // Semantic Fallback Compiler for Flint Spec: guarantees every single chart type renders data
    const xField =
      flintSpec.encodings?.x?.field ||
      flintSpec.encodings?.x ||
      flintSpec.encodings?.color?.field ||
      Object.keys(rows[0] || {})[0] ||
      "x";
    const yField =
      flintSpec.encodings?.y?.field ||
      flintSpec.encodings?.y ||
      flintSpec.encodings?.size?.field ||
      Object.keys(rows[0] || {})[1] ||
      "y";
    const cType = (flintSpec.chartType || "").toLowerCase();

    const titleObj = flintSpec.title
      ? { text: flintSpec.title, subtext: flintSpec.subtitle }
      : undefined;

    if (cType.includes("gauge") || cType.includes("meter")) {
      const val = rows.length > 0 ? Number(rows[0][yField] ?? rows[0][xField] ?? 75) : 75;
      return {
        title: titleObj,
        color: pbiColors,
        series: [
          {
            type: "gauge",
            progress: { show: true, width: 14 },
            axisLine: { lineStyle: { width: 14 } },
            axisTick: { show: false },
            splitLine: { length: 8, lineStyle: { width: 2, color: "#999" } },
            axisLabel: { distance: 18, color: "#605e5c", fontSize: 11 },
            pointer: { length: "60%", width: 5 },
            anchor: { show: true, showAbove: true, size: 14, itemStyle: { borderWidth: 2 } },
            title: { show: true, offsetCenter: [0, "70%"], fontSize: 13, color: "#252423" },
            detail: {
              valueAnimation: true,
              fontSize: 22,
              offsetCenter: [0, "40%"],
              formatter: "{value}",
              color: "#118dff",
            },
            data: [
              {
                value: val > 100 ? Math.round(val % 100) : Math.round(val),
                name: flintSpec.title || "Score",
              },
            ],
          },
        ],
      };
    }

    if (cType.includes("treemap")) {
      return {
        title: titleObj,
        color: pbiColors,
        tooltip: { trigger: "item" },
        series: [
          {
            type: "treemap",
            roam: false,
            nodeClick: false,
            breadcrumb: { show: false },
            levels: [{ itemStyle: { borderColor: "#fff", borderWidth: 2, gapWidth: 2 } }],
            data: rows.map((r: any) => ({
              name: String(r[xField] ?? ""),
              value: Number(r[yField] ?? 0),
            })),
          },
        ],
      };
    }

    if (cType.includes("sunburst")) {
      return {
        title: titleObj,
        color: pbiColors,
        tooltip: { trigger: "item" },
        series: [
          {
            type: "sunburst",
            radius: ["15%", "85%"],
            itemStyle: { borderRadius: 4, borderWidth: 2 },
            data: rows.map((r: any) => ({
              name: String(r[xField] ?? ""),
              value: Number(r[yField] ?? 0),
            })),
          },
        ],
      };
    }

    if (cType.includes("tree")) {
      return {
        title: titleObj,
        color: pbiColors,
        tooltip: { trigger: "item" },
        series: [
          {
            type: "tree",
            orient: "LR",
            initialTreeDepth: 2,
            symbolSize: 8,
            data: [
              {
                name: flintSpec.title || "Root",
                children: rows.map((r: any) => ({
                  name: String(r[xField] ?? ""),
                  value: Number(r[yField] ?? 0),
                })),
              },
            ],
          },
        ],
      };
    }

    if (cType.includes("waterfall")) {
      const categories = rows.map((r: any) => String(r[xField] ?? ""));
      const values = rows.map((r: any) => Number(r[yField] ?? 0));
      const baseData: number[] = [];
      let running = 0;
      for (let i = 0; i < values.length; i++) {
        if (i === 0) {
          baseData.push(0);
          running = values[0];
        } else {
          baseData.push(running);
          running += values[i];
        }
      }
      return {
        title: titleObj,
        color: pbiColors,
        tooltip: { trigger: "axis" },
        xAxis: { type: "category", data: categories },
        yAxis: { type: "value" },
        series: [
          {
            name: "Base",
            type: "bar",
            stack: "Total",
            itemStyle: { borderColor: "transparent", color: "transparent" },
            data: baseData,
          },
          {
            name: "Delta",
            type: "bar",
            stack: "Total",
            label: { show: true, position: "top" },
            data: values,
          },
        ],
      };
    }

    if (cType.includes("candlestick")) {
      const categories = rows.map((r: any) => String(r[xField] ?? ""));
      const ohlc = rows.map((r: any) => {
        const val = Number(r[yField] || 100);
        const cost = Number(r.cost || val * 0.75);
        return [cost, val, Math.min(cost, val) * 0.95, Math.max(cost, val) * 1.05];
      });
      return {
        title: titleObj,
        color: pbiColors,
        tooltip: { trigger: "axis" },
        xAxis: { type: "category", data: categories },
        yAxis: { type: "value", scale: true },
        series: [{ type: "candlestick", data: ohlc }],
      };
    }

    const type =
      cType.includes("line") || cType.includes("area") || cType.includes("stream")
        ? "line"
        : cType.includes("pie") || cType.includes("donut") || cType.includes("rose")
          ? "pie"
          : cType.includes("scatter") || cType.includes("bubble")
            ? "scatter"
            : "bar";

    if (type === "pie") {
      return {
        title: titleObj,
        color: pbiColors,
        dataset: { source: rows },
        series: [
          {
            type: "pie",
            radius: cType.includes("donut") ? ["45%", "70%"] : "55%",
            encode: { itemName: xField, value: yField },
          },
        ],
        tooltip: { trigger: "item" },
        legend: { orient: "vertical", left: "left" },
      };
    }

    return {
      title: titleObj,
      color: pbiColors,
      dataset: { source: rows },
      xAxis:
        type === "scatter" ? { type: "value", name: xField } : { type: "category", name: xField },
      yAxis: { type: "value", name: yField },
      series: [
        {
          type,
          encode: { x: xField, y: yField },
          smooth: type === "line",
          ...(cType.includes("area") || cType.includes("stream")
            ? { areaStyle: { opacity: 0.25 } }
            : {}),
        },
      ],
      tooltip: { trigger: type === "scatter" ? "item" : "axis" },
    };
  }

  async validate(flintSpec: Record<string, unknown>, sampleData?: any[]) {
    const parsed = FlintSpecSchema.safeParse(flintSpec);
    if (parsed.success) {
      return {
        valid: true as const,
        errors: [] as string[],
        warnings: [] as any[],
        spec: parsed.data,
      };
    }
    const errors = parsed.error.issues.map((issue) => {
      const path = issue.path.join(".");
      return `${path.length > 0 ? path : "(root)"}: ${issue.message}`;
    });
    return { valid: false as const, errors, warnings: [] as any[], spec: undefined };
  }

  async inferSemanticTypes(datasetId: number): Promise<Record<string, string>> {
    const controlPlaneService: any = this.ctx.controlPlane; // Using any as the control-plane service type is not known here, but the method is defined in requirements
    const describeInfo = await controlPlaneService.describeDataset(datasetId);
    if (!describeInfo || !describeInfo.columns) {
      return {};
    }
    return mapColumnsToSemanticTypes(describeInfo.columns);
  }

  private _inferSemanticTypes(widget: Widget, data: SupersetQueryResult): Record<string, string> {
    // Simple heuristic from data values when columns are not available, or could map from column names if present
    const semanticTypes: Record<string, string> = {};
    if (data.data && data.data.length > 0) {
      const firstRow = data.data[0];
      for (const [key, value] of Object.entries(firstRow)) {
        if (typeof value === "number") {
          if (key.toLowerCase().includes("price") || key.toLowerCase().includes("cost")) {
            semanticTypes[key] = "Price";
          } else {
            semanticTypes[key] = "Quantity"; // fallback for numeric
          }
        } else if (typeof value === "string") {
          if (Date.parse(value) && !isNaN(Date.parse(value))) {
            semanticTypes[key] = "DateTime";
          } else {
            semanticTypes[key] = "Category"; // fallback for string
          }
        } else if (typeof value === "boolean") {
          semanticTypes[key] = "Boolean";
        }
      }
    }
    return semanticTypes;
  }

  /**
   * The houses flint ships, as a picker-ready catalogue.
   *
   * Built from `THEME_PRESETS` rather than from `listThemePresets()`: that
   * helper deliberately omits `icon`, and rendering a swatch per house is the
   * point of this method. Sorted by `label` so the picker order is stable and
   * human-alphabetical rather than whatever order flint happens to declare its
   * presets in.
   */
  listThemes(): ThemeCatalogueEntry[] {
    const entries: ThemeCatalogueEntry[] = Object.values(THEME_PRESETS).map((preset) => ({
      id: preset.id,
      label: preset.label,
      description: preset.description,
      icon: preset.icon,
    }));
    entries.sort((a, b) => a.label.localeCompare(b.label, "en"));
    return entries;
  }

  /**
   * The icon to show for "no theme".
   *
   * "Not theming" is a deliberate choice, not an empty slot, so it gets a tile
   * of its own. Kept as its own accessor rather than prepended to
   * {@link FlintService.listThemes} because it has no `id`: appending a fake
   * house to the catalogue would make it look selectable by id and resolvable
   * as a preset.
   */
  defaultThemeIcon(): string {
    return DEFAULT_THEME_ICON;
  }

  /**
   * Resolve a theme selection to a concrete `ThemeSpec`.
   *
   * Accepts a dashboard `theme` selection, a bare preset name, or nullish.
   * Never throws: `resolveThemeSpec` treats an unknown preset name as an ERROR
   * (deliberately, so a typo does not silently become some other house's
   * colours), and that error becomes `valid: false` plus a report entry here.
   * Callers get a value they can render with, not an exception.
   *
   * Layering: when a selection carries both `preset` and `custom`, the preset
   * becomes the custom spec's base via `extends`, so the merge is flint's own
   * — nested policy objects MERGE, arrays and scalars REPLACE.
   */
  resolveTheme(theme?: DashboardTheme | string | null): ThemeResolution {
    if (theme === null || theme === undefined) {
      return { valid: true, source: "none", report: [], spec: undefined };
    }

    const selection: DashboardTheme = typeof theme === "string" ? { preset: theme } : theme;
    const source: "preset" | "custom" = selection.custom !== undefined ? "custom" : "preset";

    if (selection.preset === undefined && selection.custom === undefined) {
      return { valid: true, source: "none", report: [], spec: undefined };
    }

    let candidate: FlintThemeSpec | string;
    if (selection.custom !== undefined) {
      const custom = this._toFlintSpec(selection.custom);
      // `custom` may already name its own base. An explicit `extends` is the
      // author's decision and outranks the selection's `preset`.
      candidate =
        custom.extends === undefined && selection.preset !== undefined
          ? { ...custom, extends: selection.preset }
          : custom;
    } else {
      candidate = selection.preset as string;
    }

    try {
      const spec = resolveThemeSpec(candidate);
      if (spec === undefined) {
        return { valid: true, source: "none", report: [], spec: undefined };
      }
      return { valid: true, source, report: [], spec };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // Point at the field that actually failed, so the UI can put the error on
      // the input the author typed into rather than on the selection as a whole.
      const path =
        source === "preset"
          ? "theme.preset"
          : selection.custom?.extends !== undefined
            ? "theme.custom.extends"
            : "theme.preset";
      this._warnOnce(
        `resolve:${path}:${message}`,
        `flint: could not resolve theme at ${path}: ${message}`,
      );
      return {
        valid: false,
        source,
        report: [{ stage: "ground", path, message }],
        spec: undefined,
      };
    }
  }

  /**
   * The EFFECTIVE theme for one widget.
   *
   * PRECEDENCE: a per-widget `flint.theme_spec` wins over the dashboard-level
   * `theme`. The per-widget value is authored next to the chart spec and is the
   * more specific statement, so it overrides the dashboard's house; the
   * dashboard-level `theme` is the default for every widget that does not
   * override it.
   *
   * A widget's `theme_spec` is typed loosely (`string | Record<string, unknown>`)
   * because it predates this schema, so it is validated here rather than
   * trusted. A malformed per-widget override reports and falls back to the
   * dashboard-level theme rather than taking the whole widget down.
   */
  resolveWidgetTheme(
    widget: Widget,
    dashboardTheme?: DashboardTheme | string | null,
  ): ThemeResolution {
    const widgetTheme = widget.flint?.theme_spec;

    // `FlintSpecSchema.theme_spec` is `string | Record<string, unknown>`: a bare
    // preset NAME is a legal per-widget override, so it goes straight to the
    // resolver. It also wins outright — naming a house replaces the dashboard's
    // rather than layering on top of it.
    if (typeof widgetTheme === "string") {
      return this.resolveTheme({ preset: widgetTheme });
    }

    if (widgetTheme !== undefined && widgetTheme !== null) {
      const parsed = ThemeSpecSchema.safeParse(widgetTheme);
      if (!parsed.success) {
        const issues = parsed.error.issues
          .map((issue) => `${issue.path.join(".") || "(root)"}: ${issue.message}`)
          .join("; ");
        this._warnOnce(
          `widget:${widget.id}:${issues}`,
          `flint: per-widget theme_spec on widget ${widget.id} is invalid, falling back to the dashboard theme: ${issues}`,
        );
      } else {
        return this.resolveTheme({
          preset: typeof dashboardTheme === "string" ? dashboardTheme : dashboardTheme?.preset,
          custom: parsed.data,
        });
      }
    }

    return this.resolveTheme(dashboardTheme);
  }

  /**
   * Ground a spec against a chart: level 1 (`ThemeSpec`) to level 2
   * (`DesignDecisions`), still backend-neutral.
   *
   * A thin wrapper; what it adds is a `GroundingContext` that is honest about
   * what it does not know rather than an `as any` escape hatch. Fields we have
   * no fact for are left at their "unknown" value, and whatever the ground
   * could not honour because of it comes back in `report` instead of being
   * hidden behind invented semantics.
   *
   * Pass `facts` when the caller knows the real chart (mark families, canvas
   * size, whether it is titled). The defaults describe a single, unfaceted,
   * untitled chart.
   */
  groundTheme(spec: FlintThemeSpec, chartType: string, facts: GroundingFacts = {}): ThemeGrounding {
    const canvasSize = facts.canvasSize ?? { width: 480, height: 320 };
    const subplot = facts.subplotSize ?? { width: canvasSize.width, height: canvasSize.height };
    const ctx: GroundingContext = {
      chartType,
      // Empty string, not a guess: we were not told which cognitive channel the
      // marks serve, and naming one would make grounding resolve policy against
      // a channel this chart may not have. The consequence is reported.
      markChannel: facts.markChannel ?? "",
      markTypes: facts.markTypes ?? [],
      channelSemantics: facts.channelSemantics ?? {},
      layout: {
        subplotWidth: subplot.width,
        subplotHeight: subplot.height,
        xStep: 0,
        yStep: 0,
        stepPadding: 0.1,
        titleFontSize: 14,
        legendFontSize: 11,
      },
      table: facts.table ?? [],
      canvasSize,
      titled: facts.titled ?? false,
    };
    if (facts.headline !== undefined) ctx.headline = facts.headline;
    if (facts.stacked !== undefined) ctx.stacked = facts.stacked;
    if (facts.partToWhole !== undefined) ctx.partToWhole = facts.partToWhole;
    if (facts.valueLabels !== undefined) ctx.valueLabels = facts.valueLabels;
    if (facts.hostSurface !== undefined) ctx.hostSurface = facts.hostSurface;

    try {
      const decisions = flintGroundTheme(spec, ctx);
      return { valid: true, report: decisions.report ?? [], decisions };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.ctx.logger.warn(`flint: groundTheme failed for chartType ${chartType}: ${message}`);
      return {
        valid: false,
        report: [{ stage: "ground", path: chartType, message }],
        decisions: undefined,
      };
    }
  }

  /**
   * The validated authoring subset of a `ThemeSpec`, as a flint `ThemeSpec`.
   *
   * `@loams-core/types` mirrors the subset we expose and validates it, and mirrors it
   * with a passthrough index signature so unknown keys survive the round trip.
   * Flint's own `ThemeSpec` is the superset, with no index signature. One
   * documented cast in one place beats an `any` at every call site, and it is
   * sound in the direction that matters: we widen a validated subset, we do not
   * invent fields.
   */
  private _toFlintSpec(spec: NonNullable<DashboardTheme["custom"]>): FlintThemeSpec {
    return spec as unknown as FlintThemeSpec;
  }
}
