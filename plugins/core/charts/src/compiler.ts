/**
 * ECharts Compiler — converts Widget + data rows into ECharts option objects.
 *
 * The compiler is a pure function: no side effects, no network access.
 * Chart kinds are extensible via the registry pattern.
 */

/** Definition for a chart kind plugin */
export interface ChartKindDef {
  /** Optional JSON Schema for kind-specific encode fields */
  schema?: object;
  /** Compile a widget + data rows into an ECharts option */
  compile(widget: any, rows: Record<string, unknown>[]): Record<string, unknown>;
}

/** Global chart kind registry */
const registry = new Map<string, ChartKindDef>();

/** Register a new chart kind (extensible by plugins) */
export function registerChartKind(kind: string, def: ChartKindDef): void {
  registry.set(kind, def);
}

/** Get a registered chart kind */
export function getChartKind(kind: string): ChartKindDef | undefined {
  return registry.get(kind);
}

/** Allowlisted option keys that optionOverrides may set */
export const OVERRIDE_ALLOWLIST = new Set([
  "title",
  "legend",
  "tooltip",
  "grid",
  "color",
  "backgroundColor",
  "animation",
  "dataZoom",
  "visualMap",
  "toolbox",
  "xAxis",
  "yAxis",
  "series",
]);

/** Deep merge two objects (source into target) */
export function deepMerge(target: any, source: any): any {
  if (source === null || source === undefined) return target;
  if (target === null || target === undefined) return source;
  if (typeof target !== "object" || typeof source !== "object") return source;
  if (Array.isArray(source)) {
    if (Array.isArray(target)) {
      return source.map((item, i) => deepMerge(target[i] ?? {}, item));
    }
    return [...source];
  }

  const result = { ...target };
  for (const key of Object.keys(source)) {
    result[key] = deepMerge(result[key], source[key]);
  }
  return result;
}

/** Apply overrides, filtering to allowlisted keys only */
export function applyOverrides(
  option: Record<string, unknown>,
  overrides: Record<string, unknown>,
): Record<string, unknown> {
  const safe = { ...option };
  for (const [key, value] of Object.entries(overrides)) {
    if (OVERRIDE_ALLOWLIST.has(key)) {
      safe[key] = deepMerge(safe[key], value);
    }
  }
  return safe;
}

/**
 * Compile a native (non-chart-specs) widget into an ECharts option.
 * Requires widget.chart to be defined.
 */
export function compileNativeWidget(widget: any, data: any): Record<string, unknown> {
  const chart = widget.chart;
  if (!chart?.kind) {
    throw new Error("Chart kind is required");
  }

  const def = registry.get(chart.kind);
  if (!def) {
    throw new Error(`Unknown chart kind: ${chart.kind}`);
  }

  // Normalize data into a flat array of rows for ECharts
  const rows = Array.isArray(data) ? data : Array.isArray(data?.data) ? data.data : [];

  let option = def.compile(widget, rows);

  // Apply optionOverrides through the allowlist
  if (chart.optionOverrides) {
    option = applyOverrides(option, chart.optionOverrides);
  }

  return option;
}

// ─── What this package declines to render ─────────────────────────────

/**
 * The widget types this package knows how to turn into an ECharts option.
 *
 * Closed on purpose. Every type outside this set has a renderer elsewhere, and
 * `graph` in particular is a sibling type rather than a chart kind because
 * React Flow's terminal value is a `nodes`/`edges` pair -- see
 * `@loams-core/flows`.
 */
export const CHART_WIDGET_TYPES: ReadonlySet<string> = new Set(["chart"]);

/** A widget this package will not render, and the package that owns it. */
export interface RenderDecline {
  rendered: false;
  widgetType: string;
  reason: string;
}

/** Where a declined widget type actually goes. */
const DECLINE_OWNER: Record<string, string> = {
  graph: "@loams-core/flows",
  kpi: "@loams-core/kpi (unimplemented; not a chart widget)",
  table: "@loams-core/table (unimplemented; not a chart widget)",
  text: "the dashboard's text renderer",
  filter: "the dashboard's filter bar",
};

/**
 * Decline a widget this package does not own.
 *
 * Returns `undefined` for a widget that IS an ECharts widget, including one with
 * no `type` at all. That last part matters: pre-`graph` callers pass
 * `{ chart: { kind } }` and rely on the option object being produced, so an
 * absent type is not treated as a non-chart type here.
 */
export function declineChartRender(widget: unknown): RenderDecline | undefined {
  if (typeof widget !== "object" || widget === null || Array.isArray(widget)) return undefined;
  const type = (widget as { type?: unknown }).type;
  // A non-string `type` is a malformed widget, not a routing decision. Leaving
  // it to `compileNativeWidget`'s existing errors keeps those messages intact.
  if (typeof type !== "string" || CHART_WIDGET_TYPES.has(type)) return undefined;

  const owner = DECLINE_OWNER[type];
  return {
    rendered: false,
    widgetType: type,
    reason:
      `Widget type "${type}" is not a chart widget: this package terminates in a chart ` +
      `option object, and a "${type}" widget does not. Rendered instead by ${owner ?? "another renderer"}.`,
  };
}

// ─── Built-in chart kind registrations ───────────────────────────────

