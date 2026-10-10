/**
 * Matomo 5.14.0 wire types.
 *
 * Verified against Matomo `5.14.0` (`core/Version.php:25`). Every field and
 * method name below was read out of that tree. Where the research did not cover
 * something the field is OPTIONAL or absent — never invented.
 *
 * THE FOUR FACTS THAT DOMINATE THIS FILE
 * --------------------------------------
 * 1. THERE IS NO REST PREFIX. `index.php` is the whole API. Requests read
 *    `GET /index.php?module=API&method=<Module>.<methodName>&format=JSON&idSite=…`.
 *    A whole-tree grep for `api/v1` returns nothing in 5.14.0.
 *
 * 2. A METHOD NAME IS EXACTLY TWO DOT-SEPARATED PARTS. `core/API/Request.php:622-629`
 *    throws `"The method name is invalid. Expected 'module.methodName'"` otherwise,
 *    so `VisitsSummary.get` and NEVER `VisitsSummary/get`. See {@link MatomoMethod}.
 *
 * 3. THE MODULE NAME IS THE PLUGIN DIRECTORY NAME. `core/API/Request.php:351-354`
 *    resolves `\Piwik\Plugins\<module>\API`, which is why it is
 *    {@link MatomoMethod}'s `DevicesDetection.getType` — there is no `Devices`
 *    alias — and `UserCountry.getCountry`, not `UserCountries`.
 *
 * 4. THE RESPONSE SHAPE IS A UNION, NOT A SHAPE. `core/API/ResponseBuilder.php:99-142`
 *    dispatches on the PHP RETURN TYPE, so the SAME method returns a flat object
 *    for one period, a date-keyed map for a range, and a site-then-date nested
 *    map for several sites. {@link MatomoResponse} is that union, and
 *    `classifyMatomoResponse` / `normaliseMatomoResponse` in the service are the
 *    only place it is branched on.
 *
 * `format_metrics`
 * ---------------
 * `core/API/Request.php:120-123` defaults `format_metrics` to `'bc'`
 * ("backwards-compatibility: format percentages only"), under which
 * `core/API/Inconsistencies.php:29-47` renders `bounce_rate`, `conversion_rate`,
 * `exit_rate`, `abandoned_rate` and `Referrers.*_percent` as LOCALISED STRINGS
 * (`"42%"`) and `avg_time_on_site` as `"1 min 23 sec"`. The adapter therefore
 * sends `format_metrics=0` on every call. `format_metrics=all` is worse still:
 * it formats `revenue` as currency.
 *
 * Because of that default, the processed metrics are typed {@link MatomoMetric}
 * (`number | string`) rather than `number`: the adapter asks for numbers and the
 * type admits the string a non-compliant server would still send.
 * `matomoNumber` / `matomoRatio` coerce defensively so a missed
 * `format_metrics=0` degrades to a number rather than `NaN` on a chart axis.
 */

/* -------------------------------------------------------------------------- */
/* Request vocabulary                                                          */
/* -------------------------------------------------------------------------- */

/** The one entry point. There is no `/api/v1` in 5.14.0. */
export const MATOMO_ENDPOINT = "/index.php";

/**
 * `API_datatable_default_limit`, from `config/global.ini.php:356`.
 *
 * `plugins/API/Controller.php:33-39` injects this only on the HTTP entry point
 * and only when `filter_limit` is absent, so the adapter always sends it
 * explicitly rather than relying on a server default it cannot see.
 * `filter_limit=-1` returns every row.
 */
export const MATOMO_DATATABLE_DEFAULT_LIMIT = 100;

