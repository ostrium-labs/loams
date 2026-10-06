/**
 * A wired server with auth on it.
 *
 * Builds the same object graph the real server does -- router, bus, registry, host, auth
 * -- behind one `http` server, and returns a `login()` that walks a browser through the
 * code flow against the fake IdP. Manual redirects and a hand-rolled cookie jar, because
 * `fetch` will not follow a cross-origin redirect chain without an agent and the whole
 * point is to observe what the BROWSER would be handed.
 */

import http from "node:http";
import { Context, Service } from "cordis";
import { HttpRouter } from "../src/router.js";
import { AgentBus } from "../src/bus.js";
import { PluginRegistry } from "../src/registry.js";
import { PluginHost } from "../src/host.js";
import { AuthService } from "../src/auth/service.js";
import { mountAuthApi } from "../src/auth/routes.js";
import { mountCoreApi } from "../src/api.js";
import { MemoryAuthStore } from "../src/auth/session.js";
import type { PluginManifest, PluginStatus } from "../src/types.js";
import { CLIENT_ID, CLIENT_SECRET, type FakeIdP } from "./fake-idp.js";

export interface AuthAppOptions {
  idp: FakeIdP;
  /** Overrides merged over the derived defaults. */
  env?: Record<string, string | undefined>;
  /** Omit AuthService entirely, to prove the platform still mounts without it. */
  withoutAuth?: boolean;
  /** Mount /api/plugins too. */
  withCoreApi?: boolean;
}

export interface AuthApp {
  ctx: Context;
  router: HttpRouter;
  registry: PluginRegistry;
  host: PluginHost;
  auth?: AuthService;
  baseUrl: string;
  store: MemoryAuthStore;
  /** The env actually used, so a test can assert what the config resolved to. */
  env: Record<string, string | undefined>;
  close(): Promise<void>;
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** Walk the whole code flow, leaving `jar` holding the session cookie. */
  login(options?: { returnTo?: string }): Promise<LoginOutcome>;
  /** Register a plugin with a loader that exposes one route and one skill. */
  registerPlugin(manifest: PluginManifest): Promise<PluginStatus>;
}

export interface LoginOutcome {
  /** Responses along the way, in order: login 302, authorize 302, callback. */
  loginResponse: Response;
  authorizeResponse: Response;
  callbackResponse: Response;
  /** The callback body, when it was a JSON error. */
  callbackBody: any;
  setCookie?: string;
}

export class CookieJar {
  private readonly _cookies = new Map<string, string>();

  absorb(response: Response): void {
    for (const raw of response.headers.getSetCookie?.() ?? []) {
      const [pair] = raw.split(";");
      const index = pair?.indexOf("=") ?? -1;
      if (!pair || index < 0) continue;
      const name = pair.slice(0, index).trim();
      const value = pair.slice(index + 1).trim();
      if (value === "") this._cookies.delete(name);
      else this._cookies.set(name, value);
    }
  }

  header(): string | undefined {
    if (this._cookies.size === 0) return undefined;
    return [...this._cookies].map(([name, value]) => `${name}=${value}`).join("; ");
  }

  clear(): void {
    this._cookies.clear();
  }

  get(name: string): string | undefined {
    return this._cookies.get(name);
  }

  names(): string[] {
    return [...this._cookies.keys()];
  }
}

