/**
 * Loams adapter — read-only.
 *
 * SCOPE: read-only. There is no method in this file that writes, and no request
 * path that can. Loams' route table pairs most reads with a write at the same
 * path (`GET`+`DELETE` on `…/collections/{c}`, `POST` on `…/documents` and
 * `…/documents/patch_by_filter`, `PUT …/hot`), so the omission is per METHOD,
 * not per path — see {@link LoamsAdapterService} for the full list of what was
 * left out and why.
 *
 * WHY PLAIN REST AND NOT CONNECT-RPC
 * ----------------------------------
 * Loams has TWO service surfaces: the native REST table in
 * `crates/loams/src/api/mod.rs:117-189`, and a smaller Connect-RPC surface
 * (`connectrpc = "0.9.1"`, generated from the proto tree under `proto/loams/`,
 * one directory per service, each holding a `v1` package).
 *
 * Plain REST is the right choice here, for four reasons, all from source:
 *
 * 1. IT IS THE DOCUMENTED READ SURFACE. `README.md:112-139` demonstrates
 *    namespaces, collections, documents, `/query` and `/sql` entirely over REST
 *    with `curl`. The proto tree carries no equivalent of collections,
 *    documents, `/query` or `/sql` — the REST table is the wider surface.
 * 2. THE PROTO TREE IS THE UNSTABLE HALF. `loams.live.v1` and the streaming gRPC
 *    API are both "In progress" (`README.md:59-60`), and `loams.stream.v1` is
 *    opt-in. REST is what the project's own docs treat as the contract.
 * 3. THE SHARED CLIENT IS AN HTTP CLIENT. Reusing `UpstreamClient` means auth
 *    headers, timeouts and error normalisation are not reimplemented. A
 *    Connect-RPC surface would need generated stubs, its own error mapping and
 *    its own auth story — all for less coverage.
 * 4. `/health` and `/ready` are REST-only, and they are the cheapest way to
 *    answer "is this deployment up" for a dashboard.
 *
 * WHAT IS REUSED
 * --------------
 * All HTTP mechanics come from `@loams-plugins/plugin-upstream-http`: auth header assembly,
 * query encoding, timeouts, and `UpstreamError` normalization. This file owns
 * only what is genuinely Loams': the `{ns}` path shape, the `SearchRequest`
 * wire form, the `pk`-versus-`id` document asymmetry, and the `/sql` gate.
 *
 * THERE IS NO RESPONSE-HEADER OBSERVER TO USE HERE
 * -------------------------------------------------
 * Unlike the Zulip adapter, this one does not wrap `fetch`. The shared client
 * exposes no header hook, and Loams' only interesting headers are
 * `Loams-Consistency-Token` (returned on every read, `mod.rs:57-59`) and the
 * write-path `Loams-Unapplied-*` / `Retry-After` pair (`mod.rs:61-68`,
 * `api/errors.rs:69-80`) — none of which a read-only dashboard needs in order to
 * render. The consistency token is read from the response BODY instead, which
 * every read endpoint carries explicitly (`read_token`, see `LoamsCountResponse`).
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
  LOAMS_DEFAULT_LIMIT,
  LOAMS_DEFAULT_RRF_K,
  LoamsApiErrorBody,
  LoamsCollectionInfo,
  LoamsCollectionsResponse,
  LoamsConfig,
  LoamsCountResponse,
  LoamsFusion,
  LoamsGetDocumentsResponse,
  LoamsPrimaryKey,
  LoamsProjection,
  LoamsQuery,
  LoamsReadConsistency,
  LoamsRetriever,
  LoamsScrollResponse,
  LoamsSearchRequest,
  LoamsSearchResponse,
  LoamsSqlResult,
  LoamsStoredDoc,
  LoamsTrackTotalHits,
  LoamsVersionsResponse,
} from "./types.js";

/**
 * The API version prefix.
 *
 * `/v1`, NOT the Connect-RPC prefix. Every native route hangs off it except
 * `/health` and `/ready`, which are unversioned (`crates/loams/src/api/mod.rs:117-189`).
 */
export const LOAMS_API_PREFIX = "/v1";

/**
 * The largest request body Loams will accept: 16 MiB. Larger bodies get `413`
 * (`crates/loams/src/api/mod.rs:52-54`).
 */
export const LOAMS_MAX_BODY_BYTES = 16 * 1024 * 1024;

/**
 * The failure message when no namespace is configured.
 *
 * Named as a constant because the reason is not obvious from a call site and the
 * reason is the whole point: Loams exposes NO route that lists namespaces.
 * `POST /v1/namespaces` creates one and there is no GET counterpart
 * (`crates/loams/src/api/mod.rs:114`, handler `mod.rs:276-287`), and the
 * readiness probe is a bare status code with no body (`mod.rs:260-266`).
 */