/**
 * Every reporting method name this adapter is allowed to send.
 *
 * The literal union is deliberate. `VisitsSummary.getEvolution` is NOT here
 * because it does not exist — only a *controller action* `getEvolutionGraph`
 * does, and the evolution graph is `VisitsSummary.get` with
 * `period=day&date=last30`. Nor is `*.getDayOfWeek` (it is
 * `VisitTime.getByDayOfWeek`), nor any `api_key` auth, which is Matomo Cloud /
 * WordPress confusion and has zero occurrences in 5.14.0.
 *
 * `API.getProcessedReport` is deliberately absent too. It is the one method that
 * IS wrapped (`{website, prettyDate, metadata, columns, reportData, …}`) and it
 * applies `ucfirst()` to `columns`, so `nb_visits` arrives as `Nb_visits` there
 * while `reportData` keeps it lowercase — a second response dialect for no
 * benefit. The bare methods are used instead and ratios are computed client-side.
 *
 * `MultiSites.getAll`, `API.getBulkRequest` and `API.get` also exist in 5.14.0
 * and are likewise absent. All three are meta-plumbing: a cross-site roll-up, a
 * POST batch dispatcher, and the internal method-invocation endpoint behind
 * `getBulkRequest`. None produces a report a dashboard can render on its own, and
 * `API.get` in particular takes its method name as a free-form parameter, which
 * would bypass the {@link MatomoMethod} grammar that is the point of this union.
 * Their absence is a decision, not an oversight.
 */
export type MatomoMethod =
  | "VisitsSummary.get"
  | "Actions.get"
  | "Actions.getPageUrls"
  | "Actions.getPageTitles"
  | "Actions.getSiteSearchKeywords"
  | "Referrers.getAll"
  | "Referrers.getWebsites"
  | "Referrers.getUrlsFromWebsiteId"
  | "UserCountry.getCountry"
  | "UserCountry.getContinent"
  | "UserCountry.getRegion"
  | "UserCountry.getCity"
  | "DevicesDetection.getType"
  | "DevicesDetection.getOsVersions"
  | "DevicesDetection.getOsFamilies"
  | "DevicesDetection.getBrowsers"
  | "DevicesDetection.getBrowserVersions"
  | "DevicesDetection.getBrowserEngines"
  | "DevicesDetection.getBrand"
  | "DevicesDetection.getModel"
  | "Goals.get"
  | "Goals.getDaysToConversion"
  | "Goals.getVisitsUntilConversion"
  | "VisitTime.getByDayOfWeek"
  | "Live.getLastVisitsDetails"
  | "SitesManager.getAllSites"
  | "SitesManager.getAllSitesId"
  | "SitesManager.getSitesWithAdminAccess"
  | "SitesManager.getSitesWithViewAccess"
  | "API.getMatomoVersion";

/**
 * Every method name as an array, for runtime validation and for tests.
 *
 * Kept in the same order as {@link MatomoMethod} so the two cannot drift apart
 * silently: {@link MATOMO_METHOD_NAMES} is the runtime source of truth.
 */
export const MATOMO_METHOD_NAMES: readonly MatomoMethod[] = [
  "VisitsSummary.get",
  "Actions.get",
  "Actions.getPageUrls",
  "Actions.getPageTitles",
  "Actions.getSiteSearchKeywords",
  "Referrers.getAll",
  "Referrers.getWebsites",
  "Referrers.getUrlsFromWebsiteId",
  "UserCountry.getCountry",
  "UserCountry.getContinent",
  "UserCountry.getRegion",
  "UserCountry.getCity",
  "DevicesDetection.getType",
  "DevicesDetection.getOsVersions",
  "DevicesDetection.getOsFamilies",
  "DevicesDetection.getBrowsers",
  "DevicesDetection.getBrowserVersions",
  "DevicesDetection.getBrowserEngines",
  "DevicesDetection.getBrand",
  "DevicesDetection.getModel",
  "Goals.get",
  "Goals.getDaysToConversion",
  "Goals.getVisitsUntilConversion",
  "VisitTime.getByDayOfWeek",
  "Live.getLastVisitsDetails",
  "SitesManager.getAllSites",
  "SitesManager.getAllSitesId",
  "SitesManager.getSitesWithAdminAccess",
  "SitesManager.getSitesWithViewAccess",
  "API.getMatomoVersion",
] as const;

