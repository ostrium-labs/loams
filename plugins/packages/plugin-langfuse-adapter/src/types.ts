/**
 * Langfuse 4.50.0 public-API types.
 *
 * SURFACE SELECTION IS A HARD CONSTRAINT HERE, NOT A PREFERENCE
 * -----------------------------------------------------------
 * Langfuse v3 is deprecated and the legacy public surface is REMOVED on
 * 2026-11-16. As of 2026-10-03 that is six weeks out. The removed endpoints
 * (`/traces`, `/sessions`, legacy `/observations`, `/metrics`) carry an explicit
 * marker in their own responses: `"Langfuse v3 is deprecated… removed on
 * November 16, 2026"`. A dashboard that quietly keeps reading `/traces` will work
 * today and return an error page on that date, which is the worst possible
 * failure mode for a BI tool.
 *
 * So this module only models the three surviving analytics endpoints:
 *
 *   GET /api/public/v2/observations   real-time observation read path
 *   GET /api/public/v2/metrics       the aggregate/analytics endpoint
 *   GET /api/public/v3/scores        score read path
 *
 * plus the supporting catalogue endpoints. `DEPRECATED_LANGFUSE_PATHS` below is
 * not documentation: `assertSupportedLangfusePath` throws on it, so a
 * reintroduced legacy call fails in our tests rather than in production.
 *
 * Provenance: `web/public/generated/api/openapi.yml` in the Langfuse tree
 * (18k lines, all 79 `/api/public/*` paths) is the in-tree contract this module
 * was written against. Against a live instance, `GET /api/public/openapi.json`
 * is the drift check — if a field below no longer exists there, the checkout
 * moved and this file is stale.
 */

/** Base path every public Langfuse endpoint hangs off. */
export const LANGFUSE_API_PREFIX = "/api/public";

/** Env var prefix for this adapter's configuration. */
export const LANGFUSE_ENV_PREFIX = "LANGFUSE";

export interface LangfuseConfig {
  /**
   * Host root, e.g. `https://cloud.langfuse.com`. `/api/public` is appended by
   * the adapter — configure the host, never the API path, or every request 404s.
   */
  baseUrl: string;
  /**
   * Public key. Sent as the HTTP Basic USERNAME. Modern keys are `pk-lf-…`.
   *
   * Legacy keys without that prefix are accepted: the server migrated them
   * server-side, so rejecting a working credential client-side would be a bug,
   * not caution.
   */
  publicKey: string;
  /** Secret key. Sent as the HTTP Basic PASSWORD. Modern keys are `sk-lf-…`. */
  secretKey: string;
  /** Per-request timeout in ms. Defaults to 30s. */
  timeoutMs?: number;
}

/* -------------------------------------------------------------------------- */
/* Auth                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * Why Basic and not Bearer.
 *
 * Both work for a secret key: `Authorization: Bearer <sk-lf-…>` grants full read
 * access. But `Authorization: Bearer <pk-lf-…>` is deliberately restricted to a
 * scores-only presentation — the same key material yields a materially different
 * capability set depending on which header carries it. Basic has no such
 * ambiguity: username is always the public key, password is always the secret
 * key, so there is no encoding in which the two get swapped.
 *
 * `Authorization: Basic base64("<publicKey>:<secretKey>")`
 */
export function langfuseBasicAuth(
  publicKey: string,
  secretKey: string,
): {
  kind: "basic";
  username: string;
  password: string;
} {
  return { kind: "basic", username: publicKey, password: secretKey };
}

/* -------------------------------------------------------------------------- */
/* Health                                                                     */
/* -------------------------------------------------------------------------- */

export interface LangfuseHealth {
  version: string;
  status: string;
}

/* -------------------------------------------------------------------------- */
/* Envelopes                                                                  */
/* -------------------------------------------------------------------------- */

/** Offset/limit page metadata. `limit` is 1..100, default 50. */
export interface LangfusePageMeta {
  page: number;
  limit: number;
  totalItems: number;
  totalPages: number;
}

/**
 * Cursor page metadata, used by `/v2/observations` and `/v3/scores`.
 *
 * The cursor is base64 of `{lastStartTimeTo, lastTraceId, lastId}` — an opaque
 * token. It is NOT decoded here and must not be constructed by hand; a
 * hand-built cursor silently returns page 1 again and a pagination loop never
 * terminates.
 */
