/**
 * Loams wire types.
 *
 * Verified against `loams-dev` at `main`, 33 `loams-*` crates, edition 2024,
 * rust 1.97.1, Apache-2.0 (no copyleft). Every field below carries the
 * `file:line` it was read from. Nothing here is inferred from documentation
 * prose alone: where a shape could not be pinned down from source it is
 * `unknown` and says so, because a wrong field name in a dashboard adapter is
 * indistinguishable from an upstream bug.
 *
 * WHAT LOAMS IS
 * -------------
 * A Rust **hybrid-retrieval / data engine**: Lance + Tantivy + DataFusion, with
 * vector kNN fused with BM25 through RRF (`README.md:3`). It is an external
 * HTTP service this adapter READS. It is not a plugin runtime, not a Convex-style
 * BaaS, and there is **no Rust Cordis** — `grep -ril cordis --include='*.rs'`
 * over `crates/` returns nothing. The "cordis" in the loams-dev tree is
 * `web/packages/cordis`, a browser-side npm cordis `4.0.0-rc.10` that drives the
 * Rust server over HTTP. It has nothing to do with `@loams-core/host`'s cordis services
 * and nothing here bridges them.
 *
 * MATURITY — READ BEFORE RELYING ON ANY OF THIS
 * ---------------------------------------------
 * `README.md:7`: "Early and moving fast. Loams has no stable release yet, and
 * APIs, formats and flags change without notice. … nothing is published yet."
 * Most design documents carry `Status: **Proposed**` and the console API is
 * mock-only (`README.md:57`). `loams.live.v1` and the streaming gRPC API are
 * both "In progress" (`README.md:59-60`) and are deliberately NOT modelled here.
 * Optional fields below are optional because the contract is not frozen, not
 * because the server is inconsistent.
 *
 * NO CODE EXECUTION — VERIFIED, NOT ASSUMED
 * ------------------------------------------
 * There is no `/exec`, `/functions`, `/eval` or `/invoke` route anywhere in the
 * route table (`crates/loams/src/api/mod.rs:117-189`) and no interpreter in the
 * dependency graph: `wasmtime|wasmer|workerd|gvisor|extism|deno|rquickjs` appear
 * in neither `Cargo.toml` nor `Cargo.lock`. The only code-shaped RPC is
 * `loams.live.v1.Deploy` (`proto/loams/live/v1/live.proto:32`), which takes a
 * JavaScript ES module bundle and is **UNIMPLEMENTED** — `rquickjs` is absent,
 * the intended `loams-live-js` crate does not exist, and the work is deferred at
 * `crates/loams-live/src/txn.rs:644`. Its `max_js_cpu` limit is a declared field
 * with nothing enforcing it. Nothing in this adapter can reach it.
 */

/* -------------------------------------------------------------------------- */
/* Identifiers                                                                 */
/* -------------------------------------------------------------------------- */

/**
 * A document's primary key, in its JSON form
 * (`crates/loams-query/src/json/pk.rs:14-18`).
 *
 * An unsigned integer, a string, or a UUID object. The UUID form is written
 * lowercase hyphenated `8-4-4-4-12` and parsed case-insensitively.
 */
export type LoamsPrimaryKey = number | string | { uuid: string };

/**
 * A collection or stream id (`crates/loams-common/src/id.rs:26-30`).
 *
 * `#[serde(transparent)]` over `u64`, so these are JSON **numbers** on the wire,
 * not strings — unlike a UUID document key.
 */
export type LoamsNumericId = number;

/* -------------------------------------------------------------------------- */
/* Consistency                                                                 */
/* -------------------------------------------------------------------------- */

/**
 * How fresh a read must be (`crates/loams-query/src/ir.rs:110-126`).
 *
 * `snake_case` externally tagged. The server default is `Strong` (`ir.rs:114`),
 * so a native read sees every write acknowledged before the read began.
 *
 * `at_least` and `pinned` are the two that carry data. `at_least` names a
 * `ConsistencyToken`, whose text form is `v1:<s7/p3@918274>`
 * (`crates/loams-query/src/json/token.rs:1`). The SAME token can also be sent as
 * the `Loams-Consistency-Token` REQUEST HEADER
 * (`crates/loams/src/api/mod.rs:57-59`), where it upgrades a body consistency
 * rather than replacing it.
 */