export async function createAuthApp(options: AuthAppOptions): Promise<AuthApp> {
  const ctx = new Context();
  const store = new MemoryAuthStore();
  // The registry reads `ctx.store` for persisted enable flags; auth reads it for sessions.
  ctx.provide("store", store as never);

  const router = new HttpRouter(ctx);
  const bus = new AgentBus(ctx);
  const registry = new PluginRegistry(ctx);
  const host = new PluginHost(ctx);

  const env: Record<string, string | undefined> = {
    OIDC_ISSUER: options.idp.issuer,
    OIDC_CLIENT_ID: CLIENT_ID,
    OIDC_CLIENT_SECRET: CLIENT_SECRET,
    OIDC_SCOPE: "openid profile email offline_access bi:admin",
    // The fake IdP is plain http on loopback, which is exactly the local-dev case
    // OIDC_INSECURE exists for.
    OIDC_INSECURE: "true",
    NODE_ENV: "test",
    SESSION_COOKIE_SECURE: "false",
    ...options.env,
  };

  const server = http.createServer(async (req, res) => {
    if (await router.handle(req, res)) return;
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ message: "Not Found", path: req.url }));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const baseUrl = `http://127.0.0.1:${(server.address() as { port: number }).port}`;

  // The redirect URI can only be known once the server has a port.
  env.OIDC_REDIRECT_URI = `${baseUrl}/api/auth/callback`;

  // Constructed directly rather than via ctx.plugin() so the test controls init order:
  // auth before the registry and host, which both read it.
  let auth: AuthService | undefined;
  if (!options.withoutAuth) {
    auth = new AuthService(ctx, { env, store });
    await (auth as any)[Service.init]();
  }
  await (registry as any)[Service.init]();
  await (host as any)[Service.init]();

  mountAuthApi(ctx, { defaultReturnTo: "/" });
  if (options.withCoreApi !== false) {
    mountCoreApi(ctx, { baseUrl: () => baseUrl });
  }

  const jar = new CookieJar();

  async function fetchWithJar(path: string, init: RequestInit = {}): Promise<Response> {
    const headers = new Headers(init.headers);
    const cookie = jar.header();
    if (cookie) headers.set("cookie", cookie);
    const response = await fetch(path.startsWith("http") ? path : `${baseUrl}${path}`, {
      ...init,
      headers,
      redirect: "manual",
    });
    jar.absorb(response);
    return response;
  }

  return {
    ctx,
    router,
    registry,
    host,
    auth,
    baseUrl,
    store,
    env,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
    fetch: fetchWithJar,
    async login(loginOptions = {}) {
      const suffix = loginOptions.returnTo
        ? `?return_to=${encodeURIComponent(loginOptions.returnTo)}`
        : "";
      const loginResponse = await fetchWithJar(`/api/auth/login${suffix}`);
      const authorizeUrl = loginResponse.headers.get("location");
      if (!authorizeUrl) {
        const callbackBody = await loginResponse.json().catch(() => undefined);
        return {
          loginResponse,
          authorizeResponse: undefined as any,
          callbackResponse: undefined as any,
          callbackBody,
        };
      }
      const authorizeResponse = await fetchWithJar(authorizeUrl);
      const callbackUrl = authorizeResponse.headers.get("location");
      if (!callbackUrl) {
        const callbackBody = await authorizeResponse.json().catch(() => undefined);
        return {
          loginResponse,
          authorizeResponse,
          callbackResponse: authorizeResponse,
          callbackBody,
        };
      }
      const callbackResponse = await fetchWithJar(callbackUrl);
      const setCookie = callbackResponse.headers.getSetCookie?.()[0];
      const callbackBody = callbackResponse.headers
        .get("content-type")
        ?.includes("application/json")
        ? await callbackResponse.json().catch(() => undefined)
        : undefined;
      return { loginResponse, authorizeResponse, callbackResponse, callbackBody, setCookie };
    },
    async registerPlugin(pluginManifest: PluginManifest) {
      return host.register(pluginManifest, {
        routes: (runtime) => [
          {
            name: `${pluginManifest.id}:ping`,
            method: "GET",
            match: `/api/${pluginManifest.id}/ping`,
            handler: ({ sendJson }) => {
              sendJson(200, { ok: true, plugin: runtime.id });
              return true;
            },
          },
        ],
        // One handler per DECLARED skill. A test that declares `listChannels` but wires
        // only a `ping` handler gets an agent whose every call times out, which looks
        // like an A2A failure rather than a wiring mistake.
        skills: () =>
          (pluginManifest.agent?.skills ?? [{ id: "ping", name: "ping", description: "ping" }]).map(
            (skill) => ({
              id: skill.id,
              description: skill.description,
              handle: (params: Record<string, unknown>) => ({
                ok: true,
                skill: skill.id,
                params,
              }),
            }),
          ),
      });
    },
  };
}

export { CLIENT_ID, CLIENT_SECRET };

/**
 * Capture everything the service logs.
 *
 * Goes through cordis's own exporter rather than monkey-patching `ctx.logger.warn`: the
 * logger is a proxy whose methods are not replaceable, and an exporter sees the FINAL
 * formatted record, which is what a log-scraping operator would actually see.
 */
export interface CapturedLog {
  type: string;
  text: string;
}

export async function captureLogs(app: AuthApp): Promise<CapturedLog[]> {
  const captured: CapturedLog[] = [];
  // Awaited: `exporter()` returns a Disposable<Promise<void>> and the sink is installed
  // asynchronously, so a test that reads `captured` immediately can observe nothing.
  await app.ctx.logger.exporter({
    colors: false,
    export(message) {
      captured.push({
        type: String(message.type),
        text: message.args
          .map((arg) => (typeof arg === "string" ? arg : JSON.stringify(arg)))
          .join(" "),
      });
    },
  });
  return captured;
}

/** Let cordis's buffered log records reach the installed exporters. */
export async function flushLogs(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** All captured lines joined, for `toContain` assertions. */
export function logText(logs: CapturedLog[]): string {
  return logs.map((entry) => entry.text).join("\n");
}
