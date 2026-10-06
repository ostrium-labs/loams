import { Context, Service } from "cordis";
import {
  GLITCHTIP_API_PREFIX,
  GLITCHTIP_ENV_PREFIX,
  GLITCHTIP_HEADER_LINK,
  GLITCHTIP_LIST_LIMIT,
  GlitchtipApiToken,
  GlitchtipConfig,
  GlitchtipEnvironment,
  GlitchtipIssue,
  GlitchtipIssueEvent,
  GlitchtipIssueStatsQuery,
  GlitchtipIssuesQuery,
  GlitchtipListPage,
  GlitchtipLog,
  GlitchtipMonitor,
  GlitchtipOrganization,
  GlitchtipProject,
  GlitchtipRelease,
  GlitchtipRoot,
  GlitchtipSpanGroup,
  GlitchtipStaffUser,
  GlitchtipTransactionGroup,
  assertGlitchtipIssueSort,
  glitchtipBearerAuth,
  nextGlitchtipCursor,
  parseGlitchtipPaginationHeaders,
} from "./types.js";
import {
  UpstreamClient,
  UpstreamError,
  loggerFrom,
  type QueryValue,
  type UpstreamLogger,
} from "@loams-plugins/plugin-upstream-http";

export class GlitchtipAdapterService extends Service {
  static inject = [];

  public readonly config: GlitchtipConfig;
  private readonly client: UpstreamClient;
  private readonly timeoutMs: number;
  private readonly logger?: UpstreamLogger;

  constructor(ctx: Context, config: GlitchtipConfig) {
    super(ctx, "glitchtip");
    this.config = config;
    this.timeoutMs = config.timeoutMs ?? 30_000;
    this.logger = loggerFrom(ctx);
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl,
        auth: glitchtipBearerAuth(config.token),
        timeoutMs: this.timeoutMs,
      },
      this.logger,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Transport                                                           */
  /* ------------------------------------------------------------------ */

  /**
   * Object-returning requests go through the shared client.
   *
   * Nothing here is reimplemented: base URL joining, timeout, status mapping and
   * JSON parsing all come from `UpstreamClient`.
   */
  private _get<T>(suffix: string, params?: Record<string, QueryValue>): Promise<T> {
    return this.client.get<T>(apiPath(suffix), params);
  }

  /**
   * List requests need the response HEADERS, which the body alone does not carry.
   *
   * GlitchTip carries every bit of pagination metadata in `X-Hits`, `X-Max-Hits`
   * and `Link`. There is no body-borne total and no body-borne cursor, so a
   * client that returns only the parsed JSON cannot paginate at all — the
   * information is gone before the caller sees it.
   *
   * So this ONE path passes a response observer to the shared client, which hands
   * back the live `Response` before the body is consumed. Auth header assembly,
   * the timeout and status mapping all stay in the client, exactly as they are
   * for {@link _get}; there is no second transport here and nothing to drift.
   */
  private async _list<T>(
    suffix: string,
    params?: Record<string, QueryValue>,
    options: { requirePaginationHeaders?: boolean } = {},
  ): Promise<GlitchtipListPage<T>> {
    const requireHeaders = options.requirePaginationHeaders ?? true;
    const target = apiPath(suffix);
    const url = this.client.resolve(target, params);

    // Last response wins, which is what a caller wants: with no refresh hook
    // configured there is exactly one, and if one were added the replay's
    // headers are the ones the body below came from.
    let observed: { headers: Headers; status: number } | undefined;
    const parsed = await this.client.request<unknown>("GET", target, {
      params,
      onResponse: (res, meta) => {
        observed = { headers: res.headers, status: meta.status };
      },
    });

    if (observed === undefined) {
      // Unreachable: `request` only resolves for a response it received, and every
      // received response is observed. Guarded so the reads below stay typed
      // rather than asserting a non-null the compiler cannot infer.
      throw new UpstreamError("GET", url, 0, "no response was observed for this request");
    }
    const { headers, status } = observed;

    // The response body is a BARE ARRAY on every list endpoint — no `results`,
    // no `count`, no wrapper. An object here means the wrong route or an error
    // page, and reading `.results` off it would silently yield nothing.
    if (!Array.isArray(parsed)) {
      throw new UpstreamError(
        "GET",
        url,
        status,
        `expected a bare JSON array from a list endpoint, got ${
          parsed === null ? "null" : typeof parsed
        }: ${JSON.stringify(parsed).slice(0, 200)}`,
      );
    }

    const pagination = requireHeaders
      ? parseGlitchtipPaginationHeaders(headers)
      : // Only `api-tokens/` takes this path: it is a bare array whose
        // pagination headers are not part of the established contract, and
        // requiring headers that may legitimately be absent would turn a working
        // token listing into a hard error.
        {
          hits: parsed.length,
          maxHits: parsed.length,
          ...(() => {
            const next = nextGlitchtipCursor(headers.get(GLITCHTIP_HEADER_LINK));
            return next === undefined ? {} : { next };
          })(),
        };

    return { data: parsed as T[], pagination };
  }

