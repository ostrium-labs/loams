/**
 * The chart option the tile hands to ECharts.
 *
 * Extracted from `WidgetCard`'s effect without changing a line of it, for one
 * reason: the theme gate below is invisible in a screenshot. A chart still
 * renders when it is wrong -- just in the wrong ink -- so the only way to hold
 * the gate is to assert on the option object, and the only way to do that in a
 * workspace with no DOM (`renderToStaticMarkup` does not run effects, and
 * jsdom/happy-dom are not installed) is for the rule to be a pure function of
 * `(option, hasTheme)`.
 *
 * `hasTheme` must be the EFFECTIVE theme for that widget: the per-widget
 * override when there is one, otherwise the dashboard theme. `App.tsx` computes
 * it as `Boolean(widget.chartSpec?.theme_spec ?? spec.theme)`.
 */
import type { Widget } from "../api";

/** The Power BI Light categorical palette. Applied only when nothing else owns ink. */
export const POWER_BI_SERIES = [
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
 * The two defaults chart-specs and Apache ECharts ship.
 *
 * If the server left one of those in place, nobody has chosen this chart's ink
 * and the Power BI Light palette applies. Anything else was chosen deliberately
 * -- a theme, usually -- and is left exactly as it arrived.
 */
const SERVER_PALETTES = ["#0284c7", "#5470c6"];

/**
 * Apply the tile's own chrome, and -- only when nothing else owns the ink --
 * the Power BI Light greys.
 *
 * Mutates and returns `base`, which is the object `previewWidget` resolved; the
 * caller passes a fresh one per request, so the mutation is not shared.
 *
 * @param base The server-compiled option.
 * @param hasTheme Whether a theme owns this widget's colours.
 */
export function composeChartOption(base: Record<string, unknown>, hasTheme: boolean) {
  const option = (base || {}) as any;

  // Defensive: ensure dataset.source is a flat array of row objects
  if (option.dataset?.source && !Array.isArray(option.dataset.source)) {
    if (Array.isArray(option.dataset.source.data)) {
      option.dataset.source = option.dataset.source.data;
    }
  }

  // Standardize Power BI Light palette — only when no theme owns it.
  if (
    !hasTheme &&
    (!option.color || option.color.length === 0 || SERVER_PALETTES.includes(option.color[0]))
  ) {
    option.color = [...POWER_BI_SERIES];
  }

  /*
   * Ink here, chrome always.
   *
   * Font, rotation and label formatting are the card's own tidying and
   * apply regardless. The COLOURS do not: when a theme is active the
   * server has already mapped ThemeInk onto these option objects, and
   * stamping Power BI greys over them here would silently un-theme the
   * chart. So each colour falls back to `undefined` under a theme,
   * which leaves whatever the server sent intact.
   */
  const inkLabel = hasTheme ? {} : { color: "#605e5c" };
  const inkAxisLine = hasTheme ? {} : { lineStyle: { color: "#d2d0ce" } };
  const inkSplitLine = hasTheme ? {} : { lineStyle: { type: "solid", color: "#ededed" } };

  // Ensure ECharts uses Power BI Segoe UI font and precise hairline styling
  if (option.xAxis) {
    const isCategory = option.xAxis.type === "category" || !option.xAxis.type;
    option.xAxis = {
      ...option.xAxis,
      // Under a theme, the server has already set these from the grounded
      // ink. Spread the server's value and change NOTHING, rather than
      // replacing the key -- rebuilding `axisLine: {}` or `axisTick:
      // {show:false}` unconditionally dropped the themed colour and
      // silently un-themed the chart.
      axisLine: hasTheme ? (option.xAxis.axisLine ?? {}) : inkAxisLine,
      axisTick: hasTheme ? (option.xAxis.axisTick ?? { show: false }) : { show: false },
      axisLabel: {
        // Same reason: keep the server's themed label colour, then apply
        // this card's font/rotation/formatting on top.
        ...(hasTheme ? option.xAxis.axisLabel : {}),
        ...inkLabel,
        fontSize: 11,
        fontFamily: "'Segoe UI', wf_segoe-ui_normal, sans-serif",
        interval: 0,
        rotate:
          isCategory && Array.isArray(option.xAxis.data) && option.xAxis.data.length > 6 ? 25 : 0,
        formatter: (v: any) => {
          if (typeof v === "object" && v !== null) {
            return v.name || v.value || "";
          }
          return String(v ?? "");
        },
      },
      splitLine: hasTheme ? (option.xAxis.splitLine ?? { show: false }) : { show: false },
    };
  }

  if (option.yAxis) {
    option.yAxis = {
      ...option.yAxis,
      axisLine: hasTheme ? (option.yAxis.axisLine ?? { show: false }) : { show: false },
      axisTick: hasTheme ? (option.yAxis.axisTick ?? { show: false }) : { show: false },
      axisLabel: {
        ...(hasTheme ? option.yAxis.axisLabel : {}),
        ...inkLabel,
        fontSize: 11,
        fontFamily: "'Segoe UI', wf_segoe-ui_normal, sans-serif",
        formatter: (v: any) => {
          const num = Number(v);
          if (!isNaN(num) && typeof v !== "object") {
            if (Math.abs(num) >= 1000000) return `${(num / 1000000).toFixed(1)}M`;
            if (Math.abs(num) >= 1000) return `${Math.round(num / 1000)}K`;
            return `${num}`;
          }
          if (typeof v === "string") return v;
          return "";
        },
      },
      splitLine: hasTheme ? (option.yAxis.splitLine ?? {}) : inkSplitLine,
    };
  }

  // Suppress canvas internal title — the visual tile renders a prominent HTML card header
  option.title = { show: false };

  // Clean Power BI grid padding without colliding with titles
  const hasHorizontalLegend = option.legend && option.legend.orient !== "vertical";
  if (!option.grid) {
    option.grid = {
      top: hasHorizontalLegend ? 36 : 20,
      bottom: 25,
      left: 20,
      right: 20,
      containLabel: true,
    };
  } else {
    option.grid = {
      ...option.grid,
      top: hasHorizontalLegend ? 36 : 20,
      containLabel: true,
    };
  }

  // Polished Power BI tooltip — colours deferred to the server under a theme
  option.tooltip = {
    trigger: option.series?.[0]?.type === "pie" ? "item" : "axis",
    ...(hasTheme
      ? {}
      : {
          backgroundColor: "#ffffff",
          borderColor: "#e1dfdd",
          borderWidth: 1,
          textStyle: {
            color: "#252423",
            fontSize: 12,
            fontFamily: "'Segoe UI', sans-serif",
          },
          extraCssText: "box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12); border-radius: 4px;",
        }),
    ...option.tooltip,
  };

  // Polished Power BI legend with proper placement
  if (option.legend) {
    const isVertical = option.legend.orient === "vertical";
    option.legend = {
      textStyle: { ...inkLabel, fontSize: 11, fontFamily: "'Segoe UI', sans-serif" },
      itemWidth: 10,
      itemHeight: 10,
      icon: "circle",
      ...option.legend,
      ...(isVertical ? { left: 16, top: "middle" } : { top: 8, left: "center" }),
    };
  }

  // Pie chart center offset when vertical legend is on the left
  if (option.series?.[0]?.type === "pie") {
    if (!option.series[0].center && option.legend?.orient === "vertical") {
      option.series[0].center = ["60%", "50%"];
    }
  }

  return option;
}

/**
 * The click handler the tile installs for cross-widget filtering.
 *
 * Pure so the `interactions` contract can be asserted without a chart: a click
 * on `params.data[fieldName]` (falling back to the series name) writes that
 * value into every param the interaction names.
 *
 * @param params The ECharts click payload (`ECElementEvent`). Only `data` and
 * `name` are read. `data` is typed `unknown` rather than
 * `Record<string, unknown>` because that is what ECharts actually delivers: a
 * scalar or an array for most series. Only a plain object can carry the field
 * names an interaction refers to, so anything else falls back to `name`.
 */
export function paramFilterFor(
  widget: Widget,
  params: { data?: unknown; name?: unknown },
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const data =
    typeof params?.data === "object" && params.data !== null && !Array.isArray(params.data)
      ? (params.data as Record<string, unknown>)
      : undefined;
  for (const inter of widget.interactions ?? []) {
    if (inter.on !== "click") continue;
    for (const [paramName, fieldName] of Object.entries(inter.set)) {
      const value = data?.[fieldName] ?? params?.name;
      if (value) out[paramName] = value;
    }
  }
  return out;
}
