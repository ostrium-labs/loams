/**
 * It's a Plan adapter.
 *
 * SCOPE: read-only, with one deliberate exception. Every method issues a `GET`
 * except `validateChartSpec`, which POSTs a chart spec to
 * `/projects/:projectKey/charts` — a route that VALIDATES a spec and ECHOES IT
 * BACK and STORES NOTHING. It is POST only because it takes a body, and it is
 * idempotent, so it is safe to call from a dashboard that is merely checking its
 * own chart definition. Nothing else writes.
 *
 * WHAT IS REUSED
 * --------------
 * All HTTP mechanics — auth header assembly, query encoding, timeouts, error
 * normalization — come from `@loams-plugins/plugin-upstream-http`. This file owns what is
 * genuinely It's a Plan's: the absolute-path routes, the `{error, code?}` body
 * branch, and the per-route pagination and series-density differences.
 *
 * FOUR THINGS THAT ARE WORTH READING BEFORE EDITING
 * -------------------------------------------------
 * 1. THERE IS NO `/api/v1`. `apps/api/src/app.ts:26` takes `API_URL` and the
 *    feature route groups declare ABSOLUTE paths like
 *    `/projects/:projectKey/analytics/stats`. The API server root IS the API.
 * 2. THERE IS NO SUCCESS ENVELOPE. Every route returns its DTO directly. The one
 *    uniform shape is the error `{error, code?}`, so that is what `_call` checks
 *    — on the 2xx path as well as the `UpstreamError` path.
 * 3. PAGINATION IS PER-ROUTE. Offset pages, cursor feeds and whole bare lists
 *    coexist, and which applies is a property of the ROUTE. There is no shared
 *    list envelope to normalise onto, so each response type is its own.
 * 4. SERIES DENSITY IS NOT UNIFORM. `/analytics/pulse` is DENSE and complete;
 *    `/analytics/throughput` is SPARSE and omits empty weeks. Both carry a
 *    literal `density` tag so a widget cannot conflate them.
 */

import { Context, Service } from "cordis";
import {
  QueryValue,
  UpstreamClient,
  UpstreamError,
  UpstreamLogger,
  loggerFrom,
} from "@loams-plugins/plugin-upstream-http";
import type { PluginAgentSkill, PluginLoader, PluginManifest } from "@loams-plugins/core";
import {
  ITSAPLAN_ABSENT_ENDPOINTS,
  ITSAPLAN_CHART_MAX_ROWS,
  ITSAPLAN_NON_REST_PATHS,
  ITSAPLAN_RATE_LIMIT_PER_SECOND,
  ItsAPlanActivityCursor,
  ItsAPlanActivityItem,
  ItsAPlanAgentRun,
  ItsAPlanAgentRunStats,
  ItsAPlanAgentRunStatus,
  ItsAPlanAgentWorkload,
  ItsAPlanBreakdownDimension,
  ItsAPlanBreakdownItem,
  ItsAPlanBurnup,
  ItsAPlanChartSpec,
  ItsAPlanConfig,
  ItsAPlanCursorPage,
  ItsAPlanDashboard,
  ItsAPlanHealth,
  ItsAPlanIssue,
  ItsAPlanIssueFilters,
  ItsAPlanMe,
  ItsAPlanPulseScope,
  ItsAPlanPulseSeries,
  ItsAPlanPulseUnit,
  ItsAPlanProjectedDates,
  ItsAPlanStats,
  ItsAPlanThroughputSeries,
  ItsAPlanWebhookStats,
  burnupProjectedDates,
  isDenseSeries,
  isSparseSeries,
  unbucketedAgentRuns,
  unbucketedWebhookDeliveries,
} from "./types.js";

/* -------------------------------------------------------------------------- */
/* Errors and body validation                                                   */
/* -------------------------------------------------------------------------- */

/**
 * A failed It's a Plan call.
 *
 * Raised from the `{error, code?}` body, which is the only uniform shape the API
 * has. A 500 is reported by status and NEVER by the body's text: the 500 path
 * logs server-side and returns a GENERIC message, so parsing it would surface
 * noise as if it were a cause.
 */
export class ItsAPlanApiError extends Error {
  /** The HTTP status. */
  readonly status: number;
  /** ItsAPlan's `code`, on the cases that set one. */
  readonly code?: string;
  /** True when the failure is a permissions/feature problem rather than a bug. */
  readonly isConfiguration: boolean;