export const LOAMS_NO_NAMESPACE_MESSAGE =
  "loams: no namespace configured, and the native API cannot discover one — " +
  "POST /v1/namespaces creates a namespace but nothing lists them " +
  "(crates/loams/src/api/mod.rs:114). Set LoamsConfig.namespace (LOAMS_NAMESPACE), " +
  'for example "demo" as in README.md:118.';

/**
 * Path SEGMENTS that must never appear in a request this adapter issues.
 *
 * `loams.live.v1` is a SEPARATE SERVICE with its own listener, not a sub-path of
 * `/v1/namespaces/{ns}/…`, so no legitimate Loams route has a `live` segment.
 * It is banned anyway because it is the one code-shaped surface in the project:
 * `loams.live.v1.Deploy` (`proto/loams/live/v1/live.proto:32`) takes a JavaScript
 * ES module bundle and is UNIMPLEMENTED — `rquickjs` is absent from `Cargo.toml`
 * and `Cargo.lock`, the intended `loams-live-js` crate does not exist, and the
 * work is deferred at `crates/loams-live/src/txn.rs:644`. Its `max_js_cpu` limit
 * is a declared field with nothing enforcing it.
 *
 * There is NO code execution anywhere else in Loams: no `/exec`, `/functions`,
 * `/eval` or `/invoke` route exists in the table, and `wasmtime`, `wasmer`,
 * `workerd`, `gvisor`, `extism`, `deno` and `rquickjs` are all absent from both
 * `Cargo.toml` and `Cargo.lock`. This adapter therefore cannot reach any.
 */
const FORBIDDEN_PATH_SEGMENTS = ["live", "deploy", "exec", "eval", "invoke", "functions"] as const;

/** A Loams failure, with the server's own `error` code and message. */
export class LoamsApiError extends Error {
  /** The HTTP status. */
  readonly status: number;
  /** The body's `error` value, e.g. `not_found`, `invalid_argument`. */
  readonly code: string | undefined;
  /** Extra fields the error layer attached (`index`, `matched`, `limit`, …). */
  readonly details: Record<string, unknown>;

