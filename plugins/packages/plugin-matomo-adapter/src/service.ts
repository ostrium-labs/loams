/**
 * Matomo adapter.
 *
 * SCOPE: read-only. Every method here issues a `GET`. Nothing writes to Matomo.
 *
 * WHAT IS REUSED
 * --------------
 * All HTTP mechanics — auth header assembly, query encoding, timeouts, error
 * normalization — come from `@loams-core/http`. This file owns only what
 * is genuinely Matomo's: the `index.php` request shape, the dot-form method
 * grammar, the `result: "error"` body branch, and the response-shape union.
 *
 * FOUR THINGS THAT ARE WORTH READING BEFORE EDITING
 * -------------------------------------------------
 * 1. THERE IS NO `/api/v1`. `MATOMO_ENDPOINT` is `/index.php` and every request
 *    reads `?module=API&method=<Module>.<methodName>&format=JSON&idSite=…`.
 * 2. AUTH IS THE BEARER HEADER AND ONLY THAT. Matomo collects `token_auth` from
 *    the header, a JSON body, POST and GET, then calls `throwIfValuesConflict()`,
 *    which raises `BadRequestException` when two sources carry DIFFERENT values
 *    (`core/Request/AuthenticationToken.php:223-230`). One mechanism, so
 *    `token_auth` is never also sent as a query parameter — `_buildParams`
 *    throws if a caller tries.
 * 3. ERRORS BRANCH ON THE BODY, NOT THE STATUS. `plugins/API/Renderer/Json.php:39-51`
 *    renders `{"result":"error","message":"…"}` and the status is only set when
 *    the exception carries one, so a bad `token_auth` is HTTP 200. `_request`
 *    inspects the body on both the success path and the `UpstreamError` path.
 * 4. THE RESPONSE IS A UNION. `classifyMatomoResponse` is the single place it is
 *    branched on, and `normaliseMatomoResponse` is the single place it is
 *    flattened. Widgets never see any of the five raw shapes.
 */

import { Context, Service } from "cordis";
import {
  QueryValue,
  UpstreamClient,
  UpstreamError,
  UpstreamLogger,
  loggerFrom,
} from "@loams-core/http";
import type { PluginAgentSkill, PluginLoader, PluginManifest } from "@loams-core/host";
import {
  MATOMO_DATATABLE_DEFAULT_LIMIT,
  MATOMO_ENDPOINT,
  MATOMO_METHOD_NAME_PATTERN,
  MATOMO_METHOD_NAMES,
  MatomoActionsRow,
  MatomoConfig,
  MatomoDate,
  MatomoDimensionRow,
  MatomoErrorEnvelope,
  MatomoIdSite,
  MatomoLiveVisitsQuery,
  MatomoMetric,
  MatomoNormalisedResult,
  MatomoNormalisedRow,
  MatomoPeriod,
  MatomoReportQuery,
  MatomoResponse,
  MatomoRowRecord,
  MatomoScalarValue,
  MatomoSite,
  MatomoVisitsSummaryRow,
  isMultiDateRequest,
  isMultiSiteRequest,
} from "./types.js";

/* -------------------------------------------------------------------------- */
/* Errors and coercion                                                          */
/* -------------------------------------------------------------------------- */

/**
 * A failed Matomo call.
 *
 * Raised from the `result: "error"` body on an HTTP 200 as often as from a real
 * 4xx/5xx, which is the entire reason this class exists instead of relying on
 * `UpstreamError`.
 */
export class MatomoApiError extends Error {
  /** The HTTP status, or 200 when the failure arrived inside a 200 body. */
  readonly status: number;
  /** `Matomo`'s `message`, when the server supplied one. */
  readonly serverMessage?: string;

  constructor(message: string, options: { status?: number; serverMessage?: string } = {}) {
    super(message);
    this.name = "MatomoApiError";
    this.status = options.status ?? 200;
    this.serverMessage = options.serverMessage;
  }
}

/**
 * Coerce a metric that may have been formatted as a string.
 *
 * Handles the three shapes `core/API/Inconsistencies.php:29-47` produces under the
 * server's backwards-compatible default: `"42%"`, `"1 min 23 sec"` and the
 * plain number. The adapter sends `format_metrics=0` so this is a safety net, not
 * the normal path — a `NaN` on a chart axis is far harder to trace back than a
 * number that arrived dressed as text.
 */
export function matomoNumber(value: MatomoMetric | null | undefined): number | undefined {
  if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
  if (typeof value !== "string") return undefined;

  const text = value.trim();
  if (text.length === 0) return undefined;

  // "1 min 23 sec" / "2 hours 3 min" / "45 sec" — Matomo's time formatter.
  const duration = parseMatomoDuration(text);
  if (duration !== undefined) return duration;

  // "42%" and "42" — the percent formatter, localised or not.
  const numeric = Number.parseFloat(text.replace(/[%\s\u00a0]/g, ""));
  return Number.isFinite(numeric) ? numeric : undefined;
}

/** Turn a formatted time string into seconds. Unparseable input returns undefined. */
function parseMatomoDuration(text: string): number | undefined {
  const pattern =
    /(\d+(?:\.\d+)?)\s*(ms|milliseconds?|s|secs?|seconds?|m|mins?|minutes?|h|hours?|d|days?)/gi;
  let total = 0;
  let matched = false;
  for (const match of text.matchAll(pattern)) {
    const amount = Number.parseFloat(match[1] ?? "");
    if (!Number.isFinite(amount)) continue;
    const unit = (match[2] ?? "").toLowerCase();
    if (unit.startsWith("ms") || unit.startsWith("millisecond")) total += amount / 1000;
    else if (unit.startsWith("s")) total += amount;
    else if (unit.startsWith("m")) total += amount * 60;
    else if (unit.startsWith("h")) total += amount * 3600;
    else if (unit.startsWith("d")) total += amount * 86400;
    else continue;
    matched = true;
  }
  return matched ? total : undefined;
}

/**
 * Coerce a 0–1 ratio (or a formatted `"42%"`) to a 0–100 percentage.
 *
 * Matomo's `bounce_rate` is a 0–1 float, which is the single most misread number
 * in a Matomo dashboard: rendered directly it looks like a rounding error.
 */