  /**
   * Walk a cursor-paginated endpoint to exhaustion.
   *
   * There is no `page`/`offset` to increment, so termination is: an empty page,
   * a `Link` with no `rel="next"`, a cursor that repeats one already followed,
   * or the page cap.
   */
  async *walk<T>(
    suffix: string,
    params: Record<string, QueryValue> = {},
    options: { maxPages?: number } = {},
  ): AsyncGenerator<GlitchtipListPage<T>> {
    const maxPages = options.maxPages ?? 50;
    let cursor: string | undefined;
    const used = new Set<string>();

    for (let page = 0; page < maxPages; page += 1) {
      const result = await this._list<T>(suffix, {
        ...params,
        limit: params.limit ?? GLITCHTIP_LIST_LIMIT,
        ...(cursor === undefined ? {} : { cursor }),
      });
      if (result.data.length === 0) return;
      yield result;

      const next = result.pagination.next;
      if (next === undefined) return;
      if (used.has(next)) return;
      used.add(next);
      cursor = next;
    }

    this.logger?.warn(
      `glitchtip: cursor walk of ${suffix} stopped at the ${maxPages}-page cap; results are truncated.`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Root, health and tokens                                             */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/0/` → `{version, user, auth}`.
   *
   * This is the health check AND the token check in one call: `auth` is populated
   * exactly when the bearer token is valid, so a 200 with `auth: null` means the
   * route answered without a usable credential.
   */
  root(): Promise<GlitchtipRoot> {
    return this._get<GlitchtipRoot>("");
  }

  /** Token management. Create body is `{label, scopes}`. */
  listApiTokens(): Promise<GlitchtipApiToken[]> {
    return this._list<GlitchtipApiToken>("api-tokens/", undefined, {
      requirePaginationHeaders: false,
    }).then((page) => page.data);
  }

  createApiToken(label: string, scopes: string[]): Promise<GlitchtipApiToken> {
    return this.client.post<GlitchtipApiToken>(apiPath("api-tokens/"), { label, scopes });
  }

  deleteApiToken(tokenId: number): Promise<void> {
    return this.client.request<void>("DELETE", apiPath(`api-tokens/${tokenId}/`));
  }

  /* ------------------------------------------------------------------ */
  /* Organizations, projects, environments                              */
  /* ------------------------------------------------------------------ */

  listOrganizations(): Promise<GlitchtipListPage<GlitchtipOrganization>> {
    return this._list<GlitchtipOrganization>("organizations/");
  }

  getOrganization(orgSlug: string): Promise<GlitchtipOrganization> {
    return this._get<GlitchtipOrganization>(`organizations/${encodeURIComponent(orgSlug)}/`);
  }

  /** `?query=` filters the project's own search within the organization. */
  searchOrganizationProjects(
    orgSlug: string,
    query?: string,
  ): Promise<GlitchtipListPage<GlitchtipProject>> {
    return this._list<GlitchtipProject>(`organizations/${encodeURIComponent(orgSlug)}/projects/`, {
      query,
    });
  }

  listOrganizationEnvironments(orgSlug: string): Promise<GlitchtipListPage<GlitchtipEnvironment>> {
    return this._list<GlitchtipEnvironment>(
      `organizations/${encodeURIComponent(orgSlug)}/environments/`,
    );
  }

  listProjects(): Promise<GlitchtipListPage<GlitchtipProject>> {
    return this._list<GlitchtipProject>("projects/");
  }

  getProject(orgSlug: string, projectSlug: string): Promise<GlitchtipProject> {
    return this._get<GlitchtipProject>(
      `projects/${encodeURIComponent(orgSlug)}/${encodeURIComponent(projectSlug)}/`,
    );
  }

  listProjectEnvironments(
    orgSlug: string,
    projectSlug: string,
  ): Promise<GlitchtipListPage<GlitchtipEnvironment>> {
    return this._list<GlitchtipEnvironment>(
      `projects/${encodeURIComponent(orgSlug)}/${encodeURIComponent(projectSlug)}/environments/`,
    );
  }

  listProjectEvents(
    orgSlug: string,
    projectSlug: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<GlitchtipIssueEvent>> {
    return this._list<GlitchtipIssueEvent>(
      `projects/${encodeURIComponent(orgSlug)}/${encodeURIComponent(projectSlug)}/events/`,
      { limit: GLITCHTIP_LIST_LIMIT, ...params },
    );
  }

  listProjectReleases(
    orgSlug: string,
    projectSlug: string,
  ): Promise<GlitchtipListPage<GlitchtipRelease>> {
    return this._list<GlitchtipRelease>(
      `projects/${encodeURIComponent(orgSlug)}/${encodeURIComponent(projectSlug)}/releases/`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Issues                                                              */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/0/organizations/{org}/issues/`.
   *
   * Note the absence of a `status` param — see {@link GlitchtipIssuesQuery}.
   * `project` takes NUMERIC ids, and arrays are sent as REPEATED params.
   */
  listIssues(
    orgSlug: string,
    query: GlitchtipIssuesQuery = {},
  ): Promise<GlitchtipListPage<GlitchtipIssue>> {
    assertGlitchtipIssueSort(query.sort);
    return this._list<GlitchtipIssue>(`organizations/${encodeURIComponent(orgSlug)}/issues/`, {
      id: query.id,
      start: query.start,
      end: query.end,
      project: query.project,
      environment: query.environment,
      query: query.query,
      sort: query.sort,
      limit: query.limit ?? GLITCHTIP_LIST_LIMIT,
      cursor: query.cursor,
    });
  }

  listOrganizationIssues(
    orgSlug: string,
    query: GlitchtipIssuesQuery = {},
  ): Promise<GlitchtipListPage<GlitchtipIssue>> {
    return this.listIssues(orgSlug, query);
  }

  getOrganizationIssue(orgSlug: string, issueId: string): Promise<GlitchtipIssue> {
    return this._get<GlitchtipIssue>(
      `organizations/${encodeURIComponent(orgSlug)}/issues/${encodeURIComponent(issueId)}/`,
    );
  }

  getIssue(issueId: string): Promise<GlitchtipIssue> {
    return this._get<GlitchtipIssue>(`issues/${encodeURIComponent(issueId)}/`);
  }

  listIssueTags(issueId: string): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(`issues/${encodeURIComponent(issueId)}/tags/`);
  }

  listIssueHashes(issueId: string): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(`issues/${encodeURIComponent(issueId)}/hashes/`);
  }

