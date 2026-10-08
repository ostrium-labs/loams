/**
 * A mutable HTTP route table, consulted once per request.
 *
 * WHY THIS EXISTS
 * ---------------
 * The server used to answer `/api/*` with a chain of `if (path === ...)`
 * checks hardcoded into `startHttpApiServer`. That is fine for routes that
 * exist at boot and fatal for a plugin platform: "turn this plugin off" has to
 * be observable as its routes vanishing, which a chain of `if`s cannot do,
 * because there is no way to remove a branch at runtime.
 *
 * So the chain became a list. Routes are consulted in registration order and
 * the first handler that claims the request wins; removing an entry makes the
 * request fall through to whatever is registered next, and ultimately to the
 * caller's 404. That fall-through is the whole point: it is what makes an
 * unloaded plugin's route a 404 rather than a stale 200.
 *
 * ORDERING
 * The ConnectRPC bridge still runs before this table (see apps/server), and the
 * 13 original REST routes are registered first, so neither the bridge nor the
 * static surface can be shadowed by a plugin route registered later.
 */

import type { IncomingMessage, ServerResponse } from "node:http";
import { Context, Service } from "cordis";

/** Everything a handler needs to read the request and answer it. */
export interface RouteRequest {
  method: string;
  /** `url.pathname` — already decoded for params by the handler. */
  path: string;
  url: URL;
  req: IncomingMessage;
  res: ServerResponse;
  /** Write a JSON response. Safe to call at most once. */
  sendJson: (status: number, data: unknown) => void;
  /** Read and parse a JSON body. Unparseable bodies resolve to `{}`. */
  readJson: <T = any>() => Promise<T>;
}

/**
 * A handler claims the request by returning `true` (or any truthy value);
 * returning falsy lets the next route try.
 */
export type RouteHandler = (request: RouteRequest) => unknown;

export interface RouteSpec {
  /** HTTP method(s). Defaults to "*" (any). */
  method?: string | string[];
  /** Exact pathname, or a regular expression tested against the pathname. */
  match: string | RegExp;
  handler: RouteHandler;
  /**
   * Per-request authorization check, run before `handler`.
   *
   * This is the hook that makes `PluginManifest.requiredScopes` enforceable AT REQUEST
   * TIME rather than only when a plugin was enabled. A guard runs on every request, so
   * the decision is made against the principal that is actually presenting credentials
   * now -- which is the only thing that can be correct, because a session minted before a
   * plugin was deployed cannot have been granted that plugin's scopes.
   *
   * Return `false` to decline (the route falls through to the next one). Throw to answer
   * with a specific status: the router honours an `httpStatus` property on the thrown
   * value, so a guard can refuse with 401/403 instead of the generic 500.
   */
  guard?: (request: RouteRequest) => unknown | Promise<unknown>;
  /** Free-form label, used in logs and in tests. */
  name?: string;
}

interface RouteEntry {
  id: number;
  owner?: string;
  spec: RouteSpec;
}

export interface HttpRouterConfig {
  /** Log a one-line note when a route is added/removed. Defaults to false. */
  verbose?: boolean;
}

declare module "cordis" {
  interface Context {
    router: HttpRouter;
  }
}

export class HttpRouter extends Service {
  static inject = [];

  private _routes: RouteEntry[] = [];
  private _counter = 0;
  public config: HttpRouterConfig;

  constructor(ctx: Context, config?: HttpRouterConfig) {
    super(ctx, "router");
    this.config = config ?? {};
  }

  /**
   * Register a route. The returned function removes it again, which is what
   * makes an idempotent add/remove pair leak nothing.
   */
  add(spec: RouteSpec, owner?: string): () => void {
    const entry: RouteEntry = { id: ++this._counter, owner, spec };
    this._routes.push(entry);
    if (this.config.verbose) {
      this.ctx.logger.debug(
        "router: +%s %s (%s)",
        Array.isArray(spec.method) ? spec.method.join(",") : (spec.method ?? "*"),
        spec.match.toString(),
        owner ?? spec.name ?? "anonymous",
      );
    }
    let removed = false;
    return () => {
      if (removed) return;
      removed = true;
      this.remove(entry.id);
    };
  }

