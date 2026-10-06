/**
 * OpenPanel public API types.
 *
 * TWO FACTS DOMINATE EVERYTHING BELOW
 * ------------------------------------
 *
 * 1. THERE IS NO ENVELOPE.
 *    Every OpenPanel endpoint returns a bare object or a bare array. There is no
 *    `{data: ...}` wrapper, no `results`, no `count`. An adapter written against
 *    the usual convention unwraps `.data`, gets `undefined`, and renders an empty
 *    chart with a 200 in the log. The only two exceptions are
 *    `/export/events` (`{meta, data}`) and `/manage/projects` (`{data}`), both
 *    named explicitly where they appear.
 *
 * 2. THE PRODUCT HAS TWO INCOMPATIBLE NAMING CONVENTIONS.
 *    `/export/*` returns transformed camelCase with real `Date`s serialised ISO.
 *    `/insights/*` returns RAW ClickHouse rows: snake_case, and dates as
 *    `"YYYY-MM-DD HH:mm:ss"` with no `Z` and no milliseconds. Unifying them
 *    behind one shape is not possible without lying about one of them, so this
 *    module keeps them as two separate type families and never pretends a
 *    ClickHouse row is an exported event.
 *
 * A third fact catches everyone: a `"YYYY-MM-DD HH:mm:ss"` string is not reliably
 * parseable. `new Date("2026-08-07 12:34:56")` is treated as LOCAL time in some
 * engines and rejected in others, which produces charts that are silently
 * several hours off depending on where the dashboard server runs. See
 * {@link parseClickHouseDate}.
 */

export const OPENPANEL_ENV_PREFIX = "OPENPANEL";

export interface OpenPanelConfig {
  /**
   * Host root, e.g. `https://analytics.example.com`.
   *
   * The `/api` prefix is appended by default — see
   * {@link resolveOpenPanelBaseUrl} for why that is deployment-dependent.
   */
  baseUrl: string;
  /** UUIDv4-shaped client id. See {@link assertOpenPanelClientId}. */
  clientId: string;
  clientSecret: string;
  /**
   * Path prefix the deployment is reached under. Defaults to `/api`.
   *
   * The self-hosting Caddyfile uses `handle_path /api*`, which STRIPS `/api`
   * before proxying to the internal `op-api:3000`. So the public URL needs
   * `/api` and a direct container address must not have it. Set `""` when
   * pointing straight at the container.
   */
  apiPrefix?: string;
  /**
   * Encoding for the funnel's `steps`. Defaults to `repeated`; see
   * {@link OpenPanelStepEncoding} for why that is the safer default.
   */
  funnelStepEncoding?: OpenPanelStepEncoding;
  /** Per-request timeout in ms. Defaults to 30s. */
  timeoutMs?: number;
}

/* -------------------------------------------------------------------------- */
/* Auth                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * The two custom header names. Lowercase, exactly.
 *
 * Not Bearer, not a JWT, not a cookie. HTTP header names are
 * case-insensitive on the wire but OpenPanel's middleware reads these names, so
 * the adapter emits them lowercased and asserts that in its tests.
 */
export const OPENPANEL_CLIENT_ID_HEADER = "openpanel-client-id";
export const OPENPANEL_CLIENT_SECRET_HEADER = "openpanel-client-secret";

/**
 * The shape OpenPanel validates a client id against.
 *
 * Lowercase hex in 8-4-4-4-12 form. It is described as a UUIDv4, but the check
 * the server actually performs is this pattern — so the adapter matches the
 * pattern rather than also enforcing the version nibble, because rejecting a
 * credential the server would accept is its own outage.
 *
 * Why this must be checked client-side: a malformed client id is a guaranteed
 * 401, and the server's message (`Export: Client ID must be a valid UUIDv4`) is
 * only reached after a round trip. Failing locally turns a confusing auth error
 * into a named config error.
 */