export interface LangfuseCursorMeta {
  cursor: string;
}

/**
 * The `{data, meta}` envelope used by every non-metrics endpoint.
 *
 * Deprecated-but-still-live responses also carry a top-level `_deprecation`
 * object. It is absent from this type on purpose: the field is stripped by
 * `stripLangfuseDeprecation` before parsing, and a closed type that rejected it
 * would make the adapter fail on a successful response.
 */
export interface LangfusePage<T> {
  data: T[];
  meta: LangfusePageMeta | LangfuseCursorMeta;
}

/** Read the cursor out of a page's meta, or `undefined` when the page is last. */
export function cursorOf(
  meta: LangfusePageMeta | LangfuseCursorMeta | undefined,
): string | undefined {
  if (!meta) return undefined;
  const cursor = (meta as LangfuseCursorMeta).cursor;
  return typeof cursor === "string" && cursor.length > 0 ? cursor : undefined;
}

/**
 * `/v2/metrics` returns `{data: [...]}` with NO `meta` and NO pagination.
 *
 * That asymmetry is easy to forget and produces an "expected meta.page" crash
 * the first time a metric query runs, so it gets its own type.
 */
export interface LangfuseMetricsResponse {
  data: Array<Record<string, unknown>>;
}

/**
 * Split a response body's `_deprecation` marker off, returning the rest intact.
 *
 * The marker is informational. The requirement is only that its presence cannot
 * break parsing — so this returns the whole envelope unchanged alongside the
 * notice, and the service logs the notice once per path rather than letting it
 * reach chart code.
 */
export function stripLangfuseDeprecation<T extends { _deprecation?: unknown }>(
  body: T,
): { payload: Omit<T, "_deprecation">; deprecation: unknown } {
  const { _deprecation, ...payload } = body;
  return { payload: payload as Omit<T, "_deprecation">, deprecation: _deprecation };
}

/* -------------------------------------------------------------------------- */
/* Deprecated surface — refused, not merely unused                            */
/* -------------------------------------------------------------------------- */

/**
 * Legacy first-path segments under `/api/public` that no longer survive.
 *
 * Matched as a SEGMENT, not a substring: `/api/public/v2/observations` is the
 * supported endpoint and must pass, while `/api/public/observations` is the
 * removed legacy one and must not. A plain `includes("/observations")` cannot
 * tell those apart, which is exactly the confusion this function exists to
 * prevent.
 */
export const DEPRECATED_LANGFUSE_SEGMENTS: readonly string[] = [
  "traces",
  "sessions",
  "observations",
  "metrics",
];

/** Removal date for the legacy public surface. */
export const LANGFUSE_LEGACY_REMOVED_ON = "2026-11-16";

/**
 * The one legacy-prefixed path this adapter still allows, because it is a
 * different problem: present in Langfuse's source but absent from its OpenAPI.
 * See {@link LangfuseMetricsDaily}.
 */
const LANGFUSE_KNOWN_UNSTABLE_PATHS: readonly string[] = [`${LANGFUSE_API_PREFIX}/metrics/daily`];

/**
 * Throw if `path` targets a removed legacy surface.
 *
 * Versioned paths (`/v2/...`, `/v3/...`) pass; an unversioned `traces`,
 * `sessions`, `observations` or `metrics` does not. This runs on every request
 * so that a reintroduced legacy call is caught by the adapter's own tests
 * rather than by a user's dashboard on 2026-11-16.
 */
export function assertSupportedLangfusePath(path: string): void {
  if (LANGFUSE_KNOWN_UNSTABLE_PATHS.includes(path)) return;

  const withoutPrefix = path.startsWith(LANGFUSE_API_PREFIX)
    ? path.slice(LANGFUSE_API_PREFIX.length)
    : path;
  const [first] = withoutPrefix.split("/").filter(Boolean);
  if (!first) return;
  // A versioned path starts `v<digits>`; anything versioned is a supported API.
  if (/^v\d+$/.test(first)) return;

  if (DEPRECATED_LANGFUSE_SEGMENTS.includes(first)) {
    throw new Error(
      `langfuse: ${path} targets the deprecated Langfuse v3 public API, removed on ` +
        `${LANGFUSE_LEGACY_REMOVED_ON}. Use ${LANGFUSE_API_PREFIX}/v2/observations, ` +
        `${LANGFUSE_API_PREFIX}/v2/metrics or ${LANGFUSE_API_PREFIX}/v3/scores.`,
    );
  }
}