  /** Register many routes at once, returning a single disposer for all of them. */
  addAll(specs: RouteSpec[], owner?: string): () => void {
    const removers = specs.map((spec) => this.add(spec, owner));
    return () => {
      for (const remove of removers) remove();
    };
  }

  /** Drop every route registered by `owner`. Returns how many were removed. */
  removeOwner(owner: string): number {
    const before = this._routes.length;
    this._routes = this._routes.filter((entry) => entry.owner !== owner);
    const removed = before - this._routes.length;
    if (removed > 0 && this.config.verbose) {
      this.ctx.logger.debug("router: -%s (%s route(s))", owner, removed);
    }
    return removed;
  }

  /** How many routes are currently registered. Used by tests to prove teardown. */
  get size(): number {
    return this._routes.length;
  }

  /** Labels of the currently registered routes, in dispatch order. */
  paths(): string[] {
    return this._routes.map((entry) => entry.spec.name ?? entry.spec.match.toString());
  }

  private remove(id: number): boolean {
    const index = this._routes.findIndex((entry) => entry.id === id);
    if (index < 0) return false;
    this._routes.splice(index, 1);
    return true;
  }

  /**
   * Offer the request to every registered route. Returns true when one of them
   * answered, false when the caller should fall through (normally to a 404).
   *
   * A handler that throws is reported as a 500 here rather than escaping into
   * the http server, because a plugin's bug must not be able to take down the
   * request pipeline for every other plugin.
   */
  async handle(req: IncomingMessage, res: ServerResponse): Promise<boolean> {
    const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);
    const path = url.pathname;
    const method = (req.method || "GET").toUpperCase();

    const sendJson = (status: number, data: unknown) => {
      if (res.headersSent) return;
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(data));
    };

    const readJson = <T = any>(): Promise<T> =>
      new Promise((resolve) => {
        let body = "";
        req.on("data", (chunk) => {
          body += chunk;
        });
        req.on("end", () => {
          try {
            resolve(JSON.parse(body || "{}") as T);
          } catch {
            resolve({} as T);
          }
        });
      });

    const request: RouteRequest = { method, path, url, req, res, sendJson, readJson };

    // Snapshot: a handler is allowed to add or remove routes (the host does
    // exactly that during a load), and mutating the array mid-iteration is how
    // a route silently ends up dispatched twice.
    for (const entry of [...this._routes]) {
      if (!this._matches(entry.spec, method, path)) continue;
      try {
        // Guard first, and per request. Throwing from the guard is how a refusal gets its
        // own status; returning false declines, letting the next route try.
        if (entry.spec.guard && !(await entry.spec.guard(request))) continue;
        const claimed = await entry.spec.handler(request);
        if (claimed) return true;
      } catch (err) {
        this.ctx.logger.error(
          "router: %s %s refused: %s",
          method,
          entry.spec.name ?? entry.spec.match.toString(),
          err instanceof Error ? err.message : String(err),
        );
        sendJson(statusForThrown(err), { error: messageOfThrown(err) });
        return true;
      }
    }
    return false;
  }

  private _matches(spec: RouteSpec, method: string, path: string): boolean {
    const allowed = spec.method ?? "*";
    const methodOk =
      allowed === "*" ||
      (Array.isArray(allowed) ? allowed.includes(method) : allowed.toUpperCase() === method);
    if (!methodOk) return false;
    if (typeof spec.match === "string") return spec.match === path;
    spec.match.lastIndex = 0;
    return spec.match.test(path);
  }
}

/**
 * The status a thrown route/guard error should answer with.
 *
 * `AuthError` and its siblings carry `httpStatus` so an authorization refusal reports 401
 * or 403. Anything else is a bug and stays a 500: a handler that throws for an unrelated
 * reason must not be reported to the client as an access problem.
 */
function statusForThrown(err: unknown): number {
  const status = (err as { httpStatus?: unknown } | null)?.httpStatus;
  return typeof status === "number" && status >= 400 && status <= 599 ? status : 500;
}

function messageOfThrown(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
