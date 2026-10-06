import { Context, Service } from "cordis";
import {
  OPENPANEL_ACTIVE_USERS_DAYS_DEFAULT,
  OPENPANEL_ACTIVE_USERS_DAYS_MAX,
  OPENPANEL_ACTIVE_USERS_DAYS_MIN,
  OPENPANEL_DEFAULT_STEP_ENCODING,
  OPENPANEL_ENV_PREFIX,
  OPENPANEL_EXPORT_LIMIT_DEFAULT,
  OPENPANEL_EXPORT_LIMIT_MAX,
  OPENPANEL_FUNNEL_WINDOW_HOURS_DEFAULT,
  OPENPANEL_INSIGHTS_LIMIT_DEFAULT,
  OPENPANEL_INSIGHTS_LIMIT_MAX,
  OpenPanelActiveUsers,
  OpenPanelChartResponse,
  OpenPanelClickHouseRow,
  OpenPanelConfig,
  OpenPanelEngagement,
  OpenPanelExportEventsPage,
  OpenPanelFilter,
  OpenPanelFunnel,
  OpenPanelFunnelQuery,
  OpenPanelHealth,
  OpenPanelInterval,
  OpenPanelLive,
  OpenPanelManageProjects,
  OpenPanelOverview,
  OpenPanelPageRow,
  OpenPanelPagesPerformance,
  OpenPanelRetentionCohort,
  OpenPanelRetentionPoint,
  OpenPanelTimeParams,
  OpenPanelTimeWindow,
  OpenPanelTrafficBreakdown,
  OpenPanelTrafficRow,
  OpenPanelUserFlow,
  assertOpenPanelFunnelQuery,
  describeOpenPanelAuthFailure,
  describeOpenPanelTimeWindow,
  openPanelAuthHeaders,
  resolveOpenPanelBaseUrl,
  serializeOpenPanelFilters,
} from "./types.js";
import {
  UpstreamClient,
  UpstreamError,
  loggerFrom,
  type QueryValue,
  type UpstreamLogger,
} from "@loams-plugins/plugin-upstream-http";

/**
 * A sliding-window rate limiter.
 *
 * OpenPanel enforces 100 requests / 10s on `/export/*` and `/insights/*`, and
 * 20 / 10s on `/manage/*`. A dashboard with eight widgets refreshing together
 * crosses that on the first paint, and the 429s come back as
 * `{status: 429, error: "Too Many Requests"}` — indistinguishable from a busy
 * upstream unless you already know to look for it.
 *
 * Self-throttling is the right response: it costs a dashboard nothing (a few
 * hundred milliseconds of delay) and removes the failure mode entirely.
 *
 * Exported so the limits are assertable without standing up a service.
 */
export class SlidingWindowThrottle {
  private readonly stamps: number[] = [];

  constructor(
    private readonly limit: number,
    private readonly windowMs: number,
    private readonly now: () => number = () => Date.now(),
  ) {}

  /** How many requests fit in the window. */
  get capacity(): number {
    return this.limit;
  }

  /** Record a request without waiting. Used by tests. */
  record(): void {
    const at = this.now();
    while (this.stamps.length > 0 && this.stamps[0]! + this.windowMs <= at) this.stamps.shift();
    this.stamps.push(at);
  }

  /**
   * Milliseconds until a slot frees up, or 0 when there is room right now.
   *
   * The window is checked BEFORE recording, not after: a check that runs after
   * would find its own just-added stamp at the head of the queue and conclude it
   * must wait out a full window on the very first request of an idle client.
   */
  private _millisecondsUntilSlot(): number {
    if (this.stamps.length < this.limit) return 0;
    return Math.max(0, this.stamps[0]! + this.windowMs - this.now());
  }