/* -------------------------------------------------------------------------- */
/* v2/metrics                                                                */
/* -------------------------------------------------------------------------- */

export type LangfuseAggregation =
  | "sum"
  | "avg"
  | "count"
  | "max"
  | "min"
  | "p50"
  | "p75"
  | "p90"
  | "p95"
  | "p99"
  | "histogram";

export const LANGFUSE_AGGREGATIONS: readonly LangfuseAggregation[] = [
  "sum",
  "avg",
  "count",
  "max",
  "min",
  "p50",
  "p75",
  "p90",
  "p95",
  "p99",
  "histogram",
];

/**
 * The four result sets `view` can select.
 *
 * There is deliberately NO `traces` view: metrics v2 aggregates the same
 * observation rows the observations endpoint returns, and grouping by trace is
 * expressed as a `traceId` dimension rather than a separate view. Looking for
 * `view: "traces"` and finding nothing is the expected outcome.
 */
export type LangfuseMetricsView =
  | "observations"
  | "scores-numeric"
  | "scores-boolean"
  | "scores-categorical";

export const LANGFUSE_METRICS_VIEWS: readonly LangfuseMetricsView[] = [
  "observations",
  "scores-numeric",
  "scores-boolean",
  "scores-categorical",
];

export type LangfuseTimeGranularity = "auto" | "minute" | "hour" | "day" | "week" | "month";

export const LANGFUSE_TIME_GRANULARITIES: readonly LangfuseTimeGranularity[] = [
  "auto",
  "minute",
  "hour",
  "day",
  "week",
  "month",
];

export interface LangfuseMetricsConfig {
  /** Histogram bin count, 1..100. Only meaningful for `histogram` aggregations. */
  bins?: number;
  /** Max rows returned, 1..1000. */
  row_limit?: number;
}

export interface LangfuseTimeDimension {
  granularity?: LangfuseTimeGranularity;
}

/**
 * One measure plus the aggregation applied to it.
 *
 * `metrics[]` on the wire accepts bare measure names, so `LangfuseMetricInput`
 * allows the shorthand. The adapter normalizes to this shape, and the column
 * helper below works only on the normalized shape — which is what makes the
 * helper a pure function of what was actually sent.
 */
export interface LangfuseMetric {
  /** Measure name, e.g. `latency`, `totalCost`, `count`. */
  field: string;
  /** Applied aggregation. Defaults to `count` for the bare-string shorthand. */
  aggregation?: LangfuseAggregation;
}

export type LangfuseMetricInput = string | LangfuseMetric;

/** Normalize `metrics[]` entries, applying the documented `count` default. */
export function normalizeLangfuseMetric(input: LangfuseMetricInput): Required<LangfuseMetric> {
  if (typeof input === "string") return { field: input, aggregation: "count" };
  return { field: input.field, aggregation: input.aggregation ?? "count" };
}

/**
 * `orderBy[]` entries and `filters[]` entries are passed through verbatim.
 *
 * The research enumerated their existence and their position in the `query`
 * blob but not their member shape. Modelling them as invented interfaces would
 * put field names in this repo that the server does not have, so they stay
 * opaque. A caller who has read the OpenAPI can send any object.
 */
export type LangfuseOrderBy = string | { field: string; direction?: "asc" | "desc" };
export type LangfuseMetricFilter = string | Record<string, unknown>;

export interface LangfuseMetricsQuery {
  view: LangfuseMetricsView;
  /** Required. Measures to aggregate. */
  metrics: LangfuseMetricInput[];
  /** Strict ISO-8601 WITH offset, e.g. `2026-01-01T00:00:00Z`. */
  fromTimestamp: string;
  /** Strict ISO-8601 WITH offset. Must be strictly after `fromTimestamp`. */
  toTimestamp: string;
  dimensions?: string[];
  filters?: LangfuseMetricFilter[];
  timeDimension?: LangfuseTimeDimension;
  orderBy?: LangfuseOrderBy[];
  config?: LangfuseMetricsConfig;
}

/**
 * The time-bucket column is literally `time_dimension`.
 *
 * It is not named after the granularity, the field, or the dimension. This is
 * the single most-repeated surprise in the metrics API and the reason the
 * constant is exported: a chart that hardcodes `hour` renders blank.
 */