export type LoamsReadConsistency =
  | "strong"
  | "eventual"
  | { at_least: string }
  | { pinned: { manifest_version: number; token: string } };

/**
 * The consistency-token request header name
 * (`crates/loams/src/api/mod.rs:57-59`).
 *
 * Sent on every read response by the server too, so a client can chain the token
 * a read returned into its next read.
 */
export const LOAMS_CONSISTENCY_TOKEN_HEADER = "Loams-Consistency-Token";

/** A `ConsistencyToken` as text, e.g. `v1:s7/p3@918274`. */
export type LoamsConsistencyToken = string;

/* -------------------------------------------------------------------------- */
/* Collections                                                                 */
/* -------------------------------------------------------------------------- */

/**
 * `GET /v1/namespaces/{ns}/collections/{c}` — a collection, also aliased
 * (`crates/loams-query/src/types.rs:54-80`).
 *
 * `schema` goes through a bespoke wire adapter (`types.rs:58-59`) and is typed
 * `unknown` here: it is a whole `CollectionSchema` whose exact JSON was not
 * enumerated for this adapter. Callers that need it should read the schema from
 * the collection they just described rather than assuming a field.
 *
 * `hot` is present on this response but is NOT the serde form of the field at
 * `types.rs:73`: the describe handler REPLACES it with the owner node's full hot
 * status (`crates/loams/src/api/collections.rs:66-81`, `value["hot"] = …`), and
 * only when this node actually owns the collection. On a forwarded read the
 * `CollectionInfo` form survives instead. Hence `optional`.
 */
export interface LoamsCollectionInfo {
  id: LoamsNumericId;
  name: string;
  namespace: string;
  /** Bespoke wire form. NOT enumerated here — see the note above. */
  schema: unknown;
  partitions: number;
  /** Sorted. */
  aliases: string[];
  /** The collection's log stream. `#[serde(transparent)] u64`. */
  stream: LoamsNumericId;
  /** 0 before the first commit. */
  manifest_version: number;
  /** The live manifest's `live_doc_count`. */
  live_doc_count: number;
  size_bytes: number;
  /** From the `loams.created_at_ms` annotation; 0 without it. */
  created_at_ms: number;
  /** Σ over partitions of (high watermark − applied). */
  link_lag_records: number;
  /** Present only on the owner's own node — see the note above. */
  hot?: LoamsHotStatus;
  /** `#[serde(default)]` — absent on a build without the field. */
  unapplied_bytes?: number;
  /** `#[serde(default)]` — absent on a build without the field. */
  backpressure?: unknown;
}

/**
 * Hot-tier state for one structure (`crates/loams-query/src/hot.rs:114-119`).
 */
export interface LoamsHotState {
  /** `off` | `building` | `ready` (`hot.rs:121-128`). */
  state: string;
  /** The manifest version the structure reflects. */
  source_version?: number | null;
}

/** The three hot structures and their states (`crates/loams-query/src/hot.rs:107-112`). */
export interface LoamsHotStatus {
  vectors: LoamsHotState;
  text: LoamsHotState;
  fragments: LoamsHotState;
}

/**
 * `GET /v1/namespaces/{ns}/collections` (`crates/loams/src/api/collections.rs:56-64`).
 *
 * Sorted by name. An UNKNOWN namespace answers `{"collections": []}` rather than
 * a 404 — `list_collections` returns an empty vec when the namespace id does not
 * resolve (`crates/loams-query/src/service.rs:564-567`). An empty list is
 * therefore ambiguous between "no collections" and "no such namespace".
 */
export interface LoamsCollectionsResponse {
  collections: LoamsCollectionInfo[];
}

/**
 * One retained manifest version (`crates/loams-query/src/types.rs:161-169`).
 */
export interface LoamsManifestInfo {
  version: number;
  created_at_ms: number;
  size_bytes: number;
  live_doc_count: number;
  lance_version: number;
}

/**
 * `GET /v1/namespaces/{ns}/collections/{c}/versions` — oldest first
 * (`crates/loams/src/api/collections.rs:129-134`).
 */
