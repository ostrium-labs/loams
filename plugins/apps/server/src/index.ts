import http from "node:http";
import { createConnectRouter } from "@connectrpc/connect";
import { registerRpcServices, type RpcContext } from "@loams-core/bi-rpc";
import { createNodeConnectBridge } from "./connect-bridge.js";
import { Context } from "cordis";
import ConsoleLogger from "@cordisjs/plugin-logger-console";
import { StderrExporter } from "./logger.js";
import { StoreService } from "@loams-core/store";
import { BiService } from "@loams-core/bi";
import { DataService } from "@loams-core/data";
import { ChartSpecService } from "@loams-core/chart-specs";
import { RenderService } from "@loams-core/charts";
import { FlowRenderService } from "@loams-core/flows";
import { DashboardSpecService } from "@loams-core/dashboards";
import {
  LoamsLiveService,
  type LoamsStreamName,
} from "@loams-core/live";
import { AgentToolsService, startMCPServer } from "@loams-core/agent-tools";
import {
  AgentBus,
  AuthService,
  DASHBOARD_MANIFEST,
  HttpRouter,
  PluginHost,
  PluginRegistry,
  dashboardLoader,
  loadAuthConfig,
  mountAuthApi,
  mountCoreApi,
  resolveAllowedOrigins,
  type AuthConfig,
} from "@loams-core/host";
import { startMswBiMock } from "./mock-msw-server.js";
import { readEnvWithFallback } from "./env.js";
import { registerPluginCatalog } from "./plugin-catalog.js";

export const DEFAULT_DASHBOARD_ID = "e3b0c442-98fc-1c14-9afbf4c8996fb924";