export const LANGFUSE_TIME_COLUMN = "time_dimension";

export interface LangfuseMetricColumn {
  /** The key as it appears in a result row. */
  key: string;
  kind: "time" | "dimension" | "result";
  /** Set for `kind: "result"`. */
  aggregation?: LangfuseAggregation;
  /** Set for `kind: "result"`. */
  measure?: string;
  /** Set for `kind: "dimension"`. */
  dimension?: string;
  /**
   * True when the cell is a `[lower, upper, height]` tuple rather than a scalar.
   *
   * A histogram cell read as a number yields `NaN`, which a chart renders as a
   * gap with no error anywhere. Consumers must branch on this flag.
   */
  histogram: boolean;
}

/**
 * Reconstruct the column keys a metrics query will return.
 *
 * THE HIGHEST-RISK ITEM IN THIS ADAPTER
 * ------------------------------------
 * Result columns are named `{aggregation}_{measure}` — `p95_latency`,
 * `sum_totalCost`, `count_count`. They are NOT named after the measure. Dimension
 * columns are keyed by the dimension field name alone (`name`, `type`,
 * `environment`, …), with no prefix at all, and the time bucket is the literal
 * `time_dimension`.
 *
 * That gives three different naming rules inside one flat record. A consumer that
 * assumes a column is named after its measure renders a blank chart with no
 * error: the response parses fine, the field is simply `undefined`. Deriving the
 * expected keys from the query we sent is the only way to catch that at
 * development time rather than in a screenshot.
 *
 * Order matches the server's grouping order: time bucket, then dimensions in the
 * order requested, then results in the order requested.
 */
export function expectedLangfuseMetricColumns(query: LangfuseMetricsQuery): LangfuseMetricColumn[] {
  const columns: LangfuseMetricColumn[] = [];

  if (query.timeDimension) {
    columns.push({ key: LANGFUSE_TIME_COLUMN, kind: "time", histogram: false });
  }
  for (const dimension of query.dimensions ?? []) {
    columns.push({ key: dimension, kind: "dimension", dimension, histogram: false });
  }
  for (const input of query.metrics ?? []) {
    const metric = normalizeLangfuseMetric(input);
    columns.push({
      key: `${metric.aggregation}_${metric.field}`,
      kind: "result",
      aggregation: metric.aggregation,
      measure: metric.field,
      histogram: metric.aggregation === "histogram",
    });
  }

  return columns;
}

/** Convenience view for chart code: the scalar, non-time, non-histogram keys. */
export function langfuseNumericColumns(query: LangfuseMetricsQuery): string[] {
  return expectedLangfuseMetricColumns(query)
    .filter((column) => column.kind === "result" && !column.histogram)
    .map((column) => column.key);
}

/**
 * Validate a metrics query and render it as the SINGLE urlencoded `query` param.
 *
 * `/v2/metrics` does not take flattened params. It takes one parameter, `query`,
 * whose value is a JSON object — so the JSON has to survive a round trip through
 * `URLSearchParams` intact. `buildQuery` handles that; this function's job is to
 * make sure the object is one the server will accept, because a rejected blob
 * comes back as an opaque 400 that names none of the offending fields.
 *
 * Validated here: view membership, non-empty metrics, timestamp strictness and
 * ordering, granularity membership, `bins` 1..100, `row_limit` 1..1000.
 */
export function buildLangfuseMetricsParams(query: LangfuseMetricsQuery): { query: string } {
  if (!LANGFUSE_METRICS_VIEWS.includes(query.view)) {
    throw new LangfuseValidationError(
      `langfuse: view="${query.view}" is not one of ${LANGFUSE_METRICS_VIEWS.join(", ")}. ` +
        `There is no "traces" view — aggregate the observations view and group by traceId.`,
    );
  }
  const metrics = query.metrics ?? [];
  if (metrics.length === 0) {
    throw new LangfuseValidationError("langfuse: metrics[] is required and must be non-empty.");
  }
  for (const metric of metrics) {
    const normalized = normalizeLangfuseMetric(metric);
    if (!LANGFUSE_AGGREGATIONS.includes(normalized.aggregation)) {
      throw new LangfuseValidationError(
        `langfuse: aggregation "${normalized.aggregation}" for measure "${normalized.field}" is not ` +
          `one of ${LANGFUSE_AGGREGATIONS.join(", ")}.`,
      );
    }
  }
  if (query.timeDimension?.granularity !== undefined) {
    if (!LANGFUSE_TIME_GRANULARITIES.includes(query.timeDimension.granularity)) {
      throw new LangfuseValidationError(
        `langfuse: timeDimension.granularity "${query.timeDimension.granularity}" is not one of ` +
          `${LANGFUSE_TIME_GRANULARITIES.join(", ")}.`,
      );
    }
  }
  assertRange("bins", query.config?.bins, 1, 100);
  assertRange("row_limit", query.config?.row_limit, 1, 1000);
  assertLangfuseMetricsWindow(query);

  return { query: JSON.stringify(query) };
}