export interface LoamsVersionsResponse {
  versions: LoamsManifestInfo[];
}

/* -------------------------------------------------------------------------- */
/* Documents                                                                   */
/* -------------------------------------------------------------------------- */

/**
 * A stored document (`crates/loams-query/src/types.rs:148-159`), re-keyed for
 * the native API.
 *
 * THE KEY IS `id`, NOT `pk`. `StoredDoc`'s serde field is `pk`, but
 * `stored_doc_json` renames it in place on the way out
 * (`crates/loams/src/api/collections.rs:529-546`). This is the one place the
 * native API's document shape differs from the internal struct, and it is the
 * difference between a working dashboard and one that renders every row blank.
 *
 * `sparse_vectors` is omitted from JSON when empty (`types.rs:153-155`), hence
 * optional.
 */
export interface LoamsStoredDoc {
  id: LoamsPrimaryKey;
  source?: Record<string, unknown> | null;
  /** Dense vectors by name. */
  vectors: Record<string, number[]>;
  /** Omitted when empty. */
  sparse_vectors?: Record<string, unknown>;
  /** Typed (non-`_source`) fields by name. */
  fields: Record<string, unknown[]>;
  seq_no: number;
  partition: number;
}

/**
 * `POST …/collections/{c}/documents/get` (`crates/loams/src/api/collections.rs:297-331`).
 *
 * The array is in REQUEST ORDER and holds `null` for an id that does not exist
 * (`collections.rs:323-327`) — length always equals the number of ids asked for.
 */
export interface LoamsGetDocumentsResponse {
  documents: (LoamsStoredDoc | null)[];
  read_token: LoamsConsistencyToken;
}

/**
 * `POST …/collections/{c}/documents/scroll`
 * (`crates/loams/src/api/collections.rs:334-373`).
 *
 * Pagination is primary-key ORDER with an `after` cursor, not an offset
 * (`collections.rs:348-352`). `next` is the id to continue after and is `null`
 * at the end of the collection.
 */
export interface LoamsScrollResponse {
  documents: LoamsStoredDoc[];
  next: LoamsPrimaryKey | null;
  read_token: LoamsConsistencyToken;
}

/**
 * `POST …/collections/{c}/documents/count`
 * (`crates/loams/src/api/collections.rs:379-403`).
 *
 * The dashboard workhorse: a filtered document count in one round trip.
 */
export interface LoamsCountResponse {
  count: number;
  read_token: LoamsConsistencyToken;
}

/* -------------------------------------------------------------------------- */
/* Query IR                                                                    */
/* -------------------------------------------------------------------------- */

/**
 * A field value in a filter or a result (`crates/loams-query/src/ir.rs:465-479`).
 *
 * JSON: a string, a bool, an integer, a float, or `{"date": "<RFC 3339>"}`
 * (`ir.rs:465-467`, adapter at `crates/loams-query/src/json/values.rs`). Dates
 * are µs since the epoch on the Rust side (`ir.rs:477-478`) but RFC 3339 on the
 * wire. Integers above `i64::MAX` come back unsigned (`ir.rs:481-489`), which
 * JSON cannot distinguish from a positive float — `number` is the honest type.
 */
export type LoamsFieldValue = string | number | boolean | { date: string };

/**
 * A sort value (`crates/loams-query/src/ir.rs:546-556`).
 *
 * JSON: `null`, a bool, an integer as for {@link LoamsFieldValue}, a float, a
 * string, or `{"uuid": "…"}` (`ir.rs:545-547`).
 */
export type LoamsSortValue = null | string | number | boolean | { uuid: string };

/** How a field value is FINE-grained: `auto` or an edit distance 0–2 (`ir.rs:456-462`). */
export type LoamsFuzziness = "auto" | 0 | 1 | 2;

/**
 * Which part of `_source` a read returns
 * (`crates/loams-query/src/types.rs:40-50`, serialized at
 * `crates/loams-query/src/json/values.rs:227-236`).
 *
 * JSON is the string `"all"`, the string `"none"`, or an object with `include`
 * and `exclude` ES path patterns. Default `All`.
 */
export type LoamsSourceFilter = "all" | "none" | { include: string[]; exclude: string[] };