/**
 * The shape `core/API/Request.php:622-629` demands: two dot-separated parts, no
 * more. Used to reject a slash-form or over-qualified method before it reaches
 * the network.
 */
export const MATOMO_METHOD_NAME_PATTERN = /^[A-Za-z][A-Za-z0-9]*\.[A-Za-z][A-Za-z0-9]*$/;

/**
 * `period`.
 *
 * `range` was added in 3.10 and is required for a multi-date `date`. A
 * single-date request uses one of the named days/weeks/months/years below with
 * `period=day|week|month|year`.
 */
export type MatomoPeriod = "day" | "week" | "month" | "year" | "range";

/**
 * `date`.
 *
 * One of the named ranges (`today`, `yesterday`, `last7` … `last365`,
 * `lastweek`, `lastmonth`, `lastyear`), or — only with `period=range` — a comma
 * separated `YYYY-MM-DD,YYYY-MM-DD` pair, or an ISO `YYYY-MM-DD`.
 *
 * Typed `string` rather than a literal union because the named-range set is long
 * and Matomo accepts additional keywords; the comma is what
 * {@link isMultiDateRequest} looks for.
 */
export type MatomoDate = string;

/**
 * `idSite`.
 *
 * `1`, `"1,4,5"`, or `"all"`. Typed as `number | string` because a multi-site list
 * is a comma-joined string on the wire, not a repeated parameter.
 *
 * CASE-SENSITIVE: `core/Common.php:522` is a literal PHP array lookup with no
 * case folding anywhere in the tree, so `idsite=1` fails the required `$idSite`
 * with `General_PleaseSpecifyValue`. The lowercase `idsite` is only the *tracker*
 * query parameter and the *site row field name* — see {@link MatomoSite}.
 */
export type MatomoIdSite = number | string;

/** True when an `idSite` value names more than one site. */
export function isMultiSiteRequest(idSite: MatomoIdSite): boolean {
  return (
    (typeof idSite === "string" && idSite.trim().toLowerCase() === "all") ||
    (typeof idSite === "string" &&
      idSite.split(",").filter((part) => part.trim().length > 0).length > 1)
  );
}

/** True when a `date` value spans more than one period. */
export function isMultiDateRequest(period: MatomoPeriod, date: MatomoDate | undefined): boolean {
  if (period === "range") return true;
  return typeof date === "string" && date.includes(",");
}

/* -------------------------------------------------------------------------- */
/* The response union                                                           */
/* -------------------------------------------------------------------------- */

/**
 * Anything Matomo can return that is not a map of rows: `API.getMatomoVersion`
 * (a bare scalar string) and `SitesManager.getAllSitesId` (a bare `int[]`, which
 * is why the array case of this union is an array and not `number[]`).
 */
export type MatomoScalarValue = string | number | boolean | null | readonly unknown[];

/**
 * The response of ANY method, discriminated by the shape it arrived in.
 *
 * `core/API/ResponseBuilder.php:99-142` dispatches purely on the PHP return
 * type, so the SAME method lands in different arms depending on the request:
 *
 * - `flat`   a plain `DataTable` with no label column, i.e. a numeric archive for
 *            a SINGLE date. `{nb_visits, nb_actions, bounce_count, …}`.
 * - `dates`  a `DataTable\Map`, returned whenever the request resolves to more
 *            than one period OR more than one site. Keys are prettified period
 *            labels (`2026-09-01`, …). `VisitsSummary.get` with
 *            `date=2026-09-01,2026-09-30` gives `{"2026-09-01":{…},…}`.
 * - `sites`  TWO levels of nesting, because `core/Archive/DataTable.php:414-421`
 *            puts the site index FIRST ("The site ID index is always first"), so
 *            multi-site + multi-date nests site → date → row.
 * - `rows`   a flat array of row objects, which is what every dimension report
 *            returns (`Actions.getPageUrls`, `UserCountry.getCountry`,
 *            `DevicesDetection.getType`, `Referrers.getWebsites`, `Goals.get`).
 * - `scalar` a bare scalar.
 */
