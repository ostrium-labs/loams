/**
 * The REST surface of the plugin platform.
 *
 *   GET  /api/plugins              -> { plugins: PluginStatus[] }
 *   GET  /api/plugins/:id          -> PluginStatus
 *   POST /api/plugins/:id/enable   -> PluginStatus
 *   POST /api/plugins/:id/disable  -> PluginStatus
 *   GET  /api/plugins/:id/agent    -> that plugin's A2A AgentCard
 *
 * The enable/disable handlers call the HOST, not the registry. A toggle that
 * only flipped a boolean would report success for a plugin that never loaded,
 * which is the single most misleading thing this API could do.
 */

import type { Context } from "cordis";
import type { HttpRouter, RouteRequest } from "./router.js";
import type { PluginRegistry } from "./registry.js";
import type { PluginHost } from "./host.js";
import type { A2ADeps } from "./a2a.js";
import { A2AError, buildAgentCard, mountA2A } from "./a2a.js";
import type { PluginStatus } from "./types.js";
import type { AuthService, RequestLike } from "./auth/service.js";
import type { AuthPrincipal } from "./auth/scopes.js";
import { ForbiddenError } from "./auth/errors.js";

const ID_PATTERN = "([a-zA-Z0-9._-]+)";

/** The id segment of a `/api/plugins/...` path. */
function pluginIdFrom(path: string, segments: number): string | null {
  const parts = path.split("/").filter(Boolean);
  if (parts.length !== segments) return null;
  return decodeURIComponent(parts[2] ?? "");
}

/**
 * Map a host-level failure to a status code.
 *
 * Unknown plugin is 404; refusing to disable an always-on plugin is 409,
 * because the request was well-formed and the conflict is with current state.
 */
function statusForError(err: unknown): number {
  if (err instanceof A2AError) return err.httpStatus;
  // AuthError and ForbiddenError carry an explicit 401/403. Reading it here keeps an
  // insufficient-scope refusal from being reported as a server error.
  const authStatus = (err as { httpStatus?: unknown } | null)?.httpStatus;
  if (typeof authStatus === "number" && authStatus >= 400 && authStatus <= 599) return authStatus;
  const message = err instanceof Error ? err.message : String(err);
  if (message.startsWith("Unknown plugin")) return 404;
  if (message.includes("cannot be disabled")) return 409;
  return 500;
}

function errorBody(err: unknown): { error: string } {
  return { error: err instanceof Error ? err.message : String(err) };
}

/** Load a plugin through the host and report the real resulting status. */
export async function enablePlugin(host: PluginHost, id: string): Promise<PluginStatus> {
  return host.enable(id);
}

/** Unload a plugin through the host. */
export async function disablePlugin(host: PluginHost, id: string): Promise<PluginStatus> {
  return host.disable(id);
}

export interface CoreApiOptions {
  /** Base URL used in generated A2A cards. */
  baseUrl: () => string;
  /** Mount the A2A routes too. Defaults to true. */
  withA2A?: boolean;
}

/**
 * Mount `/api/plugins*` (and, unless told otherwise, the A2A surface) into a
 * router. Returns a disposer for all of them.
 */