/**
 * What a read returns of each document (`crates/loams-query/src/types.rs:25-35`).
 *
 * The field is named `source` and takes a {@link LoamsSourceFilter}, NOT a field
 * list — that is the one genuinely surprising name in the IR.
 */
export interface LoamsProjection {
  source?: LoamsSourceFilter;
  /** Dense and sparse vectors to return, by name. */
  vectors?: string[];
  /** Typed fields to return (`Hit.fields`), by name. */
  fields?: string[];
}

/**
 * A dense-vector ANN parameter override (`crates/loams-query/src/ir.rs:224-239`).
 *
 * `distance` is only meaningful with `exact: true` (`ir.rs:238-239`) and is a
 * string on the wire via the `crate::json::schema::distance` adapter, so it is
 * left as `string` rather than an invented enum.
 */
export interface LoamsAnnParams {
  exact?: boolean;
  nprobes?: number;
  refine_factor?: number;
  ef?: number;
  oversampling?: number;
  distance?: string;
}

/** Sparse-retriever parameters (`crates/loams-query/src/ir.rs:216-222`). */
export interface LoamsSparseParams {
  /** The rows IDF statistics count. Default: every live row. */
  idf_corpus?: LoamsQuery;
}

/**
 * A source of ranked candidates (`crates/loams-query/src/ir.rs:133-170`).
 *
 * Externally tagged in `snake_case`, so each variant is a single-key object.
 * `vector` and `text` are the two the adapter's skills build; `sparse`,
 * `fused` and `rescore` are modelled because they exist on the wire and a
 * caller may want them, but no skill constructs them.
 */
export type LoamsRetriever =
  | {
      vector: {
        field: string;
        /** The query vector, `f32` per dimension. */
        query: number[];
        k: number;
        params?: LoamsAnnParams;
        filter?: LoamsQuery;
      };
    }
  | { text: { query: LoamsQuery; k: number } }
  | {
      sparse: {
        field: string;
        query: unknown;
        k: number;
        filter?: LoamsQuery;
        params?: LoamsSparseParams;
      };
    }
  | { fused: { inputs: LoamsRetriever[]; fusion: LoamsFusion; k: number } }
  | { rescore: { input: LoamsRetriever; field: string; query: number[]; k: number } };

/**
 * How ranked lists are combined (`crates/loams-query/src/ir.rs:247-259`).
 *
 * NOTE THE DEFAULT. `SearchRequest.fusion` is `Option<Fusion>` with
 * `#[serde(default)]` (`ir.rs:44-45`), so an ABSENT `fusion` means NO fusion —
 * not RRF. RRF's `k = 60` default (`ir.rs:28-30`, `ir.rs:251-255`) applies only
 * once a `fusion` is present at all. This adapter does not add one behind the
 * caller's back; see `buildSearchRequest`.
 */
export type LoamsFusion =
  | { rrf: { k?: number } }
  | { dbsf: Record<string, never> }
  | { weighted_sum: { weights: number[] } };

/** The RRF rank constant Loams defaults to (`crates/loams-query/src/ir.rs:28-30`). */
export const LOAMS_DEFAULT_RRF_K = 60;

/** The `limit` a `SearchRequest` gets when the key is missing (`crates/loams-query/src/ir.rs:28-30`). */
export const LOAMS_DEFAULT_LIMIT = 10;

/** The page size `documents/scroll` uses when `limit` is absent (`crates/loams/src/api/collections.rs:29`). */
export const LOAMS_DEFAULT_SCROLL_LIMIT = 100;

/**
 * A query over a collection's fields (`crates/loams-query/src/ir.rs:269-378`).
 *
 * Externally tagged in `snake_case`, one key per variant — the same ES-Query-DSL-
 * shaped family the Elasticsearch compatibility surface reuses. Modelled in full
 * because `filter` and the `text` retriever both take one and a partial union
 * would silently reject valid filters.
 */