export type MatomoResponse<R extends MatomoRowRecord = MatomoRowRecord> =
  | { kind: "flat"; row: R }
  | { kind: "dates"; dates: Record<string, R> }
  | { kind: "sites"; sites: Record<string, Record<string, R>> }
  | { kind: "rows"; rows: R[] }
  | { kind: "scalar"; value: MatomoScalarValue };

/** A wire row. Index-signature-permissive, because Matomo merges metadata flat. */
export type MatomoRowRecord = Record<string, unknown>;

/**
 * A metric that the server may send as a number or as a formatted string.
 *
 * With `format_metrics=0` these are numbers. The string arm exists because the
 * server's own default is the backwards-compatible formatter — see the file
 * comment — and a `string` here is the difference between `0.42` and `"42%"`.
 */
export type MatomoMetric = number | string;

/* -------------------------------------------------------------------------- */
/* Rows                                                                         */
/* -------------------------------------------------------------------------- */

/**
 * A dimension-report row: `Actions.getPageUrls`, `Actions.getPageTitles`,
 * `Actions.getSiteSearchKeywords`, `Referrers.getWebsites`,
 * `Referrers.getUrlsFromWebsiteId`, `UserCountry.*`, `DevicesDetection.*`,
 * `Goals.get`, `Live.getLastVisitsDetails`.
 *
 * TRAP: ROW METADATA IS MERGED FLAT INTO THE SAME OBJECT, not nested. A
 * `Referrers.getWebsites` row's `segment` and a `UserCountry.getCountry` row's
 * `code`/`logo` sit beside `label` and `nb_visits` as siblings.
 *
 * `expanded=1` adds `idsubdatatable` and a nested `subtable` of the same shape.
 *
 * `Goals.get` also gains `goal_<idgoal>_<metric>` columns when goal metrics are
 * on. Those keys are computed at runtime, so they land on the index signature
 * rather than being declared here.
 */
export interface MatomoDimensionRow {
  /** The dimension value. For `UserCountry.getCountry` this is the TRANSLATED country name. */
  label?: string;
  nb_visits?: number;
  /** Present on hit-based reports (`getPageUrls`, `getPageTitles`). */
  nb_hits?: number;
  nb_uniq_visitors?: number;
  /** Seconds. */
  avg_time_on_page?: MatomoMetric;
  bounce_rate?: MatomoMetric;
  exit_rate?: MatomoMetric;
  avg_time_on_page_duration?: number;
  avg_time_on_visit?: number;
  /** Present when the report can be drilled into (`expanded=1`). */
  idsubdatatable?: number;
  subtable?: MatomoDimensionRow[];
  /** `Referrers.getWebsites` and `Referrers.getUrlsFromWebsiteId` rows carry a segment expression. */
  segment?: string;
  /** `UserCountry.getCountry` rows add the ISO code. */
  code?: string;
  /** `UserCountry.getCountry` rows add a flag image URL. */
  logo?: string;
  [key: string]: unknown;
}

/**
 * A `VisitsSummary.get` row.
 *
 * ARCHIVE COLUMNS (`nb_visits`, `nb_actions`, `nb_visits_converted`,
 * `bounce_count`, `sum_visit_length`, `max_actions`) are always present.
 *
 * `nb_uniq_visitors` AND `nb_users` ARE OPTIONAL AND MUST STAY OPTIONAL: they
 * exist only when unique visitors are enabled for that period, and a type that
 * declares them required makes `undefined` look like a configured zero.
 *
 * PROCESSED METRICS are added on top and are always numbers with
 * `format_metrics=0`.
 */