export const OPENPANEL_CLIENT_ID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export class OpenPanelAuthError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OpenPanelAuthError";
  }
}

export function isOpenPanelClientId(value: string): boolean {
  return OPENPANEL_CLIENT_ID_PATTERN.test(value);
}

/**
 * Throw a named, actionable error when the client id is not UUID-shaped.
 *
 * This is the difference between "OpenPanel is down" and "your client id is not
 * a UUID", which are otherwise indistinguishable from a 401.
 */
export function assertOpenPanelClientId(value: string): string {
  if (value.trim().length === 0) {
    throw new OpenPanelAuthError(
      "openpanel: clientId is empty. Create a client with type `read` (not `write`) — /export/* " +
        "and /insights/* refuse a client whose type is `write`.",
    );
  }
  if (!isOpenPanelClientId(value)) {
    throw new OpenPanelAuthError(
      `openpanel: clientId "${value}" is not a valid UUIDv4 (expected lowercase hex 8-4-4-4-12, ` +
        `e.g. 0195f2ae-7c1a-7c2b-9f3d-4e5a6b7c8d9e). OpenPanel validates this exact pattern and ` +
        `answers 401 "Client ID must be a valid UUIDv4" for anything else — fix the value rather ` +
        `than retrying.`,
    );
  }
  return value;
}

/** The exact header pair every OpenPanel request carries. */
export function openPanelAuthHeaders(
  clientId: string,
  clientSecret: string,
): Record<string, string> {
  assertOpenPanelClientId(clientId);
  return {
    [OPENPANEL_CLIENT_ID_HEADER]: clientId,
    [OPENPANEL_CLIENT_SECRET_HEADER]: clientSecret,
  };
}

/**
 * Access is gated on the CLIENT TYPE, not on the key's scope.
 *
 * `/export/*` and `/insights/*` require a client that HAS a secret and whose type
 * is not `write` — the default client type is `write`, so a freshly created
 * client 401s on every read endpoint with `Export: Client is not allowed to
 * export`. `/manage/*` additionally requires type `root`.
 */
export type OpenPanelClientType = "write" | "read" | "root";

/** Minimum client type each prefix requires. Surfaced in the manifest. */
export const OPENPANEL_REQUIRED_CLIENT_TYPE: Readonly<Record<string, OpenPanelClientType>> = {
  export: "read",
  insights: "read",
  manage: "root",
};

/**
 * How the funnel's `steps` parameter is encoded.
 *
 * GENUINELY AMBIGUOUS UPSTREAM, SO IT IS EXPLICIT RATHER THAN GUESSED
 * --------------------------------------------------------------
 * The parameter is documented as `steps=<2..10 event names>` — a single value,
 * which reads as comma-joined — while this repo's shared HTTP client explicitly
 * names OpenPanel as a product that reads REPEATED params.
 *
 * The failure modes are not symmetric, which is what decides the default:
 *
 *  - repeated (`steps=a&steps=b`) against a comma-splitting handler → the handler
 *    sees one step and rejects it. LOUD.
 *  - csv (`steps=a,b`) against a repeated-param handler → the handler sees one
 *    step literally named `"a,b"`, which matches no event. A two-step funnel
 *    with a permanently-zero last step. SILENT, and wrong.
 *
 * So the default is `repeated`, because the bad case for it is an error nobody
 * can miss. Set `csv` if a deployment's funnel demonstrably drops steps.
 */
export type OpenPanelStepEncoding = "repeated" | "csv";
export const OPENPANEL_DEFAULT_STEP_ENCODING: OpenPanelStepEncoding = "repeated";

/**
 * Pull the actionable reason out of a 401.
 *
 * OpenPanel returns `401 {error: "Unauthorized", message: "Export: Client is
 * not allowed to export"}`. The `message` is branchable and specific:
 * `Export: Client ID must be a valid UUIDv4`, `Export: Invalid client id`,
 * `Export: Client has no secret`, `Export: Client is not allowed to export`,
 * `Export: Invalid client secret`. Collapsing those into "401 Unauthorized"
 * throws away the only part that says what to fix, so it is surfaced verbatim.
 */
