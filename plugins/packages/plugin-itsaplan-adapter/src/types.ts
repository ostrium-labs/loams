/**
 * It's a Plan 1.2.1 wire types.
 *
 * Verified against the It's a Plan monorepo at version `1.2.1`. Every route and
 * field name below was read out of that tree. Where the research did not confirm
 * a column's SQL type the field is OPTIONAL and the uncertainty is named, rather
 * than guessed at.
 *
 * LICENCE
 * -------
 * It's a Plan is **AGPL-3.0**. This adapter consumes its HTTP API as a client; it
 * does not vendor, link or redistribute any of its code, so there is no copyleft
 * obligation on this project. The obligation would attach to copying ItsAPlan
 * source, which nothing here does.
 *
 * NOT A WEB-ANALYTICS PRODUCT
 * ---------------------------
 * This is a self-hosted issue tracker / project-management tool — Linear/Jira
 * shaped — with AI agents as first-class participants. There is no traffic, no
 * pageviews, no referrers and no geography. What exists is DELIVERY analytics:
 * issue counts by state, throughput, burnup, activity, and agent/webhook health.
 * Nothing in this file pretends otherwise, and no method here has a web-analytics
 * analogue because none exists.
 *
 * THREE FACTS THAT SHAPE EVERY TYPE BELOW
 * ----------------------------------------
 * 1. NO SUCCESS ENVELOPE. Every route returns its DTO directly — `{...}` or
 *    `[...]`. The ONLY uniform shape in the whole API is the error,
 *    `{error: string, code?: string}`, so that is what the adapter branches on.
 * 2. PAGINATION IS PER-ROUTE, NOT PER-COLLECTION. Three mutually exclusive
 *    schemes coexist and which one applies is a property of the ROUTE:
 *    offset pages ({items,total,page,pageSize}), cursor feeds
 *    ({items,nextCursor}), and whole lists — a bare array with no pagination at
 *    all, which is where most analytics routes live. Each is typed separately
 *    below and none of them is a shared envelope.
 * 3. SERIES DENSITY IS NOT UNIFORM. `/analytics/pulse` is DENSE and complete
 *    (`generate_series` + left join); `/analytics/throughput` is SPARSE — a week
 *    with neither a creation nor a closure is simply ABSENT. The two carry a
 *    literal `density` tag so a widget cannot treat them alike.
 */

/* -------------------------------------------------------------------------- */
/* Protocol constants                                                           */
/* -------------------------------------------------------------------------- */

/** Requests per second, per API key. Budget for it on a dashboard refresh. */
export const ITSAPLAN_RATE_LIMIT_PER_SECOND = 100;

/** `POST /projects/:projectKey/charts` rejects more than this many rows. */
export const ITSAPLAN_CHART_MAX_ROWS = 500;

/**
 * Paths that are NOT part of the REST surface, named so nobody wires them up.
 *
 * `POST /api/auth/*` is better-auth and `POST /mcp` is MCP JSON-RPC; neither is
 * the REST API. A `Bearer` token exists but is MCP-only, and SCIM has its own
 * separate bearer — neither works for REST, so neither is modelled here. The
 * session-cookie path is deliberately not modelled either.
 */
export const ITSAPLAN_NON_REST_PATHS = ["/api/auth/*", "/mcp"] as const;

/**
 * Endpoints that do NOT exist, so that "missing" is a decision and not an
 * oversight.
 *
 * There is **no `GET /projects/:projectKey/dashboards/:dashboardId`** — the
 * dashboards collection is LIST ONLY. There is also no data-source API and no
 * server-side widget-type catalogue, so there is nothing to model for a
 * dashboard beyond its `layout` jsonb, which the server never inspects.
 */
export const ITSAPLAN_ABSENT_ENDPOINTS = [
  "GET /projects/:projectKey/dashboards/:dashboardId",
  "GET /dashboards/:dashboardId",
] as const;