  /**
   * `GET /api/0/organizations/{org}/issues-stats/`.
   *
   * `groups` is REQUIRED — the endpoint errors without it — so this method takes
   * no optional form of the parameter and will not send a request without it.
   */
  async issueStats(
    orgSlug: string,
    query: GlitchtipIssueStatsQuery,
  ): Promise<GlitchtipListPage<Record<string, unknown>>> {
    if (!query.groups || query.groups.length === 0) {
      throw new Error(
        "glitchtip: issues-stats requires `groups` — a non-empty list of numeric issue ids.",
      );
    }
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/issues-stats/`,
      { groups: query.groups, statsPeriod: query.statsPeriod },
    );
  }

  listIssueEvents(
    issueId: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<GlitchtipIssueEvent>> {
    return this._list<GlitchtipIssueEvent>(`issues/${encodeURIComponent(issueId)}/events/`, {
      limit: GLITCHTIP_LIST_LIMIT,
      ...params,
    });
  }

  /** Most recent event for an issue. An object, not a list. */
  latestIssueEvent(issueId: string): Promise<GlitchtipIssueEvent> {
    return this._get<GlitchtipIssueEvent>(`issues/${encodeURIComponent(issueId)}/events/latest/`);
  }

  getIssueEvent(issueId: string, eventId: string): Promise<GlitchtipIssueEvent> {
    return this._get<GlitchtipIssueEvent>(
      `issues/${encodeURIComponent(issueId)}/events/${encodeURIComponent(eventId)}/`,
    );
  }

  listIssueCommits(issueId: string): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(`issues/${encodeURIComponent(issueId)}/commits/`);
  }

  listIssueUserReports(issueId: string): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `issues/${encodeURIComponent(issueId)}/user-reports/`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Performance                                                         */
  /* ------------------------------------------------------------------ */

  listTransactionGroups(
    orgSlug: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<GlitchtipTransactionGroup>> {
    return this._list<GlitchtipTransactionGroup>(
      `organizations/${encodeURIComponent(orgSlug)}/transaction-groups/`,
      { limit: GLITCHTIP_LIST_LIMIT, ...params },
    );
  }

  getTransactionGroup(orgSlug: string, groupId: string): Promise<GlitchtipTransactionGroup> {
    return this._get<GlitchtipTransactionGroup>(
      `organizations/${encodeURIComponent(orgSlug)}/transaction-groups/${encodeURIComponent(groupId)}/`,
    );
  }

  transactionGroupTrend(
    orgSlug: string,
    groupId: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/transaction-groups/${encodeURIComponent(groupId)}/trend/`,
      { limit: GLITCHTIP_LIST_LIMIT, ...params },
    );
  }

  listTransactionGroupSpans(
    orgSlug: string,
    groupId: string,
  ): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/transaction-groups/${encodeURIComponent(groupId)}/spans/`,
    );
  }

  listSpanGroups(orgSlug: string): Promise<GlitchtipListPage<GlitchtipSpanGroup>> {
    return this._list<GlitchtipSpanGroup>(
      `organizations/${encodeURIComponent(orgSlug)}/span-groups/`,
    );
  }

  listNPlusOne(orgSlug: string): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/n-plus-one/`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Monitors, releases, logs, staff users                               */
  /* ------------------------------------------------------------------ */

  listMonitors(orgSlug: string): Promise<GlitchtipListPage<GlitchtipMonitor>> {
    return this._list<GlitchtipMonitor>(`organizations/${encodeURIComponent(orgSlug)}/monitors/`);
  }

  listMonitorChecks(
    orgSlug: string,
    monitorId: string,
  ): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/monitors/${encodeURIComponent(monitorId)}/checks/`,
    );
  }

  listOrganizationReleases(orgSlug: string): Promise<GlitchtipListPage<GlitchtipRelease>> {
    return this._list<GlitchtipRelease>(`organizations/${encodeURIComponent(orgSlug)}/releases/`);
  }

  listReleaseDeploys(
    orgSlug: string,
    version: string,
  ): Promise<GlitchtipListPage<Record<string, unknown>>> {
    return this._list<Record<string, unknown>>(
      `organizations/${encodeURIComponent(orgSlug)}/releases/${encodeURIComponent(version)}/deploys/`,
    );
  }

  listLogs(
    orgSlug: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<GlitchtipLog>> {
    return this._list<GlitchtipLog>(`organizations/${encodeURIComponent(orgSlug)}/logs/`, {
      limit: GLITCHTIP_LIST_LIMIT,
      ...params,
    });
  }

  logStats(
    orgSlug: string,
    params: Record<string, QueryValue> = {},
  ): Promise<GlitchtipListPage<GlitchtipLog>> {
    return this._list<GlitchtipLog>(`organizations/${encodeURIComponent(orgSlug)}/logs/stats/`, {
      limit: GLITCHTIP_LIST_LIMIT,
      ...params,
    });
  }

  /**
   * `GET /api/0/users/` — STAFF users.
   *
   * These are the people who RUN GlitchTip. They are not the end users whose
   * errors these are, and no endpoint lists those. See
   * {@link GlitchtipStaffUser}.
   */
  listStaffUsers(): Promise<GlitchtipListPage<GlitchtipStaffUser>> {
    return this._list<GlitchtipStaffUser>("users/");
  }

  getCurrentUser(): Promise<GlitchtipStaffUser> {
    return this._get<GlitchtipStaffUser>("users/me");
  }

  /* ------------------------------------------------------------------ */
  /* Diagnostics                                                         */
  /* ------------------------------------------------------------------ */

  /**
   * One call that answers "can we read this deployment, and how".
   *
   * Never throws. A dead GlitchTip is a status to render on a dashboard, not an
   * exception that should take out the eight widgets beside it.
   *
   * The token's scopes are reported rather than judged: the scope STRINGS are
   * never enumerated anywhere in the frontend tree, so an under-scoped token
   * cannot be predicted client-side and only shows up as a 403 from whichever
   * endpoint needs the missing scope.
   */
  async diagnostics(): Promise<
    | {
        ok: true;
        version: string;
        apiBaseUrl: string;
        tokenScopes: string[];
        organizations: Array<{ require2fa: boolean | undefined; summary: string }>;
      }
    | { ok: false; error: string; status?: number }
  > {
    try {
      const root = await this.root();
      const organizations = await this.listOrganizations();
      return {
        ok: true,
        version: root.version,
        apiBaseUrl: apiPath(""),
        tokenScopes: root.auth?.scopes ?? [],
        organizations: organizations.data.map((org) => ({
          require2fa: org.require2fa,
          summary: String(org.name ?? org.slug ?? "(unnamed)"),
        })),
      };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
        ...(error instanceof UpstreamError ? { status: error.status } : {}),
      };
    }
  }
}

function apiPath(suffix: string): string {
  return `${GLITCHTIP_API_PREFIX}${suffix}`;
}

declare module "cordis" {
  interface Context {
    glitchtip: GlitchtipAdapterService;
  }
}