function assertRange(field: string, value: number | undefined, min: number, max: number): void {
  if (value === undefined) return;
  if (!Number.isFinite(value) || value < min || value > max) {
    throw new LangfuseValidationError(
      `langfuse: config.${field} must be ${min}..${max}; got ${value}.`,
    );
  }
}

/* -------------------------------------------------------------------------- */
/* Timestamp validation                                                       */
/* -------------------------------------------------------------------------- */

/**
 * ISO-8601 that ends in `Z` or `±HH:MM`.
 *
 * Deliberately NOT lenient. `/v2/metrics` rejects a naive timestamp, and it
 * rejects epoch millis with a 400 whose message does not name the offending
 * field. Validating here means the failure is ours and legible.
 */
const STRICT_OFFSET_ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?(Z|[+-]\d{2}:\d{2})$/;

/** An all-digits string: epoch seconds or millis, never ISO. */
const EPOCH_LIKE = /^\d{9,16}$/;

export class LangfuseValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LangfuseValidationError";
  }
}

/**
 * Assert one timestamp is strict ISO-8601 with an offset.
 *
 * Epoch gets its own message rather than falling through to the generic one: it
 * is by far the most common caller mistake (a chart library handing back a
 * `number`), and "not ISO-8601 with an offset" does not tell the caller what
 * they passed.
 */
export function assertStrictIsoWithOffset(value: string, field: string): string {
  if (EPOCH_LIKE.test(value)) {
    throw new LangfuseValidationError(
      `langfuse: ${field}="${value}" is an epoch value. /api/public/v2/metrics requires ` +
        `ISO-8601 WITH an offset, e.g. 2026-01-01T00:00:00Z. Epoch millis return 400.`,
    );
  }
  if (!STRICT_OFFSET_ISO.test(value)) {
    throw new LangfuseValidationError(
      `langfuse: ${field}="${value}" is not ISO-8601 with an offset. ` +
        `Use 2026-01-01T00:00:00Z or 2026-01-01T00:00:00+02:00; epoch millis and naive ` +
        `timestamps both return 400.`,
    );
  }
  return value;
}

/**
 * Assert a metrics window: both bounds strict-with-offset, and `from < to`.
 *
 * The ordering rule is checked here because the server rejects it with a 400
 * that does not name which bound is wrong, and a same-day range is an easy
 * mistake to make when both bounds are derived from the same calendar day.
 */
export function assertLangfuseMetricsWindow(query: {
  fromTimestamp: string;
  toTimestamp: string;
}): void {
  assertStrictIsoWithOffset(query.fromTimestamp, "fromTimestamp");
  assertStrictIsoWithOffset(query.toTimestamp, "toTimestamp");
  if (!(Date.parse(query.fromTimestamp) < Date.parse(query.toTimestamp))) {
    throw new LangfuseValidationError(
      `langfuse: fromTimestamp (${query.fromTimestamp}) must be strictly before ` +
        `toTimestamp (${query.toTimestamp}).`,
    );
  }
}

/**
 * Observation timestamp validation.
 *
 * v2 observations carry ISO-8601 strings and never epoch, so an epoch value is
 * still a caller bug worth naming. But unlike metrics, the offset is not
 * documented as mandatory for the filter params, so this check is lenient about
 * a missing offset and strict about non-ISO.
 */
export function assertObservationTimestamp(value: string, field: string): string {
  if (EPOCH_LIKE.test(value)) {
    throw new LangfuseValidationError(
      `langfuse: ${field}="${value}" is an epoch value. /api/public/v2/observations uses ISO-8601 ` +
        `timestamp strings, e.g. 2026-01-01T00:00:00Z.`,
    );
  }
  if (Number.isNaN(Date.parse(value))) {
    throw new LangfuseValidationError(
      `langfuse: ${field}="${value}" is not a parseable ISO-8601 timestamp.`,
    );
  }
  return value;
}

