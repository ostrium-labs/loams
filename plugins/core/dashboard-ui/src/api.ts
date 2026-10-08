import type { ThemeSelection } from "./theme/types";

export interface DashboardSpec {
  id: string;
  version: number;
  title: string;
  /**
   * Dashboard-level house: a preset id, a custom ThemeSpec, or absent for
   * flint's own defaults.
   *
   * Persisted through the dashboard PATCH at `/theme`. A widget's own
   * `flint.theme_spec` overrides this — per-widget wins over dashboard-level.
   */
  theme?: ThemeSelection | null;
  params: Array<{
    name: string;
    type: string;
    default?: unknown;
    datasetId?: number;
    column?: string;
  }>;
  layout: Array<{
    id: string;
    x: number;
    y: number;
    w: number;
    h: number;
  }>;
  widgets: Record<string, Widget>;
}

/**
 * A widget's data source: either the Superset warehouse, or one of Loams' own
 * cursor-resumable watch streams.
 *
 * Mirrors `SupersetDataSourceSchema | LoamsDataSourceSchema` in
 * `@loams-core/types`, restated rather than imported because this package is
 * a noEmit browser bundle and the zod schemas would pull in server-side code.
 * A `loams` widget is a projection of "what is happening now" rather than a
 * historical scan, and wants `refetchInterval` below.
 */
export type WidgetDataSource =
  | {
      source: "superset";
      datasetId?: number;
      sql?: string;
      params?: string[];
    }
  | {
      source: "loams";
      stream: "operations" | "approvals" | "notifications";
      token?: string;
      filter?: Record<string, unknown>;
    };

export interface Widget {
  id: string;
  type: "chart" | "kpi" | "table" | "text" | "filter" | "graph";
  data: WidgetDataSource;
  flint?: {
    chartType: string;
    title?: string;
    subtitle?: string;
    encodings: Record<string, any>;
    baseSize?: { width: number; height: number };
    chartProperties?: Record<string, unknown>;
    theme_spec?: string | Record<string, unknown>;
  };
  /**
   * A node/edge graph, for `type: "graph"` only.
   *
   * A SIBLING of `chart`, not another chart kind: `charts`
   * terminates in an ECharts option object and React Flow does not have one.
   * `core/flows` renders these instead. Declared here rather
   * than imported from `@loams-core/types` because this package is a
   * noEmit browser bundle and the zod schemas would pull in server-side code.
   */
  graph?: {
    title?: string;
    /**
     * Either name the nodes here, or set `derive` to read them from the rows.
     * Never both -- see `GraphDeriveSchema` for why.
     */
    nodes?: Array<{
      id: string;
      label?: string;
      labelField?: string;
      position?: { x: number; y: number };
      color?: string;
      className?: string;
    }>;
    /** Draw the graph from the data, for a source whose rows are not known ahead. */
    derive?: {
      idField: string;
      labelField?: string;
      sourceField?: string;
      targetField?: string;
      edgeLabelField?: string;
      colorField?: string;
    };
    edges?: Array<{
      id?: string;
      source: string;
      target: string;
      label?: string;
      value?: number;
      animated?: boolean;
      color?: string;
    }>;
    layout?: {
      direction?: "TB" | "LR" | "BT" | "RL";
      nodeWidth?: number;
      nodeHeight?: number;
      rankSep?: number;
      nodeSep?: number;
    };
    fitView?: boolean;
    pannable?: boolean;
    zoomable?: boolean;
  };
  chart?: {
    kind: "line" | "bar" | "pie" | "scatter" | "heatmap" | "funnel" | "sankey" | "area" | "custom";
    encode?: {
      x?: string | string[];
      y?: string | string[];
      series?: string | string[];
      value?: string | string[];
    };
    optionOverrides?: Record<string, unknown>;
  };
  interactions?: Array<{
    on: "click" | "brush";
    set: Record<string, string>;
  }>;
}

