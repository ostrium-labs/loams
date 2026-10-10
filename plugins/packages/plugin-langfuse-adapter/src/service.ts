import { Context, Service } from "cordis";
import {
  LANGFUSE_API_PREFIX,
  LangfuseCatalogueQuery,
  LangfuseCatalogueRow,
  LangfuseConfig,
  LangfuseDatasetItemsQuery,
  LangfuseHealth,
  LangfuseMetricsDaily,
  LangfuseMetricsQuery,
  LangfuseMetricColumn,
  LangfuseMetricsResponse,
  LangfuseObservationV2,
  LangfuseObservationsQuery,
  LangfuseOrganizationProjects,
  LangfusePage,
  LangfuseScoreV3,
  LangfuseScoresQuery,
  LANGFUSE_OBSERVATION_LIMIT_MAX,
  LANGFUSE_OBSERVATION_LIMIT_DEFAULT,
  assertLangfuseScoresQuery,
  assertObservationTimestamp,
  assertSupportedLangfusePath,
  buildLangfuseMetricsParams,
  cursorOf,
  expectedLangfuseMetricColumns,
  langfuseBasicAuth,
  stripLangfuseDeprecation,
} from "./types.js";
import {
  UpstreamClient,
  UpstreamError,
  loggerFrom,
  type QueryValue,
  type UpstreamLogger,
} from "@loams-core/http";

/** Base for every request: host + `/api/public`. */
function apiPath(suffix: string): string {
  return `${LANGFUSE_API_PREFIX}${suffix}`;
}

export class LangfuseAdapterService extends Service {
  static inject = [];

  public readonly config: LangfuseConfig;
  private readonly client: UpstreamClient;
  /**
   * A credential-free client for `/health`.
   *
   * The health endpoint takes no auth, and sending the Basic header to it would
   * mean a wrong key turns a connectivity probe into an auth failure — the
   * opposite of what a health check is for. One extra client is cheaper than
   * that confusion.
   */
  private readonly probe: UpstreamClient;
  private readonly logger?: UpstreamLogger;
  /** Deprecation notices already surfaced, so one notice does not log per poll. */
  private readonly loggedDeprecations = new Set<string>();

  constructor(ctx: Context, config: LangfuseConfig) {
    super(ctx, "langfuse");
    this.config = config;
    this.logger = loggerFrom(ctx);
    this.client = new UpstreamClient(
      {
        baseUrl: config.baseUrl,
        auth: langfuseBasicAuth(config.publicKey, config.secretKey),
        ...(config.timeoutMs !== undefined ? { timeoutMs: config.timeoutMs } : {}),
      },
      this.logger,
    );
    this.probe = new UpstreamClient(
      {
        baseUrl: config.baseUrl,
        auth: { kind: "none" },
        ...(config.timeoutMs !== undefined ? { timeoutMs: config.timeoutMs } : {}),
      },
      this.logger,
    );
  }

  /**
   * Every request goes through here.
   *
   * `assertSupportedLangfusePath` is the reason this wrapper exists rather than
   * `this.client.get(...)` at each call site: the legacy-surface guard has to be
   * unbypassable, and a guard that each of ~20 methods can forget is not a guard.
   */
  private async _get<T>(
    suffix: string,
    params?: Record<string, QueryValue>,
    options: { auth?: "required" | "none" } = {},
  ): Promise<T> {
    const path = apiPath(suffix);
    assertSupportedLangfusePath(path);
    const client = options.auth === "none" ? this.probe : this.client;
    const body = await client.get<T>(path, params);
    this._noteDeprecation(path, body);
    return body;
  }