export type LoamsQuery =
  | { match_all: Record<string, never> }
  | { match_none: Record<string, never> }
  | {
      match: {
        field: string;
        text: string;
        /** `and` | `or` (`BoolOperator`). */
        operator?: string;
        minimum_should_match?: string;
        fuzziness?: LoamsFuzziness;
        analyzer?: string;
      };
    }
  | { match_phrase: { field: string; text: string; slop?: number } }
  | {
      multi_match: {
        /** `[[name, boost], …]`. */
        fields: [string, number][];
        text: string;
        kind?: string;
        operator?: string;
        tie_breaker?: number;
      };
    }
  | { term: { field: string; value: LoamsFieldValue } }
  | { terms: { field: string; values: LoamsFieldValue[] } }
  | {
      range: {
        field: string;
        gt?: LoamsFieldValue;
        gte?: LoamsFieldValue;
        lt?: LoamsFieldValue;
        lte?: LoamsFieldValue;
      };
    }
  | { exists: { field: string } }
  | { is_null: { field: string } }
  | { is_empty: { field: string } }
  | {
      values_count: {
        field: string;
        gt?: number;
        gte?: number;
        lt?: number;
        lte?: number;
      };
    }
  | { prefix: { field: string; value: string } }
  | { wildcard: { field: string; pattern: string } }
  | { fuzzy: { field: string; value: string; fuzziness: LoamsFuzziness } }
  | { ids: LoamsPrimaryKey[] }
  | { query_string: { query: string; default_fields?: string[]; default_operator?: string } }
  | {
      bool: {
        must?: LoamsQuery[];
        should?: LoamsQuery[];
        must_not?: LoamsQuery[];
        filter?: LoamsQuery[];
        minimum_should_match?: string;
      };
    }
  | { boost: { query: LoamsQuery; boost: number } }
  | { constant_score: { query: LoamsQuery; score: number } };

/** Sort direction (`crates/loams-query/src/ir.rs:515-517`, `snake_case`). */
export type LoamsSortOrder = "asc" | "desc";

/** Where a missing value sorts (`ir.rs`, `MissingOrder` default is `last`). */
export type LoamsMissingOrder = "first" | "last";

/**
 * One key of the effective sort (`crates/loams-query/src/ir.rs:493-513`).
 *
 * The primary key ascending always breaks ties (R10, `ir.rs:491-492`), so a
 * stable sort needs no explicit tiebreaker.
 */
export type LoamsSortKey =
  | { score: { order?: LoamsSortOrder } }
  | { pk: { order?: LoamsSortOrder } }
  | { field: { field: string; order?: LoamsSortOrder; missing?: LoamsMissingOrder } };

/**
 * Group hits by a field value — Qdrant's `group_by`
 * (`crates/loams-query/src/ir.rs:635-646`). `group_size` defaults to 3
 * (`ir.rs:632-634`); `limit` defaults to 10 (`ir.rs:639-641`).
 */
export interface LoamsGroupBy {
  field: string;
  group_size?: number;
  limit?: number;
}

/**
 * Whether, and how far, the total number of matches is counted
 * (`crates/loams-query/src/ir.rs:655-663`).
 *
 * `snake_case` externally tagged: `"none"` (the default — do not count),
 * `"exact"`, or `{"up_to": n}`.
 */
export type LoamsTrackTotalHits = "none" | "exact" | { up_to: number };

/**
 * `POST /v1/namespaces/{ns}/query` — one search over one collection
 * (`crates/loams-query/src/ir.rs:34-66`).
 *
 * This is the CANONICAL form. The same endpoint also accepts the §05 §4 "hybrid"
 * body — `from` / `retrieve` / `fuse` (`crates/loams-query/src/json/hybrid.rs:27-33`,
 * the key README example at `README.md:118-127`) — but only when the body
 * carries `from` or `retrieve`; otherwise it is deserialized as this struct
 * (`hybrid.rs:29-32`). The adapter emits the canonical form because its defaults
 * are explicit in source, whereas the hybrid form silently drops `sort`,
 * `search_after`, `score_threshold`, `aggregations`, `highlight`, `group_by` and
 * `track_total_hits`: it allows exactly eight keys and rejects the rest
 * (`hybrid.rs:48-58`).
 */
