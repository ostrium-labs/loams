import http from "node:http";

export interface MockDataset {
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
  data: Record<string, unknown>[];
}

export const MOCK_DATASETS: Record<number, MockDataset> = {
  1: {
    id: 1,
    table_name: "quarterly_financials",
    schema: "public",
    database: { id: 1, database_name: "PostgreSQL - Analytics" },
    columns: [
      { column_name: "quarter", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
      {
        column_name: "revenue",
        type: "DECIMAL",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      { column_name: "cost", type: "DECIMAL", is_dttm: false, filterable: false, groupby: false },
      { column_name: "profit", type: "DECIMAL", is_dttm: false, filterable: false, groupby: false },
      {
        column_name: "units_sold",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      { column_name: "region", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
    ],
    metrics: [
      { metric_name: "total_revenue", expression: "SUM(revenue)", verbose_name: "Total Revenue" },
      { metric_name: "total_profit", expression: "SUM(profit)", verbose_name: "Total Profit" },
    ],
    data: [
      {
        quarter: "2024-Q1",
        revenue: 125000,
        cost: 82000,
        profit: 43000,
        units_sold: 4200,
        region: "North America",
      },
      {
        quarter: "2024-Q2",
        revenue: 148000,
        cost: 91000,
        profit: 57000,
        units_sold: 5100,
        region: "North America",
      },
      {
        quarter: "2024-Q3",
        revenue: 162000,
        cost: 98000,
        profit: 64000,
        units_sold: 5800,
        region: "North America",
      },
      {
        quarter: "2024-Q4",
        revenue: 195000,
        cost: 112000,
        profit: 83000,
        units_sold: 7200,
        region: "North America",
      },
      {
        quarter: "2025-Q1",
        revenue: 178000,
        cost: 104000,
        profit: 74000,
        units_sold: 6400,
        region: "North America",
      },
      {
        quarter: "2025-Q2",
        revenue: 215000,
        cost: 121000,
        profit: 94000,
        units_sold: 7900,
        region: "North America",
      },
      {
        quarter: "2024-Q1",
        revenue: 95000,
        cost: 68000,
        profit: 27000,
        units_sold: 3100,
        region: "Europe",
      },
      {
        quarter: "2024-Q2",
        revenue: 112000,
        cost: 74000,
        profit: 38000,
        units_sold: 3900,
        region: "Europe",
      },
      {
        quarter: "2024-Q3",
        revenue: 128000,
        cost: 81000,
        profit: 47000,
        units_sold: 4400,
        region: "Europe",
      },
      {
        quarter: "2024-Q4",
        revenue: 154000,
        cost: 93000,
        profit: 61000,
        units_sold: 5300,
        region: "Europe",
      },
      {
        quarter: "2025-Q1",
        revenue: 142000,
        cost: 88000,
        profit: 54000,
        units_sold: 4900,
        region: "Europe",
      },
      {
        quarter: "2025-Q2",
        revenue: 169000,
        cost: 99000,
        profit: 70000,
        units_sold: 5900,
        region: "Europe",
      },
      {
        quarter: "2024-Q1",
        revenue: 78000,
        cost: 52000,
        profit: 26000,
        units_sold: 2800,
        region: "Asia Pacific",
      },
      {
        quarter: "2024-Q2",
        revenue: 94000,
        cost: 59000,
        profit: 35000,
        units_sold: 3400,
        region: "Asia Pacific",
      },
      {
        quarter: "2024-Q3",
        revenue: 116000,
        cost: 68000,
        profit: 48000,
        units_sold: 4200,
        region: "Asia Pacific",
      },
      {
        quarter: "2024-Q4",
        revenue: 139000,
        cost: 79000,
        profit: 60000,
        units_sold: 5100,
        region: "Asia Pacific",
      },
      {
        quarter: "2025-Q1",
        revenue: 131000,
        cost: 75000,
        profit: 56000,
        units_sold: 4800,
        region: "Asia Pacific",
      },
      {
        quarter: "2025-Q2",
        revenue: 158000,
        cost: 86000,
        profit: 72000,
        units_sold: 5700,
        region: "Asia Pacific",
      },
    ],
  },
  2: {
    id: 2,
    table_name: "user_growth_metrics",
    schema: "public",
    database: { id: 1, database_name: "PostgreSQL - Analytics" },
    columns: [
      { column_name: "date", type: "DATE", is_dttm: true, filterable: true, groupby: true },
      {
        column_name: "visitors",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      { column_name: "signups", type: "BIGINT", is_dttm: false, filterable: false, groupby: false },
      {
        column_name: "active_users",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      { column_name: "country", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
    ],
    metrics: [
      { metric_name: "total_signups", expression: "SUM(signups)", verbose_name: "Total Signups" },
    ],
    data: [
      {
        date: "2025-01-01",
        visitors: 14200,
        signups: 820,
        active_users: 9400,
        country: "United States",
      },
      {
        date: "2025-01-02",
        visitors: 15100,
        signups: 910,
        active_users: 9900,
        country: "United States",
      },
      {
        date: "2025-01-03",
        visitors: 16800,
        signups: 1040,
        active_users: 10800,
        country: "United States",
      },
      {
        date: "2025-01-04",
        visitors: 18400,
        signups: 1190,
        active_users: 11700,
        country: "United States",
      },
      {
        date: "2025-01-05",
        visitors: 17900,
        signups: 1120,
        active_users: 11400,
        country: "United States",
      },
      {
        date: "2025-01-06",
        visitors: 20200,
        signups: 1350,
        active_users: 12900,
        country: "United States",
      },
      {
        date: "2025-01-07",
        visitors: 22100,
        signups: 1490,
        active_users: 13800,
        country: "United States",
      },
      { date: "2025-01-01", visitors: 9400, signups: 530, active_users: 6200, country: "Germany" },
      { date: "2025-01-02", visitors: 10200, signups: 610, active_users: 6700, country: "Germany" },
      { date: "2025-01-03", visitors: 11400, signups: 720, active_users: 7400, country: "Germany" },
      { date: "2025-01-04", visitors: 12600, signups: 810, active_users: 8100, country: "Germany" },
      { date: "2025-01-05", visitors: 12100, signups: 760, active_users: 7900, country: "Germany" },
      { date: "2025-01-06", visitors: 13800, signups: 920, active_users: 8900, country: "Germany" },
      {
        date: "2025-01-07",
        visitors: 14900,
        signups: 1010,
        active_users: 9600,
        country: "Germany",
      },
    ],
  },
  3: {
    id: 3,
    table_name: "product_categories",
    schema: "public",
    database: { id: 1, database_name: "PostgreSQL - Analytics" },
    columns: [
      { column_name: "category", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
      { column_name: "sales", type: "DECIMAL", is_dttm: false, filterable: false, groupby: false },
      {
        column_name: "market_share",
        type: "FLOAT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      {
        column_name: "growth_rate",
        type: "FLOAT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
    ],
    metrics: [{ metric_name: "sum_sales", expression: "SUM(sales)", verbose_name: "Sum of Sales" }],
    data: [
      { category: "Enterprise Cloud", sales: 485000, market_share: 38.5, growth_rate: 24.2 },
      { category: "Developer Tools", sales: 290000, market_share: 23.0, growth_rate: 18.7 },
      { category: "Security & Auth", sales: 215000, market_share: 17.1, growth_rate: 31.4 },
      { category: "Data & Analytics", sales: 180000, market_share: 14.3, growth_rate: 28.9 },
      { category: "Professional Services", sales: 89000, market_share: 7.1, growth_rate: 9.5 },
    ],
  },
  4: {
    id: 4,
    table_name: "server_health_metrics",
    schema: "telemetry",
    database: { id: 2, database_name: "ClickHouse - Logs" },
    columns: [
      {
        column_name: "server_name",
        type: "VARCHAR",
        is_dttm: false,
        filterable: true,
        groupby: true,
      },
      {
        column_name: "cpu_usage",
        type: "FLOAT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      {
        column_name: "memory_usage",
        type: "FLOAT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      {
        column_name: "network_mbps",
        type: "FLOAT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      { column_name: "status", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
    ],
    metrics: [
      { metric_name: "avg_cpu", expression: "AVG(cpu_usage)", verbose_name: "Average CPU" },
    ],
    data: [
      {
        server_name: "node-us-east-1",
        cpu_usage: 42.5,
        memory_usage: 68.2,
        network_mbps: 840,
        status: "Healthy",
      },
      {
        server_name: "node-us-east-2",
        cpu_usage: 55.1,
        memory_usage: 74.0,
        network_mbps: 920,
        status: "Healthy",
      },
      {
        server_name: "node-eu-west-1",
        cpu_usage: 88.4,
        memory_usage: 92.6,
        network_mbps: 1450,
        status: "Warning",
      },
      {
        server_name: "node-eu-central-1",
        cpu_usage: 34.2,
        memory_usage: 51.5,
        network_mbps: 620,
        status: "Healthy",
      },
      {
        server_name: "node-ap-south-1",
        cpu_usage: 61.8,
        memory_usage: 79.3,
        network_mbps: 1100,
        status: "Healthy",
      },
      {
        server_name: "node-ap-east-1",
        cpu_usage: 91.0,
        memory_usage: 94.2,
        network_mbps: 1680,
        status: "Critical",
      },
    ],
  },
  5: {
    id: 5,
    table_name: "audience_members_daily",
    schema: "analytics",
    database: { id: 1, database_name: "PostgreSQL - Analytics" },
    columns: [
      { column_name: "date", type: "DATE", is_dttm: true, filterable: true, groupby: true },
      {
        column_name: "audience_size",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      {
        column_name: "treatment_size",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
      {
        column_name: "holdout_size",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
    ],
    metrics: [
      {
        metric_name: "max_treatment",
        expression: "MAX(treatment_size)",
        verbose_name: "Max Treatment Size",
      },
    ],
    data: [
      { date: "Jul 25", audience_size: 333, treatment_size: 120, holdout_size: 80 },
      { date: "Jul 30", audience_size: 333, treatment_size: 150, holdout_size: 90 },
      { date: "Aug 4", audience_size: 333, treatment_size: 210, holdout_size: 110 },
      { date: "Aug 9", audience_size: 333, treatment_size: 280, holdout_size: 130 },
      { date: "Aug 15", audience_size: 333, treatment_size: 450, holdout_size: 220 },
      { date: "Aug 18", audience_size: 333, treatment_size: 1200, holdout_size: 850 },
      { date: "Aug 20", audience_size: 333, treatment_size: 24500, holdout_size: 6100 },
      { date: "Aug 21", audience_size: 333, treatment_size: 53760, holdout_size: 13350 },
    ],
  },
  6: {
    id: 6,
    table_name: "audience_user_activity",
    schema: "analytics",
    database: { id: 1, database_name: "PostgreSQL - Analytics" },
    columns: [
      { column_name: "cohort", type: "VARCHAR", is_dttm: false, filterable: true, groupby: true },
      {
        column_name: "event_name",
        type: "VARCHAR",
        is_dttm: false,
        filterable: true,
        groupby: true,
      },
      {
        column_name: "event_count",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
    ],
    metrics: [
      { metric_name: "sum_events", expression: "SUM(event_count)", verbose_name: "Total Events" },
    ],
    data: [
      { cohort: "Holdout group", event_name: "Purchase Event", event_count: 2983 },
      { cohort: "Holdout group", event_name: "Purchase History", event_count: 2983 },
      { cohort: "Treatment", event_name: "Purchase History", event_count: 12450 },
      { cohort: "Treatment", event_name: "Purchase Event", event_count: 12890 },
    ],
  },
  7: {
    id: 7,
    table_name: "sync_size_history",
    schema: "telemetry",
    database: { id: 2, database_name: "ClickHouse - Logs" },
    columns: [
      { column_name: "date", type: "DATE", is_dttm: true, filterable: true, groupby: true },
      {
        column_name: "destination",
        type: "VARCHAR",
        is_dttm: false,
        filterable: true,
        groupby: true,
      },
      {
        column_name: "sync_size",
        type: "BIGINT",
        is_dttm: false,
        filterable: false,
        groupby: false,
      },
    ],
    metrics: [
      {
        metric_name: "total_sync_size",
        expression: "SUM(sync_size)",
        verbose_name: "Total Synced Rows",
      },
    ],
    data: [
      { date: "Aug 21", destination: "Google Sheets", sync_size: 0 },
      { date: "Aug 23", destination: "Google Sheets", sync_size: 0 },
      { date: "Aug 25", destination: "Google Sheets", sync_size: 210 },
      { date: "Aug 27", destination: "Google Sheets", sync_size: 52400 },
      { date: "Aug 21", destination: "Impact.com", sync_size: 0 },
      { date: "Aug 23", destination: "Impact.com", sync_size: 0 },
      { date: "Aug 25", destination: "Impact.com", sync_size: 150 },
      { date: "Aug 27", destination: "Impact.com", sync_size: 1350 },
    ],
  },
};

/**
 * Creates and starts the Mock BI HTTP server
 */
export function startMockBiServer(port = 8088): Promise<http.Server> {
  return new Promise((resolve) => {
    const server = http.createServer(async (req, res) => {
      // CORS headers
      res.setHeader("Access-Control-Allow-Origin", "*");
      res.setHeader("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS");
      res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization, X-CSRFToken");

      if (req.method === "OPTIONS") {
        res.writeHead(204);
        res.end();
        return;
      }

      const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);
      const path = url.pathname;

      // 1. Auth: /api/v1/security/login
      if (path === "/api/v1/security/login" && req.method === "POST") {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(
          JSON.stringify({
            access_token: "mock-jwt-access-token-cordis-bi-demo",
            refresh_token: "mock-jwt-refresh-token",
          }),
        );
        return;
      }

      // 2. CSRF: /api/v1/security/csrf_token/
      if (path === "/api/v1/security/csrf_token/" && req.method === "GET") {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(
          JSON.stringify({
            result: "mock-csrf-token-xyz123",
          }),
        );
        return;
      }

      // 3. Guest Token: /api/v1/security/guest_token/
      if (path === "/api/v1/security/guest_token/" && req.method === "POST") {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(
          JSON.stringify({
            token: "mock-guest-token-abc987",
          }),
        );
        return;
      }

      // 4. List Datasets: /api/v1/dataset/
      if (path === "/api/v1/dataset/" && req.method === "GET") {
        const list = Object.values(MOCK_DATASETS).map((d) => ({
          id: d.id,
          table_name: d.table_name,
          schema: d.schema,
          database: d.database,
          columns: d.columns,
          metrics: d.metrics,
        }));
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ result: list }));
        return;
      }

      // 5. Describe Dataset: /api/v1/dataset/:id
      const datasetMatch = path.match(/^\/api\/v1\/dataset\/(\d+)$/);
      if (datasetMatch && req.method === "GET") {
        const id = parseInt(datasetMatch[1], 10);
        const ds = MOCK_DATASETS[id];
        if (!ds) {
          res.writeHead(404, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ message: "Dataset not found" }));
          return;
        }
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ result: ds }));
        return;
      }

      // 6. Chart Data Query: /api/v1/chart/data
      if (path === "/api/v1/chart/data" && req.method === "POST") {
        let body = "";
        req.on("data", (chunk) => {
          body += chunk;
        });
        req.on("end", () => {
          try {
            const queryContext = JSON.parse(body || "{}");
            const datasourceId = queryContext.datasource?.id || 1;
            const ds = MOCK_DATASETS[datasourceId] || MOCK_DATASETS[1];
            const query = queryContext.queries?.[0] || {};
            let rows = [...ds.data];

            // Apply filters if any
            if (Array.isArray(query.filters)) {
              for (const f of query.filters) {
                if (f && f.val !== undefined && f.val !== null && f.val !== "") {
                  rows = rows.filter((r) => String(r[f.col]) === String(f.val));
                }
              }
            }

            // Apply column projection if requested
            if (Array.isArray(query.columns) && query.columns.length > 0) {
              const cols = new Set(query.columns);
              rows = rows.map((r) => {
                const projected: Record<string, unknown> = {};
                for (const k of Object.keys(r)) {
                  if (cols.has(k)) projected[k] = r[k];
                }
                return projected;
              });
            }

            // Apply limit
            const limit = query.row_limit || 1000;
            rows = rows.slice(0, limit);

            res.writeHead(200, { "Content-Type": "application/json" });
            res.end(
              JSON.stringify({
                result: [
                  {
                    data: rows,
                    colnames: ds.columns.map((c) => c.column_name),
                    coltypes: ds.columns.map(() => 1),
                    rowcount: rows.length,
                  },
                ],
              }),
            );
          } catch (e: any) {
            res.writeHead(400, { "Content-Type": "application/json" });
            res.end(JSON.stringify({ message: "Invalid query payload", error: e.message }));
          }
        });
        return;
      }

      // Default 404
      res.writeHead(404, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ message: "Not found", path }));
    });

    server.listen(port, () => {
      process.stdout.write(`[Mock BI Server] listening on http://localhost:${port}\n`);
      resolve(server);
    });
  });
}