export interface Dataset {
  id: number;
  table_name: string;
  schema: string;
  database: { id: number; database_name: string };
  columns: Array<{
    column_name: string;
    type: string;
    is_dttm: boolean;
    verbose_name?: string;
    filterable: boolean;
    groupby: boolean;
  }>;
  metrics: Array<{
    metric_name: string;
    expression: string;
    verbose_name?: string;
  }>;
}

export const API_BASE = "/api";

export async function fetchDashboard(id: string): Promise<DashboardSpec> {
  const res = await fetch(`${API_BASE}/dashboards/${id}`);
  if (!res.ok) throw new Error(`Failed to load dashboard: ${res.statusText}`);
  return res.json();
}

export async function patchDashboard(
  id: string,
  baseVersion: number,
  ops: any[],
): Promise<DashboardSpec> {
  const res = await fetch(`${API_BASE}/dashboards/${id}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ baseVersion, ops }),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error(err.error || `Patch failed: ${res.statusText}`);
  }
  return res.json();
}

export async function addWidgetToDashboard(
  dashboardId: string,
  widget: Widget,
  position?: { x: number; y: number; w: number; h: number },
): Promise<DashboardSpec> {
  const res = await fetch(`${API_BASE}/dashboards/${dashboardId}/widgets`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ widget, position }),
  });
  if (!res.ok) throw new Error(`Failed to add widget: ${res.statusText}`);
  return res.json();
}

export async function removeWidgetFromDashboard(
  dashboardId: string,
  widgetId: string,
): Promise<DashboardSpec> {
  const res = await fetch(
    `${API_BASE}/dashboards/${encodeURIComponent(dashboardId)}/widgets/${encodeURIComponent(widgetId)}`,
    {
      method: "DELETE",
    },
  );
  if (!res.ok) throw new Error(`Failed to remove widget: ${res.statusText}`);
  return res.json();
}

export async function fetchDatasets(): Promise<Dataset[]> {
  const res = await fetch(`${API_BASE}/datasets`);
  if (!res.ok) throw new Error(`Failed to load datasets: ${res.statusText}`);
  const data = await res.json();
  return Array.isArray(data) ? data : data.result || [];
}

export async function fetchDataset(id: number): Promise<Dataset> {
  const res = await fetch(`${API_BASE}/datasets/${id}`);
  if (!res.ok) throw new Error(`Failed to load dataset: ${res.statusText}`);
  const data = await res.json();
  return data.result || data;
}

/**
 * Compile a widget to an ECharts option against sample data.
 *
 * `dashboardTheme` is the dashboard-level theme selection. It travels in its own
 * field rather than inside `params`, because every `params` entry is turned into
 * a SQL filter column server-side -- a theme smuggled in there would make the
 * chart query a column literally named "theme".
 */
export async function previewWidget(
  widget: Widget,
  params?: Record<string, unknown>,
  dashboardTheme?: unknown,
): Promise<{
  option: Record<string, unknown>;
  sampleRows: Record<string, unknown>[];
  rowCount: number;
}> {
  const res = await fetch(`${API_BASE}/widgets/preview`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ widget, params, dashboardTheme }),
  });
  if (!res.ok) throw new Error(`Failed to preview widget: ${res.statusText}`);
  return res.json();
}

/**
 * The compiled `{ nodes, edges }` for a `graph` widget.
 *
 * A different endpoint from `previewWidget`, not the same one with a different
 * body: the two widgets terminate in different shapes, so the routes compile
 * them through different services (`ctx.flow` and `ctx.render`). Posting a graph
 * widget to `/widgets/preview` is declined by that service, not silently
 * half-answered.
 */
export async function previewGraph(
  widget: Widget,
  params?: Record<string, unknown>,
  dashboardTheme?: unknown,
): Promise<{
  nodes: Array<Record<string, unknown>>;
  edges: Array<Record<string, unknown>>;
  theme?: Record<string, string>;
  fitView?: boolean;
  pannable?: boolean;
  zoomable?: boolean;
}> {
  const res = await fetch(`${API_BASE}/graphs/preview`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ widget, params, dashboardTheme }),
  });
  if (!res.ok) throw new Error(`Failed to preview widget: ${res.statusText}`);
  return res.json();
}