export function describeOpenPanelAuthFailure(status: number, body: string): string {
  if (status !== 401 && status !== 403) return body.slice(0, 500);
  let message: string | undefined;
  try {
    const parsed: unknown = JSON.parse(body);
    if (parsed && typeof parsed === "object") {
      const value = (parsed as { message?: unknown }).message;
      if (typeof value === "string") message = value;
    }
  } catch {
    // Not JSON: fall through to the raw body.
  }
  return message
    ? `openpanel: ${status} ${message}`
    : `openpanel: ${status} Unauthorized (no branchable reason in the body): ${body.slice(0, 300)}`;
}

/* -------------------------------------------------------------------------- */
/* Base URL                                                                   */
/* -------------------------------------------------------------------------- */

/**
 * Join the host with the API prefix, without doubling a prefix already present.
 */
export function resolveOpenPanelBaseUrl(baseUrl: string, apiPrefix = "/api"): string {
  const root = baseUrl.replace(/\/+$/, "");
  const prefix = apiPrefix.replace(/\/+$/, "");
  if (prefix === "") return root;
  if (root.endsWith(prefix)) return root;
  return `${root}${prefix}`;
}

/* -------------------------------------------------------------------------- */
/* Dates                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * Parse a ClickHouse `"YYYY-MM-DD HH:mm:ss"` timestamp as UTC.
 *
 * WHY THIS EXISTS, AND WHY IT IS NOT OPTIONAL
 * ------------------------------------------
 * `/insights/*` rows carry dates in ClickHouse's native rendering: a space
 * separator, no `Z`, no milliseconds. `new Date("2026-08-07 12:34:56")` is
 * specified to fall back to implementation-specific parsing, and engines
 * disagree — V8 reads it as LOCAL time, some others reject it outright. So the
 * same dashboard shows a day-part boundary shifted by the server's UTC offset,
 * with no error anywhere, and only on deployments not running in UTC.
 *
 * The fix is to make the missing timezone explicit: replace the space with `T`
 * and append `Z`. Every ClickHouse-sourced date in this adapter goes through
 * here, and nothing else.
 *
 * Note what this does NOT apply to: `overview.series[].date` and every
 * `/export/*` date are real ISO-8601 strings (`toISOString()` output) and must be
 * parsed with `new Date(x)` directly. Only raw ClickHouse rows need this.
 */
export function parseClickHouseDate(value: string): Date {
  const normalized = value.includes(" ") ? `${value.replace(" ", "T")}Z` : `${value}Z`;
  const parsed = new Date(normalized);
  if (Number.isNaN(parsed.getTime())) {
    throw new Error(
      `openpanel: "${value}" is not a ClickHouse timestamp. Raw /insights rows use ` +
        `"YYYY-MM-DD HH:mm:ss"; aggregate endpoints and /export/* use ISO-8601.`,
    );
  }
  return parsed;
}

/* -------------------------------------------------------------------------- */
/* Time ranges                                                                */
/* -------------------------------------------------------------------------- */

export type OpenPanelRange =
  | "30min"
  | "lastHour"
  | "last24h"
  | "today"
  | "yesterday"
  | "7d"
  | "30d"
  | "3m"
  | "6m"
  | "12m"
  | "monthToDate"
  | "lastMonth"
  | "yearToDate"
  | "lastYear"
  | "custom";

export interface OpenPanelTimeParams {
  startDate?: string;
  endDate?: string;
  range?: OpenPanelRange;
}