/** `/me` — the cheapest possible check that a key resolves to a session. */
export interface ItsAPlanMe {
  authenticated: boolean;
  user?: Record<string, unknown>;
}

/** `GET /` — the server's own liveness answer. */
export interface ItsAPlanHealth {
  name: string;
  status: string;
}

/* -------------------------------------------------------------------------- */
/* Errors                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * The one uniform error shape.
 *
 * Note there is NO `status` field: the HTTP status and this body are separate,
 * and the 500 path returns a GENERIC message, so the body text of a 500 must
 * never be parsed or shown as if it were the cause.
 */
export interface ItsAPlanErrorBody {
  error: string;
  /** Machine code on a few cases. Often absent. */
  code?: string;
}

/* -------------------------------------------------------------------------- */
/* Pagination — three schemes, none of them shared                              */
/* -------------------------------------------------------------------------- */

/**
 * Scheme 1 — offset pages. `page` is 1-based (default 1) and `pageSize` is 1–100
 * (default 25).
 */
export interface ItsAPlanOffsetPage<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
}

/**
 * The keyset cursor for `/analytics/activity`, which paginates on
 * `(created_at, id)`.
 *
 * `ts` is the `timestamptz` of the last row and `id` is that row's id. `id`'s
 * SQL type was not confirmed, so it is `string | number` and must be fed back
 * OPAQUE — never parsed, never compared, never coerced.
 */
export interface ItsAPlanActivityCursor {
  ts: string;
  id: string | number;
}

/**
 * Scheme 2 — cursor feeds. `limit` + `cursor`, used where rows arrive while you
 * page. `nextCursor` is null on the last page.
 */
export interface ItsAPlanCursorPage<T> {
  items: T[];
  nextCursor: ItsAPlanActivityCursor | null;
}

/**
 * Scheme 3 — whole list. A bare array, NO pagination.
 *
 * This is the bucket most analytics routes are in, and it is the reason the
 * adapter must never assume a shared envelope: `stats` is an object,
 * `breakdown` is a bare array, `pulse` is a bare array, and `issues` is a bare
 * array with a `limit` parameter that silently truncates rather than paging.
 */
export type ItsAPlanWholeList<T> = T[];

/* -------------------------------------------------------------------------- */
/* Analytics — issue counts and state                                           */
/* -------------------------------------------------------------------------- */

/**
 * `GET /projects/:projectKey/analytics/stats`. No parameters.
 *
 * TWO OVERWRITES, NOT TWO SUMS:
 * - `open` is DERIVED as `stateType NOT IN ('completed','canceled')`.
 * - `inProgress` and `backlog` are OVERWRITTEN, not accumulated. A custom state
 *   type contributes to `open` and to NEITHER of these.
 *
 * So `open` can exceed `inProgress + backlog`, and the gap is the custom states.
 */
export interface ItsAPlanStats {
  open: number;
  inProgress: number;
  backlog: number;
  overdue: number;
  unassigned: number;
  closedLast7d: number;
}

/** `by=` for `/analytics/breakdown`. Required — there is no default dimension. */
export type ItsAPlanBreakdownDimension = "status" | "priority" | "type" | "assignee" | "delegate";

/**
 * One `GET /projects/:projectKey/analytics/breakdown?by=` row, returned as a BARE
 * ARRAY with no envelope.
 *
 * TRAP: `key` is a STRINGIFIED id — `${columnId}::text` — not a number, and it
 * carries `'none'` sentinels for unassigned/unlabelled values. Anything that
 * does `Number(key)` produces `NaN` and a silently empty series.
 *
 * TRAP: for `by=priority`, `color` is ALWAYS null, and the labels come from a
 * hardcoded `urgent | high | medium | low | none` map rather than from data.
 */
export interface ItsAPlanBreakdownItem {
  /** Stringified `${columnId}::text`, or a `'none'` sentinel. NEVER a number. */
  key: string;
  label: string;
  count: number;
  color: string | null;
}