/* -------------------------------------------------------------------------- */
/* v2/observations                                                            */
/* -------------------------------------------------------------------------- */

export type LangfuseObservationType =
  | "GENERATION"
  | "SPAN"
  | "EVENT"
  | "AGENT"
  | "TOOL"
  | "CHAIN"
  | "RETRIEVER"
  | "EVALUATOR"
  | "EMBEDDING"
  | "GUARDRAIL";

export type LangfuseObservationLevel = "DEBUG" | "DEFAULT" | "WARNING" | "ERROR";

/**
 * `fields` groups for `/v2/observations`.
 *
 * `core` is always returned regardless. The DEFAULT request is `core` + `basic`
 * ONLY — which is the trap: asking for `latency` or `totalCost` without adding
 * the matching group returns rows with those fields absent, i.e. `undefined`,
 * with HTTP 200. A chart plots a zero and nothing anywhere reports an error.
 */
export type LangfuseObservationField =
  | "core"
  | "basic"
  | "time"
  | "io"
  | "metadata"
  | "model"
  | "usage"
  | "prompt"
  | "metrics"
  | "trace_context";

export const LANGFUSE_OBSERVATION_FIELDS: readonly LangfuseObservationField[] = [
  "core",
  "basic",
  "time",
  "io",
  "metadata",
  "model",
  "usage",
  "prompt",
  "metrics",
  "trace_context",
];

/** What you get when you send no `fields` at all. */
export const DEFAULT_LANGFUSE_OBSERVATION_FIELDS: readonly LangfuseObservationField[] = [
  "core",
  "basic",
];

/**
 * The measurement groups a caller usually reaches for, expressed as intent.
 *
 * Each entry names the field being wanted and the group that carries it, because
 * that mapping is the thing callers get wrong. `latency` and `timeToFirstToken`
 * are in `metrics`; `totalCost`, `usageDetails`, `costDetails` are in `usage`;
 * `model`, `modelId`, `inputPrice` are in `model`.
 */
export interface LangfuseObservationFieldRequest {
  /** `latency`, `timeToFirstToken` → `metrics`. */
  latency?: boolean;
  /** `totalCost`, `usageDetails`, `costDetails` → `usage`. */
  totalCost?: boolean;
  /** `model`, `modelId`, `inputPrice`, `outputPrice`, `totalPrice` → `model`. */
  model?: boolean;
  /** `input`, `output` → `io`. */
  inputOutput?: boolean;
  /** `metadata` → `metadata`. */
  metadata?: boolean;
  /** `usageDetails`, `costDetails` → `usage`. */
  usageDetails?: boolean;
  /** `startTime`, `endTime`, `completionStartTime` → `time`. */
  time?: boolean;
  /** `promptId`, `promptName`, `promptVersion` → `prompt`. */
  prompt?: boolean;
  /** `traceName`, `tags`, `release` → `trace_context`. */
  traceContext?: boolean;
}

/**
 * Resolve the `fields` list for a set of wanted measurements.
 *
 * Always includes the default `core` + `basic` so the result never loses the
 * columns a table needs to render a row at all.
 */
export function fieldsForObservations(
  request: LangfuseObservationFieldRequest = {},
): LangfuseObservationField[] {
  const fields = new Set<LangfuseObservationField>(DEFAULT_LANGFUSE_OBSERVATION_FIELDS);
  if (request.latency) fields.add("metrics");
  if (request.totalCost || request.usageDetails) fields.add("usage");
  if (request.model) fields.add("model");
  if (request.inputOutput) fields.add("io");
  if (request.metadata) fields.add("metadata");
  if (request.time) fields.add("time");
  if (request.prompt) fields.add("prompt");
  if (request.traceContext) fields.add("trace_context");
  // Declared order, not insertion order, so the emitted query is stable and
  // diffable in logs.
  return LANGFUSE_OBSERVATION_FIELDS.filter((field) => fields.has(field));
}

/**
 * An observation from `/v2/observations`.
 *
 * Every property is camelCase and every timestamp is an ISO-8601 STRING. There
 * are no epoch numbers on this endpoint at all.
 *
 * The index signature is load-bearing: the wire payload is `.loose()`, so
 * unknown enrichment fields can appear, and a closed type would reject a
 * perfectly good response. The documented members below are the ones guaranteed
 * by the OpenAPI; anything else is passed through untyped rather than dropped.
 */