export interface OpenPanelTimeWindow {
  startDate?: string;
  endDate?: string;
  range?: OpenPanelRange;
  /**
   * True when `range` was sent WITHOUT explicit bounds, so the server resolved
   * the window itself — in the PROJECT'S TIMEZONE, not the server's.
   *
   * This is a real source of off-by-hours charts: "last 24 hours" means
   * different instants for a project in Berlin and one in UTC, and nothing in
   * the response says which. Surfacing the flag is the only way a caller can
   * warn about it.
   */
  resolvedByServerInProjectTimezone: boolean;
}

export function describeOpenPanelTimeWindow(params: OpenPanelTimeParams): OpenPanelTimeWindow {
  return {
    ...(params.startDate !== undefined ? { startDate: params.startDate } : {}),
    ...(params.endDate !== undefined ? { endDate: params.endDate } : {}),
    ...(params.range !== undefined ? { range: params.range } : {}),
    resolvedByServerInProjectTimezone:
      params.range !== undefined && params.startDate === undefined && params.endDate === undefined,
  };
}

/* -------------------------------------------------------------------------- */
/* Filters                                                                    */
/* -------------------------------------------------------------------------- */

export type OpenPanelFilterOperator =
  | "is"
  | "isNot"
  | "contains"
  | "doesNotContain"
  | "startsWith"
  | "endsWith"
  | "regex"
  | "isNull"
  | "isNotNull"
  | "gt"
  | "lt"
  | "gte"
  | "lte"
  | "inCohort"
  | "notInCohort";

export interface OpenPanelFilter {
  id?: string;
  name: string;
  operator: OpenPanelFilterOperator;
  value: Array<string | number | boolean | null>;
}

export const OPENPANEL_FILTER_OPERATORS: readonly OpenPanelFilterOperator[] = [
  "is",
  "isNot",
  "contains",
  "doesNotContain",
  "startsWith",
  "endsWith",
  "regex",
  "isNull",
  "isNotNull",
  "gt",
  "lt",
  "gte",
  "lte",
  "inCohort",
  "notInCohort",
];

export class OpenPanelValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OpenPanelValidationError";
  }
}

/**
 * Fastify's `maxParamLength`, enforced upstream on every query param.
 *
 * Exceeding it does not error: the param is truncated, and a truncated
 * `filters` JSON blob parses to nothing and degrades to unfiltered results.
 */
export const OPENPANEL_MAX_PARAM_LENGTH = 15_000;

/**
 * Serialize `filters` to the single URL-encoded JSON string OpenPanel expects.
 *
 * ALWAYS call this rather than hand-building the query param. Malformed filter
 * JSON is SILENTLY DROPPED to `undefined` by the API — not rejected — so a
 * hand-built string with one bad quote does not error, it just quietly returns
 * unfiltered data, and every chart on the dashboard reads as "the filter does
 * nothing". Validating the operators here is the only place that mistake can be
 * caught.
 *
 * Note Fastify's `maxParamLength: 15_000` upstream: a filter array long enough to
 * exceed it is TRUNCATED, which also degrades to wrong data rather than an
 * error. Callers with many filters should split the query.
 */
export function serializeOpenPanelFilters(
  filters: OpenPanelFilter[] | undefined,
): string | undefined {
  if (!filters || filters.length === 0) return undefined;
  for (const filter of filters) {
    if (!OPENPANEL_FILTER_OPERATORS.includes(filter.operator)) {
      throw new OpenPanelValidationError(
        `openpanel: filter operator "${filter.operator}" is not one of ` +
          `${OPENPANEL_FILTER_OPERATORS.join(", ")}. Malformed filters are dropped silently by ` +
          `the API rather than rejected, so this is checked before sending.`,
      );
    }
  }
  const json = JSON.stringify(filters);
  if (json.length > OPENPANEL_MAX_PARAM_LENGTH) {
    throw new OpenPanelValidationError(
      `openpanel: the serialized \`filters\` param is ${json.length} characters, over Fastify's ` +
        `maxParamLength of ${OPENPANEL_MAX_PARAM_LENGTH}. It would be TRUNCATED rather than ` +
        `rejected, which degrades to unfiltered data. Split the query.`,
    );
  }
  return json;
}

