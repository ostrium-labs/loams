/**
 * Shared HTTP client for upstream analytics services.
 *
 * WHY THIS EXISTS
 * ---------------
 * Every upstream this project talks to — Superset, Zulip, Forgejo, Langfuse,
 * OpenPanel, Glitchtip, Matomo — is a separate product with its own auth scheme,
 * its own error shape, and its own pagination convention. Written naively,
 * each adapter re-implements token caching, `Authorization` header assembly,
 * query-string array encoding, and error normalization, and the sixth copy is
 * where the subtle bug lands: one adapter forgets to refresh a token, another
 * encodes a repeated query param with `join(',')` when the API wants it
 * repeated, a third swallows a 403 into a `null` result.
 *
 * Those differences are real and belong in each adapter. The mechanics around
 * them do not. This module owns the mechanics.
 *
 * RESPONSE HEADERS
 * ----------------
 * Some upstreams put load-bearing data in headers, not in the body — Zulip's
 * throttling budget, Forgejo's totals, all of GlitchTip's pagination. So
 * `request` accepts an optional `onResponse` observer and hands out the live
 * `Response` before the body is read. Without it the client would have to
 * discard the response, and every adapter needing a header would have to build
 * its own `fetch` to get one back.
 *
 * DELIBERATELY NOT A CORDIS SERVICE
 * ---------------------------------
 * A cordis service is a singleton per container, but each adapter needs its OWN
 * client with a different base URL and credentials. Making this a service would
 * force all adapters to share one misconfigured client. Adapters instantiate it
 * directly instead.
 */

import { Context } from "cordis";

/**
 * How to authenticate against an upstream.
 *
 * `kind` is the discriminating field. Each variant corresponds to a scheme one
 * of the upstreams actually uses, named for that upstream rather than for the
 * HTTP concept, because "which header shape does this product want" is the
 * question that is actually hard.
 */
export type UpstreamAuth =
  /** No credentials. Correct for a local mock server. */
  | { kind: "none" }
  /** `Authorization: Bearer <token>`. Matomo's `token_auth`, Superset's JWT. */
  | { kind: "bearer"; token: string }
  /** `Authorization: Basic base64(user:pass)`. Zulip's `email:api_key`. */
  | { kind: "basic"; username: string; password: string }
  /**
   * A raw `Authorization` value. Forgejo wants `token <access_token>`, which is
   * not Bearer and not Basic, so the literal is passed through.
   */
  | { kind: "authorization-raw"; value: string }
  /**
   * The credential travels as a query parameter. Matomo's `api_key` and
   * `token_auth` both work this way, and putting them in the URL is the API's
   * design rather than an expedient.
   */
  | { kind: "query-token"; param: string; token: string }
  /** Extra headers every request needs, merged last. */
  | { kind: "headers"; headers: Record<string, string> };

export interface UpstreamConfig {
  /** Base URL, e.g. `https://chat.example.com`. A trailing slash is trimmed. */
  baseUrl: string;
  auth: UpstreamAuth;
  /**
   * Per-request timeout in milliseconds. Defaults to 30s. An upstream that
   * hangs must not pin a dashboard request open indefinitely.
   */
  timeoutMs?: number;
  /**
   * Called when an upstream answers 401 or 403. Return a fresh credential, or
   * `null` to report the failure as-is.
   *
   * This exists because token lifetimes differ wildly across the upstreams:
   * Matomo's `token_auth` does not expire, Zulip API keys do not, but Superset's
   * access JWT expires in 15 minutes. Without a refresh hook every long-running
   * dashboard server would start failing at a fixed interval after boot.
   */
  refresh?: (response: Response) => Promise<UpstreamAuth | null>;
}

/**
 * A failed upstream call, with enough context to debug it.
 *
 * `body` is the raw response text, truncated, because these APIs put the
 * actionable part of an error there ("Invalid API key") and discarding it makes
 * every failure look identical.
 */
export class UpstreamError extends Error {
  readonly status: number;
  readonly body: string;
  readonly url: string;
  readonly method: string;