export interface LangfuseObservationV2 {
  /* core — always present */
  id: string;
  traceId: string | null;
  startTime: string;
  endTime: string | null;
  projectId: string;
  parentObservationId: string | null;
  type: LangfuseObservationType;

  /* `model` group. Always present as keys, null unless the group was requested. */
  modelId?: string | null;
  /** STRINGS (decimal), not numbers. Legacy v1 observations used numbers. */
  inputPrice?: string | null;
  outputPrice?: string | null;
  totalPrice?: string | null;

  /* basic */
  isRootObservation?: boolean;
  name?: string;
  level?: LangfuseObservationLevel;
  statusMessage?: string;
  version?: string;
  environment?: string;
  bookmarked?: boolean;
  public?: boolean;
  userId?: string;
  sessionId?: string;
  completionStartTime?: string;
  createdAt?: string;
  updatedAt?: string;

  /* io — RAW STRINGS on v2, never parsed JSON */
  input?: string;
  output?: string;

  /* metadata */
  metadata?: Record<string, unknown>;

  /* model */
  model?: string;
  internalModelId?: string;
  modelParameters?: Record<string, unknown>;

  /* usage — metric integer counts, and USD costs */
  usageDetails?: Record<string, number>;
  costDetails?: Record<string, number>;
  totalCost?: number;
  usagePricingTierName?: string;

  /* prompt */
  promptId?: string;
  promptName?: string;
  promptVersion?: string;

  /* metrics — SECONDS, both of them */
  latency?: number;
  timeToFirstToken?: number;

  /* trace_context */
  traceName?: string;
  tags?: string[];
  release?: string;

  [key: string]: unknown;
}

export interface LangfuseObservationsQuery {
  /**
   * Groups to return. Defaults to `core` + `basic` ONLY when omitted.
   * Prefer `fieldsForObservations` over hand-writing this list.
   */
  fields?: LangfuseObservationField[];
  expandMetadata?: boolean;
  /** 1..1000. Defaults to 50. */
  limit?: number;
  cursor?: string;
  name?: string;
  userId?: string;
  sessionId?: string;
  type?: LangfuseObservationType;
  traceId?: string;
  level?: LangfuseObservationLevel;
  parentObservationId?: string;
  isRootObservation?: boolean;
  /** Repeated param. */
  environment?: string[];
  fromStartTime?: string;
  toStartTime?: string;
  version?: string;
  filter?: string;
  /**
   * Not modelled, and not sendable.
   *
   * `parseIoAsJson=true` is deprecated on v2 and returns 400. It is absent from
   * this interface so it cannot be set; there is no code path that sends it.
   */
}

export const LANGFUSE_OBSERVATION_LIMIT_MAX = 1000;
export const LANGFUSE_OBSERVATION_LIMIT_DEFAULT = 50;

/* -------------------------------------------------------------------------- */
/* v3/scores                                                                  */
/* -------------------------------------------------------------------------- */

export type LangfuseScoreSource = "API" | "ANNOTATION" | "EVAL";

/**
 * A score value is POLYMORPHIC on the wire and there is no discriminator field
 * in the base shape — `dataType` is a filter parameter, not part of the payload.
 *
 *   NUMERIC                        → number
 *   BOOLEAN                        → boolean
 *   CATEGORICAL / TEXT / CORRECTION → string
 *
 * So consumers must handle all three. Treating it as always-numeric turns every
 * categorical score into `NaN` and every boolean into 0/1 with no complaint.
 */
export type LangfuseScoreValue = number | boolean | string;

/** `fields=subject` payload. `kind` values are not enumerated upstream. */
export interface LangfuseScoreSubject {
  kind: string;
  id: string;
  traceId?: string;
}

/**
 * A score from `/v3/scores` — the cleanest scores API in the product and the
 * one this adapter builds its score charts on.
 */
export interface LangfuseScoreV3 {
  id: string;
  projectId: string;
  name: string;
  source: LangfuseScoreSource;
  timestamp: string;
  environment: string;
  createdAt: string;
  updatedAt: string;
  value: LangfuseScoreValue;
  /** `fields=details` */
  comment?: string;
  /** `fields=details` */
  configId?: string;
  /** `fields=details` */
  metadata?: Record<string, unknown>;
  /** `fields=subject` */
  subject?: LangfuseScoreSubject;
  /** `fields=annotation` */
  authorUserId?: string;
  /** `fields=annotation` */
  queueId?: string;
  [key: string]: unknown;
}