/* -------------------------------------------------------------------------- */
/* Analytics — time series                                                      */
/* -------------------------------------------------------------------------- */

/** `unit=` for `/analytics/pulse`. Default `day`. */
export type ItsAPlanPulseUnit = "hour" | "day" | "week";

/** `scope=` for `/analytics/pulse`. */
export type ItsAPlanPulseScope = "project" | "me";

/** One bucket of a pulse series. `label` is a PREFORMATTED string for the unit. */
export interface ItsAPlanPulsePoint {
  label: string;
  count: number;
}

/**
 * `GET /projects/:projectKey/analytics/pulse?scope=&unit=&columns=`.
 *
 * DENSE. The axis comes from `generate_series` + a left join, so there are NO
 * MISSING BUCKETS: every label in the window is present, including the zeros.
 * Render the returned ORDER directly; the server states that the client should
 * do no date maths and no timezone reconciliation, because a bucket label is
 * already formatted for the unit.
 *
 * `columns` is clamped server-side per unit (hour 140 / day 160 / week 130,
 * default 26) — asking for more is silently clamped, not rejected.
 */
export interface ItsAPlanPulseSeries {
  density: "dense";
  scope: ItsAPlanPulseScope;
  unit: ItsAPlanPulseUnit;
  points: ItsAPlanPulsePoint[];
}

/** One week of a throughput series. `week` is the `YYYY-MM-DD` of the MONDAY. */
export interface ItsAPlanThroughputWeek {
  week: string;
  created: number;
  closed: number;
}

/**
 * `GET /projects/:projectKey/analytics/throughput?weeks=`.
 *
 * SPARSE, and deliberately unlike {@link ItsAPlanPulseSeries}. A week with
 * NEITHER a creation nor a closure is ABSENT from the response. Absence means
 * "nothing happened", NOT "the week does not exist" and NOT a rendering gap to
 * be interpolated — and it must never be silently filled with zero without the
 * caller deciding to, because a zero and a missing bucket are different claims.
 *
 * `weeks` is 1–52, default 12.
 */
export interface ItsAPlanThroughputSeries {
  density: "sparse";
  weeks: ItsAPlanThroughputWeek[];
}

/** Either series type, discriminated by `density`. */
export type ItsAPlanSeries = ItsAPlanPulseSeries | ItsAPlanThroughputSeries;

/** Narrow a series to the dense one. */
export function isDenseSeries(series: ItsAPlanSeries): series is ItsAPlanPulseSeries {
  return series.density === "dense";
}

/** Narrow a series to the sparse one. */
export function isSparseSeries(series: ItsAPlanSeries): series is ItsAPlanThroughputSeries {
  return series.density === "sparse";
}

/* -------------------------------------------------------------------------- */
/* Analytics — burnup                                                           */
/* -------------------------------------------------------------------------- */

/** One day of the burnup curve. `date` is already `YYYY-MM-DD`. */
export interface ItsAPlanBurnupDay {
  date: string;
  scope: number;
  started: number;
  completed: number;
}

/**
 * The burnup FORECAST, which is a RANGE.
 *
 * THREE projected dates, not one: {@link ItsAPlanBurnupForecast.projectedDate}
 * plus the optimistic/pessimistic PAIR. Every date field is nullable, and
 * `projectedDate` is null when nothing was closed inside the window OR when
 * nothing remains — in which case the optimistic and pessimistic dates are null
 * WITH it. That is why they are read together through
 * `burnupProjectedDates` rather than one at a time.
 */
export interface ItsAPlanBurnupForecast {
  windowDays: number | null;
  velocityPerDay: number | null;
  scopeGrowthPerDay: number | null;
  remaining: number | null;
  projectedScope: number | null;
  projectedDate: string | null;
  optimisticDate: string | null;
  pessimisticDate: string | null;
}

