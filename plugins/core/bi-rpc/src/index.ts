/**
 * The services the BI gateway exposes over ConnectRPC, and the glue that
 * adapts them to the existing cordis services.
 *
 * WHY A GATEWAY LAYER AT ALL
 * -------------------------
 * The upstreams this project talks to (the BI backend today; Forgejo, Zulip,
 * Glitchtip, Langfuse, OpenPanel and ItsAPlan if their adapters are added) all
 * speak REST over HTTP with their own auth schemes and their own response
 * shapes. None of them speak gRPC. So this package is NOT a client for those
 * upstreams -- it is the contract the BI gateway TERMINATES. The gateway
 * speaks ConnectRPC to UI apps, and translates to each upstream's REST API
 * behind its adapter.
 *
 * That direction matters: a "ConnectRPC client for Zulip" is not possible
 * without a Zulip gRPC server, which does not exist. What is possible -- and
 * what this package does -- is give every UI app one stable, typed contract
 * regardless of how many REST upstreams sit behind it.
 *
 * Row/value encoding
 * ------------------
 * The BI backend returns a column-keyed array of JSON rows whose column types are not
 * known until runtime. A `map<string, double>` would silently coerce the string
 * and boolean columns. `Row.values` is therefore a `map<string, Value>` with an
 * explicit oneof, so a numeric column stays a number and a string column stays a
 * string across the wire.
 */

import type { ConnectRouter, ServiceImpl } from "@connectrpc/connect";
import { create, fromJson, toJson } from "@bufbuild/protobuf";
import {
  StructSchema,
  ValueSchema as StructValueSchema,
  type Value as StructValueMessage,
} from "@bufbuild/protobuf/wkt";
import {
  DataService,
  DatasetRefSchema,
  ListDatasetsResponseSchema,
  DescribeDatasetResponseSchema,
  ColumnSchema,
  QueryResponseSchema,
  RowSchema,
  type ListDatasetsResponse,
  type DescribeDatasetResponse,
  type QueryResponse,
  type Row as PbRow,
} from "./gen/bi/v1/data_pb.js";
import {
  DashboardService,
  ListDashboardsResponseSchema,
  DashboardSummarySchema,
  GetDashboardResponseSchema,
  WidgetDataResponseSchema,
  PreviewWidgetResponseSchema,
  ThemeService,
  ListThemesResponseSchema,
  ThemeSummarySchema,
  GetThemeResponseSchema,
  ThemeReportEntrySchema,
  type ListDashboardsResponse,
  type GetDashboardResponse,
  type WidgetDataResponse,
  type PreviewWidgetResponse,
  type ListThemesResponse,
  type GetThemeResponse,
} from "./gen/bi/v1/dashboard_pb.js";

export { DataService } from "./gen/bi/v1/data_pb.js";

export { DashboardService, ThemeService } from "./gen/bi/v1/dashboard_pb.js";

export type { ServiceImpl } from "@connectrpc/connect";

// ---------------------------------------------------------------------------
// The slice of the cordis context these handlers use.
//
// Declared structurally rather than imported from the owning packages so this
// transport layer creates no build-order dependency on every service package,
// and so a partially-booted container fails at the call site with a clear
// message instead of failing to resolve at import time.
// ---------------------------------------------------------------------------

export interface BiPort {
  listDatasets(): Promise<unknown>;
  describeDataset(
    id: number,
  ): Promise<{ id?: number; table_name?: string; columns?: unknown[] } | undefined>;
  queryData(
    datasetIdOrPayload: unknown,
    columns?: string[],
    filters?: unknown[],
    orderby?: unknown[],
    rowLimit?: number,
  ): Promise<unknown>;
}

export interface DashboardPort {
  listDashboards(): Promise<unknown>;
  load(id: string): Promise<unknown>;
  create(title: string, spec?: unknown): Promise<unknown>;
  patch(
    id: string,
    baseVersion: number | undefined,
    ops: unknown[],
    actor: string,
  ): Promise<unknown>;
}