export interface MatomoVisitsSummaryRow {
  nb_visits: number;
  nb_actions: number;
  nb_visits_converted?: number;
  bounce_count: number;
  /** Total visit length in SECONDS. */
  sum_visit_length: number;
  max_actions: number;
  /** Only when unique visitors are enabled for the period. */
  nb_uniq_visitors?: number;
  /** Only when unique visitors are enabled for the period. */
  nb_users?: number;
  /** 0–1 float, or `"42%"` if the server formatted it. */
  bounce_rate?: MatomoMetric;
  nb_actions_per_visit?: MatomoMetric;
  /** Seconds, or `"1 min 23 sec"` if the server formatted it. */
  avg_time_on_site?: MatomoMetric;
  [key: string]: unknown;
}

/** An `Actions.get` row: everything in a summary row, plus the generation metric. */
export interface MatomoActionsRow extends MatomoVisitsSummaryRow {
  /** Seconds between the first and last action of the visit. */
  avg_time_generation?: MatomoMetric;
}

/**
 * One row of a normalised result.
 *
 * `date` and `siteId` are present only when the source was date- or site-keyed,
 * which is what makes a single-date response and a date-range response the same
 * type for a widget.
 */
export interface MatomoNormalisedRow extends MatomoDimensionRow {
  /** The prettified period label this row came from, e.g. `2026-09-01`. */
  date?: string;
  /** The site key this row came from, when the response was site-keyed. */
  siteId?: string;
}

/**
 * The single form widgets consume.
 *
 * `rows` is always a flat, ascending array. `dates` is present only when the
 * source was date-keyed; `sites` only when it was site-keyed. A single-date
 * `VisitsSummary.get` and a thirty-day one both arrive here, differing only in
 * `rows.length` and whether `dates` exists.
 */
export interface MatomoNormalisedResult<R extends MatomoRowRecord = MatomoRowRecord> {
  rows: Array<R & MatomoNormalisedRow>;
  /** Ascending period keys. Absent for flat and array responses. */
  dates?: string[];
  /**
   * Site keys, ascending. Absent for anything but a site-keyed response.
   *
   * Note that a site-keyed-and-date-keyed response reports `sites` but NOT
   * `dates`: the date axis differs per site, so it lives on each row instead of
   * being flattened into one list that would be a lie for any site that lacks a
   * bucket.
   */
  sites?: string[];
}

/**
 * One row of `SitesManager.getAllSites`.
 *
 * THE ROW IS ALL-LOWERCASE. `idsite`, not `idSite` — the opposite direction of
 * the REQUEST parameter, which is camelCase and case-sensitive. The response
 * method is additionally keyed BY `idsite` rather than returned as a list, which
 * is why {@link MatomoSiteList} is a map.
 *
 * `creator_login` is present only for a superuser, so it is optional here and the
 * other access-level fields are not modelled at all.
 */
export interface MatomoSite {
  idsite: string;
  name: string;
  description: string;
  main_url: string;
  ts_created: string;
  timezone: string;
  currency: string;
  ecommerce?: boolean | string;
  sitesearch?: number;
  exclude_unknown_urls?: boolean;
  excluded_ips?: string;
  excluded_parameters?: string;
  excluded_user_agents?: string;
  group?: string;
  type?: string;
  keep_url_fragment?: boolean;
  timezone_name?: string;
  currency_name?: string;
  /** Superuser only. */
  creator_login?: string;
  [key: string]: unknown;
}

/**
 * `SitesManager.getAllSites` — an OBJECT keyed by `idsite`, NOT a list.
 *
 * `SitesManager.getAllSitesId` is the cheaper flat `int[]` alternative, and
 * `getSitesWithAdminAccess` / `getSitesWithViewAccess` are the non-superuser
 * equivalents, so a view-only token can still resolve its sites.
 */
export type MatomoSiteList = Record<string, MatomoSite>;

/* -------------------------------------------------------------------------- */
/* The error envelope                                                           */
/* -------------------------------------------------------------------------- */

/**
 * What `plugins/API/Renderer/Json.php:39-51` renders for a failed call:
 * `{"result":"error","message":"…"}`.
 *
 * THE HTTP STATUS IS NOT A RELIABLE SIGNAL. It is only set when the thrown
 * exception carries one, so a bad `token_auth` comes back **HTTP 200 with this
 * body**. Any adapter that branches on the status alone turns every auth failure
 * into an empty chart. `backtrace` may also be present and is ignored.
 */