/**
 * `GET /projects/:projectKey/analytics/burnup?days=&initiativeId=&forecastWeeks=`.
 *
 * `days` is 7–730 (default 90) and `forecastWeeks` is 1–12 (default 4).
 */
export interface ItsAPlanBurnup {
  days: ItsAPlanBurnupDay[];
  forecast: ItsAPlanBurnupForecast;
  targetDate: string | null;
}

/** The three projected dates, or `null` when the forecast made none. */
export interface ItsAPlanProjectedDates {
  projectedDate: string;
  optimisticDate: string | null;
  pessimisticDate: string | null;
}

/**
 * Read a burnup forecast's three projected dates TOGETHER.
 *
 * Returns `null` when `projectedDate` is null — which the server does when
 * nothing closed inside the window or nothing remains — and the optimistic and
 * pessimistic dates are null with it. Reading the three independently is how a
 * dashboard ends up rendering an "optimistic" date the server declined to
 * produce.
 */
export function burnupProjectedDates(burnup: ItsAPlanBurnup): ItsAPlanProjectedDates | null {
  const { projectedDate, optimisticDate, pessimisticDate } = burnup.forecast;
  if (projectedDate === null || projectedDate === undefined) return null;
  return { projectedDate, optimisticDate, pessimisticDate };
}

/**
 * How many runs `total` counted that landed in NONE of the three buckets.
 *
 * This exists because the four numbers are routinely presented as if they summed,
 * and they do not: `total` accumulates while `success`/`failed`/`pending`
 * overwrite, so a run with a status outside those three is counted in `total` and
 * in no bucket. Returning the excess as a number makes the non-partition
 * visible instead of leaving a chart to imply one that does not exist. Clamped at
 * zero because a negative difference would only mean the buckets overwrote each
 * other, which is not a shape a chart can draw either.
 */
export function unbucketedAgentRuns(stats: ItsAPlanAgentRunStats): number {
  return Math.max(0, stats.total - (stats.success + stats.failed + stats.pending));
}

/** As {@link unbucketedAgentRuns}, for webhook deliveries. */
export function unbucketedWebhookDeliveries(stats: ItsAPlanWebhookStats): number {
  return Math.max(0, stats.total - (stats.success + stats.failed + stats.pending));
}

/* -------------------------------------------------------------------------- */
/* Analytics — activity                                                         */
/* -------------------------------------------------------------------------- */

/** `payload` on an activity item: what changed, from what, to what. */
export interface ItsAPlanActivityPayload {
  subject?: string;
  from?: string | null;
  to?: string | null;
}

/**
 * One item of `GET /projects/:projectKey/analytics/activity`.
 *
 * Newest first, keyset-paginated on `(created_at, id)`.
 */
export interface ItsAPlanActivityItem {
  id: string;
  issueId: string;
  issueSequence: number;
  issueTitle: string;
  /** Event kind. The full closed set was not enumerated; left as a string. */
  kind: string;
  actorUserId: string | null;
  actorName: string;
  body: string | null;
  action: string;
  payload: ItsAPlanActivityPayload;
  /** `timestamptz`, ISO-8601. */
  createdAt: string;
}

/* -------------------------------------------------------------------------- */
/* Analytics — agents and webhooks                                              */
/* -------------------------------------------------------------------------- */

/** A CLOSED union. Anything else is not a trigger this server can emit. */
export type ItsAPlanAgentRunTrigger = "mention" | "delegation" | "field" | "schedule" | "manual";

/** `status=` filter for `/analytics/agent-runs`. */
export type ItsAPlanAgentRunStatus = "pending" | "success" | "failed";

/** One row of `GET /projects/:projectKey/analytics/agent-runs`, a bare array. */
export interface ItsAPlanAgentRun {
  id: string;
  status: ItsAPlanAgentRunStatus;
  trigger: ItsAPlanAgentRunTrigger;
  agentId: string;
  agentName: string;
  /** Null when the run is not attached to an issue. */
  issueId: string | null;
  issueSequence: number | null;
  lastError: string | null;
  createdAt: string;
}