export type LangfuseScoreField = "core" | "details" | "subject" | "annotation";

export const LANGFUSE_SCORE_FIELDS: readonly LangfuseScoreField[] = [
  "core",
  "details",
  "subject",
  "annotation",
];

export interface LangfuseScoresQuery {
  /** 1..100. Above 100 is a 400. Defaults to 50. */
  limit?: number;
  cursor?: string;
  fields?: LangfuseScoreField[];
  id?: string;
  name?: string;
  source?: LangfuseScoreSource;
  dataType?: string;
  environment?: string;
  configId?: string;
  queueId?: string;
  authorUserId?: string;
  traceId?: string;
  sessionId?: string;
  observationId?: string;
  experimentId?: string;
  /** Numeric comparison against the score value. */
  value?: number;
  /** Requires `dataType`. */
  valueMin?: number;
  /** Requires `dataType`. */
  valueMax?: number;
  fromTimestamp?: string;
  toTimestamp?: string;
}

export const LANGFUSE_SCORE_LIMIT_MAX = 100;
export const LANGFUSE_SCORE_LIMIT_DEFAULT = 50;

/**
 * Validate the cross-field rules `/v3/scores` enforces server-side.
 *
 * All three are 400s whose message names the constraint but not the query, and
 * all three are trivially avoidable at the call site. `valueMin`/`valueMax` are
 * listed separately from `value` because the research is explicit that EACH
 * requires a single `dataType`, not "at least one of them has one".
 */
export function assertLangfuseScoresQuery(query: LangfuseScoresQuery): void {
  if (query.observationId !== undefined && query.traceId === undefined) {
    throw new LangfuseValidationError(
      "langfuse: /api/public/v3/scores requires traceId whenever observationId is set.",
    );
  }
  for (const field of ["value", "valueMin", "valueMax"] as const) {
    if (query[field] !== undefined && !query.dataType) {
      throw new LangfuseValidationError(
        `langfuse: /api/public/v3/scores requires dataType when ${field} is set.`,
      );
    }
  }
  if (query.limit !== undefined && (query.limit < 1 || query.limit > LANGFUSE_SCORE_LIMIT_MAX)) {
    throw new LangfuseValidationError(
      `langfuse: score limit must be 1..${LANGFUSE_SCORE_LIMIT_MAX} (above ${LANGFUSE_SCORE_LIMIT_MAX} is a 400); got ${query.limit}.`,
    );
  }
}

/* -------------------------------------------------------------------------- */
/* Catalogue endpoints                                                        */
/* -------------------------------------------------------------------------- */

/**
 * Datasets, dataset items, score configs, experiments, projects and models are
 * exposed as rows with no modelled members.
 *
 * Deliberate. The research confirmed the paths exist under `/api/public/v2/...`
 * and that they use the same `{data, meta}` envelope as everything else, but did
 * not enumerate their fields. Naming a field here that the server does not have
 * would be worse than admitting the gap: the row is returned verbatim and
 * `Record<string, unknown>` says exactly that much.
 */
export type LangfuseCatalogueRow = Record<string, unknown>;

export interface LangfuseCatalogueQuery {
  limit?: number;
  page?: number;
}

/** `GET /api/public/organizations/projects` → `{projects: [...]}`, not `{data}`. */
export interface LangfuseOrganizationProjects {
  projects: LangfuseCatalogueRow[];
}

export interface LangfuseDatasetItemsQuery extends LangfuseCatalogueQuery {
  datasetName?: string;
  page?: number;
  limit?: number;
}

/* -------------------------------------------------------------------------- */
/* Known-unstable                                                             */
/* -------------------------------------------------------------------------- */

/**
 * `GET /api/public/metrics/daily` — unstable, exposed but not a contract.
 *
 * It exists in Langfuse's own source (`web/src/pages/api/public/metrics/daily.ts`)
 * but is ABSENT from the generated `openapi.yml`. A route that is in the code
 * but not in the spec is not a contract; it can change or vanish in any release
 * without a deprecation notice. Callers must be prepared for a 404.
 */
export type LangfuseMetricsDaily = unknown;