async function bootstrap() {
  const BI_PORT = parseInt(readEnvWithFallback("LOAMS_BI_PORT", "SUPERSET_PORT") || "8088", 10);
  const API_PORT = parseInt(process.env.PORT || "3001", 10);

  // 1. Start Feature-rich MSW Mock BI Server
  // stderr, not stdout: this process may be the MCP stdio server, and stdout is
  // reserved there for JSON-RPC framing. A bare status line there would corrupt it.
  process.stderr.write("Starting MSW Mock BI Server...\n");
  await startMswBiMock(BI_PORT);

  // 2. Initialize Cordis Root Context
  const ctx = new Context();

  // Register a log exporter FIRST: cordis core only buffers log records, it has no
  // built-in stdout exporter, so this must be attached before any plugin loads or its
  // load-time log lines are already lost. Do not move this below the ctx.plugin() calls.
  //
  // The destination depends on ENABLE_MCP, because this process also hosts the MCP
  // stdio server when it is enabled:
  //   - MCP disabled: ConsoleLogger writes to stdout, which is the normal, expected
  //     place for a server's logs.
  //   - MCP enabled:  stdout is RESERVED for JSON-RPC framing. ConsoleLogger's Node
  //     entry writes to stdout, so registering it would inject non-JSON-RPC
  //     bytes into the transport and corrupt the MCP session. Use StderrExporter
  //     instead so logs are still visible but never touch stdout.
  // Do not "simplify" this back into an unconditional ConsoleLogger.
  if (process.env.ENABLE_MCP === "true") {
    await ctx.logger.exporter(new StderrExporter());
  } else {
    // No `showDiff`: it appends a `+Nms` suffix to every rendered line, including the
    // blank separator lines of the multi-line startup banner, which mangles it.
    await ctx.plugin(ConsoleLogger);
  }

  // 3. Register plugins in dependency order
  await ctx.plugin(StoreService, {
    connectionString: process.env.DATABASE_URL || "memory",
  });

  await ctx.plugin(BiService, {
    baseUrl: readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL") || `http://localhost:${BI_PORT}`,
    username: readEnvWithFallback("LOAMS_BI_USER", "SUPERSET_USER") || "admin",
    password: readEnvWithFallback("LOAMS_BI_PASS", "SUPERSET_PASS") || "admin",
  });

  await ctx.plugin(DataService);
  await ctx.plugin(ChartSpecService);
  await ctx.plugin(RenderService);
  // Graph widgets, not chart widgets. Registered after `RenderService` and for a
  // stated reason: the two renderers are mutually exclusive per widget -- a
  // `graph` widget is declined by `ctx.render` and a `chart` widget is refused
  // by `ctx.flow` -- so their order does not matter, but both must exist before
  // the REST routes below are mounted, since `/api/graphs/preview` calls
  // `ctx.flow` and `/api/widgets/preview` calls `ctx.render`.
  await ctx.plugin(FlowRenderService);
  await ctx.plugin(DashboardSpecService);
  await ctx.plugin(AgentToolsService);

  // 3a-bis. Loams' own live state, as a dashboard data source.
  //
  // After DataService, which it injects, and started explicitly rather than in
  // the constructor: it opens watch streams, so booting it unconditionally
  // would make a dashboard-only deployment (no Loams server configured) log a
  // connection error on startup for a feature nobody asked for.
  if (process.env.LOAMS_URL) {
    await ctx.plugin(LoamsLiveService, {
      baseUrl: process.env.LOAMS_URL,
      token: process.env.LOAMS_TOKEN || undefined,
      streams: (process.env.LOAMS_LIVE_STREAMS?.split(",").filter(Boolean) as
        | LoamsStreamName[]
        | undefined) ?? undefined,
    });
    ctx.loamsLive.start();
  } else {
    console.warn(
      "[loams-live] LOAMS_URL is unset: widgets with `data.source: \"loams\"` will fall back to the BI backend",
    );
  }

  // 3b. Core plugin platform.
  //
  // Registered after the feature plugins and in dependency order: the route table
  // and the agent bus have to exist before the host, because the host injects
  // both, and the registry has to exist before the host for the same reason. The
  // registry reads `ctx.store` for persisted enable flags, so it also has to come
  // after StoreService -- if the store were missing it would still boot, but every
  // toggle would silently revert on the next restart.
  await ctx.plugin(HttpRouter);
  await ctx.plugin(AgentBus);

  // Authentication. Registered before the registry and the host because both read
  // `ctx.auth`: the registry annotates the plugin catalog per session, and the host wraps
  // every route a plugin registers in a `requiredScopes` guard. Reading it late would work
  // only by accident.
  //
  // Auth is disable-able. With no OIDC_ISSUER/OIDC_CLIENT_ID the service logs a warning
  // and the server runs unauthenticated, so `npm run dev` works with no IdP. It never
  // grants admin powers in that mode: `auth.adminScope` is undefined, and the console's
  // enable/disable endpoints refuse.
  await ctx.plugin(AuthService, { env: process.env });
  // The store is handed over explicitly rather than left to the registry's
  // `ctx.store` fallback. Cordis resolves an inherited context key only on a
  // context that declared it in `inject`, and the registry's own context does
  // not declare `store` -- so the fallback would throw, be swallowed by the
  // registry's try/catch, and silently persist nothing: every toggle would revert
  // on the next restart. Reading it here, from the root context where it is
  // visible, is what makes the flag survive.
  await ctx.plugin(PluginRegistry, { store: ctx.store });
  await ctx.plugin(PluginHost);

  // The dashboard is the platform's first plugin and is `alwaysOn`, so the host
  // loads it here rather than waiting for someone to press a toggle. Its skills
  // read the same `ctx.store` / `ctx.dashboard` services the REST routes below do,
  // which is why it is registered only after they are up.
  const dashboardStatus = await ctx.coreHost.register(DASHBOARD_MANIFEST, dashboardLoader);

  // The upstream adapters, registered immediately after the dashboard so the
  // catalog has its pinned rows first in the code as well as in the UI.
  //
  // Registering a manifest is not enabling it: `registerPluginCatalog` hands each
  // one to the HOST, which loads only what resolves to enabled and disposes the
  // cordis service again on `POST /api/plugins/<id>/disable`. That is why no
  // adapter service is `ctx.plugin(...)`-ed below in the boot chain -- doing that
  // would attach them for the life of the process and make "off" a lie.
  //
  // An adapter whose environment variables are absent stays in the catalog, off,
  // and carries the names of the variables it is waiting for. See
  // `plugin-catalog.ts` for the auto-enable policy.
  const { statuses: catalogStatuses } = await registerPluginCatalog(ctx);

  ctx.logger.info(
    "✓ Cordis context initialized with plugins: %s",
    [
      "store",
      "bi",
      "data",
      "chartSpecs",
      "render",
      "flow",
      "dashboard",
      "agentTools",
      "core",
    ].join(", "),
  );
  ctx.logger.info(
    "✓ Plugin host ready: %s (%s); control plane at /api/plugins, agent cards at /.well-known/agent-card.json",
    dashboardStatus.name,
    dashboardStatus.id,
  );
  ctx.logger.info(
    "✓ Plugin catalog: %s (%d enabled, %d not loaded)",
    catalogStatuses.length,
    catalogStatuses.filter((entry) => entry.enabled).length,
    catalogStatuses.filter((entry) => !entry.enabled).length,
  );

  // 4. Seed Default 6-Widget Dashboard
  await seedDefaultDashboard(ctx);

  // 5. Start MCP Server if requested
  if (process.env.ENABLE_MCP === "true") {
    ctx.logger.info("Starting MCP stdio server...");
    await startMCPServer(ctx);
  }

  // 6. Start Backend HTTP API Server for the UI
  startHttpApiServer(ctx, API_PORT);
}