/**
 * `GET /projects/:projectKey/analytics/agent-run-stats?days=`.
 *
 * **THE THREE BUCKETS DO NOT PARTITION `total`.** `total` accumulates, but
 * `success`, `failed` and `pending` are OVERWRITTEN: a run whose status is
 * outside those three increments `total` and NO bucket. So `total` can EXCEED
 * `success + failed + pending`, and presenting these four numbers as if they
 * summed would be a lie. `unbucketedAgentRuns` reports the difference rather than
 * hiding it.
 *
 * `days` is 1–90, default 30.
 */
export interface ItsAPlanAgentRunStats {
  total: number;
  success: number;
  failed: number;
  pending: number;
}

/**
 * `GET /projects/:projectKey/analytics/webhook-stats?days=`.
 *
 * Same overwrite caveat as {@link ItsAPlanAgentRunStats}, plus the webhook
 * inventory.
 */
export interface ItsAPlanWebhookStats {
  total: number;
  success: number;
  failed: number;
  pending: number;
  activeWebhooks: number;
  disabledWebhooks: number;
}

/**
 * One row of `GET /projects/:projectKey/analytics/agent-workload`, a bare array
 * sorted by `delegatedOpen` desc, then `runsTotal` desc, then name.
 *
 * TRAP: `agentName` is the agent's USERNAME, not a display name.
 */
export interface ItsAPlanAgentWorkload {
  agentId: string;
  /** The agent's `username`. NOT a display name. */
  agentName: string;
  kind: string;
  delegatedOpen: number;
  runsTotal: number;
  runsSuccess: number;
  runsFailed: number;
}

/* -------------------------------------------------------------------------- */
/* Issues                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * An issue from `GET /projects/:projectKey/issues`.
 *
 * The route returns a BARE ARRAY with NO pagination — `limit` (1–500, default 50)
 * silently TRUNCATES rather than paging, so a caller asking for 500 of 800 gets
 * 500 and no signal that more exist.
 *
 * CSV filters (`labelIds` and friends) are AND-semantics.
 *
 * The id columns' SQL types were not confirmed by the research, so they are
 * modelled as `string` and MUST be treated as opaque. `identifier` is the human
 * form (`"MKT-42"`); `sequenceNumber` is its numeric half.
 */
export interface ItsAPlanIssue {
  id: string;
  sequenceNumber: number;
  /** The human identifier, e.g. `"MKT-42"`. */
  identifier: string;
  title: string;
  columnId: string;
  typeId: string;
  /** Optional: an issue need not belong to an initiative, cycle or parent. */
  initiativeId?: string | null;
  cycleId?: string | null;
  parentId?: string | null;
  assigneeUserId?: string | null;
  delegateUserId?: string | null;
  /** Optional. The `by=priority` breakdown sends a `'none'` sentinel. */
  priority?: string | null;
  dueDate?: string | null;
  labelIds: string[];
  archived: boolean;
}

/** Filters for `GET /projects/:projectKey/issues`. CSV fields are AND-semantics. */
export interface ItsAPlanIssueFilters {
  /** CSV. */
  columnId?: string;
  /** CSV. */
  typeId?: string;
  /** CSV. */
  initiativeId?: string;
  /** CSV. */
  cycleId?: string;
  /** CSV. */
  parentId?: string;
  assigneeUserId?: string;
  delegateUserId?: string;
  priority?: string;
  /** CSV. */
  labelIds?: string;
  /** `YYYY-MM-DD`. */
  dueFrom?: string;
  /** `YYYY-MM-DD`. */
  dueTo?: string;
  /** The literal string `'true'`. */
  includeArchived?: "true";
  /** 1–500, default 50. TRUNCATES; does not page. */
  limit?: number;
}