export interface LoamsSearchRequest {
  /** A collection name or an alias. */
  collection: string;
  /** Default `Strong` (`ir.rs:39-41`). */
  consistency: LoamsReadConsistency;
  retrievers: LoamsRetriever[];
  /** ABSENT means no fusion — NOT RRF. See {@link LoamsFusion}. */
  fusion?: LoamsFusion;
  filter?: LoamsQuery;
  sort: LoamsSortKey[];
  offset: number;
  /** Default 10 (`ir.rs:52-54`). */
  limit: number;
  search_after?: LoamsSortValue[];
  score_threshold?: number;
  select: LoamsProjection;
  /**
   * An Elasticsearch aggregation request, translated to Tantivy on the server
   * (`ir.rs:59-61`). Passed through as opaque JSON: the aggregation grammar is
   * Tantivy's, not Loams', and was not enumerated here.
   */
  aggregations?: unknown;
  /** Also Tantivy-shaped; not enumerated here. */
  highlight?: unknown;
  group_by?: LoamsGroupBy;
  track_total_hits: LoamsTrackTotalHits;
}

/**
 * What a search returns (`crates/loams-query/src/ir.rs:679-691`).
 *
 * `hot_used` is OMITTED from JSON when empty (`ir.rs:687-688`).
 */
export interface LoamsSearchResponse {
  hits: LoamsHit[];
  total?: LoamsTotalHits | null;
  aggregations?: unknown;
  groups?: LoamsHitGroup[] | null;
  /** The state the read saw, in token text form (`ir.rs:685-686`). */
  read_token: LoamsConsistencyToken;
  /** Omitted when empty. */
  hot_used?: string[];
}

/** Whether `total.value` is exact or a floor (`crates/loams-query/src/ir.rs:664-678`). */
export interface LoamsTotalHits {
  value: number;
  /** `eq` | `gte` (`ir.rs:675-678`). */
  relation: string;
}

/** One group of a grouped search (`crates/loams-query/src/ir.rs:649-653`). */
export interface LoamsHitGroup {
  key: LoamsFieldValue;
  hits: LoamsHit[];
}

/**
 * One hit (`crates/loams-query/src/ir.rs:693-710`).
 *
 * Note `pk`, NOT `id`: a hit carries the raw `StoredDoc` key under `pk`
 * (`ir.rs:695-696`), while `documents/get` and `documents/scroll` rename it to
 * `id`. Same value, different name, different endpoint — see
 * {@link LoamsStoredDoc}.
 *
 * `sparse_vectors` and `fields` are omitted when empty (`ir.rs:702-703`,
 * `ir.rs:707-708`).
 */
export interface LoamsHit {
  pk: LoamsPrimaryKey;
  score: number;
  sort_values: LoamsSortValue[];
  source?: Record<string, unknown> | null;
  vectors: Record<string, number[]>;
  sparse_vectors?: Record<string, unknown>;
  highlight: Record<string, string[]>;
  fields?: Record<string, unknown[]>;
}

/* -------------------------------------------------------------------------- */
/* SQL                                                                         */
/* -------------------------------------------------------------------------- */

/**
 * One result column (`crates/loams-query/src/sql/json_rows.rs:20-25`).
 *
 * `type` is `format!("{}", field.data_type())` — DataFusion's own type name,
 * e.g. `Utf8`, `Int64`, `Float64`.
 */
export interface LoamsSqlColumn {
  name: string;
  type: string;
}

/**
 * `POST /v1/namespaces/{ns}/sql` (`crates/loams/src/api/sql.rs:11-40`,
 * `rows_to_json` at `crates/loams-query/src/sql/json_rows.rs:19-39`).
 *
 * `rows` is an array of positional arrays, NOT objects — `columns[i].name` is
 * what maps row `i` to a field (`json_rows.rs:26-37`). `truncated` is the
 * server's own row cap; a `true` here means the answer is a PREFIX and must not
 * be reported as a complete count.
 *
 * The request body is `{"query": string, "consistency"?: ReadConsistency}` with
 * `deny_unknown_fields` (`crates/loams/src/api/sql.rs:17-21`).
 */
export interface LoamsSqlResult {
  columns: LoamsSqlColumn[];
  rows: unknown[][];
  truncated: boolean;
}