/** Seed a realistic dashboard: warehouse tiles plus Loams' own live state. */
  async function seedDefaultDashboard(ctx: Context) {
  try {
    const defaultSpec = {
      id: DEFAULT_DASHBOARD_ID,
      // Bump when the seed's shape changes, so an already-seeded store picks the
      // new tiles up. See the guard below.
      version: 3,
      title: "Executive Analytics & Performance",
      params: [
        { name: "time_range", type: "select", default: "30d", datasetId: 1 },
        { name: "region", type: "select", default: "North America", datasetId: 1 },
      ],
      layout: [
        { id: "widget-revenue-trend", x: 0, y: 0, w: 6, h: 4 },
        { id: "widget-user-growth", x: 6, y: 0, w: 6, h: 4 },
        { id: "widget-category-sales", x: 0, y: 4, w: 6, h: 4 },
        { id: "widget-market-share", x: 6, y: 4, w: 6, h: 4 },
        { id: "widget-server-health", x: 0, y: 8, w: 12, h: 4 },
        { id: "widget-live-operations", x: 0, y: 12, w: 6, h: 4 },
        { id: "widget-live-approvals", x: 6, y: 12, w: 6, h: 4 },
        { id: "widget-live-topology", x: 0, y: 16, w: 12, h: 6 },
      ],
      widgets: {
        "widget-revenue-trend": {
          id: "widget-revenue-trend",
          type: "chart",
          data: { source: "bi", datasetId: 1 },
          chart: {
            kind: "line",
            encode: { x: "quarter", y: ["revenue", "profit"] },
            optionOverrides: {
              title: {
                text: "Revenue & Profit Trends",
                subtext: "Quarterly financial performance ($ USD)",
              },
            },
          },
        },
        "widget-user-growth": {
          id: "widget-user-growth",
          type: "chart",
          data: { source: "bi", datasetId: 2 },
          chart: {
            kind: "line",
            encode: { x: "date", y: ["visitors", "active_users"] },
            optionOverrides: {
              title: {
                text: "User Acquisition & Growth",
                subtext: "Daily visitors and active members",
              },
            },
          },
        },
        "widget-category-sales": {
          id: "widget-category-sales",
          type: "chart",
          data: { source: "bi", datasetId: 3 },
          chart: {
            kind: "bar",
            encode: { x: "category", y: "sales" },
            optionOverrides: {
              title: {
                text: "Sales by Product Category",
                subtext: "Revenue distribution across product lines",
              },
            },
          },
          interactions: [{ on: "click", set: { category: "category" } }],
        },
        "widget-market-share": {
          id: "widget-market-share",
          type: "chart",
          data: { source: "bi", datasetId: 3 },
          chart: {
            kind: "pie",
            encode: { x: "category", value: "market_share" },
            optionOverrides: {
              title: { text: "Market Share Breakdown", subtext: "Category percentage share" },
            },
          },
          interactions: [{ on: "click", set: { category: "category" } }],
        },
        "widget-server-health": {
          id: "widget-server-health",
          type: "chart",
          data: { source: "bi", datasetId: 4 },
          chart: {
            kind: "bar",
            encode: { x: "server_name", y: ["cpu_usage", "memory_usage"] },
            optionOverrides: {
              title: {
                text: "Cluster Node Utilization",
                subtext: "CPU & Memory utilization (%) across cluster",
              },
            },
          },
        },
        // Loams' own state, from the Watch* streams rather than the warehouse.
        // These three tiles are the reason the live plugin exists: the tiles
        // above describe a period, these describe right now, and they re-read on
        // an interval instead of on a filter change.
        "widget-live-operations": {
          id: "widget-live-operations",
          type: "chart",
          data: { source: "loams", stream: "operations" },
          chart: {
            kind: "bar",
            encode: { x: "kind", y: "count" },
            optionOverrides: {
              title: {
                text: "Operations in flight",
                subtext: "Live from loams.operations.v1/WatchOperations",
              },
            },
          },
        },
        "widget-live-approvals": {
          id: "widget-live-approvals",
          type: "chart",
          data: { source: "loams", stream: "approvals" },
          chart: {
            kind: "pie",
            encode: { x: "state", value: "count" },
            optionOverrides: {
              title: {
                text: "Approvals awaiting a decision",
                subtext: "Live from loams.approvals.v1/WatchApprovals",
              },
            },
          },
        },
        // Derived from the approvals stream rather than operations: `Operation` has no
        // parent pointer at all, so a graph over operations would draw unrelated
        // nodes, while `Approval.operation_id` is the one real relationship
        // between the two.
        //
        // One node per gated operation, and no edges: the node id has to come
        // from a row, and the two entities here arrive on different streams, so
        // there is no row that carries both ends of an operation/approval edge.
        // Naming `sourceField`/`targetField` here would produce edges the
        // compiler then drops for having an endpoint it never saw -- a tile that
        // looks like a topology and is not one.
        "widget-live-topology": {
          id: "widget-live-topology",
          type: "graph",
          data: { source: "loams", stream: "approvals" },
          graph: {
            title: "Operations waiting on an approval",
            derive: {
              idField: "operationId",
              labelField: "kind",
              colorField: "state",
            },
            layout: { direction: "LR" },
          },
        },
      },
    };

    const existing = await ctx.store.getDashboard(DEFAULT_DASHBOARD_ID).catch(() => null);
    // The version guard is what lets a change to the seed reach a store that was
    // already seeded. Without it, tiles added here would never appear for anyone
    // who started the server before this revision, and the seed would only ever
    // apply to a fresh database.
    if (
      !existing ||
      existing.title === "Purchase Last 30 Days" ||
      !existing.widgets["widget-revenue-trend"] ||
      (existing.version ?? 0) < defaultSpec.version
    ) {
      await ctx.store.saveDashboard(defaultSpec, "system-seed");
      ctx.logger.info(
        `✓ Seeded default dashboard: "${defaultSpec.title}" (id: ${DEFAULT_DASHBOARD_ID})`,
      );
    }
  } catch (err: any) {
    ctx.logger.warn("Could not seed default dashboard: %s", err.message);
  }
}

