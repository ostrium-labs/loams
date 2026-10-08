import { describe, it, expect } from "vite-plus/test";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { createConnectRouter, createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import {
  registerRpcServices,
  DataService,
  ThemeService,
  type RpcContext,
} from "@loams-core/bi-rpc";
import { createNodeConnectBridge } from "../src/connect-bridge.js";

/**
 * The unit tests in `@loams-core/bi-rpc` prove the service implementations. These prove
 * the WIRING: that a real Connect client talking to a real Node http server,
 * through the hand-written bridge, gets a real answer.
 *
 * That wiring is the part most likely to break silently — a wrong dispatch path
 * or a dropped query string produces a 404 that a mock-level test never sees.
 */

const noop = () => {};

function ctx(): RpcContext {
  return {
    controlPlane: {
      listDatasets: async () => ({ result: [{ id: 1, table_name: "sales" }] }),
      describeDataset: async () => ({ id: 7, table_name: "sales", columns: [] }),
      queryData: async () => ({ data: [{ region: "emea", revenue: 1200.5, active: true }] }),
    },
    dashboard: {
      listDashboards: async () => [],
      load: async () => ({ id: "d1" }),
      create: async (title: string) => ({ id: "d1", title }),
      patch: async () => ({ id: "d1" }),
    },
    data: { fetchWidgetData: async () => ({ data: [] }) },
    render: { compileWidget: async () => ({ series: [] }) },
    flint: {
      listThemes: () => [
        { id: "economist", label: "Economist", description: "Print-first", icon: "<svg/>" },
      ],
      resolveTheme: (theme) =>
        theme === "economist"
          ? { valid: true, spec: { ink: {} }, report: [] }
          : {
              valid: false,
              report: [{ stage: "ground", path: "theme.preset", message: "unknown" }],
            },
    },
    logger: { info: noop, warn: noop, error: noop },
  };
}

async function withServer(run: (baseUrl: string) => Promise<void>): Promise<void> {
  const bridge = createNodeConnectBridge(registerRpcServices(createConnectRouter(), ctx()));
  const server = http.createServer(async (req, res) => {
    res.setHeader("Access-Control-Allow-Origin", "*");
    if (await bridge(req, res)) return;
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ message: "Not Found" }));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  try {
    await run(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("ConnectRPC over HTTP", () => {
  it("serves ListDatasets end to end", async () => {
    await withServer(async (baseUrl) => {
      const client = createClient(DataService, createGrpcWebTransport({ baseUrl }));
      const res = await client.listDatasets({});
      expect(res.count).toBe(1);
      expect(Number(res.datasets[0]!.id)).toBe(1);
      expect(res.datasets[0]!.tableName).toBe("sales");
    });
  });

  it("serves Query end to end with typed column values", async () => {
    await withServer(async (baseUrl) => {
      const client = createClient(DataService, createGrpcWebTransport({ baseUrl }));
      const res = await client.query({ datasetId: 1n, columns: ["region", "revenue", "active"] });
      expect(res.rows).toHaveLength(1);
      expect(res.rows[0]!.values.region).toBeDefined();
    });
  });

  it("serves ThemeService.ListThemes end to end", async () => {
    await withServer(async (baseUrl) => {
      const client = createClient(ThemeService, createGrpcWebTransport({ baseUrl }));
      const res = await client.listThemes({});
      expect(res.themes).toHaveLength(1);
      expect(res.themes[0]!.id).toBe("economist");
      expect(res.themes[0]!.icon).toBe("<svg/>");
    });
  });

  it("serves an invalid theme as valid:false with a report, not an error", async () => {
    await withServer(async (baseUrl) => {
      const client = createClient(ThemeService, createGrpcWebTransport({ baseUrl }));
      const res = await client.getTheme({ id: "nope" });
      expect(res.valid).toBe(false);
      expect(res.report).toHaveLength(1);
      expect(res.report[0]!.path).toBe("theme.preset");
    });
  });

  it("does not claim an unknown path", async () => {
    await withServer(async (baseUrl) => {
      // An unregistered RPC path must fall through, not be swallowed by the bridge.
      const res = await fetch(`${baseUrl}/bi.v1.NopeService/Nope`, { method: "POST" });
      expect(res.status).toBe(404);
    });
  });

  it("rejects a wrong HTTP method on a known path", async () => {
    await withServer(async (baseUrl) => {
      const res = await fetch(`${baseUrl}/bi.v1.DataService/ListDatasets`, { method: "GET" });
      // Not 404: the bridge recognized the path but the method is not allowed.
      expect(res.status).not.toBe(404);
    });
  });
});