  constructor(method: string, url: string, status: number, body: string) {
    super(`${method} ${url} failed with ${status}: ${body.slice(0, 500)}`);
    this.name = "UpstreamError";
    this.status = status;
    this.body = body;
    this.url = url;
    this.method = method;
  }

  /** True for the statuses that mean "your credential is wrong or expired". */
  get isAuthFailure(): boolean {
    return this.status === 401 || this.status === 403;
  }
}

/** A query value. Arrays become REPEATED params, which is what these APIs expect. */
export type QueryValue =
  | string
  | number
  | boolean
  | null
  | undefined
  | Array<string | number | boolean>;

/**
 * Serialize query params.
 *
 * An array is emitted as a REPEATED parameter (`?a=1&a=2`), never as a
 * comma-joined string. This is the single most common way an adapter silently
 * breaks: Zulip, Glitchtip and OpenPanel all read repeated params, and a joined
 * string arrives as one literal value `"1,2"` that the API cannot interpret.
 * `null` and `undefined` are omitted so an unset filter is absent rather than
 * the literal text "undefined".
 */
export function buildQuery(params: Record<string, QueryValue> | undefined): string {
  if (!params) return "";
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue;
    if (Array.isArray(value)) {
      for (const item of value) {
        if (item === undefined || item === null) continue;
        search.append(key, String(item));
      }
    } else {
      search.append(key, String(value));
    }
  }
  const encoded = search.toString();
  return encoded.length > 0 ? `?${encoded}` : "";
}

export interface UpstreamLogger {
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

/**
 * What an {@link UpstreamResponseObserver} is told about the response it is
 * handed, beyond the response itself.
 */
export interface UpstreamResponseMeta {
  /** The HTTP status of THIS response, which may differ from the final outcome. */
  status: number;
  /**
   * The absolute URL this response was requested from.
   *
   * This is the URL the client BUILT for this attempt, so it is the request's own
   * URL — never a lookup keyed on something the response merely claims.
   */
  url: string;
  /**
   * Which attempt produced this response. `1` is the original request; `2` is the
   * single replay that follows a successful credential refresh.
   *
   * It never exceeds 2: the client gives up after one refresh rather than retrying
   * forever. An adapter that only wants the response whose body it will read can
   * therefore keep the LAST value it saw, and one that must not be confused by a
   * pre-refresh 401 can compare `meta.attempt` against 2.
   */
  attempt: number;
}

/**
 * An optional per-request observer, invoked with the live `Response`.
 *
 * WHY IT EXISTS: three of the upstreams put load-bearing data in HEADERS rather
 * than in the body. Zulip's throttling is `X-RateLimit-Remaining`, Forgejo's
 * pagination is `X-Total-Count` plus an RFC-5988 `Link`, and GlitchTip's is
 * `X-Hits`/`X-Max-Hits`/`Link` with no body-borne equivalent at all. The client
 * consumes the body to return parsed JSON and would otherwise discard the
 * `Response` entirely, so those adapters each had to find a way back to the
 * headers — a `fetch` wrapper, a second hand-built request, or both. This is the
 * one supported way, and it replaces all three.
 *
 * GUARANTEES
 * - Called BEFORE the body is read, so `res.bodyUsed` is false and headers are
 *   intact.
 * - Called for EVERY response actually received, including a 401/403 that
 *   precedes a refresh replay. `meta.attempt` distinguishes them.
 * - Not called when there was no response at all: a timeout (408), a DNS or
 *   connection failure (status 0) or an abort has nothing to observe.
 * - Never allowed to fail the request it observes. A throwing observer is caught
 *   and reported once per distinct message through the injected logger.
 */
export type UpstreamResponseObserver = (res: Response, meta: UpstreamResponseMeta) => void;

/** The optional per-call knobs that are not about the request body. */
export interface UpstreamRequestOptions {
  params?: Record<string, QueryValue>;
  body?: unknown;
  headers?: Record<string, string>;
  /** Skip the JSON parse and return raw text. */
  raw?: boolean;
  /** See {@link UpstreamResponseObserver}. Optional; absent means "observe nothing". */
  onResponse?: UpstreamResponseObserver;
}

/**
 * A configured HTTP client for one upstream.
 *
 * Constructed by an adapter, never registered as a cordis service. See the
 * module comment for why.
 */
export class UpstreamClient {
  private readonly baseUrl: string;
  private readonly timeoutMs: number;
  private auth: UpstreamAuth;
  private readonly refresh?: (response: Response) => Promise<UpstreamAuth | null>;
  private readonly logger?: UpstreamLogger;
  /**
   * Fingerprints of refresh failures already logged.
   *
   * `refresh` runs on every 401/403, so an upstream whose token is simply wrong
   * would otherwise log an identical stack trace per dashboard poll.
   */
  private readonly loggedRefreshFailures = new Set<string>();
  /**
   * Fingerprints of `onResponse` failures already logged.
   *
   * An observer runs on every response, so a broken one would otherwise log an
   * identical stack trace per dashboard poll. Same shape and same cap as
   * {@link loggedRefreshFailures}: dedupe by message, then stop recording so a
   * pathological caller cannot grow the set without bound.
   */
  private readonly loggedObserverFailures = new Set<string>();