/** HTTP Server providing JSON APIs to the Frontend UI */
function startHttpApiServer(ctx: Context, port: number) {
  // Core plugin control plane + A2A, mounted into the mutable route table:
  //   GET  /api/plugins                 GET  /.well-known/agent-card.json
  //   GET  /api/plugins/:id             GET  /.well-known/agent.json  (legacy alias)
  //   POST /api/plugins/:id/enable      GET  /.well-known/agent-card/:id
  //   POST /api/plugins/:id/disable     POST /a2a/v1/message:send
  //   GET  /api/plugins/:id/agent
  //
  // The disposer this returns is intentionally dropped: the route table lives as
  // long as the process, and there is no shutdown path here that would need to
  // unregister the platform itself. Plugins registered later DO get their
  // disposers -- the host owns those, which is what makes a plugin's routes
  // disappear on `POST /api/plugins/:id/disable`.
  mountCoreApi(ctx, { baseUrl: () => `http://localhost:${port}` });

  //   GET  /api/auth/login              POST /api/auth/backchannel-logout
  //   GET  /api/auth/callback           GET  /api/auth/me
  //   GET  /api/auth/logout
  //
  // Registered into the same route table as everything else, so it sits at the same point
  // in the dispatch order: after the ConnectRPC bridge and after the thirteen original
  // REST routes, none of which it can shadow.
  mountAuthApi(ctx, { defaultReturnTo: "/" });

  // ConnectRPC is mounted on the SAME http server as the REST routes above.
  // Both transports are live at once: the hand-rolled `/api/*` JSON routes are
  // what the dashboard UI calls today, while `bi.v1.*` is the typed contract
  // that new clients (and future adapters) use.
  //
  // Connect-ES v2 exposes one HTTP handler PER RPC, each tagged with the request
  // path it serves. There is no aggregate handler to delegate to, so they are
  // indexed by path and dispatched explicitly. This runs before the REST
  // fallbacks claim the request.
  const handleConnectRpc = createNodeConnectBridge(
    registerRpcServices(createConnectRouter(), ctx as unknown as RpcContext),
  );

  // Resolved once, at boot. A wildcard in the configuration is rejected here rather than
  // being silently honoured, so it cannot reach a running server.
  const allowedOrigins = new Set<string>(
    resolveAllowedOrigins(readAuthConfig(ctx), [
      `http://localhost:${port}`,
      `http://127.0.0.1:${port}`,
      `http://localhost:5173`,
    ]),
  );
  ctx.logger.info(
    "CORS allowlist: %s",
    allowedOrigins.size > 0
      ? [...allowedOrigins].join(", ")
      : "(none; browser access is same-origin only)",
  );

  const server = http.createServer(async (req, res) => {
    // CORS, with an EXPLICIT origin allowlist.
    //
    // This used to be `Access-Control-Allow-Origin: *`, which is not merely loose -- the
    // CORS specification makes a wildcard origin incompatible with credentialed requests,
    // so while it stood the session cookie could not have been sent by a browser at all.
    // The allowlist is a prerequisite for login, not a hardening pass on top of it.
    //
    // A request with no `Origin` header (curl, same-origin navigation, server-to-server)
    // gets no CORS headers at all, which is correct: CORS only constrains browsers, and
    // the session cookie's own SameSite=Lax plus the admin scope are what protect those.
    const requestOrigin = req.headers.origin;
    if (requestOrigin && allowedOrigins.has(normalizeOrigin(requestOrigin))) {
      res.setHeader("Access-Control-Allow-Origin", requestOrigin);
      // Required alongside a specific origin for the browser to send cookies at all.
      res.setHeader("Access-Control-Allow-Credentials", "true");
      res.setHeader("Vary", "Origin");
    }
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS");
    res.setHeader(
      "Access-Control-Allow-Headers",
      "Content-Type, Authorization, Connect-Protocol-Version, Connect-Timeout-Ms, Grpc-Timeout, X-Grpc-Web, X-User-Agent",
    );

    if (req.method === "OPTIONS") {
      // A disallowed origin gets no Access-Control-Allow-Origin above, so the browser
      // rejects the preflight. Answering 204 anyway keeps the shape of the old behaviour
      // without pretending the request was authorized.
      res.writeHead(204);
      res.end();
      return;
    }

    // ConnectRPC first: it owns an exact set of paths, and returning false means
    // "not mine", so this never shadows a REST route below.
    if (await handleConnectRpc(req, res)) return;

    const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);
    const path = url.pathname;

    const sendJson = (status: number, data: any) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(data));
    };

    const readBody = async (): Promise<any> => {
      return new Promise((resolve) => {
        let body = "";
        req.on("data", (chunk) => {
          body += chunk;
        });
        req.on("end", () => {
          try {
            resolve(JSON.parse(body || "{}"));
          } catch {
            resolve({});
          }
        });
      });
    };

    try {
      // 1. GET /api/dashboards
      if (path === "/api/dashboards" && req.method === "GET") {
        const list = await ctx.store.listDashboards();
        sendJson(200, list);
        return;
      }

      // 2. GET /api/dashboards/:id
      const dashGetMatch = path.match(/^\/api\/dashboards\/([a-zA-Z0-9-]+)$/);
      if (dashGetMatch && req.method === "GET") {
        const id = dashGetMatch[1];
        const spec = await ctx.dashboard.load(id);
        sendJson(200, spec);
        return;
      }

      // 3. POST /api/dashboards
      if (path === "/api/dashboards" && req.method === "POST") {
        const body = await readBody();
        const spec = await ctx.dashboard.create(body.title || "New Dashboard", body.spec);
        sendJson(201, spec);
        return;
      }

      // 4. PATCH /api/dashboards/:id
      if (dashGetMatch && req.method === "PATCH") {
        const id = dashGetMatch[1];
        const body = await readBody();
        const patched = await ctx.dashboard.patch(
          id,
          body.baseVersion,
          body.ops || [],
          body.actor || "human-ui",
        );
        sendJson(200, patched);
        return;
      }

      // 5. POST /api/dashboards/:id/widgets
      const addWidgetMatch = path.match(/^\/api\/dashboards\/([^/]+)\/widgets$/);
      if (addWidgetMatch && req.method === "POST") {
        const id = decodeURIComponent(addWidgetMatch[1]);
        const body = await readBody();
        const updated = await ctx.dashboard.addWidget(id, body.widget, body.position, "human-ui");
        sendJson(200, updated);
        return;
      }

      // 6. DELETE /api/dashboards/:id/widgets/:widgetId
      const delWidgetMatch = path.match(/^\/api\/dashboards\/([^/]+)\/widgets\/([^/]+)$/);
      if (delWidgetMatch && req.method === "DELETE") {
        const id = decodeURIComponent(delWidgetMatch[1]);
        const widgetId = decodeURIComponent(delWidgetMatch[2]);
        const updated = await ctx.dashboard.removeWidget(id, widgetId, "human-ui");
        sendJson(200, updated);
        return;
      }

      // 7. GET /api/datasets
      if (path === "/api/datasets" && req.method === "GET") {
        const datasets = await ctx.bi.listDatasets();
        sendJson(200, datasets);
        return;
      }

      // 8. GET /api/datasets/:id
      const datasetMatch = path.match(/^\/api\/datasets\/(\d+)$/);
      if (datasetMatch && req.method === "GET") {
        const ds = await ctx.bi.describeDataset(parseInt(datasetMatch[1], 10));
        sendJson(200, ds);
        return;
      }

      // 9. POST /api/widgets/data (fetch data for a widget)
      if (path === "/api/widgets/data" && req.method === "POST") {
        const body = await readBody();
        const data = await ctx.data.fetchWidgetData(body.widget);
        sendJson(200, data);
        return;
      }

      // 10. POST /api/widgets/preview (compile widget to ECharts option + sample rows)
      if (path === "/api/widgets/preview" && req.method === "POST") {
        const body = await readBody();
        const params = body.params || {};
        // `body.dashboardTheme` is optional. Omitting it previews the widget with
        // no dashboard theme, which is exactly right for a widget that carries
        // its own `chartSpec.theme_spec`.
        const option = await ctx.render.compileWidget(body.widget, params, body.dashboardTheme);
        const data = await ctx.data.fetchWidgetData(body.widget, params);
        const rows = Array.isArray(data?.data) ? data.data : Array.isArray(data) ? data : [];
        sendJson(200, {
          option,
          sampleRows: rows.slice(0, 5),
          rowCount: data?.rowcount ?? rows.length,
        });
        return;
      }

      // 14. POST /api/graphs/preview (compile a `graph` widget to React Flow
      // nodes + edges).
      //
      // A SEPARATE route from `/api/widgets/preview` rather than a branch inside
      // it, because the two widgets terminate in different shapes and are
      // compiled by different services: this one calls `ctx.flow`, that one calls
      // `ctx.render`. Posting a graph widget to the echarts route is declined by
      // `ctx.render` rather than silently half-answered.
      if (path === "/api/graphs/preview" && req.method === "POST") {
        const body = await readBody();
        const params = body.params || {};
        // `tryCompileGraphWidget`, not `compileGraphWidget`: this route accepts
        // whatever widget it is posted, so a non-graph widget is a 400 with a
        // reason rather than a 500 from a thrown refusal.
        const compiled = await ctx.flow.tryCompileGraphWidget(
          body.widget,
          params,
          body.dashboardTheme,
        );
        if (!compiled.ok) {
          sendJson(400, { error: compiled.reason });
          return;
        }
        const { graph } = compiled;
        sendJson(200, {
          nodes: graph.nodes,
          edges: graph.edges,
          theme: graph.theme,
          fitView: graph.fitView,
          pannable: graph.pannable,
          zoomable: graph.zoomable,
        });
        return;
      }

      // 11. GET /api/themes — the theme catalogue, sorted by label.
      if (path === "/api/themes" && req.method === "GET") {
        // Served from `ctx.chartSpecs`, which reads chart-specs' own THEME_PRESETS rather
        // than `listThemePresets()`: the picker renders a swatch per theme and
        // that helper omits `icon`.
        sendJson(200, { themes: ctx.chartSpecs.listThemes() });
        return;
      }

      // 12. GET /api/themes/:id — one theme, resolved, plus chart-specs' report.
      //
      // A resolved theme that carries report entries is VALID but downgraded;
      // an unresolvable one comes back `valid: false` with the report attached,
      // because chart-specs treats an unknown house name as an error rather than
      // silently rendering some other house's colours. The UI shows the report
      // either way.
      const themeMatch = path.match(/^\/api\/themes\/([a-zA-Z0-9._-]+)$/);
      if (themeMatch && req.method === "GET") {
        const resolution = ctx.chartSpecs.resolveTheme(decodeURIComponent(themeMatch[1]));
        sendJson(200, {
          valid: resolution.valid,
          spec: resolution.spec,
          report: resolution.report,
        });
        return;
      }

      // 13. POST /api/agent/tools/:name (execute an agent tool)
      const toolMatch = path.match(/^\/api\/agent\/tools\/([a-zA-Z0-9_]+)$/);
      if (toolMatch && req.method === "POST") {
        const toolName = toolMatch[1];
        const body = await readBody();
        const tools = ctx.agentTools.getToolDefinitions();
        const tool = tools.find((t: any) => t.name === toolName);
        if (!tool) {
          sendJson(404, { error: `Tool not found: ${toolName}` });
          return;
        }
        const result = await tool.handler(body);
        sendJson(200, result);
        return;
      }

      // Core plugin platform, and any route a loaded plugin registered with it.
      //
      // Consulted LAST, after the ConnectRPC bridge and after all thirteen routes
      // above, so a plugin can never shadow the bridge or a pre-existing REST
      // endpoint -- and so a plugin that has been disabled falls straight through
      // to the 404 on the next line instead of hitting a stale handler.
      if (await ctx.router.handle(req, res)) return;

      sendJson(404, { message: "Not Found", path });
    } catch (err: any) {
      ctx.logger.error("API Error: %s", err);
      sendJson(500, { error: err.message || "Internal Server Error" });
    }
  });

  server.listen(port, () => {
    ctx.logger.info(`\n======================================================`);
    ctx.logger.info(`🚀 CDP Dashboard Backend API Server listening on:`);
    ctx.logger.info(`   http://localhost:${port}`);
    ctx.logger.info(
      `   Default Dashboard URL: http://localhost:${port}/api/dashboards/${DEFAULT_DASHBOARD_ID}`,
    );
    ctx.logger.info(`======================================================\n`);
  });
}

bootstrap().catch((err) => {
  process.stderr.write(
    `Failed to start server: ${err instanceof Error ? (err.stack ?? err.message) : String(err)}\n`,
  );
  process.exit(1);
});

/** Trailing slashes are not significant in an origin, and users paste them. */
function normalizeOrigin(origin: string): string {
  return origin.trim().replace(/\/+$/, "");
}

/**
 * The auth config the CORS allowlist is derived from.
 *
 * Falls back to a disabled config when no AuthService is loaded, so the server still
 * starts without one instead of throwing inside CORS setup.
 */
function readAuthConfig(ctx: Context): AuthConfig {
  try {
    const auth = (ctx as unknown as { auth?: { config: AuthConfig } }).auth;
    if (auth?.config) return auth.config;
  } catch {
    /* auth not loaded */
  }
  return loadAuthConfig({});
}