  constructor(
    message: string,
    status: number,
    code?: string,
    details: Record<string, unknown> = {},
  ) {
    super(message);
    this.name = "LoamsApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

/* -------------------------------------------------------------------------- */
/* Search request construction                                                  */
/* -------------------------------------------------------------------------- */

/** What a caller passes to {@link buildSearchRequest}. Every key is optional
 *  except `collection`; the rest map one-for-one onto `LoamsSearchRequest`. */
export interface LoamsSearchInput {
  /** A collection name or an alias. `SearchRequest.collection`, ir.rs:37. */
  collection: string;
  /** Default `"strong"` — the server default is `Strong` (ir.rs:39-41). */
  consistency?: LoamsSearchRequest["consistency"];
  retrievers?: LoamsRetriever[];
  /**
   * Left ABSENT when not given, because absent means NO FUSION on the server
   * (`ir.rs:44-45`) — not RRF. Adding `{rrf:{}}` behind a caller's back would
   * change what their query means.
   */
  fusion?: LoamsFusion;
  filter?: LoamsQuery;
  sort?: LoamsSearchRequest["sort"];
  offset?: number;
  /** Default 10 — the server's own default (`ir.rs:28-30`, `ir.rs:52-54`). */
  limit?: number;
  search_after?: LoamsSearchRequest["search_after"];
  score_threshold?: number;
  select?: LoamsProjection;
  aggregations?: unknown;
  highlight?: unknown;
  group_by?: LoamsSearchRequest["group_by"];
  track_total_hits?: LoamsTrackTotalHits;
}

/**
 * Fill RRF's `k` with the server default.
 *
 * `{"rrf": {}}` deserializes to `Fusion::Rrf { k: 60 }`
 * (`crates/loams-query/src/ir.rs:250-255`, `default_rrf_k` at `ir.rs:28-30`).
 * Doing it here makes the wire form explicit and testable instead of relying on
 * a `#[serde(default = …)]` the client cannot see.
 */
function withDefaultRrfK(fusion: LoamsFusion): LoamsFusion {
  if ("rrf" in fusion) {
    return { rrf: { k: fusion.rrf.k ?? LOAMS_DEFAULT_RRF_K } };
  }
  return fusion;
}

/**
 * Turn a {@link LoamsSearchInput} into the exact `SearchRequest` body
 * `POST /v1/namespaces/{ns}/query` expects.
 *
 * Exported and PURE so the wire shape can be asserted without a fetch. The
 * defaults it fills are the SERVER's defaults, stated explicitly rather than
 * inherited:
 *
 * - `consistency` → `"strong"` (`ReadConsistency::Strong` is `#[default]`,
 *   `crates/loams-query/src/ir.rs:113-115`)
 * - `limit` → `10` (`default_limit`, `ir.rs:28-30`, `ir.rs:52-54`)
 * - `offset` → `0` (`#[serde(default)]` on a `usize`, `ir.rs:49-50`)
 * - `retrievers` → `[]`, `sort` → `[]`, `select` → `{}`, `track_total_hits` →
 *   `"none"` (`ir.rs:42-66`)
 * - `fusion` → left ABSENT unless asked for. See {@link LoamsSearchInput.fusion}.
 */
export function buildSearchRequest(input: LoamsSearchInput): LoamsSearchRequest {
  const request: LoamsSearchRequest = {
    collection: input.collection,
    consistency: input.consistency ?? "strong",
    retrievers: input.retrievers ?? [],
    sort: input.sort ?? [],
    offset: input.offset ?? 0,
    limit: input.limit ?? LOAMS_DEFAULT_LIMIT,
    select: input.select ?? {},
    track_total_hits: input.track_total_hits ?? "none",
  };
  // Assigned conditionally rather than with `?? undefined` so that an absent
  // key is genuinely absent from the serialized body, not present-and-null.
  if (input.fusion !== undefined) request.fusion = withDefaultRrfK(input.fusion);
  if (input.filter !== undefined) request.filter = input.filter;
  if (input.search_after !== undefined) request.search_after = input.search_after;
  if (input.score_threshold !== undefined) request.score_threshold = input.score_threshold;
  if (input.aggregations !== undefined) request.aggregations = input.aggregations;
  if (input.highlight !== undefined) request.highlight = input.highlight;
  if (input.group_by !== undefined) request.group_by = input.group_by;
  return request;
}

/* -------------------------------------------------------------------------- */
/* Response helpers                                                            */
/* -------------------------------------------------------------------------- */

/** Read `payload[key]` when it is a plain object, else `undefined`. */
function asRecord(value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  return value as Record<string, unknown>;
}

/** Read `payload[key]` when it is an array, else `undefined`. Graceful degradation. */
function asArray(value: unknown): unknown[] | undefined {
  return Array.isArray(value) ? value : undefined;
}

/**
 * Read a number that may be absent.
 *
 * Loams has no stable release and changes fields without notice
 * (`README.md:7`), so every response read that CAN degrade, does: a field the
 * server dropped comes back `undefined` rather than `NaN`, and a caller
 * rendering a dashboard sees a blank instead of a broken chart.
 */
function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function optionalText(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

/**
 * Parse a Loams error body out of an `UpstreamError`.
 *
 * Every Loams error is `{"error": <code>, "message": <text>}` plus extra fields
 * (`crates/loams/src/api/errors.rs:64-79`), so the shared client's
 * `UpstreamError` — which carries the raw body — has the actionable half of the
 * failure in it. A body that is not that shape (a proxy's HTML 502, say) falls
 * back to the `UpstreamError`'s own message rather than being swallowed.
 */
function parseApiError(err: UpstreamError): LoamsApiError {
  let body: LoamsApiErrorBody | undefined;
  try {
    body = asRecord(JSON.parse(err.body)) as LoamsApiErrorBody | undefined;
  } catch {
    body = undefined;
  }
  const code = optionalText(body?.error);
  const message = optionalText(body?.message);
  if (body === undefined || code === undefined || message === undefined) {
    return new LoamsApiError(err.message, err.status, undefined, { body: err.body });
  }
  const { error: _error, message: _message, ...details } = body;
  return new LoamsApiError(message, err.status, code, details);
}

/* -------------------------------------------------------------------------- */
/* Service                                                                     */
/* -------------------------------------------------------------------------- */

export class LoamsAdapterService extends Service {
  static inject = [];

  readonly config: LoamsConfig;
  private readonly client: UpstreamClient;
  private readonly baseUrl: string;
  private readonly log: UpstreamLogger | undefined;

  constructor(ctx: Context, config: LoamsConfig) {
    super(ctx, "loams");
    this.config = config;
    this.baseUrl = config.baseUrl.replace(/\/+$/, "");
    this.log = loggerFrom(ctx);

    // `none` is the correct answer for an unsecured deployment, and it is not a
    // guess: Loams has no auth today (`README.md:62` lists "Auth and tenancy" as
    // Planned; the native router has no auth layer). `bearer` is here for the
    // reverse-proxy-fronted deployment that README says is coming.
    this.client = new UpstreamClient(
      {
        baseUrl: this.baseUrl,
        auth: config.token ? { kind: "bearer", token: config.token } : { kind: "none" },
        timeoutMs: config.timeoutMs,
      },
      this.log,
    );
  }

  /* ---------------------------------------------------------------------- */
  /* Plumbing                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * The namespace every call is addressed under.
   *
   * @throws {LoamsApiError} with {@link LOAMS_NO_NAMESPACE_MESSAGE} when unset.
   * Discovery is not offered because the server cannot do it — see that
   * constant.
   */
  get namespace(): string {
    const ns = this.config.namespace;
    if (typeof ns !== "string" || ns.length === 0) {
      throw new LoamsApiError(LOAMS_NO_NAMESPACE_MESSAGE, 0, "no_namespace");
    }
    return ns;
  }

  /** `encodeURIComponent` a path segment, rejecting an empty one. */
  private _segment(value: string, what: string): string {
    if (typeof value !== "string" || value.length === 0) {
      throw new LoamsApiError(
        `loams: ${what} is required and must be a non-empty string`,
        0,
        "invalid_argument",
      );
    }
    return encodeURIComponent(value);
  }

  /**
   * Reject any path carrying a segment that could reach the code-shaped surface.
   *
   * SEGMENTS, not substrings: a substring test would refuse a collection
   * legitimately named `olive` or `deployment`, which is a self-inflicted outage
   * in exchange for a guarantee the rest of the class already provides. Nothing
   * here builds such a path; the check exists so that a future edit which does
   * fails loudly instead of shipping.
   */
  private _assertSafe(path: string): string {
    for (const segment of path.toLowerCase().split("/")) {
      const decoded = decodeURIComponent(segment).toLowerCase();
      if ((FORBIDDEN_PATH_SEGMENTS as readonly string[]).includes(decoded)) {
        throw new LoamsApiError(
          `loams: refusing to build a request path with a "${decoded}" segment — ` +
            "loams.live.v1.Deploy is a code-deployment RPC and this adapter never targets it",
          0,
          "forbidden_path",
        );
      }
    }
    return path;
  }

  /** `/v1/namespaces/{ns}{suffix}` with `suffix` already encoded. */
  private _ns(suffix: string): string {
    return this._assertSafe(
      `${LOAMS_API_PREFIX}/namespaces/${this._segment(this.namespace, "namespace")}${suffix}`,
    );
  }

  /** `/v1/namespaces/{ns}/collections/{collection}{suffix}`. */
  private _collection(collection: string, suffix = ""): string {
    return this._ns(`/collections/${this._segment(collection, "collection")}${suffix}`);
  }

  /** Perform one request, translating a non-2xx into a {@link LoamsApiError}. */
  private async _request<T>(
    method: "GET" | "POST",
    path: string,
    options: { body?: unknown; params?: Record<string, QueryValue> } = {},
  ): Promise<T> {
    try {
      return await this.client.request<T>(method, path, options);
    } catch (err) {
      if (err instanceof UpstreamError) throw parseApiError(err);
      throw err;
    }
  }

  /** A GET, with no query params. */
  private async _get<T>(path: string): Promise<T> {
    return this._request<T>("GET", path);
  }

  /* ---------------------------------------------------------------------- */
  /* Liveness                                                               */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /health` — `200 OK` with an EMPTY body (`crates/loams/src/api/mod.rs:257-259`).
   *
   * Answers with `true` on success and `false` on any failure. A liveness probe
   * is the one call where "false" is more useful to a caller than a throw: a
   * dashboard polling a deployment that is down wants a red dot, not an
   * exception.
   */
  async health(): Promise<boolean> {
    try {
      await this.client.get<void>("/health");
      return true;
    } catch (err) {
      this.log?.debug(
        `loams: GET /health failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return false;
    }
  }

  /**
   * `GET /ready` — `200` when the metastore is ready, `503` otherwise
   * (`crates/loams/src/api/mod.rs:260-266`).
   *
   * Also an empty body, and also answered as a boolean. A `503` from `/ready`
   * carries `Retry-After: 1` (`crates/loams/src/api/errors.rs:75-77`), which the
   * shared client does not surface; the boolean is the contract this adapter
   * exposes.
   */
  async ready(): Promise<boolean> {
    try {
      await this.client.get<void>("/ready");
      return true;
    } catch (err) {
      this.log?.debug(
        `loams: GET /ready failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return false;
    }
  }

  /* ---------------------------------------------------------------------- */
  /* Collections                                                            */
  /* ---------------------------------------------------------------------- */

  /**
   * `GET /v1/namespaces/{ns}/collections` — every collection, sorted by name
   * (`crates/loams/src/api/collections.rs:56-64`).
   *
   * GRACEFUL DEGRADATION: a server that omits `collections`, or sends something
   * that is not an array, yields `[]` rather than throwing. An empty list is
   * ALSO what an unknown namespace produces — `list_collections` returns an
   * empty vec instead of an error when the namespace does not resolve
   * (`crates/loams-query/src/service.rs:564-567`) — so an empty result is
   * ambiguous and this method does not claim to have distinguished the two.
   */
  async listCollections(): Promise<LoamsCollectionInfo[]> {
    const body = await this._get<unknown>(this._ns("/collections"));
    const collections = asArray(asRecord(body)?.["collections"]);
    if (collections === undefined) {
      this.log?.warn(
        "loams: GET /collections returned no `collections` array; treating it as empty",
      );
      return [];
    }
    return collections as LoamsCollectionInfo[];
  }

  /**
   * `GET /v1/namespaces/{ns}/collections/{c}` — one collection, by NAME OR BY
   * ALIAS (`crates/loams/src/api/collections.rs:66-81`).
   *
   * `hot` on the result is the owner node's full hot status, and only when this
   * node owns the collection (`collections.rs:76-79`); a forwarded read keeps the
   * plain `CollectionInfo` form instead. Hence `LoamsCollectionInfo.hot` is
   * optional.
   */
  async describeCollection(collection: string): Promise<LoamsCollectionInfo> {
    const body = await this._get<unknown>(this._collection(collection));
    const record = asRecord(body);
    if (record === undefined) {
      throw new LoamsApiError(
        `loams: GET /collections/${collection} did not return a collection object`,
        0,
        "unexpected_shape",
      );
    }
    return record as unknown as LoamsCollectionInfo;
  }

  /**
   * `GET /v1/namespaces/{ns}/collections/{c}/versions` — retained manifest
   * versions, OLDEST FIRST (`crates/loams/src/api/collections.rs:129-134`).
   *
   * An absent `versions` key degrades to `[]`.
   */
  async listVersions(collection: string): Promise<LoamsVersionsResponse["versions"]> {
    const body = await this._get<unknown>(this._collection(collection, "/versions"));
    const versions = asArray(asRecord(body)?.["versions"]);
    return versions === undefined ? [] : (versions as LoamsVersionsResponse["versions"]);
  }

  /* ---------------------------------------------------------------------- */
  /* Search                                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * `POST /v1/namespaces/{ns}/query` — one search over one collection
   * (`crates/loams/src/api/query.rs:14-29`).
   *
   * The body is the canonical `SearchRequest` form
   * (`crates/loams-query/src/ir.rs:34-66`), built by {@link buildSearchRequest}
   * so its defaults are visible rather than inherited. See that function for
   * why `fusion` is left absent unless asked for.
   *
   * Reads are STRONG by default, so a search sees every write acknowledged
   * before it began (`ir.rs:39-41`).
   */
  async search(input: LoamsSearchInput): Promise<LoamsSearchResponse> {
    const body = buildSearchRequest(input);
    const response = await this._request<unknown>("POST", this._ns("/query"), { body });
    const record = asRecord(response);
    if (record === undefined) {
      throw new LoamsApiError(
        "loams: POST /query did not return a search response",
        0,
        "unexpected_shape",
      );
    }
    // `hits` is not optional on the server (`ir.rs:681`), but a server that has
    // changed shape should render an empty result, not crash a dashboard.
    const hits = asArray(record["hits"]);
    if (hits === undefined) {
      this.log?.warn("loams: POST /query returned no `hits` array; treating it as empty");
    }
    return {
      ...(record as unknown as LoamsSearchResponse),
      hits: (hits ?? []) as LoamsSearchResponse["hits"],
    };
  }

  /* ---------------------------------------------------------------------- */
  /* Documents                                                              */
  /* ---------------------------------------------------------------------- */

  /**
   * `POST /v1/namespaces/{ns}/collections/{c}/documents/get` — read documents
   * by id (`crates/loams/src/api/collections.rs:297-331`).
   *
   * The body is `{"ids": [...], "select"?: Projection, "consistency"?:
   * ReadConsistency}` (`collections.rs:297-304`), and the answer holds one entry
   * per requested id IN REQUEST ORDER with `null` for a missing one
   * (`collections.rs:323-327`). A caller that zips `ids` against `documents` is
   * therefore always aligned.
   *
   * A missing `documents` key degrades to `[]`.
   */
  async getDocuments(
    collection: string,
    ids: readonly LoamsPrimaryKey[],
    options: { select?: LoamsProjection; consistency?: LoamsReadConsistency } = {},
  ): Promise<LoamsGetDocumentsResponse> {
    if (ids.length === 0) return { documents: [], read_token: "" };
    const request: Record<string, unknown> = { ids: [...ids] };
    if (options.select !== undefined) request["select"] = options.select;
    if (options.consistency !== undefined) request["consistency"] = options.consistency;

    const body = asRecord(
      await this._request<unknown>("POST", this._collection(collection, "/documents/get"), {
        body: request,
      }),
    );
    const documents = asArray(body?.["documents"]);
    if (documents === undefined) {
      this.log?.warn("loams: documents/get returned no `documents` array; treating it as empty");
    }
    return {
      documents: (documents ?? []) as (LoamsStoredDoc | null)[],
      read_token: optionalText(body?.["read_token"]) ?? "",
    };
  }

  /**
   * `POST /v1/namespaces/{ns}/collections/{c}/documents/scroll` — the next page
   * in PRIMARY-KEY ORDER (`crates/loams/src/api/collections.rs:334-373`).
   *
   * Pagination is a cursor, not an offset: pass the previous `next` back as
   * `after` (`collections.rs:348-352`). `limit` defaults to 100 server-side
   * (`collections.rs:29`), and this adapter states that default rather than
   * omitting it.
   *
   * Returns the page and the cursor together so a caller cannot advance without
   * the id it needs.
   */
  async scrollDocuments(
    collection: string,
    options: {
      filter?: LoamsQuery;
      after?: LoamsPrimaryKey;
      limit?: number;
      select?: LoamsProjection;
      consistency?: LoamsReadConsistency;
    } = {},
  ): Promise<LoamsScrollResponse> {
    const request: Record<string, unknown> = { limit: options.limit ?? 100 };
    if (options.filter !== undefined) request["filter"] = options.filter;
    if (options.after !== undefined) request["after"] = options.after;
    if (options.select !== undefined) request["select"] = options.select;
    if (options.consistency !== undefined) request["consistency"] = options.consistency;

    const body = asRecord(
      await this._request<unknown>("POST", this._collection(collection, "/documents/scroll"), {
        body: request,
      }),
    );
    const documents = asArray(body?.["documents"]);
    if (documents === undefined) {
      this.log?.warn("loams: documents/scroll returned no `documents` array; treating it as empty");
    }
    return {
      documents: (documents ?? []) as LoamsStoredDoc[],
      next: (body?.["next"] as LoamsPrimaryKey | null | undefined) ?? null,
      read_token: optionalText(body?.["read_token"]) ?? "",
    };
  }

  /**
   * `POST /v1/namespaces/{ns}/collections/{c}/documents/count` — how many
   * documents match a filter (`crates/loams/src/api/collections.rs:379-403`).
   *
   * The body is `{"filter"?: Query, "consistency"?: ReadConsistency}`; an absent
   * `filter` counts the WHOLE collection (`collections.rs:379-385`). This is the
   * cheapest dashboard read Loams offers and the one most worth a skill.
   *
   * A missing or non-numeric `count` degrades to `0` — with a warning — rather
   * than rendering `NaN` in a tile.
   */
  async countDocuments(
    collection: string,
    options: { filter?: LoamsQuery; consistency?: LoamsReadConsistency } = {},
  ): Promise<LoamsCountResponse> {
    const request: Record<string, unknown> = {};
    if (options.filter !== undefined) request["filter"] = options.filter;
    if (options.consistency !== undefined) request["consistency"] = options.consistency;

    const body = asRecord(
      await this._request<unknown>("POST", this._collection(collection, "/documents/count"), {
        body: request,
      }),
    );
    const count = optionalNumber(body?.["count"]);
    if (count === undefined) {
      this.log?.warn("loams: documents/count returned no numeric `count`; reporting 0");
    }
    return { count: count ?? 0, read_token: optionalText(body?.["read_token"]) ?? "" };
  }

  /* ---------------------------------------------------------------------- */
  /* SQL — opt-in only                                                      */
  /* ---------------------------------------------------------------------- */

  /**
   * `POST /v1/namespaces/{ns}/sql` — run ONE read-only SQL statement against
   * DataFusion (`crates/loams/src/api/sql.rs:11-40`). **Disabled by default.**
   *
   * ## WHY THIS IS GATED
   *
   * This endpoint takes **raw SQL text** from the caller and executes it. It is
   * an injection surface by design, and the adapter treats it as one:
   *
   * - It throws unless `LoamsConfig.allowSql` is explicitly `true`. No config,
   *   no skill, no UI path reaches it otherwise. The
   *   {@link LoamsAdapterService.sql} handler is additionally absent from the
   *   agent skill list, so no agent can call it — a human editing config is the
   *   only way in, which is the point.
   * - The statement is passed through **verbatim**. This client does NOT
   *   sanitise it, does not parse it, and does not check that it starts with
   *   `SELECT`.
   *
   * That last point is not an oversight. `SELECT`-only enforcement happens
   * SERVER-side: `run_read_only` (`crates/loams-query/src/sql/mod.rs:176-182`)
   * verifies the logical plan and refuses anything else with "only read-only
   * queries are allowed" (`sql/mod.rs:206-221`). A client-side string check
   * would be theatre on top of a real control, and would give false confidence
   * that it was not.
   *
   * The mitigating factor, stated precisely: DataFusion accepts no user-supplied
   * code here. Its extensions are a fixed set of internally-registered table
   * functions (`vector_search`, `text_search`, `hybrid_search`, `rrf`,
   * `rerank`) and a few scalar UDFs (`sql/mod.rs:127-144`). What remains is
   * arbitrary READ against the data — exfiltration, and the query cost of
   * whatever the caller wrote, both bounded by the server's own
   * `max_rows`/`timeout` (`sql/mod.rs:184-188`).
   *
   * The response is `{"columns", "rows", "truncated"}`
   * (`crates/loams-query/src/sql/json_rows.rs:19-39`); `rows` is positional, and
   * a `truncated: true` means the answer is a PREFIX, not a total.
   */
  async sql(
    query: string,
    options: { consistency?: LoamsReadConsistency } = {},
  ): Promise<LoamsSqlResult> {
    if (this.config.allowSql !== true) {
      throw new LoamsApiError(
        "loams: POST /sql is disabled. It executes caller-supplied SQL against DataFusion, " +
          "so it is refused unless LoamsConfig.allowSql is explicitly true. " +
          "This client does not sanitise the statement — read-only enforcement is server-side " +
          "(crates/loams-query/src/sql/mod.rs:206-221). Use search() or countDocuments() instead.",
        0,
        "sql_disabled",
      );
    }
    if (typeof query !== "string" || query.trim().length === 0) {
      throw new LoamsApiError("loams: sql() requires a non-empty `query`", 0, "invalid_argument");
    }
    const request: Record<string, unknown> = { query };
    if (options.consistency !== undefined) request["consistency"] = options.consistency;

    const body = asRecord(
      await this._request<unknown>("POST", this._ns("/sql"), { body: request }),
    );
    return {
      columns: (asArray(body?.["columns"]) ?? []) as LoamsSqlResult["columns"],
      rows: (asArray(body?.["rows"]) ?? []) as unknown[][],
      truncated: body?.["truncated"] === true,
    };
  }
}

declare module "cordis" {
  interface Context {
    loams: LoamsAdapterService;
  }
}

/* -------------------------------------------------------------------------- */
/* Manifest                                                                    */
/* -------------------------------------------------------------------------- */

export const LOAMS_SKILLS: PluginAgentSkill[] = [
  {
    id: "listCollections",
    name: "List collections",
    description:
      "List every collection in the configured Loams namespace with its schema, partition count, aliases, live document count and size.",
    tags: ["loams", "collections", "read"],
    examples: ["listCollections"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "describeCollection",
    name: "Describe collection",
    description:
      "Describe one collection by name or alias: schema, aliases, manifest version, live document count, size and hot-tier state.",
    tags: ["loams", "collections", "read"],
    examples: ['describeCollection {"collection":"kb"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listVersions",
    name: "List collection versions",
    description:
      "List a collection's retained manifest versions, oldest first, with each version's live document count and size.",
    tags: ["loams", "collections", "read"],
    examples: ['listVersions {"collection":"kb"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "search",
    name: "Hybrid search",
    description:
      "Run a Loams search: vector kNN, BM25 text, or both fused with RRF, over one collection. Reads are strong by default and the result limit defaults to 10.",
    tags: ["loams", "search", "read"],
    examples: [
      'search {"collection":"kb","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}},"k":10}}]}',
      'search {"collection":"kb","retrievers":[{"vector":{"field":"embedding","query":[1,0,0],"k":10}}],"fusion":{"rrf":{}},"limit":25}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getDocuments",
    name: "Get documents",
    description:
      "Read documents from a collection by id. Results come back in request order with null for an id that does not exist.",
    tags: ["loams", "documents", "read"],
    examples: ['getDocuments {"collection":"kb","ids":[1,2,3]}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "scrollDocuments",
    name: "Scroll documents",
    description:
      "Page through a collection's documents in primary-key order using an `after` cursor. Omit `after` for the first page; the server's default page size is 100.",
    tags: ["loams", "documents", "read"],
    examples: [
      'scrollDocuments {"collection":"kb"}',
      'scrollDocuments {"collection":"kb","after":100,"limit":50}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "countDocuments",
    name: "Count documents",
    description:
      "Count the documents in a collection that match a filter, with no filter counting the whole collection. The cheapest dashboard read Loams offers.",
    tags: ["loams", "documents", "metrics", "read"],
    examples: [
      'countDocuments {"collection":"kb"}',
      'countDocuments {"collection":"kb","filter":{"term":{"field":"tenant","value":"a"}}}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getStatus",
    name: "Get Loams status",
    description:
      "Report whether the Loams deployment answers /health and /ready. Neither endpoint returns a body; both are answered as booleans.",
    tags: ["loams", "health", "read"],
    examples: ["getStatus"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const loamsManifest: PluginManifest = {
  id: "loams",
  name: "Loams",
  description:
    "Read-only analytics over a Loams deployment: collections, hybrid retrieval (vector kNN fused with BM25 via RRF), document reads and filtered counts. " +
    "Loams is early software with no stable release — its APIs, formats and flags change without notice — so this adapter degrades gracefully rather than assuming a stable contract. " +
    "It reads the native REST API only, never targets the unimplemented loams.live.v1.Deploy code-deployment RPC, and leaves the raw-SQL endpoint disabled unless explicitly opted in.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/loams",
  icon: "database",
  // The other adapters occupy 30-36 (langfuse 30, zulip 30, forgejo 31,
  // openpanel 31, glitchtip 32, matomo 35, itsaplan 36). 38 is outside that band.
  order: 38,
  defaultEnabled: true,
  upstream: { product: "Loams", envPrefix: "LOAMS" },
  agent: {
    name: "Loams Agent",
    description:
      "Queries a Loams namespace read-only over the native REST API. Reads collections, runs hybrid retrieval, pages documents and counts matches. " +
      "The namespace must be configured (LOAMS_NAMESPACE) because the native API has no route that lists namespaces. " +
      "No authentication exists in Loams yet, so an optional bearer token is supported for a proxy-fronted deployment. " +
      "The raw-SQL endpoint is not exposed to the agent at all.",
    version: "1.0.0",
    skills: LOAMS_SKILLS,
  },
};

/* -------------------------------------------------------------------------- */
/* Loader                                                                      */
/* -------------------------------------------------------------------------- */

function requireText(params: Record<string, unknown>, field: string, skill: string): string {
  const value = params[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${skill}: "${field}" is required and must be a non-empty string`);
  }
  return value;
}

/**
 * Read a filter or retriever list from skill params.
 *
 * Passed through with no validation beyond shape: the filter grammar is Loams'
 * and rejecting a variant here would mean reimplementing it in TypeScript
 * against a contract that is not frozen (`README.md:7`). The server validates
 * it properly and answers `400 invalid_argument`.
 */
function optionalJson(params: Record<string, unknown>, field: string): unknown {
  const value = params[field];
  return value === undefined || value === null ? undefined : value;
}

/**
 * `api.ctx` rather than `this`: a skill handler is a plain method on an object
 * literal, so `this` is the handler record and not the plugin's context. The
 * read is guarded because an inherited cordis context key can THROW rather than
 * return undefined when the service is not loaded.
 */
function loamsApi(ctx: Context): LoamsAdapterService {
  try {
    return ctx.loams;
  } catch {
    throw new Error("loams adapter is not loaded");
  }
}

export const loamsLoader: PluginLoader = {
  service: LoamsAdapterService,
  skills: () => [
    { id: "listCollections", handle: async (_params, api) => loamsApi(api.ctx).listCollections() },
    {
      id: "describeCollection",
      handle: async (params, api) =>
        loamsApi(api.ctx).describeCollection(
          requireText(params, "collection", "describeCollection"),
        ),
    },
    {
      id: "listVersions",
      handle: async (params, api) =>
        loamsApi(api.ctx).listVersions(requireText(params, "collection", "listVersions")),
    },
    {
      id: "search",
      handle: async (params, api) =>
        loamsApi(api.ctx).search({
          collection: requireText(params, "collection", "search"),
          retrievers: optionalJson(params, "retrievers") as LoamsRetriever[] | undefined,
          filter: optionalJson(params, "filter") as LoamsQuery | undefined,
          fusion: optionalJson(params, "fusion") as LoamsFusion | undefined,
          limit: typeof params.limit === "number" ? params.limit : undefined,
          offset: typeof params.offset === "number" ? params.offset : undefined,
          sort: optionalJson(params, "sort") as LoamsSearchRequest["sort"] | undefined,
          search_after: optionalJson(params, "search_after") as
            | LoamsSearchRequest["search_after"]
            | undefined,
          score_threshold:
            typeof params.score_threshold === "number" ? params.score_threshold : undefined,
          select: optionalJson(params, "select") as LoamsProjection | undefined,
          aggregations: optionalJson(params, "aggregations"),
          group_by: optionalJson(params, "group_by") as LoamsSearchRequest["group_by"] | undefined,
          track_total_hits: optionalJson(params, "track_total_hits") as
            | LoamsTrackTotalHits
            | undefined,
        }),
    },
    {
      id: "getDocuments",
      handle: async (params, api) => {
        const ids = params.ids;
        if (!Array.isArray(ids) || ids.length === 0) {
          throw new Error('getDocuments: "ids" is required and must be a non-empty array');
        }
        return loamsApi(api.ctx).getDocuments(
          requireText(params, "collection", "getDocuments"),
          ids as LoamsPrimaryKey[],
          {
            select: optionalJson(params, "select") as LoamsProjection | undefined,
          },
        );
      },
    },
    {
      id: "scrollDocuments",
      handle: async (params, api) =>
        loamsApi(api.ctx).scrollDocuments(requireText(params, "collection", "scrollDocuments"), {
          after: params.after as LoamsPrimaryKey | undefined,
          limit: typeof params.limit === "number" ? params.limit : undefined,
          filter: optionalJson(params, "filter") as LoamsQuery | undefined,
          select: optionalJson(params, "select") as LoamsProjection | undefined,
        }),
    },
    {
      id: "countDocuments",
      handle: async (params, api) =>
        loamsApi(api.ctx).countDocuments(requireText(params, "collection", "countDocuments"), {
          filter: optionalJson(params, "filter") as LoamsQuery | undefined,
        }),
    },
    {
      id: "getStatus",
      handle: async (_params, api) => {
        const service = loamsApi(api.ctx);
        const [healthy, ready] = await Promise.all([service.health(), service.ready()]);
        return { healthy, ready };
      },
    },
  ],
};

export { UpstreamError, LOAMS_DEFAULT_LIMIT, LOAMS_DEFAULT_RRF_K };