/* -------------------------------------------------------------------------- */
/* Dashboards                                                                   */
/* -------------------------------------------------------------------------- */

/**
 * One dashboard from `GET /projects/:projectKey/dashboards` — LIST ONLY.
 *
 * There is no `GET /dashboards/:dashboardId`, so this is the only way to read a
 * dashboard.
 *
 * **`layout` is `unknown` ON PURPOSE.** It is `t.Any()` — untyped jsonb that the
 * server never inspects and never validates. Its shape is only interpretable
 * against the FRONTEND's `WidgetInstance` (`id, type, x, y, w, h, title?,
 * config?`, with widget types `stat | recent_issues | activity_feed | pulse |
 * throughput | burnup | breakdown | agent_runs | agent_health | webhook_health |
 * agent_workload`). That catalogue is FRONTEND-ONLY and is NOT part of the API
 * contract, so it is described in this comment and not encoded as a type: a
 * dashboard written by a newer frontend may contain widgets this version's
 * frontend cannot render, and typing them would assert a contract the server
 * does not make.
 *
 * There is also no data-source API: a widget's numbers come from the analytics
 * routes above, resolved client-side. Nothing to model server-side.
 */
export interface ItsAPlanDashboard {
  id: string;
  projectId: string;
  name: string;
  /** Optional: the icon is nullable on the server and was not type-confirmed. */
  icon?: string | null;
  /** Untyped jsonb. See the note above. */
  layout: unknown;
  /** Optional: the research confirmed the NAME but not the column type. */
  position?: number;
  /** `timestamptz`, ISO-8601. */
  createdAt: string;
}

/* -------------------------------------------------------------------------- */
/* Chart specs                                                                   */
/* -------------------------------------------------------------------------- */

/** `type` on a chart spec. A closed union. */
export type ItsAPlanChartType =
  | "bar"
  | "line"
  | "area"
  | "pie"
  | "radial"
  | "scatter"
  | "funnel"
  | "treemap";

/** One series of a chart spec. `type` is an optional per-series override. */
export interface ItsAPlanChartSeries {
  key: string;
  label?: string;
  color?: string;
  type?: string;
}

/**
 * A chart spec.
 *
 * `POST /projects/:projectKey/charts` VALIDATES a spec and ECHOES IT BACK.
 * Nothing is stored. It is POST only because it takes a body, and it is
 * genuinely read-only and idempotent — which makes it useful as a server-side
 * validator for a spec built client-side.
 *
 * `data` is capped at {@link ITSAPLAN_CHART_MAX_ROWS} rows.
 */
export interface ItsAPlanChartSpec {
  type: ItsAPlanChartType;
  title?: string;
  x: string;
  series: ItsAPlanChartSeries[];
  /** At most {@link ITSAPLAN_CHART_MAX_ROWS} rows. */
  data: Array<Record<string, unknown>>;
  stacked?: boolean;
  horizontal?: boolean;
  curve?: boolean;
  showValues?: boolean;
}

/* -------------------------------------------------------------------------- */
/* Config                                                                       */
/* -------------------------------------------------------------------------- */

export interface ItsAPlanConfig {
  /** The API server root, e.g. `https://plan.example.com`. No `/api/v1`. */
  baseUrl: string;
  /**
   * An `itp_…` API key, sent as `x-api-key`.
   *
   * PROVISIONING IS MANUAL AND THE EXPIRY IS FIXED AT CREATION: keys are created
   * via `POST /api/auth/api-key/create` with a SIGNED-IN SESSION, are shown ONCE,
   * and an API key can neither mint another key nor change its own expiry. So a
   * key must be ROTATED, not extended.
   *
   * A key carries the same permissions as its owner. better-auth's apiKey plugin
   * runs with `enableSessionForAPIKeys: true`, so the key resolves to its owner's
   * session and satisfies every guard with no special-casing.
   */
  apiKey: string;
  timeoutMs?: number;
}