export function mountCoreApi(ctx: Context, options: CoreApiOptions): () => void {
  const router: HttpRouter = ctx.router;
  const registry: PluginRegistry = ctx.coreRegistry;
  const host: PluginHost = ctx.coreHost;

  // Auth is optional at this layer so the platform still mounts without an IdP. When it
  // IS present, the toggle endpoints are admin-gated below.
  const auth = readAuth(ctx);

  /**
   * Require `ADMIN_SCOPE` for a privileged action.
   *
   * Enabling or disabling a plugin reconfigures what the whole system can do, so it is
   * gated on a SCOPE rather than on "is authenticated" -- otherwise every logged-in user
   * could turn off the adapters the deployment depends on. Returns the principal so the
   * caller can pass it on to `enable`, which needs it to evaluate `requiredScopes`.
   */
  const requireAdmin = async (request: RouteRequest): Promise<AuthPrincipal | undefined> => {
    if (!auth) return undefined;
    const adminScope = auth.adminScope;
    // No admin scope at all means auth is disabled. Refusing here is the concrete form of
    // "never silently enable admin powers without auth configured": the console cannot
    // reconfigure a dev-mode server that has no IdP.
    if (!adminScope) {
      throw new ForbiddenError(
        "Plugin enable/disable is unavailable: authentication is not configured, so no " +
          "principal can be proven to hold the admin scope.",
        { code: "admin_unavailable" },
      );
    }
    return auth.requireScope(request, adminScope);
  };

  /** The principal of this request, for the per-session catalog view. */
  const principalFor = async (request: RequestLike): Promise<AuthPrincipal | undefined> =>
    auth ? auth.principal(request) : undefined;

  const routes = router.addAll(
    [
      {
        name: "core:list-plugins",
        method: "GET",
        match: "/api/plugins",
        handler: async ({ req, sendJson }) => {
          // Recomputed per request: a plugin this session lacks the scopes for reads as
          // `blocked` with the missing scopes named, for this session only.
          sendJson(200, { plugins: registry.listFor(await principalFor({ req })) });
          return true;
        },
      },
      {
        name: "core:get-plugin",
        method: "GET",
        match: new RegExp(`^/api/plugins/${ID_PATTERN}$`),
        handler: async ({ req, sendJson, path }) => {
          const id = pluginIdFrom(path, 3);
          const status = id ? registry.findFor(id, await principalFor({ req })) : undefined;
          if (!status) {
            sendJson(404, { error: `Unknown plugin: ${id ?? path}` });
            return true;
          }
          sendJson(200, status);
          return true;
        },
      },
      {
        name: "core:enable-plugin",
        method: "POST",
        match: new RegExp(`^/api/plugins/${ID_PATTERN}/enable$`),
        handler: async (request) => {
          const { sendJson, path } = request;
          const id = pluginIdFrom(path, 4);
          if (!id || !registry.has(id)) {
            sendJson(404, { error: `Unknown plugin: ${id ?? path}` });
            return true;
          }
          try {
            const principal = await requireAdmin(request);
            sendJson(200, await host.enable(id, principal));
          } catch (err) {
            sendJson(statusForError(err), errorBody(err));
          }
          return true;
        },
      },
      {
        name: "core:disable-plugin",
        method: "POST",
        match: new RegExp(`^/api/plugins/${ID_PATTERN}/disable$`),
        handler: async (request) => {
          const { sendJson, path } = request;
          const id = pluginIdFrom(path, 4);
          if (!id || !registry.has(id)) {
            sendJson(404, { error: `Unknown plugin: ${id ?? path}` });
            return true;
          }
          try {
            await requireAdmin(request);
            sendJson(200, await disablePlugin(host, id));
          } catch (err) {
            // An always-on plugin refuses to unload. 409 says "conflicts with
            // current state", which is what it is; 500 would send the console
            // looking for a bug that does not exist.
            sendJson(statusForError(err), errorBody(err));
          }
          return true;
        },
      },
      {
        name: "core:plugin-agent-card",
        method: "GET",
        match: new RegExp(`^/api/plugins/${ID_PATTERN}/agent$`),
        handler: ({ sendJson, path }) => {
          const id = pluginIdFrom(path, 4);
          const status = id ? registry.find(id) : undefined;
          if (!status) {
            sendJson(404, { error: `Unknown plugin: ${id ?? path}` });
            return true;
          }
          if (!status.agent) {
            sendJson(404, { error: `Plugin "${id}" exposes no agent.` });
            return true;
          }
          sendJson(200, buildAgentCard(status, { baseUrl: options.baseUrl() }));
          return true;
        },
      },
    ],
    "core:api",
  );

  if (options.withA2A === false) return routes;

  // `auth` is passed through so `message:send` can authenticate its caller. Without it
  // the endpoint is anonymous, which is exactly the hole it exists to close.
  const a2aDeps: A2ADeps = {
    registry,
    bus: ctx.agentBus,
    ctx,
    baseUrl: options.baseUrl,
    auth,
  };
  const removeA2A = mountA2A(a2aDeps, router);
  return () => {
    removeA2A();
    routes();
  };
}

/**
 * The auth service, when one is loaded.
 *
 * Structural and defensive: the control plane mounts with or without an IdP, and the
 * `ctx.auth` key throws when it was never provided.
 */
function readAuth(ctx: Context): AuthService | undefined {
  try {
    const auth = (ctx as unknown as { auth?: AuthService }).auth;
    return auth && typeof auth.principal === "function" ? auth : undefined;
  } catch {
    return undefined;
  }
}