  constructor(config: UpstreamConfig, logger?: UpstreamLogger) {
    this.baseUrl = config.baseUrl.replace(/\/+$/, "");
    this.auth = config.auth;
    this.timeoutMs = config.timeoutMs ?? 30_000;
    this.refresh = config.refresh;
    this.logger = logger;
  }

  /** The absolute URL for `path`, with `params` appended. */
  resolve(path: string, params?: Record<string, QueryValue>): string {
    const suffix = path.startsWith("/") || path.startsWith("?") ? path : `/${path}`;
    return `${this.baseUrl}${suffix}${buildQuery(params)}`;
  }

  /**
   * Headers for a request, derived from the current auth.
   *
   * A `query-token` auth contributes nothing here: its credential belongs in
   * the URL, which `send` handles. Exported so an adapter that must build its
   * own request can reuse the same logic rather than reimplement it.
   */
  private headersFor(auth: UpstreamAuth, extra?: Record<string, string>): Headers {
    const headers = new Headers();
    headers.set("Accept", "application/json");

    switch (auth.kind) {
      case "none":
        break;
      case "bearer":
        headers.set("Authorization", `Bearer ${auth.token}`);
        break;
      case "basic":
        headers.set(
          "Authorization",
          `Basic ${Buffer.from(`${auth.username}:${auth.password}`).toString("base64")}`,
        );
        break;
      case "authorization-raw":
        headers.set("Authorization", auth.value);
        break;
      case "query-token":
        // Carried in the query string, not the headers.
        break;
      case "headers":
        for (const [key, value] of Object.entries(auth.headers)) {
          headers.set(key, value);
        }
        break;
    }

    // Adapter-supplied headers win, so a single call can override Content-Type
    // or add a tracing header without mutating the configured auth.
    for (const [key, value] of Object.entries(extra ?? {})) {
      headers.set(key, value);
    }
    return headers;
  }

  /**
   * Perform a request and parse the JSON response.
   *
   * On 401/403, calls `refresh` once and replays. A second failure is
   * propagated as an `UpstreamError` rather than retried again, so a
   * permanently-invalid credential cannot spin.
   *
   * `options.onResponse`, if given, sees every response this call receives —
   * including the 401 that triggers the refresh — before its body is consumed.
   */
  async request<T>(method: string, path: string, options: UpstreamRequestOptions = {}): Promise<T> {
    const { params, body, headers, raw, onResponse } = options;

    // A query-token credential is appended here rather than in `resolve` so that
    // an adapter calling `resolve` for a log line does not leak the token into
    // that log line.
    const attempt = async (auth: UpstreamAuth, attemptNumber: number): Promise<Response> => {
      const withToken =
        auth.kind === "query-token" ? { ...params, [auth.param]: auth.token } : params;
      const url = this.resolve(path, withToken);

      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), this.timeoutMs);
      try {
        const init: RequestInit = {
          method,
          headers: this.headersFor(auth, {
            ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
            ...headers,
          }),
          signal: controller.signal,
        };
        if (body !== undefined) init.body = JSON.stringify(body);
        const response = await fetch(url, init);
        // Observed HERE, between receiving the response and touching its body:
        // that is the only window in which the headers are guaranteed readable.
        this._observe(onResponse, response, url, attemptNumber);
        return response;
      } catch (err) {
        // An abort is a timeout, and saying "fetch failed" hides that the
        // upstream was slow rather than absent.
        if (err instanceof Error && err.name === "AbortError") {
          throw new UpstreamError(method, url, 408, `timed out after ${this.timeoutMs}ms`);
        }
        throw new UpstreamError(method, url, 0, err instanceof Error ? err.message : String(err));
      } finally {
        clearTimeout(timer);
      }
    };