  /**
   * Surface a `_deprecation` marker, once per path.
   *
   * The requirement is that it never breaks parsing, which it does not — the
   * adapter indexes `data` and ignores everything else. Logging it matters
   * anyway: a caller still pointed at a to-be-removed surface should hear about
   * it before the date rather than after.
   */
  private _noteDeprecation(path: string, body: unknown): void {
    if (!body || typeof body !== "object") return;
    const marker = (body as { _deprecation?: unknown })._deprecation;
    if (marker === undefined || this.loggedDeprecations.has(path)) return;
    if (this.loggedDeprecations.size >= 32) return;
    this.loggedDeprecations.add(path);
    this.logger?.warn(
      `langfuse: ${path} returned a _deprecation marker: ${JSON.stringify(marker).slice(0, 300)}`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Health                                                              */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/public/health` → `{version, status}`.
   *
   * No auth. This is the health check AND the version probe: the version it
   * returns is how you confirm the deployment is on a surface where `/v2/metrics`
   * means what this adapter assumes it means.
   */
  async health(): Promise<LangfuseHealth> {
    return this._get<LangfuseHealth>("/health", undefined, { auth: "none" });
  }

  /* ------------------------------------------------------------------ */
  /* v2/metrics — the analytics endpoint                                */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/public/v2/metrics`.
   *
   * Not paginated and carries NO `meta`: the response is `{data: [...]}` and
   * that is all. Callers wanting to know what the columns are called should call
   * {@link metricsWithColumns}, which returns the derived column keys alongside
   * the rows rather than making every consumer re-derive them.
   */
  async metrics(query: LangfuseMetricsQuery): Promise<LangfuseMetricsResponse> {
    const response = await this._get<LangfuseMetricsResponse>(
      "/v2/metrics",
      buildLangfuseMetricsParams(query),
    );
    return { data: response?.data ?? [] };
  }

  /**
   * {@link metrics} plus the column keys the response is expected to carry.
   *
   * Preferred over calling `metrics` directly for anything that charts. Result
   * columns are `{aggregation}_{measure}` — `p95_latency`, not `latency` — so a
   * consumer that guesses the key gets `undefined` on a 200 response and renders
   * an empty chart with no error anywhere. Returning the derived keys makes the
   * mismatch visible at the call site, and marks which cells are histogram
   * `[lower, upper, height]` tuples rather than scalars.
   */
  async metricsWithColumns(query: LangfuseMetricsQuery): Promise<{
    columns: LangfuseMetricColumn[];
    rows: Array<Record<string, unknown>>;
  }> {
    const { data } = await this.metrics(query);
    return { columns: expectedLangfuseMetricColumns(query), rows: data };
  }

  /* ------------------------------------------------------------------ */
  /* v2/observations — the real-time read path                           */
  /* ------------------------------------------------------------------ */

  /** One page. `limit` 1..1000, default 50. */
  async listObservations(
    query: LangfuseObservationsQuery = {},
  ): Promise<LangfusePage<LangfuseObservationV2>> {
    const params = this._observationParams(query);
    const body = await this._get<LangfusePage<LangfuseObservationV2> & { _deprecation?: unknown }>(
      "/v2/observations",
      params,
    );
    const { payload } = stripLangfuseDeprecation(body);
    return { data: payload.data ?? [], meta: payload.meta };
  }

  /**
   * Walk `/v2/observations` to exhaustion, one page at a time.
   *
   * Termination is guarded four ways, because an infinite pagination loop is a
   * hung dashboard: an empty page ends it, a page with no cursor ends it, a
   * cursor identical to the previous one ends it (a server that echoes the
   * request cursor back instead of advancing), and `maxPages` bounds it.
   */
  async *paginateObservations(
    query: LangfuseObservationsQuery = {},
    options: { maxPages?: number } = {},
  ): AsyncGenerator<LangfuseObservationV2[]> {
    yield* this._paginate<LangfuseObservationV2>(
      (cursor) => this.listObservations(cursor === undefined ? query : { ...query, cursor }),
      options,
    );
  }

  private _observationParams(query: LangfuseObservationsQuery): Record<string, QueryValue> {
    if (
      query.limit !== undefined &&
      (query.limit < 1 || query.limit > LANGFUSE_OBSERVATION_LIMIT_MAX)
    ) {
      throw new Error(
        `langfuse: observation limit must be 1..${LANGFUSE_OBSERVATION_LIMIT_MAX}; got ${query.limit}.`,
      );
    }
    const fromStartTime =
      query.fromStartTime === undefined
        ? undefined
        : assertObservationTimestamp(query.fromStartTime, "fromStartTime");
    const toStartTime =
      query.toStartTime === undefined
        ? undefined
        : assertObservationTimestamp(query.toStartTime, "toStartTime");

    return {
      // `fields` is a repeated param: `fields=core&fields=basic`, never `core,basic`.
      fields: query.fields,
      expandMetadata: query.expandMetadata,
      limit: query.limit,
      cursor: query.cursor,
      name: query.name,
      userId: query.userId,
      sessionId: query.sessionId,
      type: query.type,
      traceId: query.traceId,
      level: query.level,
      parentObservationId: query.parentObservationId,
      isRootObservation: query.isRootObservation,
      environment: query.environment,
      fromStartTime,
      toStartTime,
      version: query.version,
      filter: query.filter,
    };
  }

  /* ------------------------------------------------------------------ */
  /* v3/scores                                                           */
  /* ------------------------------------------------------------------ */

  /** One page. `limit` 1..100, default 50 — above 100 is a 400. */
  async listScores(query: LangfuseScoresQuery = {}): Promise<LangfusePage<LangfuseScoreV3>> {
    assertLangfuseScoresQuery(query);
    const body = await this._get<LangfusePage<LangfuseScoreV3> & { _deprecation?: unknown }>(
      "/v3/scores",
      {
        limit: query.limit,
        cursor: query.cursor,
        fields: query.fields,
        id: query.id,
        name: query.name,
        source: query.source,
        dataType: query.dataType,
        environment: query.environment,
        configId: query.configId,
        queueId: query.queueId,
        authorUserId: query.authorUserId,
        traceId: query.traceId,
        sessionId: query.sessionId,
        observationId: query.observationId,
        experimentId: query.experimentId,
        value: query.value,
        valueMin: query.valueMin,
        valueMax: query.valueMax,
        fromTimestamp:
          query.fromTimestamp === undefined
            ? undefined
            : assertObservationTimestamp(query.fromTimestamp, "fromTimestamp"),
        toTimestamp:
          query.toTimestamp === undefined
            ? undefined
            : assertObservationTimestamp(query.toTimestamp, "toTimestamp"),
      },
    );
    const { payload } = stripLangfuseDeprecation(body);
    return { data: payload.data ?? [], meta: payload.meta };
  }

  async *paginateScores(
    query: LangfuseScoresQuery = {},
    options: { maxPages?: number } = {},
  ): AsyncGenerator<LangfuseScoreV3[]> {
    yield* this._paginate<LangfuseScoreV3>(
      (cursor) => this.listScores(cursor === undefined ? query : { ...query, cursor }),
      options,
    );
  }

  /**
   * Shared cursor walk for `/v2/observations` and `/v3/scores`.
   *
   * Both are cursor-paginated with a base64 token encoding
   * `{lastStartTimeTo, lastTraceId, lastId}`. The token is opaque and is never
   * decoded or constructed here — a hand-built one makes the server return page
   * 1 again and the loop runs until `maxPages`.
   *
   * Three termination rules, because a stalled cursor is an ordinary failure
   * mode and an unbounded loop is a hung dashboard:
   *
   *  - an empty page is the end;
   *  - a page with no cursor is the end;
   *  - a page whose cursor has already been followed is a REPLAY and is dropped.
   *
   * The third deserves the note. `used` holds the cursors that were sent as
   * *inputs*. If the server hands back a cursor that is already an input, then
   * the page fetched after it would be byte-identical to the page just fetched,
   * so that page is a duplicate and is discarded rather than yielded. Yielding
   * it would double-count rows on every chart.
   */
  private async *_paginate<T>(
    fetchPage: (cursor: string | undefined) => Promise<LangfusePage<T>>,
    options: { maxPages?: number } = {},
  ): AsyncGenerator<T[]> {
    const maxPages = options.maxPages ?? 100;
    let cursor: string | undefined;
    const used = new Set<string>();

    for (let page = 0; page < maxPages; page += 1) {
      const result = await fetchPage(cursor);
      const data = result.data ?? [];
      if (data.length === 0) return;

      const next = cursorOf(result.meta);
      if (!next) {
        yield data;
        return;
      }
      if (used.has(next)) return;

      yield data;
      used.add(next);
      cursor = next;
    }

    this.logger?.warn(
      `langfuse: cursor pagination stopped at the ${maxPages}-page cap; results are truncated.`,
    );
  }

  /* ------------------------------------------------------------------ */
  /* Catalogue endpoints                                                 */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/public/v2/datasets`.
   *
   * The remaining catalogue endpoints return rows whose fields this adapter does
   * not model — see {@link LangfuseCatalogueRow}. They are typed as unmodelled
   * on purpose rather than guessed at.
   */
  listDatasets(query: LangfuseCatalogueQuery = {}): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/datasets", {
      limit: query.limit,
      page: query.page,
    });
  }

  getDataset(datasetName: string): Promise<LangfuseCatalogueRow> {
    return this._get<LangfuseCatalogueRow>(`/v2/datasets/${encodeURIComponent(datasetName)}`);
  }

  listDatasetItems(
    query: LangfuseDatasetItemsQuery = {},
  ): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/dataset-items", {
      datasetName: query.datasetName,
      limit: query.limit,
      page: query.page,
    });
  }

  getDatasetItem(itemId: string): Promise<LangfuseCatalogueRow> {
    return this._get<LangfuseCatalogueRow>(`/v2/dataset-items/${encodeURIComponent(itemId)}`);
  }

  listScoreConfigs(
    query: LangfuseCatalogueQuery = {},
  ): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/score-configs", {
      limit: query.limit,
      page: query.page,
    });
  }

  listExperiments(query: LangfuseCatalogueQuery = {}): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/experiments", {
      limit: query.limit,
      page: query.page,
    });
  }

  listExperimentItems(
    query: LangfuseCatalogueQuery = {},
  ): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/experiment-items", {
      limit: query.limit,
      page: query.page,
    });
  }

  listModels(query: LangfuseCatalogueQuery = {}): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/v2/models", {
      limit: query.limit,
      page: query.page,
    });
  }

  /**
   * `GET /api/public/projects`.
   *
   * REQUIRES a project-scoped key. An organization-scoped key gets a 403 here and
   * must call {@link listOrganizationProjects} instead — which is the more common
   * key shape, so a wrong choice reads as "Langfuse auth is broken" when it is
   * really "right key, wrong scope".
   */
  listProjects(query: LangfuseCatalogueQuery = {}): Promise<LangfusePage<LangfuseCatalogueRow>> {
    return this._get<LangfusePage<LangfuseCatalogueRow>>("/projects", {
      limit: query.limit,
      page: query.page,
    });
  }

  /**
   * `GET /api/public/organizations/projects` → `{projects: [...]}`.
   *
   * Note the shape: NOT the `{data, meta}` envelope, because this is the one
   * endpoint in the catalogue that returns a named key. Typed accordingly so a
   * consumer reading `.data` here gets `undefined` rather than silently empty.
   */
  listOrganizationProjects(): Promise<LangfuseOrganizationProjects> {
    return this._get<LangfuseOrganizationProjects>("/organizations/projects");
  }

  /* ------------------------------------------------------------------ */
  /* Known-unstable                                                      */
  /* ------------------------------------------------------------------ */

  /**
   * `GET /api/public/metrics/daily` — UNSTABLE, DO NOT BUILD A DASHBOARD ON IT.
   *
   * The route exists in Langfuse's source (`web/src/pages/api/public/metrics/daily.ts`)
   * but is absent from its own generated `openapi.yml`. A path in the code and
   * not in the spec is not a contract: it can change shape or disappear in any
   * release with no deprecation notice, and a 404 here is expected behaviour
   * rather than a misconfiguration.
   *
   * Returned as `unknown` for the same reason — no response type is claimed.
   */
  async metricsDaily(): Promise<LangfuseMetricsDaily> {
    return this._get<LangfuseMetricsDaily>("/metrics/daily");
  }

  /* ------------------------------------------------------------------ */
  /* Diagnostics                                                         */
  /* ------------------------------------------------------------------ */

  /**
   * Connectivity + version + credential check in one call.
   *
   * Never throws: an unreachable Langfuse is a status to render, not an
   * exception to propagate through a dashboard that has eight other widgets.
   */
  async status(): Promise<
    | { ok: true; version: string; upstreamStatus: string }
    | { ok: false; error: string; status?: number }
  > {
    try {
      const health = await this.health();
      return { ok: true, version: health.version, upstreamStatus: health.status };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
        ...(error instanceof UpstreamError ? { status: error.status } : {}),
      };
    }
  }
}

declare module "cordis" {
  interface Context {
    langfuse: LangfuseAdapterService;
  }
}