export interface DataPort {
  fetchWidgetData(widget: unknown, params?: Record<string, unknown>): Promise<unknown>;
}

export interface RenderPort {
  compileWidget(
    widget: unknown,
    params?: Record<string, unknown>,
    dashboardTheme?: unknown,
  ): Promise<unknown>;
}

export interface ChartSpecPort {
  listThemes(): Array<{ id: string; label: string; description: string; icon: string }>;
  resolveTheme(theme?: unknown): {
    valid: boolean;
    spec?: unknown;
    report: Array<{ stage: string; path: string; message: string }>;
  };
}

export interface RpcContext {
  bi: BiPort;
  dashboard: DashboardPort;
  data: DataPort;
  render: RenderPort;
  chartSpecs: ChartSpecPort;
  logger: {
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
    error(...args: unknown[]): void;
  };
}

function requirePort<T>(ctx: RpcContext, name: keyof RpcContext, description: string): T {
  const port = ctx[name];
  if (!port) {
    throw new Error(
      `ConnectRPC handler for ${description} needs "ctx.${String(name)}", which is not available. ` +
        `Is the owning plugin loaded?`,
    );
  }
  return port as T;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Unwrap a protobuf `google.protobuf.Value` (e.g. a JSON Patch op value). */
function valueToJson(value: StructValueMessage | undefined): unknown {
  if (value === undefined) return undefined;
  return toJson(StructValueSchema, value) as unknown;
}

/**
 * Convert one JSON row into a protobuf row.
 *
 * Column values are `google.protobuf.Value`, so every JSON scalar keeps its type
 * and round-trips back to plain JSON on the client. A `Date` is sent as its ISO
 * string because JSON has no date scalar. Nested JSON (a PostgREST aggregate,
 * say) is preserved as a struct rather than stringified, since `Value` can
 * represent it natively.
 */
function toPbRow(row: unknown): PbRow {
  if (!isRecord(row)) return create(RowSchema, {});

  const values: Record<string, StructValueMessage> = {};
  for (const [key, raw] of Object.entries(row)) {
    const json = raw instanceof Date ? raw.toISOString() : raw;
    values[key] = fromJson(StructValueSchema, (json ?? null) as never);
  }
  return create(RowSchema, { values });
}

/**
 * Normalize a the BI backend result payload into rows.
 *
 * `queryData` returns `result[0]` (or the whole body) and the dataset list
 * returns a `{ result: [...] }` envelope, so both shapes are unwrapped here
 * rather than at each call site.
 */
function extractRows(payload: unknown): unknown[] {
  if (Array.isArray(payload)) return payload;
  if (isRecord(payload)) {
    if (Array.isArray(payload.data)) return payload.data;
    if (Array.isArray(payload.result)) {
      const first = payload.result[0];
      if (isRecord(first) && Array.isArray(first.data)) return first.data;
      if (Array.isArray(first)) return first;
    }
  }
  return [];
}

function extractCount(payload: unknown, rows: unknown[]): number {
  if (isRecord(payload) && typeof payload.rowcount === "number") return payload.rowcount;
  return rows.length;
}

export function createDataService(ctx: RpcContext): ServiceImpl<typeof DataService> {
  return {
    async listDatasets(): Promise<ListDatasetsResponse> {
      const bi = requirePort<BiPort>(
        ctx,
        "bi",
        "DataService.ListDatasets",
      );
      const payload = await bi.listDatasets();
      const list = isRecord(payload) && Array.isArray(payload.result) ? payload.result : [];
      const datasets = list.filter(isRecord).map((row) =>
        create(DatasetRefSchema, {
          id: BigInt(typeof row.id === "number" ? row.id : Number(row.id ?? 0)),
          tableName:
            typeof row.table_name === "string" ? row.table_name : String(row.table_name ?? ""),
          label: typeof row.table_name === "string" ? row.table_name : "",
        }),
      );
      return create(ListDatasetsResponseSchema, { datasets, count: datasets.length });
    },

    async describeDataset(request): Promise<DescribeDatasetResponse> {
      const bi = requirePort<BiPort>(
        ctx,
        "bi",
        "DataService.DescribeDataset",
      );
      const described = await bi.describeDataset(Number(request.id));
      const rawColumns = Array.isArray(described?.columns) ? described.columns : [];
      return create(DescribeDatasetResponseSchema, {
        id: BigInt(Number(described?.id ?? request.id)),
        tableName: described?.table_name ?? "",
        columns: rawColumns.filter(isRecord).map((col) =>
          create(ColumnSchema, {
            columnName:
              typeof col.column_name === "string" ? col.column_name : String(col.column_name ?? ""),
            type: typeof col.type === "string" ? col.type : String(col.type ?? ""),
            semanticType: typeof col.semantic_type === "string" ? col.semantic_type : "",
            isDttm: col.is_dttm === true,
          }),
        ),
      });
    },

    async query(request): Promise<QueryResponse> {
      const bi = requirePort<BiPort>(ctx, "bi", "DataService.Query");
      // Repeated fields default to empty on a real message, but these handlers
      // are also called directly in tests with plain literals. Defaulting here
      // keeps both paths honest instead of throwing on `undefined.map`.
      const filters = (request.filters ?? []).map((f) => ({
        col: f.col,
        op: f.op || "==",
        val: f.values.length === 1 ? f.values[0] : f.values,
      }));
      const columns = request.columns ?? [];
      const orderby = request.orderby ?? [];
      const payload = await bi.queryData(
        Number(request.datasetId),
        columns,
        filters.length > 0 ? filters : undefined,
        orderby.length > 0 ? orderby : undefined,
        request.rowLimit > 0 ? request.rowLimit : undefined,
      );
      const rows = extractRows(payload);
      return create(QueryResponseSchema, {
        rows: rows.map(toPbRow),
        rowcount: extractCount(payload, rows),
      });
    },
  };
}

export function createDashboardService(ctx: RpcContext): ServiceImpl<typeof DashboardService> {
  return {
    async listDashboards(): Promise<ListDashboardsResponse> {
      const dashboard = requirePort<DashboardPort>(
        ctx,
        "dashboard",
        "DashboardService.ListDashboards",
      );
      const payload = await dashboard.listDashboards();
      const list = Array.isArray(payload)
        ? payload
        : isRecord(payload) && Array.isArray(payload.dashboards)
          ? payload.dashboards
          : [];
      return create(ListDashboardsResponseSchema, {
        dashboards: list.filter(isRecord).map((row) =>
          create(DashboardSummarySchema, {
            id: String(row.id ?? ""),
            title: String(row.title ?? ""),
            version: typeof row.version === "number" ? row.version : 0,
            widgetCount: isRecord(row.widgets) ? Object.keys(row.widgets).length : 0,
          }),
        ),
      });
    },

    async getDashboard(request): Promise<GetDashboardResponse> {
      const dashboard = requirePort<DashboardPort>(
        ctx,
        "dashboard",
        "DashboardService.GetDashboard",
      );
      const spec = await dashboard.load(request.id);
      // A Struct-typed field takes JSON at construction and yields a message back,
      // so `create` is given the plain document.
      return create(GetDashboardResponseSchema, { spec: (spec ?? {}) as never });
    },

    async createDashboard(request): Promise<GetDashboardResponse> {
      const dashboard = requirePort<DashboardPort>(
        ctx,
        "dashboard",
        "DashboardService.CreateDashboard",
      );
      const spec = await dashboard.create(
        request.title || "New Dashboard",
        request.spec as unknown,
      );
      return create(GetDashboardResponseSchema, { spec: (spec ?? {}) as never });
    },

    async patchDashboard(request): Promise<GetDashboardResponse> {
      const dashboard = requirePort<DashboardPort>(
        ctx,
        "dashboard",
        "DashboardService.PatchDashboard",
      );
      // fast-json-patch wants a plain value; a `google.protobuf.Value` wrapper
      // applied literally would write `{kind: ...}` into the document.
      const ops = (request.ops ?? []).map((op) => ({
        op: op.op,
        path: op.path,
        value: valueToJson(op.value),
      }));
      const spec = await dashboard.patch(
        request.id,
        request.baseVersion > 0 ? request.baseVersion : undefined,
        ops,
        request.actor || "connectrpc",
      );
      return create(GetDashboardResponseSchema, { spec: (spec ?? {}) as never });
    },

    async fetchWidgetData(request): Promise<WidgetDataResponse> {
      const data = requirePort<DataPort>(ctx, "data", "DashboardService.FetchWidgetData");
      const params = (request.params as Record<string, unknown> | undefined) ?? {};
      const payload = await data.fetchWidgetData(request.widget?.spec as unknown, params);
      const rows = extractRows(payload);
      return create(WidgetDataResponseSchema, {
        data: create(QueryResponseSchema, {
          rows: rows.map(toPbRow),
          rowcount: extractCount(payload, rows),
        }),
        sampleRows: rows.slice(0, 5).map(toPbRow),
      });
    },

    async previewWidget(request): Promise<PreviewWidgetResponse> {
      const render = requirePort<RenderPort>(ctx, "render", "DashboardService.PreviewWidget");
      const data = requirePort<DataPort>(ctx, "data", "DashboardService.PreviewWidget");
      // The dashboard theme travels as a Struct so a richer selection shape
      // later needs no contract change; the renderer takes it as-is.
      const params = (request.params as Record<string, unknown> | undefined) ?? {};
      const widget = request.widget?.spec as unknown;
      const option = await render.compileWidget(widget, params, request.dashboardTheme as unknown);
      const payload = await data.fetchWidgetData(widget, params);
      const rows = extractRows(payload);
      return create(PreviewWidgetResponseSchema, {
        option: (option ?? {}) as never,
        sampleRows: rows.slice(0, 5).map(toPbRow),
        rowCount: extractCount(payload, rows),
      });
    },
  };
}

export function createThemeService(ctx: RpcContext): ServiceImpl<typeof ThemeService> {
  return {
    async listThemes(): Promise<ListThemesResponse> {
      const chartSpecs = requirePort<ChartSpecPort>(ctx, "chartSpecs", "ThemeService.ListThemes");
      return create(ListThemesResponseSchema, {
        themes: chartSpecs.listThemes().map((t) =>
          create(ThemeSummarySchema, {
            id: t.id,
            label: t.label,
            description: t.description,
            icon: t.icon,
          }),
        ),
      });
    },

    async getTheme(request): Promise<GetThemeResponse> {
      const chartSpecs = requirePort<ChartSpecPort>(ctx, "chartSpecs", "ThemeService.GetTheme");
      const resolution = chartSpecs.resolveTheme(request.id);
      return create(GetThemeResponseSchema, {
        valid: resolution.valid === true,
        // A resolved-but-downgraded theme is still valid and carries a report.
        // The client shows the report without blocking the theme.
        spec:
          resolution.valid && resolution.spec !== undefined
            ? (resolution.spec as never)
            : undefined,
        report: (resolution.report ?? []).map((entry) =>
          create(ThemeReportEntrySchema, {
            stage: entry.stage,
            path: entry.path,
            message: entry.message,
          }),
        ),
      });
    },
  };
}

/**
 * Register every service implementation onto a Connect router.
 *
 * The descriptor-to-implementation mapping lives here rather than at each call
 * site so a new service cannot be added to the transport without also being
 * routed. Returns the router for chaining.
 */
export function registerRpcServices(router: ConnectRouter, ctx: RpcContext): ConnectRouter {
  router.service(DataService, createDataService(ctx));
  router.service(DashboardService, createDashboardService(ctx));
  router.service(ThemeService, createThemeService(ctx));
  return router;
}