    let response = await attempt(this.auth, 1);

    if ((response.status === 401 || response.status === 403) && this.refresh) {
      const fresh = await this._tryRefresh(method, path, response);
      if (fresh) {
        this.auth = fresh;
        response = await attempt(fresh, 2);
      }
    }

    if (!response.ok) {
      throw new UpstreamError(method, response.url || path, response.status, await response.text());
    }

    if (raw) return (await response.text()) as unknown as T;
    const text = await response.text();
    if (text.length === 0) return undefined as unknown as T;
    try {
      return JSON.parse(text) as T;
    } catch {
      // A 2xx body that is not JSON is an upstream bug; surfacing the text beats
      // a bare "Unexpected token" JSON parse error.
      throw new UpstreamError(
        method,
        response.url || path,
        response.status,
        `non-JSON body: ${text.slice(0, 200)}`,
      );
    }
  }

  /**
   * Hand a received response to the caller's observer, if it supplied one.
   *
   * An observer is instrumentation, so it is never allowed to fail the request it
   * is measuring — a throw is swallowed and reported once per distinct message,
   * for the same reason `refresh` failures are deduplicated.
   */
  private _observe(
    onResponse: UpstreamResponseObserver | undefined,
    response: Response,
    url: string,
    attempt: number,
  ): void {
    if (!onResponse) return;
    try {
      onResponse(response, { status: response.status, url, attempt });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      const key = `${response.status}: ${message}`;
      if (!this.loggedObserverFailures.has(key) && this.loggedObserverFailures.size < 32) {
        this.loggedObserverFailures.add(key);
        this.logger?.warn(`upstream: response observer threw for ${url}: ${message}`);
      }
    }
  }

  /** Run `refresh`, swallowing and deduplicating its failure. */
  private async _tryRefresh(
    method: string,
    path: string,
    response: Response,
  ): Promise<UpstreamAuth | null> {
    if (!this.refresh) return null;
    try {
      return await this.refresh(response);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      const key = `${method} ${path}: ${message}`;
      if (!this.loggedRefreshFailures.has(key)) {
        if (this.loggedRefreshFailures.size < 32) {
          this.loggedRefreshFailures.add(key);
          this.logger?.warn(
            `upstream: credential refresh failed for ${method} ${path}: ${message}`,
          );
        }
      }
      return null;
    }
  }

  get<T>(
    path: string,
    params?: Record<string, QueryValue>,
    options: { onResponse?: UpstreamResponseObserver } = {},
  ): Promise<T> {
    return this.request<T>("GET", path, { params, onResponse: options.onResponse });
  }

  post<T>(
    path: string,
    body?: unknown,
    params?: Record<string, QueryValue>,
    options: { onResponse?: UpstreamResponseObserver } = {},
  ): Promise<T> {
    return this.request<T>("POST", path, { params, body, onResponse: options.onResponse });
  }

  put<T>(
    path: string,
    body?: unknown,
    params?: Record<string, QueryValue>,
    options: { onResponse?: UpstreamResponseObserver } = {},
  ): Promise<T> {
    return this.request<T>("PUT", path, { params, body, onResponse: options.onResponse });
  }
}

/** Extract a cordis logger from a context, when one is available. */
export function loggerFrom(ctx: Context): UpstreamLogger | undefined {
  const logger = (ctx as unknown as { logger?: UpstreamLogger }).logger;
  return typeof logger?.warn === "function" ? logger : undefined;
}