/* -------------------------------------------------------------------------- */
/* Errors                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * Loams' error body (`crates/loams/src/api/errors.rs:12-18`, rendered at
 * `errors.rs:64-79`).
 *
 * EVERY error is `{"error": <code>, "message": <text>}` plus whatever extra
 * fields the layer attached — `"index"` on a failed document op
 * (`crates/loams/src/api/collections.rs:199-201`), `"matched"`/`"limit"` on a
 * filter write over its budget (`collections.rs:451-478`). Every 503 carries
 * `Retry-After: 1` (`errors.rs:75-77`).
 */
export interface LoamsApiErrorBody {
  /** e.g. `invalid_argument`, `not_found`, `unavailable`. */
  error: string;
  message: string;
  [key: string]: unknown;
}

/* -------------------------------------------------------------------------- */
/* Config                                                                      */
/* -------------------------------------------------------------------------- */

export interface LoamsConfig {
  /**
   * The native HTTP listener root, e.g. `http://127.0.0.1:8080`
   * (`README.md:112` — `loams dev`'s default for the native HTTP API).
   *
   * NOT a Connect-RPC endpoint and NOT a Qdrant/Elasticsearch port; see the
   * service module comment for why the plain REST surface is the one used.
   */
  baseUrl: string;

  /**
   * The namespace every call is addressed under.
   *
   * REQUIRED, and there is no way around it: the native API has no
   * list-namespaces route. `POST /v1/namespaces` CREATES one
   * (`crates/loams/src/api/mod.rs:114`, handler at `mod.rs:277-287`) and there
   * is no GET counterpart anywhere in the table (`mod.rs:117-189`). `/ready` is a
   * bare status code with no body (`mod.rs:260-266`) and `/health` likewise
   * (`mod.rs:257-259`). Nothing on the server will enumerate namespaces, so this
   * adapter never guesses one and fails with an explicit message instead.
   *
   * The README example uses `demo` (`README.md:118`).
   */
  namespace?: string;

  /**
   * OPTIONAL bearer token, sent as `Authorization: Bearer <token>`.
   *
   * Loams has NO authentication today, and this is not a guess: `README.md:62`
   * lists "Auth and tenancy — API keys, authorization, tenant quotas and a
   * namespace router" as **Planned**, and the source agrees — there is no auth
   * middleware anywhere on the native router
   * (`crates/loams/src/api/mod.rs:107-176`), and the internal routes are
   * documented as "Unauthenticated, like every listener in M1"
   * (`crates/loams/src/api/internal.rs:5`).
   *
   * So this field exists for the SECURED deployment that `README.md:62` says is
   * coming — a reverse proxy or gateway in front of Loams, which is the normal
   * way this will actually be run. When it is absent the adapter sends no
   * `Authorization` header at all and talks to Loams as `{kind: "none"}`.
   */
  token?: string;

  /**
   * Opt in to {@link LoamsAdapterService.sql}. **Defaults to false.**
   *
   * `POST /v1/namespaces/{ns}/sql` takes RAW SQL TEXT and runs it against
   * DataFusion (`README.md:138`), e.g.
   * `{"query":"SELECT _id, body, _score FROM rrf(vector_search('kb',[1,0,0],'embedding',10), text_search('kb','refund','body',10)) LIMIT 3"}`.
   *
   * The mitigating factor is real: this SQL surface accepts no user-supplied
   * code. DataFusion's extensions here are a fixed set of internally-registered
   * table functions — `vector_search`, `text_search`, `hybrid_search`, `rrf`,
   * `rerank` — plus a handful of scalar UDFs
   * (`crates/loams-query/src/sql/mod.rs:127-144`). And `SELECT`-only enforcement
   * happens SERVER-side: `run_read_only`
   * (`crates/loams-query/src/sql/mod.rs:176-182`) walks the plan through
   * `read_only_options().verify_plan` and refuses anything else with "only
   * read-only queries are allowed" (`sql/mod.rs:206-221`), and it is that
   * function the handler calls (`crates/loams/src/api/sql.rs:34`).
   *
   * That enforcement is NOT in this client. This adapter makes no claim to
   * sanitise the statement — it gates the capability and nothing more.
   */
  allowSql?: boolean;

  /** Per-request timeout in ms. Defaults to 30s via the shared client. */
  timeoutMs?: number;
}