  async acquire(): Promise<void> {
    while (true) {
      const wait = this._millisecondsUntilSlot();
      if (wait <= 0) {
        this.record();
        return;
      }
      await sleep(wait);
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * The documented rate limits, per prefix.
 *
 * These are the numbers OpenPanel enforces server-side. Kept as data rather than
 * literals in each call site so the throttles can be asserted directly.
 */
export const OPENPANEL_RATE_LIMITS = {
  export: { limit: 100, windowMs: 10_000 },
  insights: { limit: 100, windowMs: 10_000 },
  manage: { limit: 20, windowMs: 10_000 },
} as const;

export type OpenPanelPrefix = keyof typeof OPENPANEL_RATE_LIMITS;

export class OpenPanelAdapterService extends Service {
  static inject = [];

  public readonly config: OpenPanelConfig;
  /** Absolute base every request is built from, `/api` prefix already applied. */
  public readonly apiBaseUrl: string;
  private readonly client: UpstreamClient;
  private readonly logger?: UpstreamLogger;
  private readonly throttles: Record<OpenPanelPrefix, SlidingWindowThrottle>;

  constructor(ctx: Context, config: OpenPanelConfig) {
    super(ctx, "openpanel");
    this.config = config;
    this.apiBaseUrl = resolveOpenPanelBaseUrl(config.baseUrl, config.apiPrefix ?? "/api");
    this.logger = loggerFrom(ctx);
    // Throws before a client is built if the client id is not UUID-shaped: a
    // malformed one is a guaranteed 401, and failing here names the config field
    // instead of surfacing as "Unauthorized" twenty dashboard loads later.
    this.client = new UpstreamClient(
      {
        baseUrl: this.apiBaseUrl,
        auth: {
          kind: "headers",
          headers: openPanelAuthHeaders(config.clientId, config.clientSecret),
        },
        ...(config.timeoutMs !== undefined ? { timeoutMs: config.timeoutMs } : {}),
      },
      this.logger,
    );
    this.throttles = {
      export: new SlidingWindowThrottle(
        OPENPANEL_RATE_LIMITS.export.limit,
        OPENPANEL_RATE_LIMITS.export.windowMs,
      ),
      insights: new SlidingWindowThrottle(
        OPENPANEL_RATE_LIMITS.insights.limit,
        OPENPANEL_RATE_LIMITS.insights.windowMs,
      ),
      manage: new SlidingWindowThrottle(
        OPENPANEL_RATE_LIMITS.manage.limit,
        OPENPANEL_RATE_LIMITS.manage.windowMs,
      ),
    };
  }

  /**
   * Every request goes through here: throttle, send, and translate a 401 into
   * OpenPanel's own branchable reason.
   *
   * No envelope unwrapping happens at this layer, deliberately. OpenPanel returns
   * bare objects and bare arrays, and the adapter's job is to hand that back
   * untouched rather than invent a `{data}` shape the caller then has to undo.
   */
  private async _get<T>(
    prefix: OpenPanelPrefix,
    path: string,
    params?: Record<string, QueryValue>,
  ): Promise<T> {
    await this.throttles[prefix].acquire();
    try {
      return await this.client.get<T>(path, params);
    } catch (error) {
      if (error instanceof UpstreamError && (error.status === 401 || error.status === 403)) {
        throw new UpstreamError(
          error.method,
          error.url,
          error.status,
          describeOpenPanelAuthFailure(error.status, error.body),
        );
      }
      throw error;
    }
  }

  /* ------------------------------------------------------------------ */
  /* Health                                                              */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /healthcheck` → `{status: "ok", ...}`.
   *
   * Sent unauthenticated, and relative to the configured API base. Whether
   * `/healthcheck` sits under the `/api` prefix is deployment-dependent, so a
   * deployment that serves it at the site root should be configured with
   * `apiPrefix: ""`.
   */
  async health(): Promise<OpenPanelHealth> {
    await this.throttles.insights.acquire();
    return this.client.get<OpenPanelHealth>("/healthcheck");
  }

  /* ------------------------------------------------------------------ */
  /* /export/* — camelCase, ISO dates                                    */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /export/events`, the one endpoint with offset pagination.
   *
   * `page` is 1-based and `limit` is clamped 1..1000 (default 50) — clamped
   * server-side, but clamped here too so the caller gets the number it asked for
   * back in `meta.current` rather than discovering the clamp from a truncated
   * chart.
   */
  async listExportEvents(
    query: OpenPanelTimeParams & {
      projectId?: string;
      page?: number;
      limit?: number;
      filters?: OpenPanelFilter[];
    } = {},
  ): Promise<OpenPanelExportEventsPage> {
    return this._get<OpenPanelExportEventsPage>("export", "/export/events", {
      projectId: query.projectId,
      page: query.page ?? 1,
      limit: clamp(query.limit, OPENPANEL_EXPORT_LIMIT_MAX, OPENPANEL_EXPORT_LIMIT_DEFAULT),
      ...this._timeParams(query),
      ...this._filtersParam(query.filters),
    });
  }

  /**
   * `GET /export/charts`.
   *
   * Always executed server-side as `chartType: 'linear', metric: 'sum'` — there
   * is no way to ask for anything else. Returns `unknown`; see
   * {@link OpenPanelChartResponse}.
   */
  exportCharts(
    query: OpenPanelTimeParams & { projectId?: string; eventName?: string } = {},
  ): Promise<OpenPanelChartResponse> {
    return this._get<OpenPanelChartResponse>("export", "/export/charts", {
      projectId: query.projectId,
      eventName: query.eventName,
      chartType: "linear",
      metric: "sum",
      ...this._timeParams(query),
    });
  }

  /* ------------------------------------------------------------------ */
  /* /insights/* — snake_case aggregates                                 */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /insights/:projectId/overview`.
   *
   * `summary.avg_session_duration` is SECONDS. See
   * {@link OpenPanelPagesPerformance} for the MINUTES-valued counterpart.
   */
  overview(
    projectId: string,
    query: OpenPanelTimeParams & { interval?: OpenPanelInterval } = {},
  ): Promise<OpenPanelOverview> {
    return this._get<OpenPanelOverview>("insights", `/insights/${projectId}/overview`, {
      ...this._timeParams(query),
      interval: query.interval ?? "day",
    });
  }

  /**
   * `GET /insights/:projectId/funnel`.
   *
   * An empty funnel is a well-defined all-zero shape, not an error — callers
   * should render it rather than treat it as a failure.
   */
  async funnel(projectId: string, query: OpenPanelFunnelQuery): Promise<OpenPanelFunnel> {
    assertOpenPanelFunnelQuery(query);
    const encoding = this.config.funnelStepEncoding ?? OPENPANEL_DEFAULT_STEP_ENCODING;
    return this._get<OpenPanelFunnel>("insights", `/insights/${projectId}/funnel`, {
      ...this._timeParams(query),
      steps: encoding === "csv" ? query.steps.join(",") : query.steps,
      windowHours: query.windowHours ?? OPENPANEL_FUNNEL_WINDOW_HOURS_DEFAULT,
      ...(query.groupBy !== undefined ? { groupBy: query.groupBy } : {}),
    });
  }

  /** `retention` is a PERCENTAGE 0..100 float, not a 0..1 fraction. */
  retention(
    projectId: string,
    query: OpenPanelTimeParams = {},
  ): Promise<OpenPanelRetentionPoint[]> {
    return this._get<OpenPanelRetentionPoint[]>("insights", `/insights/${projectId}/retention`, {
      ...this._timeParams(query),
    });
  }

  /** Twelve weekly cohorts, ascending. */
  retentionCohort(
    projectId: string,
    query: OpenPanelTimeParams = {},
  ): Promise<OpenPanelRetentionCohort[]> {
    return this._get<OpenPanelRetentionCohort[]>(
      "insights",
      `/insights/${projectId}/retention/cohort`,
      { ...this._timeParams(query) },
    );
  }

  /** `days` is 1..90, default 7. `label` is DAU/WAU/MAU or "<n>d active". */
  async activeUsers(
    projectId: string,
    query: OpenPanelTimeParams & { days?: number } = {},
  ): Promise<OpenPanelActiveUsers> {
    const days = query.days ?? OPENPANEL_ACTIVE_USERS_DAYS_DEFAULT;
    if (days < OPENPANEL_ACTIVE_USERS_DAYS_MIN || days > OPENPANEL_ACTIVE_USERS_DAYS_MAX) {
      throw new Error(
        `openpanel: activeUsers days must be ` +
          `${OPENPANEL_ACTIVE_USERS_DAYS_MIN}..${OPENPANEL_ACTIVE_USERS_DAYS_MAX}; got ${days}.`,
      );
    }
    return this._get<OpenPanelActiveUsers>("insights", `/insights/${projectId}/active_users`, {
      ...this._timeParams(query),
      days,
    });
  }

  engagement(projectId: string, query: OpenPanelTimeParams = {}): Promise<OpenPanelEngagement> {
    return this._get<OpenPanelEngagement>("insights", `/insights/${projectId}/engagement`, {
      ...this._timeParams(query),
    });
  }

  topPages(
    projectId: string,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelPageRow[]> {
    return this._get<OpenPanelPageRow[]>("insights", `/insights/${projectId}/pages/top`, {
      ...this._timeParams(query),
      ...this._insightsLimit(query.limit),
    });
  }

  entryExitPages(
    projectId: string,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelPageRow[]> {
    return this._get<OpenPanelPageRow[]>("insights", `/insights/${projectId}/pages/entry_exit`, {
      ...this._timeParams(query),
      ...this._insightsLimit(query.limit),
    });
  }

  /**
   * `GET /insights/:projectId/events` — RAW ClickHouse rows.
   *
   * The one insights endpoint that returns events rather than an aggregate, and
   * it uses the OTHER naming convention: snake_case, with `created_at` and
   * friends as unzoned `"YYYY-MM-DD HH:mm:ss"` strings. Parse them with
   * `parseClickHouseDate`, never with `new Date`.
   *
   * Deliberately NOT returned as {@link OpenPanelEvent}: the two shapes are not
   * reconcilable, and pretending they are would make `created_at` look like a
   * usable ISO date.
   */
  insightEvents(
    projectId: string,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelClickHouseRow[]> {
    return this._get<OpenPanelClickHouseRow[]>("insights", `/insights/${projectId}/events`, {
      ...this._timeParams(query),
      ...this._insightsLimit(query.limit),
    });
  }

  /** `/insights/:projectId/sessions` — same raw ClickHouse convention as events. */
  insightSessions(
    projectId: string,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelClickHouseRow[]> {
    return this._get<OpenPanelClickHouseRow[]>("insights", `/insights/${projectId}/sessions`, {
      ...this._timeParams(query),
      ...this._insightsLimit(query.limit),
    });
  }

  /**
   * `GET /insights/:projectId/pages/performance`.
   *
   * `pages[].avg_duration` is MINUTES here, while `overview`'s
   * `avg_session_duration` is seconds. Two near-identical names, two units.
   */
  pagesPerformance(
    projectId: string,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelPagesPerformance> {
    return this._get<OpenPanelPagesPerformance>(
      "insights",
      `/insights/${projectId}/pages/performance`,
      {
        ...this._timeParams(query),
        ...this._insightsLimit(query.limit),
      },
    );
  }

  /** `breakdown` selects the column to group by; `referrers` also returns `prefix`. */
  traffic(
    projectId: string,
    breakdown: OpenPanelTrafficBreakdown,
    query: OpenPanelTimeParams & { limit?: number } = {},
  ): Promise<OpenPanelTrafficRow[]> {
    return this._get<OpenPanelTrafficRow[]>(
      "insights",
      `/insights/${projectId}/traffic/${breakdown}`,
      {
        ...this._timeParams(query),
        ...this._insightsLimit(query.limit),
      },
    );
  }

  live(projectId: string): Promise<OpenPanelLive> {
    return this._get<OpenPanelLive>("insights", `/insights/${projectId}/live`);
  }

  /**
   * `GET /insights/:projectId/user_flow` (Sankey).
   *
   * Response shape NOT VERIFIED. Returns `unknown`; see
   * {@link OpenPanelUserFlow}. Exposed rather than omitted so the route is
   * reachable for probing, not so it can be charted blind.
   */
  userFlow(projectId: string, query: OpenPanelTimeParams = {}): Promise<OpenPanelUserFlow> {
    return this._get<OpenPanelUserFlow>("insights", `/insights/${projectId}/user_flow`, {
      ...this._timeParams(query),
    });
  }

  /* ------------------------------------------------------------------ */
  /* /manage/* — requires a `root` client                               */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /manage/projects` → `{data: Project[]}`.
   *
   * One of only two enveloped endpoints. These are raw Prisma rows whose
   * `createdAt`/`updatedAt` ARE ISO strings — the opposite of the ClickHouse
   * rows, where the same field names are unzoned strings.
   *
   * Requires a client of type `root`; `/manage/*` is rate limited to 20/10s.
   */
  manageProjects(): Promise<OpenPanelManageProjects> {
    return this._get<OpenPanelManageProjects>("manage", "/manage/projects");
  }

  /* ------------------------------------------------------------------ */
  /* Helpers                                                             */
  /* ------------------------------------------------------------------ */

  private _timeParams(query: OpenPanelTimeParams): Record<string, QueryValue> {
    const window = describeOpenPanelTimeWindow(query);
    // Surfaced on every request because a `range` with no explicit bounds is
    // resolved in the PROJECT's timezone, and that is the usual explanation for
    // a chart that is off by a few hours.
    if (window.resolvedByServerInProjectTimezone) {
      this.logger?.debug(
        `openpanel: range="${window.range}" resolved by the server in the project's timezone; ` +
          `pass startDate/endDate to pin the window explicitly.`,
      );
    }
    return {
      ...(window.startDate !== undefined ? { startDate: window.startDate } : {}),
      ...(window.endDate !== undefined ? { endDate: window.endDate } : {}),
      ...(window.range !== undefined ? { range: window.range } : {}),
    };
  }

  private _filtersParam(filters: OpenPanelFilter[] | undefined): Record<string, QueryValue> {
    const serialized = serializeOpenPanelFilters(filters);
    // Serialized here and sent as one string; never hand-built at a call site,
    // because a malformed blob is silently dropped to undefined by the API.
    return serialized === undefined ? {} : { filters: serialized };
  }

  /**
   * `limit` for `/insights/*` — max 100, and the ONLY pagination control there.
   *
   * Omitted when not asked for so the server's own default of 20 applies; sent
   * explicitly otherwise, clamped to 100. There is no page, offset or cursor on
   * these endpoints, so this helper's absence of those is the contract.
   */
  private _insightsLimit(limit: number | undefined): Record<string, QueryValue> {
    return limit === undefined
      ? {}
      : { limit: clamp(limit, OPENPANEL_INSIGHTS_LIMIT_MAX, OPENPANEL_INSIGHTS_LIMIT_DEFAULT) };
  }

  /* ------------------------------------------------------------------ */
  /* Diagnostics                                                         */
  /* ------------------------------------------------------------------ */

  /** Never throws: an unreachable OpenPanel is a status to render, not an error. */
  async status(): Promise<
    | { ok: true; apiBaseUrl: string; upstreamStatus: string }
    | { ok: false; error: string; status?: number }
  > {
    try {
      const health = await this.health();
      return { ok: true, apiBaseUrl: this.apiBaseUrl, upstreamStatus: health.status };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
        ...(error instanceof UpstreamError ? { status: error.status } : {}),
      };
    }
  }
}

function clamp(value: number | undefined, max: number, fallback: number): number {
  if (value === undefined) return fallback;
  if (!Number.isFinite(value)) return fallback;
  return Math.min(Math.max(Math.trunc(value), 1), max);
}

declare module "cordis" {
  interface Context {
    openpanel: OpenPanelAdapterService;
  }
}