/* -------------------------------------------------------------------------- */
/* Health                                                                     */
/* -------------------------------------------------------------------------- */

export interface OpenPanelHealth {
  status: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* /export/* — camelCase, ISO dates                                            */
/* -------------------------------------------------------------------------- */

/**
 * An event from `/export/*`.
 *
 * Transformed camelCase with real dates serialised as ISO-8601. `revenue` and
 * `groups` are present only on some event types, so both are optional.
 */
export interface OpenPanelEvent {
  id: string;
  name: string;
  deviceId: string;
  profileId: string;
  projectId: string;
  sessionId: string;
  properties: Record<string, unknown>;
  createdAt: string;
  country?: string;
  city?: string;
  region?: string;
  longitude?: number;
  latitude?: number;
  os?: string;
  osVersion?: string;
  browser?: string;
  browserVersion?: string;
  device?: string;
  brand?: string;
  model?: string;
  duration?: number;
  path?: string;
  origin?: string;
  referrer?: string;
  referrerName?: string;
  referrerType?: string;
  importedAt?: string;
  sdkName?: string;
  sdkVersion?: string;
  revenue?: number;
  groups?: string[];
}

/**
 * `/export/events` is the ONE list endpoint with pagination, and it is offset
 * based: `page` (1-based) plus `limit` (clamped 1..1000, default 50).
 *
 * There is no cursor anywhere in OpenPanel. Do not send one.
 */
export interface OpenPanelExportEventsPage {
  meta: {
    count: number;
    totalCount: number;
    pages: number;
    current: number;
  };
  data: OpenPanelEvent[];
}

export const OPENPANEL_EXPORT_LIMIT_MAX = 1000;
export const OPENPANEL_EXPORT_LIMIT_DEFAULT = 50;

/**
 * `/export/charts` — aggregated time series.
 *
 * Typed `unknown` on purpose. The endpoint exists and is always executed by the
 * server as `chartType: 'linear', metric: 'sum'`, but the research did not pin
 * down its response members. Inventing a type here would put field names in this
 * repo that the server does not have, which is worse than saying "probe this".
 */
export type OpenPanelChartResponse = unknown;

/* -------------------------------------------------------------------------- */
/* /insights/* — raw ClickHouse rows, snake_case                              */
/* -------------------------------------------------------------------------- */

/**
 * A raw ClickHouse event row from `/insights/:projectId/events`.
 *
 * snake_case throughout, and every date is `"YYYY-MM-DD HH:mm:ss"` with no `Z`.
 * Parse dates with {@link parseClickHouseDate} and nothing else.
 *
 * A session row from `/insights/:projectId/sessions` has the same convention.
 * Only the members confirmed in the research are named; the rest is an index
 * signature rather than a guess.
 */
export interface OpenPanelClickHouseRow {
  created_at?: string;
  profile_id?: string;
  session_id?: string;
  referrer_name?: string;
  os_version?: string;
  browser_version?: string;
  event_count?: number;
  is_bounce?: boolean;
  entry_path?: string;
  exit_origin?: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* /insights/* aggregates                                                     */
/* -------------------------------------------------------------------------- */

export type OpenPanelInterval = "hour" | "day" | "week" | "month";

export interface OpenPanelOverviewSummary {
  bounce_rate: number;
  unique_visitors: number;
  total_sessions: number;
  /** SECONDS. */
  avg_session_duration: number;
  total_screen_views: number;
  views_per_session: number;
  total_revenue: number;
}

export interface OpenPanelOverviewSeriesPoint extends OpenPanelOverviewSummary {
  /**
   * A real ISO-8601 string — the ONE place in `/insights/*` that is not a raw
   * ClickHouse timestamp. Parse with `new Date(...)`, not
   * {@link parseClickHouseDate}.
   */
  date: string;
}

export interface OpenPanelOverview {
  summary: OpenPanelOverviewSummary;
  series: OpenPanelOverviewSeriesPoint[];
  interval: OpenPanelInterval;
  startDate: string;
  endDate: string;
}

export interface OpenPanelFunnelStep {
  step: number;
  eventName: string;
  users: number;
  conversionRateFromStart: number;
  dropoffPercent: number | null;
  isHighestDropoff: boolean;
}

export interface OpenPanelFunnel {
  steps: OpenPanelFunnelStep[];
  totalUsers: number;
  completedUsers: number;
  /** Percentage to 2dp. */
  overallConversionRate: number;
}

export const OPENPANEL_FUNNEL_STEPS_MIN = 2;
export const OPENPANEL_FUNNEL_STEPS_MAX = 10;
export const OPENPANEL_FUNNEL_WINDOW_HOURS_MIN = 1;
export const OPENPANEL_FUNNEL_WINDOW_HOURS_MAX = 720;
export const OPENPANEL_FUNNEL_WINDOW_HOURS_DEFAULT = 24;

export interface OpenPanelFunnelQuery extends OpenPanelTimeParams {
  /** 2..10 event names, in funnel order. */
  steps: string[];
  /** 1..720, default 24. */
  windowHours?: number;
  groupBy?: "session_id" | "profile_id";
}

/**
 * Validate a funnel request.
 *
 * Both bounds are server-enforced and both are easy to exceed from a UI: a
 * ten-step funnel is the cap, and a 30-day window is a common "month" click that
 * is 6× over the 720-hour limit.
 */
export function assertOpenPanelFunnelQuery(query: OpenPanelFunnelQuery): void {
  const steps = query.steps ?? [];
  if (steps.length < OPENPANEL_FUNNEL_STEPS_MIN || steps.length > OPENPANEL_FUNNEL_STEPS_MAX) {
    throw new OpenPanelValidationError(
      `openpanel: funnel needs ${OPENPANEL_FUNNEL_STEPS_MIN}..${OPENPANEL_FUNNEL_STEPS_MAX} steps; got ${steps.length}.`,
    );
  }
  const hours = query.windowHours ?? OPENPANEL_FUNNEL_WINDOW_HOURS_DEFAULT;
  if (
    !Number.isFinite(hours) ||
    hours < OPENPANEL_FUNNEL_WINDOW_HOURS_MIN ||
    hours > OPENPANEL_FUNNEL_WINDOW_HOURS_MAX
  ) {
    throw new OpenPanelValidationError(
      `openpanel: funnel windowHours must be ` +
        `${OPENPANEL_FUNNEL_WINDOW_HOURS_MIN}..${OPENPANEL_FUNNEL_WINDOW_HOURS_MAX}; got ${hours}.`,
    );
  }
}

/** `retention` is a PERCENTAGE 0..100 float, not a 0..1 fraction. */
export interface OpenPanelRetentionPoint {
  date: string;
  active_users: number;
  retained_users: number;
  retention: number;
}

/** Twelve weekly cohorts. */
export interface OpenPanelRetentionCohort {
  cohort_interval: string;
  sum: number;
  values: number[];
  percentages: number[];
}

export type OpenPanelActiveUsersLabel = "DAU" | "WAU" | "MAU" | string;

export interface OpenPanelActiveUsers {
  window_days: number;
  label: OpenPanelActiveUsersLabel;
  series: Array<{ date: string; users: number }>;
}

export const OPENPANEL_ACTIVE_USERS_DAYS_MIN = 1;
export const OPENPANEL_ACTIVE_USERS_DAYS_MAX = 90;
export const OPENPANEL_ACTIVE_USERS_DAYS_DEFAULT = 7;

export interface OpenPanelEngagement {
  summary: {
    total_identified_users: number;
    active_last_7_days: number;
    active_8_to_14_days: number;
    active_15_to_30_days: number;
    inactive_31_to_60_days: number;
    churned_60_plus_days: number;
  };
  distribution: Array<{ days: number; users: number }>;
}

export interface OpenPanelPageRow {
  origin: string;
  path: string;
  sessions: number;
  pageviews: number;
  revenue?: number;
}

export interface OpenPanelPagesPerformance {
  total_pages: number;
  shown: number;
  pages: Array<{
    origin: string;
    path: string;
    title: string;
    sessions: number;
    pageviews: number;
    /**
     * MINUTES.
     *
     * NOT seconds, even though `overview.summary.avg_session_duration` on the
     * same product is seconds. Two fields with near-identical names in the same
     * API carry different units; this is the single most likely unit bug in the
     * whole adapter and the reason both are documented at their declarations.
     */
    avg_duration: number;
    bounce_rate: number;
    seo_signals: {
      high_bounce: boolean;
      low_engagement: boolean;
      good_landing_page: boolean;
    };
  }>;
}

export interface OpenPanelTrafficRow {
  prefix?: string;
  name: string;
  sessions: number;
  pageviews: number;
  revenue?: number;
}

export type OpenPanelTrafficBreakdown = "referrers" | "geo" | "devices";

export interface OpenPanelLive {
  visitors: number;
}

/**
 * `/insights/:projectId/user_flow` (Sankey) — SHAPE NOT VERIFIED.
 *
 * Exposed as `unknown` and nothing more. A Sankey node/link structure is the
 * obvious guess, but a guessed node/link schema that does not match renders a
 * blank diagram with no error, which is exactly the failure this adapter exists
 * to avoid. Probe a live instance before typing it.
 */
export type OpenPanelUserFlow = unknown;

/* -------------------------------------------------------------------------- */
/* /manage/*                                                                  */
/* -------------------------------------------------------------------------- */

/**
 * A project row from `/manage/projects` → `{data: Project[]}`.
 *
 * Raw Prisma rows. Meaningfully different from the ClickHouse rows: `createdAt`
 * and `updatedAt` here ARE ISO-8601 strings, so the same field name means
 * different things on the two sides of this product.
 *
 * Only `createdAt`/`updatedAt` are confirmed, so the rest is an index signature.
 */
export type OpenPanelProject = { createdAt?: string; updatedAt?: string } & Record<string, unknown>;

export interface OpenPanelManageProjects {
  data: OpenPanelProject[];
}

/**
 * Session replay: NOT MODELED, ON PURPOSE.
 *
 * There is no public read endpoint for replay. A `has_replay` flag exists in the
 * data layer but no REST route returns it, and replay is ingest-only. There is
 * nothing to read, so there is nothing to type — and a replay widget that can
 * never load is worse than no replay widget.
 */

/* -------------------------------------------------------------------------- */
/* Pagination model                                                           */
/* -------------------------------------------------------------------------- */

/**
 * OpenPanel has no cursor.
 *
 * Two different models, and mixing them up is a silent wrong-answer:
 *
 *  - `/export/*` list endpoints: offset pagination via `page` (1-based) + `limit`
 *    (clamped 1..1000, default 50). Only `/export/events` reports `meta`.
 *  - `/insights/*` list endpoints: NO PAGINATION AT ALL. `limit` only, max 100,
 *    default 20. There is no page, no offset, no cursor.
 *
 * `/insights/:projectId/pages` accepts `cursor` and `limit` and the handler
 * SILENTLY IGNORES BOTH. Do not rely on them; see
 * {@link OPENPANEL_IGNORED_INSIGHTS_PARAMS}.
 */
export const OPENPANEL_INSIGHTS_LIMIT_MAX = 100;
export const OPENPANEL_INSIGHTS_LIMIT_DEFAULT = 20;

/** Accepted by `/insights/:projectId/pages`, honoured by nothing. */
export const OPENPANEL_IGNORED_INSIGHTS_PARAMS: readonly string[] = ["cursor", "limit"];
