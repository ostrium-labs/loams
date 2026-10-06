/**
 * Test harness for the core platform.
 *
 * The host's central promise — a disabled plugin's route really 404s — is an
 * HTTP-level claim, so the tests make real requests against a real `http`
 * server wired to a real `HttpRouter`. A unit test that called the handler
 * directly would pass even if the router stopped consulting its table, which is
 * exactly the bug this suite exists to catch.
 */

import http from "node:http";
import { Context, Service } from "cordis";
import { HttpRouter } from "../src/router.js";
import { PluginRegistry } from "../src/registry.js";
import { AgentBus } from "../src/bus.js";
import { PluginHost } from "../src/host.js";
import type { PluginManifest, PluginStatus } from "../src/types.js";

/** An in-memory stand-in for the two plugin-state methods of `StoreService`. */
export class FakePluginStore {
  readonly state = new Map<string, boolean>();

  async getPluginState(id: string): Promise<boolean | undefined> {
    return this.state.get(id);
  }

  async setPluginState(id: string, enabled: boolean): Promise<void> {
    this.state.set(id, enabled);
  }
}

export interface Harness {
  ctx: Context;
  router: HttpRouter;
  registry: PluginRegistry;
  bus: AgentBus;
  host: PluginHost;
  store: FakePluginStore;
  request(method: string, path: string, body?: unknown): Promise<{ status: number; body: any }>;
  close(): Promise<void>;
  baseUrl: string;
}

export interface HarnessOptions {
  store?: FakePluginStore;
  busConfig?: ConstructorParameters<typeof AgentBus>[1];
  hostConfig?: ConstructorParameters<typeof PluginHost>[1];
}

/**
 * Build a fully wired context, starting an HTTP server that dispatches through
 * the router and 404s anything the router declines.
 */
export async function createHarness(options: HarnessOptions = {}): Promise<Harness> {
  const ctx = new Context();
  const store = options.store ?? new FakePluginStore();
  ctx.provide("store", store);

  // A cordis service constructed against a context registers its own key, so
  // only the store — which is a plain stub here — needs `provide`.
  const router = new HttpRouter(ctx);
  const bus = new AgentBus(ctx, options.busConfig);
  const registry = new PluginRegistry(ctx);
  const host = new PluginHost(ctx, options.hostConfig);

  const server = http.createServer(async (req, res) => {
    if (await router.handle(req, res)) return;
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ message: "Not Found", path: req.url }));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as { port: number };
  const baseUrl = `http://127.0.0.1:${address.port}`;

  return {
    ctx,
    router,
    registry,
    bus,
    host,
    store,
    baseUrl,
    async request(method, path, body) {
      const response = await fetch(`${baseUrl}${path}`, {
        method,
        headers: body === undefined ? {} : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await response.text();
      let parsed: any = text;
      try {
        parsed = text ? JSON.parse(text) : null;
      } catch {
        /* keep the raw text */
      }
      return { status: response.status, body: parsed };
    },
    async close() {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

/** Run a registry service's `[Service.init]` body explicitly. */
export async function initService(service: object): Promise<void> {
  await (service as any)[Service.init]();
}

export function manifest(overrides: Partial<PluginManifest> & { id: string }): PluginManifest {
  return {
    name: overrides.id,
    description: `${overrides.id} plugin`,
    version: "1.0.0",
    uiPath: `/plugins/${overrides.id}`,
    ...overrides,
  } as PluginManifest;
}

export function statusOf(list: PluginStatus[], id: string): PluginStatus {
  const found = list.find((entry) => entry.id === id);
  if (!found) throw new Error(`no such plugin in list: ${id}`);
  return found;
}