export interface MatomoErrorEnvelope {
  result: "error";
  message: string;
  /** Server-side stack, when the exception carries one. Ignored by this adapter. */
  backtrace?: string;
}

/* -------------------------------------------------------------------------- */
/* Query shape                                                                  */
/* -------------------------------------------------------------------------- */

/**
 * Everything a reporting call can carry, in one shape.
 *
 * `filter_limit` is NOT here on purpose: the adapter always sends it, so a caller
 * cannot produce the request that omits it.
 */
export interface MatomoReportQuery {
  idSite: MatomoIdSite;
  period?: MatomoPeriod;
  date?: MatomoDate;
  segment?: string;
  columns?: string | readonly string[];
  /** `filter_limit`. -1 returns every row. Defaults to the server's 100. */
  rowLimit?: number;
  /** `filter_offset`, default 0. */
  offset?: number;
  /** `filter_column`, default `label`. */
  filterColumn?: string;
  filterPattern?: string;
  filterColumnRecursive?: string;
  filterPatternRecursive?: string;
  filterExcludeLowPop?: boolean;
  filterExcludeLowPopValue?: number;
  /** `filter_sort_column`. The server's default ORDER is descending. */
  filterSortColumn?: string;
  filterSortColumnSecondary?: string;
  filterTruncate?: string | number;
  /** `keep_summary_row`, default 0. */
  keepSummaryRow?: boolean;
  /** `totals`, default 1. */
  totals?: number;
  /** `showMetadata`, default TRUE on the server. */
  showMetadata?: boolean;
  flat?: boolean;
  hideColumns?: string | readonly string[];
  showColumns?: string | readonly string[];
  pivotBy?: string;
  pivotByColumn?: string;
}

/**
 * `Live.getLastVisitsDetails` paging.
 *
 * This method reads `filter_limit` and `filter_offset` DIRECTLY and DISABLES the
 * generic limit/sort filters, so both are passed explicitly rather than relying
 * on anything. Sorting is unsupported by the method — it logs a warning — so
 * there is deliberately no sort option here.
 */
export interface MatomoLiveVisitsQuery {
  idSite: MatomoIdSite;
  period?: MatomoPeriod;
  date?: MatomoDate;
  segment?: string;
  /** `filter_limit`. */
  rowLimit?: number;
  /** `filter_offset`. */
  offset?: number;
  /** `countVisitorsToFetch`. */
  countVisitorsToFetch?: boolean;
  minTimestamp?: MatomoMetric;
  /** `doNotFetchActions` — skip the per-visit action lists. */
  doNotFetchActions?: boolean;
  /** `enhanced`. */
  enhanced?: boolean;
}

/* -------------------------------------------------------------------------- */
/* Config                                                                       */
/* -------------------------------------------------------------------------- */

export interface MatomoConfig {
  /** Instance root, e.g. `https://analytics.example.com`. Trailing slash trimmed. */
  baseUrl: string;
  /**
   * The `token_auth` value, sent as `Authorization: Bearer <token>`.
   *
   * PROVISIONING IS MANUAL: there is NO create-token API method. Tokens are
   * minted in the Matomo web UI (User settings → API auth tokens) or by a
   * superuser. A token inherits its owner's permissions, and every reporting
   * method begins with a view-access check on `idSite`, so a view-only token
   * cannot read a site its owner cannot see.
   *
   * `api_key` DOES NOT EXIST in 5.14.0 — it is Matomo Cloud / WordPress
   * confusion and must never be written into a request here.
   */
  apiToken: string;
  timeoutMs?: number;
  /** `filter_limit` when a call does not set one. Defaults to the server's 100. */
  defaultRowLimit?: number;
  /** `period` when a call does not set one. */
  defaultPeriod?: MatomoPeriod;
  /** `date` when a call does not set one. */
  defaultDate?: MatomoDate;
}