export function matomoPercent(value: MatomoMetric | null | undefined): number | undefined {
  if (typeof value === "string" && value.includes("%")) {
    const parsed = matomoNumber(value);
    return parsed;
  }
  const ratio = matomoNumber(value);
  return ratio === undefined ? undefined : ratio * 100;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Narrow `MatomoScalarValue` to the non-array arm. `Array.isArray` does not narrow a readonly array. */
function isNonArrayScalar(value: MatomoScalarValue): value is string | number | boolean | null {
  return !Array.isArray(value);
}

function tryParseObject(text: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(text) as unknown;
    return isPlainObject(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

/**
 * A prettified Matomo period label, as used as a `DataTable\Map` key.
 *
 * `2026-09-01` (day), `2026-09-01 14:00` (hour), `2026-09` (month), `2026` (year),
 * `2026-09-01` (week — the label is the week's first day). This is what
 * distinguishes a date-keyed map from a site-keyed one whose values are flat rows.
 */
const MATOMO_PERIOD_KEY = /^\d{4}(?:-\d{2}){0,2}(?:[ T]\d{2}:\d{2}(?::\d{2})?)?$/;

/* -------------------------------------------------------------------------- */
/* The response-shape guard                                                     */
/* -------------------------------------------------------------------------- */

/**
 * Branch on the shape Matomo actually returned.
 *
 * The decision order is not arbitrary:
 *
 * 1. Not an object at all → `scalar` (a bare number, string, boolean, null, or
 *    the `int[]` of `SitesManager.getAllSitesId`).
 * 2. An array whose entries are ALL objects → `rows` (every dimension report).
 *    An array of anything else stays `scalar`.
 * 3. An object with no object-valued entry → `flat`. A single-period numeric
 *    archive has only scalar columns; `{}` lands here too and normalises to no
 *    rows rather than to one empty row.
 * 4. An object whose values CONTAIN a further object → `sites`. That is the
 *    two-level map `core/Archive/DataTable.php:414-421` produces, and the site
 *    index is always first.
 * 5. Otherwise, if every key looks like a period label → `dates`. Otherwise →
 *    `sites`, whose single-date values are flat rows.
 *
 * Step 5 is the subtle one: a multi-site request with a SINGLE date is a
 * site-keyed map of flat rows, and only the key vocabulary tells it apart from a
 * date-keyed map.
 */
export function classifyMatomoResponse<R extends MatomoRowRecord = MatomoRowRecord>(
  input: unknown,
): MatomoResponse<R> {
  if (input === null || input === undefined) {
    return { kind: "scalar", value: null };
  }

  if (Array.isArray(input)) {
    // An EMPTY array is an empty report, which is a perfectly ordinary answer for
    // a day with no traffic or a referrer report with no rows. So the test is
    // "every entry is an object", which an empty array satisfies vacuously. Only
    // an array of NON-objects — `SitesManager.getAllSitesId`'s `int[]` — is a
    // scalar list.
    const objects = input.filter(isPlainObject);
    if (objects.length === input.length) {
      return { kind: "rows", rows: input as R[] };
    }
    return { kind: "scalar", value: input as readonly unknown[] };
  }

  if (!isPlainObject(input)) {
    return { kind: "scalar", value: input as MatomoScalarValue };
  }

  const entries = Object.entries(input);
  const nested = entries.filter((entry): entry is [string, Record<string, unknown>] =>
    isPlainObject(entry[1]),
  );
  if (nested.length === 0) {
    return { kind: "flat", row: input as R };
  }

  const hasSecondLevel = nested.some(([, value]) => Object.values(value).some(isPlainObject));
  if (hasSecondLevel) {
    const sites: Record<string, Record<string, R>> = {};
    for (const [siteKey, value] of entries) {
      sites[siteKey] = isPlainObject(value) ? (value as Record<string, R>) : {};
    }
    return { kind: "sites", sites };
  }

  const allKeysArePeriods =
    entries.length > 0 && entries.every(([key]) => MATOMO_PERIOD_KEY.test(key));
  if (allKeysArePeriods) {
    const dates: Record<string, R> = {};
    for (const [dateKey, value] of entries) {
      dates[dateKey] = isPlainObject(value) ? (value as R) : ({} as R);
    }
    return { kind: "dates", dates };
  }

  const sites: Record<string, Record<string, R>> = {};
  for (const [siteKey, value] of entries) {
    sites[siteKey] = isPlainObject(value) ? (value as Record<string, R>) : {};
  }
  return { kind: "sites", sites };
}

/**
 * Flatten any of the five shapes into `{rows, dates?, sites?}`.
 *
 * This is the ONLY function a widget should be handed. A single-date
 * `VisitsSummary.get` and a thirty-day one both come out of here as the same
 * type, differing only in `rows.length` and whether `dates` exists — which is
 * the whole reason the raw union does not leak into the dashboard.
 *
 * One cast lives here and nowhere else: this is the boundary where untyped JSON
 * becomes typed rows, and the row type is chosen by the CALLER (which method was
 * called), not by anything visible in the payload. `R` is a caller assertion, not
 * something the payload can prove.
 */
export function normaliseMatomoResponse<R extends MatomoRowRecord = MatomoRowRecord>(
  input: unknown,
): MatomoNormalisedResult<R> {
  const classified = classifyMatomoResponse<R>(input);

  switch (classified.kind) {
    case "scalar":
      // A bare scalar carries no row. `API.getMatomoVersion` reads `kind`/`value`
      // directly instead of going through here.
      return { rows: [] };

    case "flat":
      // An empty object is "no archive for this period", not one empty row.
      return Object.keys(classified.row).length === 0
        ? { rows: [] }
        : { rows: [asNormalised<R>(classified.row)] };

    case "rows":
      return { rows: classified.rows.map((row) => asNormalised<R>(row)) };

    case "dates": {
      const dates = Object.keys(classified.dates).sort();
      const rows = dates.map((date) =>
        asNormalised<R>(classified.dates[date] ?? {}, date, undefined),
      );
      return { rows, dates };
    }

    case "sites": {
      const sites = Object.keys(classified.sites).sort();
      const rows: MatomoNormalisedRow[] = [];
      for (const siteId of sites) {
        const byDate = classified.sites[siteId] ?? {};
        const innerKeys = Object.keys(byDate);
        // A site value is a date map only when its keys really are period labels.
        // In the multi-site SINGLE-date case the site value is one flat row, and
        // iterating it as if it were keyed by date turns `nb_visits` into a
        // "date" — five fake buckets for one row.
        const isDateMap =
          innerKeys.length > 0 &&
          innerKeys.every((key) => MATOMO_PERIOD_KEY.test(key)) &&
          innerKeys.every((key) => isPlainObject(byDate[key]));

        if (!isDateMap) {
          rows.push(asNormalised(byDate as MatomoRowRecord, undefined, siteId));
          continue;
        }
        for (const date of innerKeys.sort()) {
          rows.push(asNormalised(byDate[date] as MatomoRowRecord, date, siteId));
        }
      }
      // `dates` is deliberately absent: the date axis is per-site, so collapsing
      // it into one list would report a bucket that some site never had.
      return { rows: rows as unknown as Array<R & MatomoNormalisedRow>, sites };
    }
  }
}

function asNormalised<R extends MatomoRowRecord>(
  row: MatomoRowRecord,
  date?: string,
  siteId?: string,
): R & MatomoNormalisedRow {
  const merged: MatomoNormalisedRow = {
    ...row,
    ...(date === undefined ? {} : { date }),
    ...(siteId === undefined ? {} : { siteId }),
  };
  return merged as unknown as R & MatomoNormalisedRow;
}

/* -------------------------------------------------------------------------- */
/* Service                                                                      */
/* -------------------------------------------------------------------------- */

export class MatomoAdapterService extends Service {
  static inject = [];

  readonly config: MatomoConfig;
  private readonly client: UpstreamClient;
  private readonly log: UpstreamLogger | undefined;

  constructor(ctx: Context, config: MatomoConfig) {
    super(ctx, "matomo");
    this.config = config;

    if (typeof config.apiToken !== "string" || config.apiToken.length === 0) {
      throw new Error(
        "matomo: `apiToken` is required. Matomo has NO create-token API method — mint the token " +
          "in the web UI (User settings -> API auth tokens) or as a superuser, then configure it. " +
          "(`api_key` is Matomo Cloud / WordPress confusion and does not exist in 5.14.0.)",
      );
    }

    this.log = loggerFrom(ctx);
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl.replace(/\/+$/, ""),
        // The Bearer header is Matomo's PREFERRED route: it sets the internal
        // "provided securely" flag, and the query-string form of `token_auth` is
        // marked deprecated for Matomo 6.
        auth: { kind: "bearer", token: config.apiToken },
        timeoutMs: config.timeoutMs,
      },
      this.log,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Plumbing                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * The query for one API call.
   *
   * `format=JSON` is matched case-insensitively server-side; `format_metrics=0` is
   * not optional — see the types file. `filter_limit` is always explicit because
   * the 100-row default is only injected on the HTTP entry point.
   *
   * THE THREE ASSERTIONS HERE ARE DELIBERATE GUARD RAILS, not validation noise:
   * a `token_auth` key would collide with the Bearer header and raise
   * `BadRequestException`; a lowercase `idsite` fails the required `$idSite`; and
   * a slash-form method is rejected by `core/API/Request.php:622-629`. Each is a
   * bug that is invisible in review and fatal at runtime.
   */
  private _buildParams(
    method: MatomoMethodName,
    params: Record<string, QueryValue> = {},
    rowLimit?: number,
  ): Record<string, QueryValue> {
    if (!MATOMO_METHOD_NAME_PATTERN.test(method)) {
      throw new MatomoApiError(
        `matomo: "${method}" is not a valid method name. Expected exactly 'Module.methodName' ` +
          "with a DOT — core/API/Request.php:622-629 rejects anything else.",
      );
    }
    for (const key of Object.keys(params)) {
      if (key === "token_auth") {
        throw new MatomoApiError(
          "matomo: refusing to send `token_auth` as a query parameter. The token already travels as " +
            "the Authorization: Bearer header, and core/Request/AuthenticationToken.php calls " +
            "throwIfValuesConflict() when two sources carry different values. Send exactly one.",
        );
      }
      if (key === "api_key") {
        throw new MatomoApiError(
          "matomo: `api_key` does not exist in Matomo 5.14.0 — it is Matomo Cloud / WordPress " +
            "confusion. Use the `token_auth` value as a Bearer token.",
        );
      }
      if (key === "idsite") {
        throw new MatomoApiError(
          "matomo: the request parameter is `idSite` with a capital S and it is CASE-SENSITIVE " +
            "(core/Common.php:522 is a literal array lookup with no case folding). Lowercase `idsite` " +
            "is only the tracker parameter and the site row field name.",
        );
      }
      if (key === "apiMethod") {
        throw new MatomoApiError(
          "matomo: `apiMethod` is an internal PHP variable, not a request parameter. Send `method`.",
        );
      }
    }

    return {
      module: "API",
      method,
      format: "JSON",
      // Backwards-compatibility formatting is the server default; opt out.
      format_metrics: 0,
      filter_limit: rowLimit ?? this.config.defaultRowLimit ?? MATOMO_DATATABLE_DEFAULT_LIMIT,
      ...params,
    };
  }

  /**
   * One API call, error-branched, returning the RAW payload.
   *
   * The body is checked for `result === "error"` on BOTH paths: on the 200 path
   * because a bad `token_auth` returns HTTP 200 with an error body, and on the
   * `UpstreamError` path because the shared client turns a real 4xx/5xx into an
   * error carrying the same JSON body, which holds the actionable message.
   *
   * It returns the payload UNCLASSIFIED on purpose: classifying happens exactly
   * once, in {@link _classified} or in `normaliseMatomoResponse`. Classifying here
   * and again downstream silently re-reads `{kind, row}` as a site map.
   */
  private async _request(
    method: MatomoMethodName,
    params: Record<string, QueryValue> = {},
    rowLimit?: number,
  ): Promise<unknown> {
    const query = this._buildParams(method, params, rowLimit);

    let payload: unknown;
    try {
      payload = await this.client.get<unknown>(MATOMO_ENDPOINT, query);
    } catch (err) {
      if (err instanceof UpstreamError) {
        const envelope = tryParseObject(err.body);
        const message = matomoErrorMessage(envelope);
        if (message !== undefined) {
          throw new MatomoApiError(`matomo: ${method} failed: ${message}`, {
            status: err.status,
            serverMessage: message,
          });
        }
      }
      throw err;
    }

    const envelope = isPlainObject(payload) ? payload : undefined;
    const message = matomoErrorMessage(envelope);
    if (message !== undefined) {
      // HTTP 200 with an error body. Not treating this as a failure is how every
      // auth and permission problem turns into an empty chart.
      throw new MatomoApiError(`matomo: ${method} failed: ${message}`, {
        status: 200,
        serverMessage: message,
      });
    }

    return payload;
  }

  /** {@link _request} for the methods that read the union themselves. */
  private async _classified<R extends MatomoRowRecord = MatomoRowRecord>(
    method: MatomoMethodName,
    params: Record<string, QueryValue> = {},
    rowLimit?: number,
  ): Promise<MatomoResponse<R>> {
    return classifyMatomoResponse<R>(await this._request(method, params, rowLimit));
  }

  /** The common path: a report call, already normalised into `{rows, dates?, sites?}`. */
  private async _report<R extends MatomoRowRecord = MatomoRowRecord>(
    method: MatomoMethodName,
    params: Record<string, QueryValue>,
    rowLimit?: number,
  ): Promise<MatomoNormalisedResult<R>> {
    return normaliseMatomoResponse<R>(await this._request(method, params, rowLimit));
  }

  /**
   * Map a {@link MatomoReportQuery} onto wire parameters.
   *
   * `columns` and the `*Columns` filters are comma-joined strings, not repeated
   * parameters: Matomo reads them as comma lists.
   */
  private _reportParams(query: MatomoReportQuery): Record<string, QueryValue> {
    return {
      idSite: query.idSite,
      period: query.period ?? this.config.defaultPeriod ?? "day",
      date: query.date ?? this.config.defaultDate ?? "yesterday",
      segment: query.segment,
      columns: joinList(query.columns),
      filter_offset: query.offset,
      filter_column: query.filterColumn,
      filter_pattern: query.filterPattern,
      filter_column_recursive: query.filterColumnRecursive,
      filter_pattern_recursive: query.filterPatternRecursive,
      filter_excludelowpop: query.filterExcludeLowPop,
      filter_excludelowpop_value: query.filterExcludeLowPopValue,
      filter_sort_column: query.filterSortColumn,
      filter_sort_column_secondary: query.filterSortColumnSecondary,
      filter_truncate: query.filterTruncate,
      keep_summary_row: query.keepSummaryRow,
      totals: query.totals,
      showMetadata: query.showMetadata,
      flat: query.flat,
      hideColumns: joinList(query.hideColumns),
      showColumns: joinList(query.showColumns),
      pivotBy: query.pivotBy,
      pivotByColumn: query.pivotByColumn,
    };
  }

  /** Refuse the requests two methods reject server-side, before spending a round trip. */
  private _assertSingleSiteAndSingleDate(
    method: MatomoMethodName,
    idSite: MatomoIdSite,
    period: MatomoPeriod | undefined,
    date: MatomoDate | undefined,
  ): void {
    const resolvedPeriod = period ?? this.config.defaultPeriod ?? "day";
    const resolvedDate = date ?? this.config.defaultDate ?? "yesterday";
    if (isMultiSiteRequest(idSite)) {
      throw new MatomoApiError(
        `matomo: ${method} supports a SINGLE site only. "all" and a comma-separated idSite list are ` +
          `rejected by checkSingleSite(). Split the request, or use a method that accepts a range.`,
      );
    }
    if (isMultiDateRequest(resolvedPeriod, resolvedDate)) {
      throw new MatomoApiError(
        `matomo: ${method} supports a SINGLE date only — the server throws ` +
          `"${method} with multiple sites or dates is not supported (yet)." for a period range. ` +
          "Narrow the window, or use a method that accepts a range.",
      );
    }
  }

  /* ---------------------------------------------------------------------- */
  /* Visits                                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * `VisitsSummary.get` — visits, actions, bounce and visit length.
   *
   * Three required parameters: `idSite`, `period`, `date`. `nb_uniq_visitors` and
   * `nb_users` are optional on the row type because they exist only when unique
   * visitors are enabled for the period.
   *
   * This is also the evolution graph: `period=day&date=last30`. There is no
   * `VisitsSummary.getEvolution` method in 5.14.0.
   */
  async getVisitsSummary(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoVisitsSummaryRow>> {
    return this._report<MatomoVisitsSummaryRow>(
      "VisitsSummary.get",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /**
   * `Actions.get` — the same archive columns as a summary plus
   * `avg_time_generation`.
   *
   * A SINGLE date returns a flat object; a range returns a date-keyed map. Both
   * arrive here as `{rows}`.
   */
  async getActions(query: MatomoReportQuery): Promise<MatomoNormalisedResult<MatomoActionsRow>> {
    return this._report<MatomoActionsRow>("Actions.get", this._reportParams(query), query.rowLimit);
  }

  /**
   * The day-of-week histogram.
   *
   * NOTE THE NAME: the method is `VisitTime.getByDayOfWeek`. There is no
   * `*.getDayOfWeek` anywhere in 5.14.0. The method THROWS on multiple dates or
   * multiple sites, so that is refused here rather than round-tripped.
   */
  async getVisitsByDayOfWeek(
    query: Pick<MatomoReportQuery, "idSite" | "period" | "date" | "segment" | "rowLimit">,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    this._assertSingleSiteAndSingleDate(
      "VisitTime.getByDayOfWeek",
      query.idSite,
      query.period,
      query.date,
    );
    return this._report<MatomoDimensionRow>(
      "VisitTime.getByDayOfWeek",
      {
        idSite: query.idSite,
        period: query.period ?? this.config.defaultPeriod ?? "day",
        date: query.date ?? this.config.defaultDate ?? "yesterday",
        segment: query.segment,
      },
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Pages and search                                                       */
  /* ---------------------------------------------------------------------- */

  /**
   * `Actions.getPageUrls` — page views.
   *
   * The signature carries an extra `depth` that `getPageTitles` does NOT have:
   * `($idSite, $period, $date, $segment, $expanded, $idSubtable, $depth, $flat)`.
   * `depth` is sent only here.
   */
  async getPageUrls(
    query: MatomoReportQuery & { expanded?: boolean; depth?: number; idSubtable?: number },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Actions.getPageUrls",
      {
        ...this._reportParams(query),
        expanded: query.expanded,
        idSubtable: query.idSubtable,
        depth: query.depth,
      },
      query.rowLimit,
    );
  }

  /**
   * `Actions.getPageTitles` — page titles.
   *
   * SIGNATURE DIFFERS FROM `getPageUrls`: there is NO `depth`
   * (`$idSite, $period, $date, $segment, $expanded, $idSubtable, $flat`). Sending
   * `depth` here is a silently ignored extra, so this method does not accept one.
   */
  async getPageTitles(
    query: MatomoReportQuery & { expanded?: boolean; idSubtable?: number },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Actions.getPageTitles",
      {
        ...this._reportParams(query),
        expanded: query.expanded,
        idSubtable: query.idSubtable,
      },
      query.rowLimit,
    );
  }

  /**
   * `Actions.getSiteSearchKeywords` — internal search terms.
   *
   * The method's own `limit` parameter is separate from the generic `filter_limit`,
   * and both are sent when the caller sets them.
   */
  async getSiteSearchKeywords(
    query: MatomoReportQuery & { limit?: number; flat?: boolean },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Actions.getSiteSearchKeywords",
      { ...this._reportParams(query), limit: query.limit },
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Referrers                                                              */
  /* ---------------------------------------------------------------------- */

  /**
   * `Referrers.getAll` — every referrer type.
   *
   * SINGLE SITE AND SINGLE DATE ONLY. `plugins/Referrers/API.php:235-265` throws
   * on a range and `checkSingleSite` rejects `idSite=all`. Use
   * {@link getReferrerWebsites} for ranges.
   */
  async getReferrers(
    query: Pick<MatomoReportQuery, "idSite" | "period" | "date" | "segment" | "rowLimit">,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    this._assertSingleSiteAndSingleDate("Referrers.getAll", query.idSite, query.period, query.date);
    return this._report<MatomoDimensionRow>(
      "Referrers.getAll",
      {
        idSite: query.idSite,
        period: query.period ?? this.config.defaultPeriod ?? "day",
        date: query.date ?? this.config.defaultDate ?? "yesterday",
        segment: query.segment,
      },
      query.rowLimit,
    );
  }

  /**
   * `Referrers.getWebsites` — referring websites.
   *
   * Unlike `getAll` this ACCEPTS a range, which is what makes it the method for a
   * multi-week referrer chart. Each row carries a `segment` metadata key, which is
   * what a drill-down passes back to `Referrers.getUrlsFromWebsiteId`.
   */
  async getReferrerWebsites(
    query: MatomoReportQuery & { expanded?: boolean },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Referrers.getWebsites",
      { ...this._reportParams(query), expanded: query.expanded },
      query.rowLimit,
    );
  }

  /**
   * `Referrers.getUrlsFromWebsiteId` — the URLs one referrer drove.
   *
   * `idSubtable` is REQUIRED and typed `int` server-side, which is why it is a
   * required number here rather than an option: sending it as absent produces a
   * "Please specify a value for idSubtable" error instead of a chart.
   */
  async getUrlsFromWebsiteId(
    query: MatomoReportQuery & { idSubtable: number },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    if (!Number.isInteger(query.idSubtable)) {
      throw new MatomoApiError(
        "matomo: Referrers.getUrlsFromWebsiteId requires an integer `idSubtable` " +
          "($idSubtable is typed int on the server).",
      );
    }
    return this._report<MatomoDimensionRow>(
      "Referrers.getUrlsFromWebsiteId",
      { ...this._reportParams(query), idSubtable: query.idSubtable },
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Geography                                                              */
  /* ---------------------------------------------------------------------- */

  /**
   * `UserCountry.getCountry` — visitors by country.
   *
   * THE MODULE IS `UserCountry`, not `UserCountries`; the API resolves
   * `\Piwik\Plugins\UserCountry\API` and anything else is a class-not-found.
   * `label` is the TRANSLATED country name, `code` is the ISO code.
   */
  async getCountries(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "UserCountry.getCountry",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `UserCountry.getContinent`. */
  async getContinents(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "UserCountry.getContinent",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `UserCountry.getRegion`. */
  async getRegions(query: MatomoReportQuery): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "UserCountry.getRegion",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `UserCountry.getCity`. */
  async getCities(query: MatomoReportQuery): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "UserCountry.getCity",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Devices                                                                */
  /* ---------------------------------------------------------------------- */

  /**
   * `DevicesDetection.getType` — device type.
   *
   * THE MODULE IS `DevicesDetection`. There is no `Devices` alias: the API
   * resolves `\Piwik\Plugins\DevicesDetection\API`, so `Devices.getType` is a
   * class-not-found, not a 400.
   */
  async getDeviceTypes(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getType",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getOsVersions`. */
  async getOsVersions(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getOsVersions",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getOsFamilies`. */
  async getOsFamilies(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getOsFamilies",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getBrowsers`. */
  async getBrowsers(query: MatomoReportQuery): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getBrowsers",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getBrowserVersions`. */
  async getBrowserVersions(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getBrowserVersions",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getBrowserEngines`. */
  async getBrowserEngines(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getBrowserEngines",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getBrand`. */
  async getDeviceBrands(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getBrand",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `DevicesDetection.getModel`. */
  async getDeviceModels(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "DevicesDetection.getModel",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Goals                                                                  */
  /* ---------------------------------------------------------------------- */

  /**
   * `Goals.get` — conversions per goal.
   *
   * Goal-specific columns arrive as `goal_<idgoal>_<metric>` keys, which are
   * computed at runtime; they land on the row's index signature rather than
   * being declared, because `hasGoalMetrics` is what decides whether they exist.
   */
  async getGoals(
    query: MatomoReportQuery & {
      idGoal?: number | string;
      columns?: string | readonly string[];
      showAllGoalSpecificMetrics?: boolean;
      compare?: number | string | readonly (number | string)[];
    },
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Goals.get",
      {
        ...this._reportParams(query),
        idGoal: query.idGoal,
        columns: joinList(query.columns),
        showAllGoalSpecificMetrics: query.showAllGoalSpecificMetrics,
        compare: joinAnyList(query.compare),
      },
      query.rowLimit,
    );
  }

  /** `Goals.getDaysToConversion` — the days-to-conversion distribution. */
  async getDaysToConversion(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Goals.getDaysToConversion",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /** `Goals.getVisitsUntilConversion` — the visits-until-conversion distribution. */
  async getVisitsUntilConversion(
    query: MatomoReportQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Goals.getVisitsUntilConversion",
      this._reportParams(query),
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Live visit log                                                         */
  /* ---------------------------------------------------------------------- */

  /**
   * `Live.getLastVisitsDetails` — the live visit log.
   *
   * Only `idSite` is required. This method reads `filter_limit` / `filter_offset`
   * DIRECTLY and DISABLES the generic limit/sort filters, so both are sent here
   * explicitly instead of through the generic path.
   *
   * SORTING IS UNSUPPORTED — the method logs a warning and returns the log in its
   * own order — so this query has no sort option at all. It also throws
   * `'Visits log is deactivated for all given websites'` when tracking is off,
   * which arrives as a `result: "error"` body and is raised as such.
   */
  async getLastVisitsDetails(
    query: MatomoLiveVisitsQuery,
  ): Promise<MatomoNormalisedResult<MatomoDimensionRow>> {
    return this._report<MatomoDimensionRow>(
      "Live.getLastVisitsDetails",
      {
        idSite: query.idSite,
        period: query.period,
        date: query.date,
        segment: query.segment,
        filter_offset: query.offset,
        countVisitorsToFetch: query.countVisitorsToFetch,
        minTimestamp: query.minTimestamp,
        doNotFetchActions: query.doNotFetchActions,
        enhanced: query.enhanced,
      },
      query.rowLimit,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Sites                                                                  */
  /* ---------------------------------------------------------------------- */

  /**
   * `SitesManager.getAllSites` — SUPERUSER ONLY, and an object keyed by `idsite`
   * rather than a list.
   *
   * The lower-case row field is the other half of the case asymmetry: `idSite`
   * goes OUT camelCase in the query and `idsite` comes back lower-case in the
   * response, and the response map is keyed by that lower-case id too.
   */
  async listSitesRaw(): Promise<Record<string, MatomoSite>> {
    const response = await this._classified("SitesManager.getAllSites");
    if (response.kind === "flat" || response.kind === "sites" || response.kind === "dates") {
      const source =
        response.kind === "flat"
          ? response.row
          : response.kind === "dates"
            ? response.dates
            : response.sites;
      const out: Record<string, MatomoSite> = {};
      for (const [key, value] of Object.entries(source)) {
        out[key] = value as MatomoSite;
      }
      return out;
    }
    throw new MatomoApiError(
      "matomo: SitesManager.getAllSites did not return a map keyed by idsite " +
        `(got a ${response.kind}). The token may not belong to a superuser — use ` +
        "getSitesWithAdminAccess or getSitesWithViewAccess instead.",
    );
  }

  /** {@link listSitesRaw} as an array, which is what a site picker wants. */
  async listSites(): Promise<MatomoSite[]> {
    return Object.values(await this.listSitesRaw());
  }

  /**
   * `SitesManager.getAllSitesId` — superuser, a flat `int[]`.
   *
   * Cheaper than {@link listSitesRaw} when only ids are needed. NOTE the method
   * requires NO `idSite`.
   *
   * A bare array of NUMBERS classifies as `scalar`, but an EMPTY one classifies
   * as `rows` (an empty report and an empty id list are indistinguishable by
   * shape alone), so both arms are accepted and `rows` yields `[]`.
   */
  async listSiteIds(): Promise<number[]> {
    const response = await this._classified("SitesManager.getAllSitesId");
    if (response.kind === "rows") return [];
    if (response.kind === "scalar" && Array.isArray(response.value)) {
      return response.value.filter(
        (value): value is number => typeof value === "number" && Number.isFinite(value),
      );
    }
    throw new MatomoApiError(
      `matomo: SitesManager.getAllSitesId did not return an int[] (got a ${response.kind}).`,
    );
  }

  /**
   * `SitesManager.getSitesWithAdminAccess` — the NON-superuser alternative.
   *
   * Returns the same object-keyed-by-`idsite` shape as `getAllSites`, so a
   * view-only or admin token can still resolve the sites it may read. Without
   * these two methods a restricted token has no way to enumerate anything.
   */
  async listSitesWithAdminAccess(): Promise<MatomoSite[]> {
    return this._sitesFrom("SitesManager.getSitesWithAdminAccess");
  }

  /** `SitesManager.getSitesWithViewAccess` — sites the token's owner can only view. */
  async listSitesWithViewAccess(): Promise<MatomoSite[]> {
    return this._sitesFrom("SitesManager.getSitesWithViewAccess");
  }

  private async _sitesFrom(method: MatomoMethodName): Promise<MatomoSite[]> {
    const response = await this._classified(method);
    if (response.kind === "flat" || response.kind === "sites" || response.kind === "dates") {
      const source =
        response.kind === "flat"
          ? response.row
          : response.kind === "dates"
            ? response.dates
            : response.sites;
      return Object.values(source).map((value) => value as MatomoSite);
    }
    throw new MatomoApiError(
      `matomo: ${method} did not return a map keyed by idsite (got a ${response.kind}).`,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Server metadata                                                        */
  /* ---------------------------------------------------------------------- */

  /**
   * `API.getMatomoVersion` — a BARE SCALAR, the one response that is not a map or
   * an array. Takes no parameters at all, not even `idSite`.
   */
  async getVersion(): Promise<string | number | boolean | null> {
    const response = await this._classified("API.getMatomoVersion");
    if (response.kind === "scalar" && isNonArrayScalar(response.value)) {
      return response.value;
    }
    throw new MatomoApiError(
      `matomo: API.getMatomoVersion returned a ${response.kind}, but the method is documented to ` +
        "return a bare scalar.",
    );
  }
}

/* -------------------------------------------------------------------------- */
/* Module-scope helpers shared by the service, the guard and the tests          */
/* -------------------------------------------------------------------------- */

/** The method-name type used at the call boundary. Alias of the wire union. */
export type MatomoMethodName = (typeof MATOMO_METHOD_NAMES)[number];

/** Join a list-valued parameter. Matomo reads these as comma lists, not repeats. */
function joinList(value: string | readonly string[] | undefined): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "string") return value;
  return value.length > 0 ? value.join(",") : undefined;
}

/** As {@link joinList}, for a parameter whose entries may be numbers (`Goals.get`'s `compare`). */
function joinAnyList(
  value: number | string | readonly (string | number)[] | undefined,
): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return value;
  return value.length > 0 ? value.join(",") : undefined;
}

/** The `message` of an error envelope, or undefined when this is not one. */
function matomoErrorMessage(envelope: Record<string, unknown> | undefined): string | undefined {
  if (!envelope) return undefined;
  if (envelope["result"] !== "error") return undefined;
  const message = envelope["message"];
  if (typeof message === "string" && message.length > 0) return message;
  // `result: "error"` with no usable message is still an error. Returning a
  // placeholder keeps the raise rather than letting an error body read as data.
  return "(Matomo returned result=error with no message)";
}

/** Re-exported so a caller can type a catch without a second import. */
export type { MatomoErrorEnvelope };

declare module "cordis" {
  interface Context {
    matomo: MatomoAdapterService;
  }
}

/* -------------------------------------------------------------------------- */
/* Manifest                                                                    */
/* -------------------------------------------------------------------------- */

export const MATOMO_SKILLS: PluginAgentSkill[] = [
  {
    id: "getServerVersion",
    name: "Get Matomo version",
    description:
      "Read the Matomo instance version via API.getMatomoVersion, which returns a bare scalar rather than a report.",
    tags: ["matomo", "meta", "read"],
    examples: ["getServerVersion"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listSites",
    name: "List sites",
    description:
      "List the sites the configured token may read. SitesManager.getAllSites is superuser-only and returns an object keyed by the lower-case `idsite`; getSitesWithAdminAccess / getSitesWithViewAccess are the restricted-token alternatives.",
    tags: ["matomo", "sites", "read"],
    examples: ["listSites"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getVisitsSummary",
    name: "Get visits summary",
    description:
      "Visits, actions, bounce count/rate, average visit length and (when enabled) unique visitors for one site and period. A single date returns one row; a range returns one row per date. This is also the evolution graph: period=day with date=last30.",
    tags: ["matomo", "visits", "read"],
    examples: [
      'getVisitsSummary {"idSite":1}',
      'getVisitsSummary {"idSite":1,"period":"range","date":"2026-09-01,2026-09-30"}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getPageUrls",
    name: "Get page URLs",
    description:
      "Actions.getPageUrls: page views with hits, visits, average time on page and bounce/exit rates. Supports `expanded` for a subtable drill-down and `depth` for its nesting (a parameter getPageTitles does not have).",
    tags: ["matomo", "pages", "read"],
    examples: ['getPageUrls {"idSite":1,"rowLimit":25}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getPageTitles",
    name: "Get page titles",
    description:
      "Actions.getPageTitles: the same page report keyed by title instead of URL. Note this method has no `depth` parameter, unlike getPageUrls.",
    tags: ["matomo", "pages", "read"],
    examples: ['getPageTitles {"idSite":1}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getReferrers",
    name: "Get referrers",
    description:
      "Referrers.getWebsites for referring sites, which accepts a date range. Referrers.getAll covers every referrer type but the server rejects it for multiple sites or dates, so the adapter refuses those before sending.",
    tags: ["matomo", "referrers", "read"],
    examples: ['getReferrers {"idSite":1,"mode":"websites"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getCountries",
    name: "Get countries",
    description:
      "UserCountry.getCountry: visitors per country. `label` is the translated country name and `code` is the ISO code — switch on `code`, not `label`.",
    tags: ["matomo", "geo", "read"],
    examples: ['getCountries {"idSite":1}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getDevices",
    name: "Get devices and browsers",
    description:
      "DevicesDetection.getType, getBrowsers, getOsFamilies and friends. The module is `DevicesDetection` — there is no `Devices` alias, so a wrong module name is a class-not-found rather than a 400.",
    tags: ["matomo", "devices", "read"],
    examples: ['getDevices {"idSite":1,"dimension":"type"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getGoals",
    name: "Get goals",
    description:
      "Goals.get: conversions per goal. Goal-specific columns arrive as runtime-computed `goal_<idgoal>_<metric>` keys.",
    tags: ["matomo", "goals", "read"],
    examples: ['getGoals {"idSite":1}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getVisitsByDayOfWeek",
    name: "Get visits by day of week",
    description:
      "VisitTime.getByDayOfWeek. The method is named getByDayOfWeek — no `getDayOfWeek` method exists — and the server throws for multiple dates or multiple sites.",
    tags: ["matomo", "visits", "read"],
    examples: ['getVisitsByDayOfWeek {"idSite":1,"period":"week","date":"last4"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getLastVisitsDetails",
    name: "Get live visits",
    description:
      "Live.getLastVisitsDetails: the live visit log. It pages with its own filter_limit/filter_offset, does not support sorting, and fails with 'Visits log is deactivated for all given websites' when tracking is off.",
    tags: ["matomo", "live", "read"],
    examples: ['getLastVisitsDetails {"idSite":1,"rowLimit":50}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const matomoManifest: PluginManifest = {
  id: "matomo",
  name: "Matomo",
  description:
    "Read-only web analytics from Matomo 5: visits, actions, page reports, referrers, geography, devices, goals and the live visit log.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/matomo",
  icon: "globe",
  order: 35,
  defaultEnabled: true,
  upstream: { product: "Matomo", envPrefix: "MATOMO" },
  agent: {
    name: "Matomo Agent",
    description:
      "Queries a Matomo 5 instance read-only through /index.php. Authenticates by sending the token_auth value as an Authorization: Bearer header (never as a query parameter, which would collide), always sends format_metrics=0 so rates arrive as numbers rather than '42%' strings, and branches on the response body rather than the HTTP status because a bad token returns 200 with an error body.",
    version: "1.0.0",
    skills: MATOMO_SKILLS,
  },
};

function requireIdSite(params: Record<string, unknown>, skill: string): MatomoIdSite {
  const value = params["idSite"] ?? params["idsite"];
  if (typeof value === "number") return value;
  if (typeof value === "string" && value.length > 0) return value;
  throw new Error(`${skill}: "idSite" is required and must be a site id, a comma list, or "all"`);
}

function optionalPeriod(params: Record<string, unknown>): MatomoPeriod | undefined {
  const value = params["period"];
  if (value === undefined) return undefined;
  if (
    value === "day" ||
    value === "week" ||
    value === "month" ||
    value === "year" ||
    value === "range"
  ) {
    return value;
  }
  throw new Error(`${String(params["period"])} is not a valid Matomo period`);
}

function optionalDate(params: Record<string, unknown>): MatomoDate | undefined {
  const value = params["date"];
  if (value === undefined) return undefined;
  if (typeof value === "string") return value;
  throw new Error("matomo: `date` must be a string");
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

function queryFrom(params: Record<string, unknown>, skill: string): MatomoReportQuery {
  return {
    idSite: requireIdSite(params, skill),
    period: optionalPeriod(params),
    date: optionalDate(params),
    segment: optionalString(params, "segment"),
    rowLimit: optionalNumber(params, "rowLimit"),
    offset: optionalNumber(params, "offset"),
    filterSortColumn: optionalString(params, "filterSortColumn"),
    filterPattern: optionalString(params, "filterPattern"),
    filterColumn: optionalString(params, "filterColumn"),
  };
}

/**
 * Skill handlers.
 *
 * `api.ctx` rather than `this`: a handler is a plain method on an object literal,
 * so `this` is the handler record. `ctx.matomo` can THROW rather than return
 * undefined when the service is not reachable, so the read is guarded.
 */
function matomoApi(ctx: Context): MatomoAdapterService {
  try {
    return ctx.matomo;
  } catch {
    throw new Error("matomo adapter is not loaded");
  }
}

export const matomoLoader: PluginLoader = {
  service: MatomoAdapterService,
  skills: () => [
    { id: "getServerVersion", handle: async (_p, api) => matomoApi(api.ctx).getVersion() },
    { id: "listSites", handle: async (_p, api) => matomoApi(api.ctx).listSites() },
    {
      id: "getVisitsSummary",
      handle: async (params, api) =>
        matomoApi(api.ctx).getVisitsSummary(queryFrom(params, "getVisitsSummary")),
    },
    {
      id: "getPageUrls",
      handle: async (params, api) =>
        matomoApi(api.ctx).getPageUrls({
          ...queryFrom(params, "getPageUrls"),
          expanded: params["expanded"] === true,
          depth: optionalNumber(params, "depth"),
        }),
    },
    {
      id: "getPageTitles",
      handle: async (params, api) =>
        matomoApi(api.ctx).getPageTitles({
          ...queryFrom(params, "getPageTitles"),
          expanded: params["expanded"] === true,
        }),
    },
    {
      id: "getReferrers",
      handle: async (params, api) => {
        const service = matomoApi(api.ctx);
        const query = queryFrom(params, "getReferrers");
        if (params["mode"] === "all") return service.getReferrers(query);
        return service.getReferrerWebsites(query);
      },
    },
    {
      id: "getCountries",
      handle: async (params, api) =>
        matomoApi(api.ctx).getCountries(queryFrom(params, "getCountries")),
    },
    {
      id: "getDevices",
      handle: async (params, api) => {
        const service = matomoApi(api.ctx);
        const query = queryFrom(params, "getDevices");
        switch (params["dimension"]) {
          case "browsers":
            return service.getBrowsers(query);
          case "osFamilies":
            return service.getOsFamilies(query);
          case "osVersions":
            return service.getOsVersions(query);
          case "browserEngines":
            return service.getBrowserEngines(query);
          case "brand":
            return service.getDeviceBrands(query);
          case "model":
            return service.getDeviceModels(query);
          default:
            return service.getDeviceTypes(query);
        }
      },
    },
    {
      id: "getGoals",
      handle: async (params, api) => matomoApi(api.ctx).getGoals(queryFrom(params, "getGoals")),
    },
    {
      id: "getVisitsByDayOfWeek",
      handle: async (params, api) =>
        matomoApi(api.ctx).getVisitsByDayOfWeek({
          idSite: requireIdSite(params, "getVisitsByDayOfWeek"),
          period: optionalPeriod(params),
          date: optionalDate(params),
          segment: optionalString(params, "segment"),
        }),
    },
    {
      id: "getLastVisitsDetails",
      handle: async (params, api) => {
        const idSite = requireIdSite(params, "getLastVisitsDetails");
        return matomoApi(api.ctx).getLastVisitsDetails({
          idSite,
          period: optionalPeriod(params),
          date: optionalDate(params),
          segment: optionalString(params, "segment"),
          rowLimit: optionalNumber(params, "rowLimit"),
          offset: optionalNumber(params, "offset"),
        });
      },
    },
  ],
};

export { UpstreamError, MATOMO_METHOD_NAMES };