  constructor(
    message: string,
    options: { status: number; code?: string; isConfiguration?: boolean },
  ) {
    super(message);
    this.name = "ItsAPlanApiError";
    this.status = options.status;
    this.code = options.code;
    this.isConfiguration = options.isConfiguration ?? false;
  }
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function tryParseObject(text: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(text) as unknown;
    return isPlainObject(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

/** The `error` field of an error body, or undefined when this is not one. */
function errorMessageOf(body: unknown): string | undefined {
  if (!isPlainObject(body)) return undefined;
  const message = body["error"];
  if (typeof message !== "string" || message.length === 0) return undefined;
  return message;
}

/** The `code` field of an error body, when present. */
function errorCodeOf(body: unknown): string | undefined {
  if (!isPlainObject(body)) return undefined;
  const code = body["code"];
  return typeof code === "string" && code.length > 0 ? code : undefined;
}

function requireObject<T>(value: unknown, what: string): T {
  if (!isPlainObject(value)) {
    throw new ItsAPlanApiError(`${what}: expected an object, got ${describe(value)}`, {
      status: 200,
    });
  }
  return value as T;
}

/**
 * Require a BARE ARRAY.
 *
 * The single most common ItsAPlan bug: assuming a list envelope where there is
 * none. Most analytics routes answer `[...]` directly, so a response parsed as
 * `{items}` is silently empty rather than an error.
 */
function requireArray<T>(value: unknown, what: string): T[] {
  if (!Array.isArray(value)) {
    throw new ItsAPlanApiError(
      `${what}: expected a bare array (this route has NO list envelope), got ${describe(value)}`,
      { status: 200 },
    );
  }
  return value as T[];
}

function describe(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return `an array of ${value.length}`;
  if (isPlainObject(value)) return "an object";
  return typeof value;
}

/* -------------------------------------------------------------------------- */
/* Service                                                                      */
/* -------------------------------------------------------------------------- */

export class ItsAPlanAdapterService extends Service {
  static inject = [];

  readonly config: ItsAPlanConfig;
  private readonly client: UpstreamClient;
  private readonly log: UpstreamLogger | undefined;

  constructor(ctx: Context, config: ItsAPlanConfig) {
    super(ctx, "itsaplan");
    this.config = config;

    if (typeof config.apiKey !== "string" || config.apiKey.length === 0) {
      throw new Error(
        "itsaplan: `apiKey` is required. It's a Plan has no self-service key creation for a " +
          "client: create one with POST /api/auth/api-key/create while signed in, note the itp_… " +
          "value shown once, and configure it here.",
      );
    }

    this.log = loggerFrom(ctx);
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl.replace(/\/+$/, ""),
        // `x-api-key`, not `Authorization: Bearer`. The Bearer token that exists
        // is MCP-only and SCIM has its own separate bearer; neither is the REST
        // surface, and the session-cookie path is not modelled here at all.
        auth: { kind: "headers", headers: { "x-api-key": config.apiKey } },
        timeoutMs: config.timeoutMs,
      },
      this.log,
    );
  }

  /** The documented per-key rate limit, for a caller budgeting a fan-out. */
  get rateLimitPerSecond(): number {
    return ITSAPLAN_RATE_LIMIT_PER_SECOND;
  }

  /* ---------------------------------------------------------------------- */
  /* Plumbing                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * Perform one REST call and unwrap its DTO.
   *
   * Two error branches, because ItsAPlan can signal a failure on either:
   *
   * - The `UpstreamError` path: a real non-2xx. The body is parsed for the
   *   `{error, code?}` shape to recover the actionable message. A 5xx message is
   *   deliberately DISCARDED — the 500 handler returns a generic string.
   * - The 2xx path: a body that still carries `error`. There is no success
   *   envelope to check `result` against, so the presence of `error` is the only
   *   available signal and it must be checked.
   *
   * A 403/404 on an analytics path is additionally labelled a CONFIGURATION
   * failure: those routes require `permission:['dashboards','read']` plus
   * `feature:'dashboards'`, so a project with dashboards disabled answers 403/404
   * and that is a setup problem, not a bug worth a generic message.
   */
  private async _call<T>(
    path: string,
    params?: Record<string, QueryValue>,
    options: { method?: "GET" | "POST"; body?: unknown; analytics?: boolean } = {},
  ): Promise<T> {
    const { method = "GET", body, analytics = false } = options;
    const label = `${method} ${path}`;

    let payload: unknown;
    try {
      payload =
        method === "POST"
          ? await this.client.post<unknown>(path, body, params)
          : await this.client.get<unknown>(path, params);
    } catch (err) {
      if (err instanceof UpstreamError) {
        const parsed = tryParseObject(err.body);
        const message = errorMessageOf(parsed);
        const isServer = err.status >= 500;

        if (message !== undefined && !isServer) {
          throw this._failure(label, err.status, message, errorCodeOf(parsed), analytics);
        }
        if (message !== undefined) {
          // The 500 body is GENERIC by design. Naming it would put noise in the
          // message and imply it identified the cause.
          this.log?.warn(
            `itsaplan: ${label} returned ${err.status}; the server's message is generic and is not surfaced`,
          );
          throw new ItsAPlanApiError(
            `itsaplan: ${label} failed with ${err.status}. The server's error text on a 5xx is ` +
              "generic by design and is not parsed — check the server logs.",
            { status: err.status },
          );
        }
        if (analytics && (err.status === 403 || err.status === 404)) {
          throw this._failure(label, err.status, undefined, undefined, true);
        }
      }
      throw err;
    }

    const message = errorMessageOf(payload);
    if (message !== undefined) {
      throw this._failure(label, 200, message, errorCodeOf(payload), analytics);
    }
    return payload as T;
  }

  /** Build a failure, translating a 403/404 on an analytics route into setup advice. */
  private _failure(
    label: string,
    status: number,
    message: string | undefined,
    code: string | undefined,
    analytics: boolean,
  ): ItsAPlanApiError {
    if (analytics && (status === 403 || status === 404)) {
      return new ItsAPlanApiError(
        `itsaplan: ${label} returned ${status}. ItsAPlan's analytics routes require the ` +
          "project's `dashboards` feature to be enabled AND `permission:['dashboards','read']` for " +
          "the API key's owner, so this is a project configuration or key-permission problem, not a " +
          "request error." +
          (message === undefined ? "" : ` Server said: ${message}`),
        { status, code, isConfiguration: true },
      );
    }
    return new ItsAPlanApiError(
      `itsaplan: ${label} failed with ${status}${message === undefined ? "" : `: ${message}`}`,
      { status, code },
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Server metadata                                                        */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /` — `{"name":"It's a Plan api","status":"ok"}`.
   *
   * Takes no key in practice, but the key is sent anyway: a dashboard that cannot
   * authenticate is better diagnosed here than on a data route.
   */
  async health(): Promise<ItsAPlanHealth> {
    return requireObject<ItsAPlanHealth>(await this._call<ItsAPlanHealth>("/"), "/");
  }

  /**
   * `GET /me` → `{authenticated, user?}`.
   *
   * The cheapest proof that an `itp_…` key resolves to its owner's session,
   * which is what makes it satisfy every guard downstream.
   */
  async me(): Promise<ItsAPlanMe> {
    return requireObject<ItsAPlanMe>(await this._call<ItsAPlanMe>("/me"), "GET /me");
  }

  /**
   * `GET /auth-config` — the instance's public auth configuration.
   *
   * Typed `unknown` ON PURPOSE: the research confirmed the route is public but
   * not its body, and inventing a DTO for it would be a guess presented as a
   * contract. Callers that need it read the raw document.
   */
  async authConfig(): Promise<unknown> {
    return this._call<unknown>("/auth-config");
  }

  /**
   * `GET /docs/json` — the LIVE OpenAPI spec.
   *
   * Useful as a version-drift check: compare it against the routes this adapter
   * calls. Typed as `unknown` because it is the server's own document, not a
   * contract this adapter owns.
   */
  async openApiSpec(): Promise<unknown> {
    return this._call<unknown>("/docs/json");
  }

  /* ---------------------------------------------------------------------- */
  /* Analytics — issue counts                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/analytics/stats` — no parameters.
   *
   * REMEMBER `inProgress` AND `backlog` ARE OVERWRITTEN, not accumulated: a
   * custom state type contributes to the derived `open` and to neither. See
   * {@link ItsAPlanStats}.
   */
  async getStats(projectKey: string): Promise<ItsAPlanStats> {
    return requireObject<ItsAPlanStats>(
      await this._call<ItsAPlanStats>(analyticsPath(projectKey, "stats"), undefined, {
        analytics: true,
      }),
      "GET /analytics/stats",
    );
  }

  /**
   * `GET /projects/:projectKey/analytics/breakdown?by=` — a BARE ARRAY.
   *
   * `by` is REQUIRED and there is no default dimension, so this method takes it
   * as a required argument rather than defaulting one. `key` on each row is a
   * stringified id with `'none'` sentinels — see {@link ItsAPlanBreakdownItem}.
   */
  async getBreakdown(
    projectKey: string,
    by: ItsAPlanBreakdownDimension,
  ): Promise<ItsAPlanBreakdownItem[]> {
    return requireArray<ItsAPlanBreakdownItem>(
      await this._call<unknown>(
        analyticsPath(projectKey, "breakdown"),
        { by },
        { analytics: true },
      ),
      "GET /analytics/breakdown",
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Analytics — time series                                                */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/analytics/pulse?scope=&unit=&columns=` —
   * DENSE.
   *
   * The axis is generated server-side (`generate_series` + left join), so it is
   * complete: no missing buckets, and `label` is already a preformatted string
   * for the unit. The returned ORDER is authoritative — the server states that
   * the client should do no date maths and no timezone reconciliation, so this
   * adapter does none and never re-sorts.
   *
   * `columns` is clamped per unit (hour 140 / day 160 / week 130, default 26).
   */
  async getPulse(
    projectKey: string,
    options: { scope?: ItsAPlanPulseScope; unit?: ItsAPlanPulseUnit; columns?: number } = {},
  ): Promise<ItsAPlanPulseSeries> {
    const unit = options.unit ?? "day";
    const points = requireArray<{ label: string; count: number }>(
      await this._call<unknown>(
        analyticsPath(projectKey, "pulse"),
        { scope: options.scope, unit, columns: options.columns },
        { analytics: true },
      ),
      "GET /analytics/pulse",
    );
    return { density: "dense", scope: options.scope ?? "project", unit, points };
  }

  /**
   * `GET /projects/:projectKey/analytics/throughput?weeks=` — SPARSE.
   *
   * A week with neither a creation nor a closure is ABSENT. That is the whole
   * difference from {@link getPulse}, and it is why the result carries
   * `density: "sparse"`: a widget that gap-fills this series invents weeks the
   * server did not report, and one that renders missing buckets as holes implies
   * a data problem that does not exist.
   */
  async getThroughput(
    projectKey: string,
    options: { weeks?: number } = {},
  ): Promise<ItsAPlanThroughputSeries> {
    const weeks = requireArray<{ week: string; created: number; closed: number }>(
      await this._call<unknown>(
        analyticsPath(projectKey, "throughput"),
        { weeks: options.weeks },
        { analytics: true },
      ),
      "GET /analytics/throughput",
    );
    return { density: "sparse", weeks };
  }

  /* ---------------------------------------------------------------------- */
  /* Analytics — burnup                                                     */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/analytics/burnup?days=&initiativeId=&forecastWeeks=`.
   *
   * The forecast is a RANGE of THREE projected dates, not one. Read them
   * together through {@link burnupProjectedDates}: `projectedDate` is null when
   * nothing closed inside the window or nothing remains, and the
   * optimistic/pessimistic pair is null WITH it. Rendering `projectedDate` alone
   * and falling back to `optimisticDate` invents a date the server declined to
   * give.
   */
  async getBurnup(
    projectKey: string,
    options: { days?: number; initiativeId?: string; forecastWeeks?: number } = {},
  ): Promise<ItsAPlanBurnup> {
    const body = requireObject<ItsAPlanBurnup>(
      await this._call<ItsAPlanBurnup>(
        analyticsPath(projectKey, "burnup"),
        {
          days: options.days,
          initiativeId: options.initiativeId,
          forecastWeeks: options.forecastWeeks,
        },
        { analytics: true },
      ),
      "GET /analytics/burnup",
    );
    return {
      days: Array.isArray(body.days) ? body.days : [],
      forecast: requireObject(
        "forecast" in body ? body.forecast : {},
        "GET /analytics/burnup: forecast",
      ),
      targetDate: typeof body.targetDate === "string" ? body.targetDate : null,
    };
  }

  /** The three projected dates from a burnup response, or `null` when there are none. */
  burnupDates(burnup: ItsAPlanBurnup): ItsAPlanProjectedDates | null {
    return burnupProjectedDates(burnup);
  }

  /* ---------------------------------------------------------------------- */
  /* Analytics — activity (cursor pagination)                              */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/analytics/activity?limit=&cursor=&actorUserId=&action=&issueIds=`.
   *
   * Scheme 2, a CURSOR FEED: `{items, nextCursor}` where the cursor is
   * `{ts, id}`, newest first, keyset-paginated on `(created_at, id)`. There are
   * no page numbers and no totals — feed `nextCursor` back as `cursor` until it
   * is null.
   *
   * `limit` defaults to 25 and is clamped 1–100.
   *
   * **`issueIds` IS CSV AND AN EMPTY MATCH YIELDS AN EMPTY FEED.** That is
   * documented behaviour, not a bug, and the distinction matters: filtering by
   * ids that match nothing returns `items: []` and `nextCursor: null`, which is
   * indistinguishable from "you reached the end" unless the caller knows it
   * asked. A widget that renders "no activity" from this response is making a
   * claim the API did not make.
   */
  async getActivity(
    projectKey: string,
    options: {
      limit?: number;
      cursor?: ItsAPlanActivityCursor | null;
      actorUserId?: string;
      action?: string;
      /** CSV. An empty match yields an EMPTY feed, by design. */
      issueIds?: string;
    } = {},
  ): Promise<ItsAPlanCursorPage<ItsAPlanActivityItem>> {
    const body = requireObject<{ items?: unknown; nextCursor?: unknown }>(
      await this._call<unknown>(
        analyticsPath(projectKey, "activity"),
        {
          limit: options.limit,
          cursor: options.cursor ? `${options.cursor.ts},${String(options.cursor.id)}` : undefined,
          actorUserId: options.actorUserId,
          action: options.action,
          issueIds: options.issueIds,
        },
        { analytics: true },
      ),
      "GET /analytics/activity",
    );
    const items = Array.isArray(body.items) ? (body.items as ItsAPlanActivityItem[]) : [];
    const cursor = body.nextCursor;
    const nextCursor: ItsAPlanActivityCursor | null =
      isPlainObject(cursor) && typeof cursor["ts"] === "string" && cursor["id"] !== undefined
        ? { ts: cursor["ts"] as string, id: cursor["id"] as string | number }
        : null;
    return { items, nextCursor };
  }

  /**
   * Walk `/analytics/activity` to the end, or to `maxPages`.
   *
   * The loop stops on a null `nextCursor` and also on a cursor that does not
   * ADVANCE: a repeated cursor means the server is not making progress, and
   * looping on it would never terminate. `maxPages` bounds a large feed.
   */
  async getAllActivity(
    projectKey: string,
    options: { limit?: number; issueIds?: string; maxPages?: number } = {},
  ): Promise<ItsAPlanActivityItem[]> {
    const maxPages = options.maxPages ?? 20;
    const collected: ItsAPlanActivityItem[] = [];
    let cursor: ItsAPlanActivityCursor | null = null;

    for (let page = 0; page < maxPages; page += 1) {
      const result = await this.getActivity(projectKey, {
        limit: options.limit,
        issueIds: options.issueIds,
        cursor,
      });
      collected.push(...result.items);
      if (result.nextCursor === null) return collected;
      if (
        cursor !== null &&
        result.nextCursor.ts === cursor.ts &&
        String(result.nextCursor.id) === String(cursor.id)
      ) {
        this.log?.warn("itsaplan: activity cursor did not advance; stopping the feed walk");
        return collected;
      }
      cursor = result.nextCursor;
    }
    this.log?.warn(
      `itsaplan: activity feed stopped after ${maxPages} pages; raise maxPages or narrow issueIds`,
    );
    return collected;
  }

  /* ---------------------------------------------------------------------- */
  /* Analytics — agents and webhooks                                        */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/analytics/agent-runs?status=&limit=` — a BARE ARRAY.
   *
   * `trigger` is a closed union (`mention|delegation|field|schedule|manual`), so
   * an unexpected value is a version drift rather than a new kind of run.
   */
  async getAgentRuns(
    projectKey: string,
    options: { status?: ItsAPlanAgentRunStatus; limit?: number } = {},
  ): Promise<ItsAPlanAgentRun[]> {
    return requireArray<ItsAPlanAgentRun>(
      await this._call<unknown>(
        analyticsPath(projectKey, "agent-runs"),
        { status: options.status, limit: options.limit },
        { analytics: true },
      ),
      "GET /analytics/agent-runs",
    );
  }

  /**
   * `GET /projects/:projectKey/analytics/agent-run-stats?days=`.
   *
   * `total` ACCUMULATES but the three buckets OVERWRITE, so `total` can EXCEED
   * `success + failed + pending`. Nothing here asserts otherwise; read the
   * difference through `unbucketedAgentRuns`.
   */
  async getAgentRunStats(
    projectKey: string,
    options: { days?: number } = {},
  ): Promise<ItsAPlanAgentRunStats> {
    const body = requireObject<Partial<ItsAPlanAgentRunStats>>(
      await this._call<unknown>(
        analyticsPath(projectKey, "agent-run-stats"),
        { days: options.days },
        { analytics: true },
      ),
      "GET /analytics/agent-run-stats",
    );
    return {
      total: numberOr(body.total, 0),
      success: numberOr(body.success, 0),
      failed: numberOr(body.failed, 0),
      pending: numberOr(body.pending, 0),
    };
  }

  /** `GET /projects/:projectKey/analytics/webhook-stats?days=` — same overwrite caveat. */
  async getWebhookStats(
    projectKey: string,
    options: { days?: number } = {},
  ): Promise<ItsAPlanWebhookStats> {
    const body = requireObject<Partial<ItsAPlanWebhookStats>>(
      await this._call<unknown>(
        analyticsPath(projectKey, "webhook-stats"),
        { days: options.days },
        { analytics: true },
      ),
      "GET /analytics/webhook-stats",
    );
    return {
      total: numberOr(body.total, 0),
      success: numberOr(body.success, 0),
      failed: numberOr(body.failed, 0),
      pending: numberOr(body.pending, 0),
      activeWebhooks: numberOr(body.activeWebhooks, 0),
      disabledWebhooks: numberOr(body.disabledWebhooks, 0),
    };
  }

  /**
   * `GET /projects/:projectKey/analytics/agent-workload` — no parameters, a bare array.
   *
   * Server-sorted by `delegatedOpen` desc, then `runsTotal` desc, then name.
   * `agentName` is the agent's USERNAME, not a display name.
   */
  async getAgentWorkload(projectKey: string): Promise<ItsAPlanAgentWorkload[]> {
    return requireArray<ItsAPlanAgentWorkload>(
      await this._call<unknown>(analyticsPath(projectKey, "agent-workload"), undefined, {
        analytics: true,
      }),
      "GET /analytics/agent-workload",
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Issues                                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/issues` — a BARE ARRAY, NO PAGINATION.
   *
   * `limit` (1–500, default 50) TRUNCATES. A response of exactly `limit` rows is
   * therefore ambiguous, and there is no `total` and no cursor to disambiguate
   * with. CSV filters are AND-semantics. `identifier` is the human `"MKT-42"`.
   */
  async listIssues(
    projectKey: string,
    filters: ItsAPlanIssueFilters = {},
  ): Promise<ItsAPlanIssue[]> {
    return requireArray<ItsAPlanIssue>(
      await this._call<unknown>(`/projects/${encodeURIComponent(projectKey)}/issues`, {
        columnId: filters.columnId,
        typeId: filters.typeId,
        initiativeId: filters.initiativeId,
        cycleId: filters.cycleId,
        parentId: filters.parentId,
        assigneeUserId: filters.assigneeUserId,
        delegateUserId: filters.delegateUserId,
        priority: filters.priority,
        labelIds: filters.labelIds,
        dueFrom: filters.dueFrom,
        dueTo: filters.dueTo,
        includeArchived: filters.includeArchived,
        limit: filters.limit,
      }),
      "GET /projects/:projectKey/issues",
    );
  }

  /* -------------------------------------------------------------------------- */
  /* Dashboards — LIST ONLY                                                      */
  /* -------------------------------------------------------------------------- */

  /**
   * `GET /projects/:projectKey/dashboards` — LIST ONLY.
   *
   * There is **NO `GET /dashboards/:dashboardId`**, so this is the only way to
   * read a dashboard; see {@link ITSAPLAN_ABSENT_ENDPOINTS}. Deliberately no
   * companion `getDashboard(id)` method exists, and adding one would produce a
   * guaranteed 404 at runtime.
   *
   * `layout` is typed `unknown` because it is `t.Any()` jsonb the server never
   * inspects. Its widget vocabulary is FRONTEND-ONLY and not part of this
   * contract — see the note on {@link ItsAPlanDashboard}.
   */
  async listDashboards(projectKey: string): Promise<ItsAPlanDashboard[]> {
    return requireArray<ItsAPlanDashboard>(
      await this._call<unknown>(`/projects/${encodeURIComponent(projectKey)}/dashboards`),
      "GET /projects/:projectKey/dashboards",
    );
  }

  /* -------------------------------------------------------------------------- */
  /* Chart specs — validated and echoed, nothing stored                            */
  /* -------------------------------------------------------------------------- */

  /**
   * `POST /projects/:projectKey/charts` — VALIDATE a spec; NOTHING IS STORED.
   *
   * The only non-GET in this adapter, and it is idempotent: the route exists to
   * validate a spec and echo it back, and it is POST only because it takes a
   * body. The row cap is checked client-side first so an oversized spec fails
   * with a useful message instead of a server-side rejection.
   */
  async validateChartSpec(projectKey: string, spec: ItsAPlanChartSpec): Promise<ItsAPlanChartSpec> {
    if (!Array.isArray(spec.data) || spec.data.length > ITSAPLAN_CHART_MAX_ROWS) {
      throw new ItsAPlanApiError(
        `itsaplan: chart spec has ${Array.isArray(spec.data) ? spec.data.length : 0} data rows; ` +
          `the route accepts at most ${ITSAPLAN_CHART_MAX_ROWS}.`,
        { status: 0 },
      );
    }
    return requireObject<ItsAPlanChartSpec>(
      await this._call<unknown>(`/projects/${encodeURIComponent(projectKey)}/charts`, undefined, {
        method: "POST",
        body: spec,
      }),
      "POST /projects/:projectKey/charts",
    );
  }
}

/* -------------------------------------------------------------------------- */
/* Module-scope helpers                                                          */
/* -------------------------------------------------------------------------- */

/**
 * Build an analytics path.
 *
 * Absolute, and NOT under `/api/v1`: the route groups declare full paths and the
 * server root IS the API.
 */
function analyticsPath(projectKey: string, leaf: string): string {
  return `/projects/${encodeURIComponent(projectKey)}/analytics/${leaf}`;
}

function numberOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

declare module "cordis" {
  interface Context {
    itsaplan: ItsAPlanAdapterService;
  }
}

/* -------------------------------------------------------------------------- */
/* Manifest                                                                     */
/* -------------------------------------------------------------------------- */

export const ITSAPLAN_SKILLS: PluginAgentSkill[] = [
  {
    id: "getStats",
    name: "Get project stats",
    description:
      "GET /projects/:projectKey/analytics/stats: open, inProgress, backlog, overdue, unassigned and closedLast7d. `open` is derived from stateType and inProgress/backlog are OVERWRITTEN rather than accumulated, so open can exceed their sum.",
    tags: ["itsaplan", "issues", "read"],
    examples: ['getStats {"projectKey":"MKT"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getBreakdown",
    name: "Get issue breakdown",
    description:
      "GET /projects/:projectKey/analytics/breakdown?by=status|priority|type|assignee|delegate, returned as a BARE ARRAY with no envelope. Each row's `key` is a STRINGIFIED id (`${columnId}::text`) with 'none' sentinels — never a number. `color` is always null for by=priority.",
    tags: ["itsaplan", "issues", "read"],
    examples: ['getBreakdown {"projectKey":"MKT","by":"status"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getPulse",
    name: "Get pulse series",
    description:
      "GET /projects/:projectKey/analytics/pulse?scope&unit&columns. The series is DENSE and complete (generate_series + left join, no missing buckets) and `label` is preformatted per unit. Render the returned order directly; do no client-side date maths or timezone reconciliation.",
    tags: ["itsaplan", "timeseries", "read"],
    examples: ['getPulse {"projectKey":"MKT","unit":"day"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getThroughput",
    name: "Get throughput series",
    description:
      "GET /projects/:projectKey/analytics/throughput?weeks, one row per week with created/closed. SPARSE, unlike pulse: a week with neither is ABSENT. Do not gap-fill it as if the buckets were dense.",
    tags: ["itsaplan", "timeseries", "read"],
    examples: ['getThroughput {"projectKey":"MKT","weeks":12}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getBurnup",
    name: "Get burnup",
    description:
      "GET /projects/:projectKey/analytics/burnup?days&initiativeId&forecastWeeks: the daily scope/started/completed curve plus a forecast carrying THREE projected dates (projected, optimistic, pessimistic). All three are null together when nothing closed in the window or nothing remains.",
    tags: ["itsaplan", "timeseries", "read"],
    examples: ['getBurnup {"projectKey":"MKT","days":90}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getActivity",
    name: "Get activity feed",
    description:
      "GET /projects/:projectKey/analytics/activity, a CURSOR feed of {items, nextCursor} keyset-paginated on (created_at, id), newest first. Note `issueIds` is CSV and an empty match yields an EMPTY feed — documented, not a bug.",
    tags: ["itsaplan", "activity", "read"],
    examples: ['getActivity {"projectKey":"MKT","limit":50}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getAgentHealth",
    name: "Get agent health",
    description:
      "GET /projects/:projectKey/analytics/agent-run-stats, /agent-runs and /agent-workload. `total` accumulates but success/failed/pending overwrite, so total can EXCEED their sum — do not present them as a partition. `agentName` in agent-workload is a username, not a display name.",
    tags: ["itsaplan", "agents", "read"],
    examples: ['getAgentHealth {"projectKey":"MKT"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getWebhookStats",
    name: "Get webhook stats",
    description:
      "GET /projects/:projectKey/analytics/webhook-stats: delivery counts with the same overwrite caveat as agent-run-stats, plus active and disabled webhook counts.",
    tags: ["itsaplan", "webhooks", "read"],
    examples: ['getWebhookStats {"projectKey":"MKT"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listIssues",
    name: "List issues",
    description:
      "GET /projects/:projectKey/issues with CSV filters (AND-semantics) and a 1-500 `limit` that TRUNCATES rather than paging — there is no total and no cursor. `identifier` is the human form, e.g. MKT-42.",
    tags: ["itsaplan", "issues", "read"],
    examples: ['listIssues {"projectKey":"MKT","limit":50}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listDashboards",
    name: "List dashboards",
    description:
      "GET /projects/:projectKey/dashboards — LIST ONLY. There is no GET /dashboards/:dashboardId and no data-source API. `layout` is untyped jsonb the server never inspects; its widget vocabulary is frontend-only and not part of the API contract.",
    tags: ["itsaplan", "dashboards", "read"],
    examples: ['listDashboards {"projectKey":"MKT"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const itsaplanManifest: PluginManifest = {
  id: "itsaplan",
  name: "It's a Plan",
  description:
    "Read-only delivery analytics from It's a Plan: issue counts by state, throughput, burnup forecast, activity feed, and AI-agent and webhook health.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/itsaplan",
  icon: "kanban",
  order: 36,
  defaultEnabled: true,
  upstream: { product: "It's a Plan", envPrefix: "ITSAPLAN" },
  agent: {
    name: "It's a Plan Agent",
    description:
      "Queries a self-hosted It's a Plan instance read-only. Authenticates with an itp_… key in the x-api-key header, branches on the {error, code?} body because there is no success envelope, and treats each route's pagination and series density on its own terms: analytics lists are bare arrays with no envelope, /activity is a cursor feed, /pulse is dense and complete while /throughput is sparse and omits empty weeks. It's a Plan is AGPL-3.0; only its HTTP API is consumed, and no ItsAPlan code is vendored.",
    version: "1.0.0",
    skills: ITSAPLAN_SKILLS,
  },
};

function requireProjectKey(params: Record<string, unknown>, skill: string): string {
  const value = params["projectKey"];
  if (typeof value === "string" && value.length > 0) return value;
  throw new Error(`${skill}: "projectKey" is required and must be a non-empty string`);
}

function optionalNumber(params: Record<string, unknown>, field: string): number | undefined {
  const value = params[field];
  if (value === undefined) return undefined;
  if (typeof value === "number" && Number.isFinite(value)) return value;
  throw new Error(`"${field}" must be a number`);
}

function optionalString(params: Record<string, unknown>, field: string): string | undefined {
  const value = params[field];
  if (value === undefined) return undefined;
  if (typeof value === "string") return value;
  throw new Error(`"${field}" must be a string`);
}

const BREAKDOWN_DIMENSIONS: readonly ItsAPlanBreakdownDimension[] = [
  "status",
  "priority",
  "type",
  "assignee",
  "delegate",
];

/**
 * Skill handlers.
 *
 * `api.ctx` rather than `this`: a handler is a plain method on an object literal,
 * so `this` is the handler record. `ctx.itsaplan` can THROW rather than return
 * undefined when the service is not reachable, so the read is guarded.
 */
function itsaplanApi(ctx: Context): ItsAPlanAdapterService {
  try {
    return ctx.itsaplan;
  } catch {
    throw new Error("itsaplan adapter is not loaded");
  }
}

export const itsaplanLoader: PluginLoader = {
  service: ItsAPlanAdapterService,
  skills: () => [
    {
      id: "getStats",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).getStats(requireProjectKey(params, "getStats")),
    },
    {
      id: "getBreakdown",
      handle: async (params, api) => {
        const by = params["by"];
        if (
          typeof by !== "string" ||
          !BREAKDOWN_DIMENSIONS.includes(by as ItsAPlanBreakdownDimension)
        ) {
          throw new Error(
            `getBreakdown: "by" is required and must be one of ${BREAKDOWN_DIMENSIONS.join(" | ")}`,
          );
        }
        return itsaplanApi(api.ctx).getBreakdown(
          requireProjectKey(params, "getBreakdown"),
          by as ItsAPlanBreakdownDimension,
        );
      },
    },
    {
      id: "getPulse",
      handle: async (params, api) => {
        const unit = optionalString(params, "unit");
        const scope = optionalString(params, "scope");
        return itsaplanApi(api.ctx).getPulse(requireProjectKey(params, "getPulse"), {
          unit: unit === "hour" || unit === "day" || unit === "week" ? unit : undefined,
          scope: scope === "me" ? "me" : scope === "project" ? "project" : undefined,
          columns: optionalNumber(params, "columns"),
        });
      },
    },
    {
      id: "getThroughput",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).getThroughput(requireProjectKey(params, "getThroughput"), {
          weeks: optionalNumber(params, "weeks"),
        }),
    },
    {
      id: "getBurnup",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).getBurnup(requireProjectKey(params, "getBurnup"), {
          days: optionalNumber(params, "days"),
          initiativeId: optionalString(params, "initiativeId"),
          forecastWeeks: optionalNumber(params, "forecastWeeks"),
        }),
    },
    {
      id: "getActivity",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).getActivity(requireProjectKey(params, "getActivity"), {
          limit: optionalNumber(params, "limit"),
          issueIds: optionalString(params, "issueIds"),
          actorUserId: optionalString(params, "actorUserId"),
          action: optionalString(params, "action"),
        }),
    },
    {
      id: "getAgentHealth",
      handle: async (params, api) => {
        const service = itsaplanApi(api.ctx);
        const projectKey = requireProjectKey(params, "getAgentHealth");
        const [stats, runs, workload] = await Promise.all([
          service.getAgentRunStats(projectKey),
          service.getAgentRuns(projectKey),
          service.getAgentWorkload(projectKey),
        ]);
        // The three buckets do not partition `total`; the excess is reported
        // rather than quietly absorbed, so a chart cannot imply a partition.
        return {
          stats,
          unbucketed: unbucketedAgentRuns(stats),
          runs,
          workload,
        };
      },
    },
    {
      id: "getWebhookStats",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).getWebhookStats(requireProjectKey(params, "getWebhookStats"), {
          days: optionalNumber(params, "days"),
        }),
    },
    {
      id: "listIssues",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).listIssues(requireProjectKey(params, "listIssues"), {
          limit: optionalNumber(params, "limit"),
          assigneeUserId: optionalString(params, "assigneeUserId"),
          priority: optionalString(params, "priority"),
          labelIds: optionalString(params, "labelIds"),
          includeArchived: params["includeArchived"] === true ? "true" : undefined,
        }),
    },
    {
      id: "listDashboards",
      handle: async (params, api) =>
        itsaplanApi(api.ctx).listDashboards(requireProjectKey(params, "listDashboards")),
    },
  ],
};

export {
  ITSAPLAN_ABSENT_ENDPOINTS,
  ITSAPLAN_CHART_MAX_ROWS,
  ITSAPLAN_NON_REST_PATHS,
  ITSAPLAN_RATE_LIMIT_PER_SECOND,
  UpstreamError,
  burnupProjectedDates,
  isDenseSeries,
  isSparseSeries,
  unbucketedAgentRuns,
  unbucketedWebhookDeliveries,
};