registerChartKind("line", {
  compile(widget, rows) {
    const encode = widget.chart?.encode;
    if (!encode || (!encode.x && !encode.y)) {
      return {
        dataset: { source: rows },
        series: [{ type: "line" }],
      };
    }
    const { x, y } = encode;
    const yFields = Array.isArray(y) ? y : y ? [y] : [];
    return {
      dataset: { source: rows },
      xAxis: { type: "category" },
      yAxis: { type: "value" },
      series:
        yFields.length > 0
          ? yFields.map((field: string) => ({
              name: field,
              type: "line",
              encode: { x, y: field },
              smooth: false,
            }))
          : [{ type: "line", encode: { x } }],
      tooltip: { trigger: "axis" },
      ...(yFields.length > 1 ? { legend: {} } : {}),
    };
  },
});

registerChartKind("area", {
  compile(widget, rows) {
    const encode = widget.chart?.encode;
    if (!encode || (!encode.x && !encode.y)) {
      return {
        dataset: { source: rows },
        series: [{ type: "line", areaStyle: { opacity: 0.25 }, smooth: true }],
      };
    }
    const { x, y } = encode;
    const yFields = Array.isArray(y) ? y : y ? [y] : [];
    return {
      dataset: { source: rows },
      xAxis: { type: "category" },
      yAxis: { type: "value" },
      series:
        yFields.length > 0
          ? yFields.map((field: string) => ({
              type: "line",
              encode: { x, y: field },
              smooth: true,
              areaStyle: { opacity: 0.25 },
            }))
          : [{ type: "line", encode: { x }, smooth: true, areaStyle: { opacity: 0.25 } }],
      tooltip: { trigger: "axis" },
      ...(yFields.length > 1 ? { legend: {} } : {}),
    };
  },
});

registerChartKind("bar", {
  compile(widget, rows) {
    const encode = widget.chart?.encode;
    if (!encode || (!encode.x && !encode.y)) {
      return {
        dataset: { source: rows },
        series: [{ type: "bar" }],
      };
    }
    const { x, y, series } = encode;
    const yFields = Array.isArray(y) ? y : y ? [y] : [];
    return {
      dataset: { source: rows },
      xAxis: { type: "category" },
      yAxis: { type: "value" },
      series:
        yFields.length > 0
          ? yFields.map((field: string) => ({
              name: field,
              type: "bar",
              encode: { x, y: field },
            }))
          : [{ type: "bar", encode: { x } }],
      tooltip: { trigger: "axis" },
      ...(yFields.length > 1 ? { legend: {} } : {}),
    };
  },
});

registerChartKind("pie", {
  compile(widget, rows) {
    const encode = widget.chart?.encode;
    if (!encode || (!encode.x && !encode.value)) {
      return {
        dataset: { source: rows },
        series: [{ type: "pie" }],
      };
    }
    const { x, value } = encode;
    const nameField = x ?? Object.keys(rows[0] ?? {})[0];
    const valueField = value ?? Object.keys(rows[0] ?? {})[1];
    return {
      dataset: { source: rows },
      series: [
        {
          type: "pie",
          radius: "60%",
          encode: { itemName: nameField, value: valueField },
        },
      ],
      tooltip: { trigger: "item" },
      legend: { orient: "vertical", left: "left" },
    };
  },
});

registerChartKind("scatter", {
  compile(widget, rows) {
    const encode = widget.chart?.encode;
    const x = encode?.x;
    const y = encode?.y;
    const yField = Array.isArray(y) ? y[0] : y;
    return {
      dataset: { source: rows },
      xAxis: { type: "value" },
      yAxis: { type: "value" },
      series: [
        {
          type: "scatter",
          encode: { x, y: yField },
        },
      ],
      tooltip: { trigger: "item" },
    };
  },
});

registerChartKind("heatmap", {
  compile(widget, rows) {
    const { x, y, value } = widget.chart.encode;
    const yField = Array.isArray(y) ? y[0] : y;
    return {
      dataset: { source: rows },
      xAxis: { type: "category" },
      yAxis: { type: "category" },
      visualMap: {
        min: 0,
        max: 100,
        calculable: true,
      },
      series: [
        {
          type: "heatmap",
          encode: { x, y: yField, value },
        },
      ],
      tooltip: { trigger: "item" },
    };
  },
});

registerChartKind("funnel", {
  compile(widget, rows) {
    const { x, value } = widget.chart.encode;
    return {
      dataset: { source: rows },
      series: [
        {
          type: "funnel",
          encode: { itemName: x, value },
        },
      ],
      tooltip: { trigger: "item" },
    };
  },
});

registerChartKind("sankey", {
  compile(widget, rows) {
    // Sankey requires nodes/links data transform
    return {
      series: [
        {
          type: "sankey",
          data: [],
          links: [],
        },
      ],
      tooltip: { trigger: "item" },
    };
  },
});

registerChartKind("waterfall", {
  compile(widget, rows) {
    const encode = widget.chart?.encode || {};
    const xField = encode.x || Object.keys(rows[0] || {})[0] || "x";
    const yField =
      (Array.isArray(encode.y) ? encode.y[0] : encode.y) || Object.keys(rows[0] || {})[1] || "y";
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
      tooltip: { trigger: "axis" },
      xAxis: { type: "category", data: categories },
      yAxis: { type: "value" },
      series: [
        {
          name: "Base",
          type: "bar",
          stack: "Total",
          itemStyle: { borderColor: "transparent", color: "transparent" },
          emphasis: { itemStyle: { borderColor: "transparent", color: "transparent" } },
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
  },
});

registerChartKind("custom", {
  compile(widget, rows) {
    return {
      dataset: { source: rows },
      xAxis: { type: "category" },
      yAxis: { type: "value" },
      series: [{ type: "bar" }],
    };
  },
});
