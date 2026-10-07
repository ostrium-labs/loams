//! `loams.collection.v1` `QueryService/Search` on the main port (design §44
//! §4 and §5.1, API1 plan ruling 1; `docs/api/route-map.md` line 57), against
//! the real `loams` server over its `--listen` address.
//!
//! Every test here is a port of a REST test in `crates/loams/tests/it/` by
//! its name with an `_rpc` suffix, or a test the API1 plan names outright
//! (`scroll_streams_pages_with_cursor`), or an invariant this file pins on its
//! own. The behaviour is the REST behaviour: the handler calls the same
//! `CollectionService::search` the REST handler calls, only the shape of the
//! call changed.
//!
//! ## How these tests reach the RPCs
//!
//! Nothing here names a generated Rust type: a compile error is not a red
//! test, and it is how the first draft of this file (and of
//! `connect_documents.rs`) failed to build at all. Every RPC is a Connect
//! unary `POST` of JSON to `/<package>.<Service>/<Method>` over `reqwest`,
//! the shape `curl` sends (design §44 §4), and every answer is read as
//! `serde_json::Value`. The path is one constant at the top of this file, so
//! the package stays the design's: **`loams.collection.v1`**.
//!
//! Every `{got}` in this file formats a type that implements `Display`
//! (a `serde_json::Value`, a `&str`, a `String`, a number); every `{got:?}`
//! formats a map, a vector or a set, which is the correction the previous
//! draft of this task needed.
//!
//! ## Seeding data
//!
//! Collections and namespaces exist as RPCs (`loams.collection.v1`, Task 2)
//! and documents as `DocumentService/WriteDocuments` (Task 3), so **every**
//! document in this file is written by an RPC: the `kb` and `sp` fixtures are
//! created over `CreateCollection` and written over `WriteDocuments`,
//! deliberately none over the native REST routes, which would let a test pass
//! while the RPCs are broken. The REST route is used in exactly two places,
//! and both are the *point* of the test: `routing_and_ranking_match_rest_for_every_fixture`
//! runs the same fixture through both surfaces, and
//! `loams_hot_used_is_none_without_a_hot_tier_and_off_is_honoured_rpc`
//! checks the one header a REST reader of the hot layer still uses.
//!
//! ## The central assertion: the RPC **equals** REST
//!
//! `routing_and_ranking_match_rest_for_every_fixture` runs each fixture's
//! **same body** through `POST /v1/namespaces/{ns}/query` and through
//! `QueryService/Search` and compares the two answers **against each other**,
//! never against a hardcoded expectation: the ids, their order, their scores,
//! their sources and their totals. That is the test that fails if a filter, a
//! retriever, a fusion or a sort key silently degrades on one surface — and
//! it fails with a diff rather than with a stale golden number.
//!
//! Every other test keeps the REST suite's own hardcoded expectations, so a
//! *change* to ranking on both surfaces at once is still caught.
//!
//! ## **WIRE** — `snake_case` → `lowerCamelCase`: the mapping table
//!
//! The REST IR is serde `snake_case`; **proto3 JSON is `lowerCamelCase`**, and
//! that is an unavoidable divergence between the two surfaces. These are the
//! names this file sends and expects. A key with no `_` in it is the same
//! word on both surfaces and is not in the table; a key with a `_` in it that
//! is **not** in this table fails [`camel`] at runtime, so the table cannot
//! drift away from the fixtures.
//!
//! | REST (`SearchRequest`, the IR) | RPC (`SearchRequest`) |
//! |---|---|
//! | `collection` | `collection` (also spelled `from`, see below) |
//! | `consistency` | `consistency` (the `Consistency` message, Task 3) |
//! | `retrievers` | `retrievers` (also spelled `retrieve`, see below) |
//! | `fusion` | `fusion` (also spelled `fuse`, see below) |
//! | `filter` | `filter` |
//! | `sort` | `sort` |
//! | `offset` | `offset` |
//! | `limit` | `limit` |
//! | `search_after` | `searchAfter` |
//! | `score_threshold` | `scoreThreshold` |
//! | `select` | `select` (a `Struct` projection, Task 2/3's ruling) |
//! | `aggregations` | `aggregations` |
//! | `highlight` | `highlight` |
//! | `group_by` | `groupBy` |
//! | `track_total_hits` | `trackTotalHits` |
//!
//! Inside the retrievers, the filter IR and the answer:
//!
//! | REST | RPC |
//! |---|---|
//! | `vector` / `text` / `fused` / `rescore` / `sparse` (arm) | identical (a proto oneof is spelled like serde's external tagging) |
//! | `refine_factor` | `refineFactor` |
//! | `idf_corpus` | `idfCorpus` |
//! | `weighted_sum` (arm) | `weightedSum` |
//! | `match_all` / `match_none` (value, see below) | `{"matchAll": {}}` / `{"matchNone": {}}` |
//! | `match_phrase`, `multi_match`, `values_count`, `is_null`, `is_empty`, `query_string`, `constant_score` (arms) | `matchPhrase`, `multiMatch`, `valuesCount`, `isNull`, `isEmpty`, `queryString`, `constantScore` |
//! | `minimum_should_match` | `minimumShouldMatch` |
//! | `tie_breaker` | `tieBreaker` |
//! | `must_not` | `mustNot` |
//! | `default_fields` | `defaultFields` |
//! | `default_operator` | `defaultOperator` |
//! | `pre_tag` / `post_tag` | `preTag` / `postTag` |
//! | `fragment_size` / `number_of_fragments` | `fragmentSize` / `numberOfFragments` |
//! | `group_size` | `groupSize` |
//! | `up_to` | `upTo` |
//! | `ids: [1, {"uuid": "…"}]` (bare primary keys) | `ids: [{"uint": "1"}, {"uuid": "…"}]` (`DocumentId`, Task 3) |
//! | `read_token` (answer) | `readToken` |
//! | `hot_used` (answer) | `hotUsed` |
//! | `sort_values` | `sortValues` |
//! | `sparse_vectors` | `sparseVectors` |
//!
//! Three more rules, all deliberate:
//!
//! - **A `null` is an absent field.** proto3 JSON has no `null` for a scalar
//!   or a message field, and a field at its default is *absent*. The REST
//!   fixture spells its defaults out (`"nprobes": null`,
//!   `"minimum_should_match": null`, `"fusion": null`), so [`camel`] **drops**
//!   a null key rather than sending it: on the RPC the same value is the field
//!   being absent.
//! - **A oneof arm is `{"arm": {…}}` on both surfaces.** `WriteOp` set this
//!   precedent in this package ("a proto oneof, so the JSON is the REST
//!   route's `{"upsert": {…}}` unchanged"), and `Query`, `Retriever`,
//!   `Fusion` and `SortKey` follow it: the arm names do not change, and a unit
//!   arm's value becomes an empty object because `"match_all"` has no oneof
//!   spelling.
//! - **A hit's `pk` is the `DocumentId` oneof**, the same message
//!   `Document.id` already uses, for Task 3's reason: `google.protobuf.Value`
//!   holds numbers in a `double`, so the fixture's `u64::MAX` id would come
//!   back as `18446744073709552000` — a different document.
//!
//! ## **WIRE** — enums
//!
//! The IR's enums are serde `rename_all = "snake_case"`
//! (`ReadConsistency::AtLeast` → `"at_least"`, `SortOrder::Asc` → `"asc"`),
//! while proto3 JSON emits the **proto name** in `UPPER_SNAKE`. The values
//! this file sends and expects, with this package's `ENUM_NAME_VALUE` prefix
//! convention (the same one `OpResult` and `HotStateKind` already follow):
//!
//! | REST | RPC |
//! |---|---|
//! | `ReadConsistency::Strong` / `Eventual` | `consistency.freshness`: `FRESHNESS_STRONG` / `FRESHNESS_EVENTUAL` (Task 3's `Freshness`) |
//! | `ReadConsistency::AtLeast(token)` | `consistency.atLeast` (a string, Task 3) |
//! | `BoolOperator::Or` / `And` | `BOOL_OPERATOR_OR` / `BOOL_OPERATOR_AND` |
//! | `MultiMatchKind::BestFields`, … | `MULTI_MATCH_KIND_BEST_FIELDS`, … |
//! | `SortOrder::Asc` / `Desc` | `SORT_ORDER_ASC` / `SORT_ORDER_DESC` |
//! | `MissingOrder::First` / `Last` | `MISSING_ORDER_FIRST` / `MISSING_ORDER_LAST` |
//! | `TotalRelation::Eq` / `Gte` | `TOTAL_RELATION_EQ` / `TOTAL_RELATION_GTE` |
//!
//! An enum **read** is asserted by *value* through [`enum_is`], which accepts
//! any spelling that ends in the value's name, so the numbering and the prefix
//! stay the implementer's choice. An enum **sent** cannot be: this file sends
//! exactly one (`operator: BOOL_OPERATOR_OR`, in
//! `filter_ir_is_accepted_in_every_form_the_rest_route_takes`) and that one
//! name is pinned. Everything else this file sends takes its default, which is
//! why no fixture has to name an enum to be meaningful.
//!
//! ## **WIRE** — the §05 §4 body, and what "accepted as the JSON mapping" means
//!
//! Design §05 §4's example body is a **different spelling of the same
//! request**, not a different request, and this file pins how each of its
//! shorthands lands on the message:
//!
//! | §05 §4 | RPC |
//! |---|
//! | `"from": "collections.kb"` | the same key, the same value: `SearchRequest.from`, the alias of `collection`, `"collections.kb"` and `"kb"` both accepted |
//! | `"retrieve": [...]` | the same key: `SearchRequest.retrieve`, the alias of `retrievers`, same element shape |
//! | `"fuse": {"method": "rrf", "k": 60}` | the same key, **verbatim**: `SearchRequest.fuse` is a `google.protobuf.Struct`, so `method` stays the §05 §4 string and needs no `FUSION_METHOD_*` name |
//! | `"consistency": "strong"` | `"consistency": {"freshness": "FRESHNESS_STRONG"}` — `consistency` is the `Consistency` message of this package (Task 3), and a message field has no bare-string form |
//! | `"select": ["id", "_score", "body"]` | `"select": {"source": {"include": ["body"]}}` — `select` is the projection `Struct` of Task 2/3; `id` and `_score` are always answered, so they are not in it |
//! | `"text": {"field": "body", "query": "refund", "k": 10}` | `{"text": {"query": {"match": {"field": "body", "text": "refund"}}, "k": 10}}` — §05 §4's `field` + string `query` *is* the IR's `Query::Match` |
//!
//! Everything else of the §05 §4 body is the IR's own spelling, lowerCamel.
//! The two M3 stages stay refused on both surfaces: a body carrying `expand`
//! or `rerank` is `invalid_argument` and says so, rather than being ignored
//! the way proto3 JSON ignores an unknown field — which is precisely why the
//! hybrid aliases are read *before* the typed decode.
//!
//! ## **WIRE** — `page_token` on the shipped `ScrollDocuments`
//!
//! Plan ruling 2 says `ScrollDocuments` "is server-streaming of pages with a
//! cursor field (provisional default for Q614; **Task 4 may switch to unary
//! pagination if Q614 is answered so) … unary pagination also offered via
//! `page_token`". Q614 is unanswered and the **unary** RPC already shipped in
//! Task 3 with 18 green tests behind it, so this file pins the *additive*
//! reading rather than breaking a shipped contract:
//!
//! - `loams.collection.v1.DocumentService/ScrollDocuments` **stays unary**.
//! - It **gains `page_token`**, the AIP-158 spelling of the same cursor
//!   pagination `after` already is. `page_token` is a `DocumentId`, the same
//!   type as `after`, so there is exactly **one** cursor encoding on the wire
//!   and the two are interchangeable: `scroll_streams_pages_with_cursor`
//!   drives a whole pagination through `page_token`, drives the same
//!   pagination through `after`, and asserts the pages are identical.
//! - `ScrollDocumentsResponse.next` is unchanged: the shipped field.
//! - If both are set they must name the same cursor; a request that sets them
//!   to different cursors is `invalid_argument`. (Not asserted here: it is the
//!   one behaviour with no REST precedent to compare against.)
//!
//! **If the owner wants true server-streaming, that is a *separate additive*
//! RPC** (a `StreamScrollDocuments`, or the same service with a streaming
//! method) and not a change to this one. Q614 stays the question that decides
//! it.
//!
//! ## **WIRE** — `filter` on the document RPCs stays a `Value`
//!
//! Task 4 types the filter IR, but `ScrollDocuments.filter`,
//! `CountDocuments.filter`, `DeleteByFilter.filter` and
//! `PatchByFilter.filter` are already shipped as `google.protobuf.Value` and
//! Task 3's 18 green tests send the **bare string** `"match_all"`, which a
//! typed oneof cannot spell. This file therefore drives every document RPC's
//! filter in the REST flat form (`{"term": {"field": "tenant", "value": "b"}}`),
//! which is valid as a `Value` *and* as a typed `Query::Term` — so this file
//! passes whichever way the implementer reads that field, and Task 3's tests
//! keep passing. The typed `Query` lands on `SearchRequest.filter`.
//!
//! ## `performance`
//!
//! Design §05 §4 and the plan name a `performance` block on every native
//! search answer. Neither surface has one today: `SearchResponse` in
//! `crates/loams-query/src/ir.rs` has no such field and nothing in the query
//! crate computes it. So `SearchResponse`'s documented keys **allow**
//! `performance` (Task 4 is the task that names it) but no test asserts it,
//! and `routing_and_ranking_match_rest_for_every_fixture` compares *ranking*
//! — ids, order, scores, sources, totals — rather than whole bodies, so a
//! `performance` block added to one surface only cannot break the equality.
//! Adding it to one surface and not the other is what this test exists to
//! prevent; adding it to both is a separate task.
//!
//! ## The hot layer reaches the Connect path, or this file cannot pass
//!
//! `api::router` registers the Connect routes with `.merge(connect)` **after**
//! `.layer(hot_layer)`, so today a Connect answer carries no
//! `loams-hot-used` header and `Loams-Hot` is never parsed on an RPC path.
//! Ruling 11 puts every route inside the hot layer and §05 §4 makes the hot
//! tier a query stage, so a `Search` that could not use a hot structure would
//! be a different query engine from the REST one. Merging the Connect router
//! inside the layer (or layering it) is part of this task.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use buffa::Message as _;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The consistency-token header (the REST `Loams-Consistency-Token`).
const TOKEN: &str = "loams-consistency-token";

/// The RPC path, row for row of `docs/api/route-map.md` line 57: §44 §7.2
/// puts `QueryService/Search` in `loams.collection.v1`, and
/// `crates/loams/tests/route_map.rs` fails any other package.
const SEARCH: &str = "/loams.collection.v1.QueryService/Search";

/// Task 3's RPCs, for seeding and for the scroll this task extends.
const CREATE_COLLECTION: &str = "/loams.collection.v1.CollectionService/CreateCollection";
const WRITE_DOCUMENTS: &str = "/loams.collection.v1.DocumentService/WriteDocuments";
const GET_DOCUMENTS: &str = "/loams.collection.v1.DocumentService/GetDocuments";
const SCROLL_DOCUMENTS: &str = "/loams.collection.v1.DocumentService/ScrollDocuments";

/// `SearchResponse`'s documented fields, in proto3 JSON. `performance` is
/// *allowed* and never asserted (see the module's `performance` note).
const SEARCH_KEYS: [&str; 7] = [
    "aggregations",
    "groups",
    "hits",
    "hotUsed",
    "performance",
    "readToken",
    "total",
];

/// `Hit`'s documented fields, in proto3 JSON.
const HIT_KEYS: [&str; 8] = [
    "fields",
    "highlight",
    "pk",
    "score",
    "sortValues",
    "source",
    "sparseVectors",
    "vectors",
];

/// `TotalHits`'s documented fields, in proto3 JSON (`value` is a `uint64`).
const TOTAL_KEYS: [&str; 2] = ["relation", "value"];

/// `ScrollDocumentsResponse`'s documented fields, unchanged by this task.
const SCROLL_KEYS: [&str; 3] = ["documents", "next", "readToken"];

/// `Document`'s documented fields, in proto3 JSON (Task 3).
const DOC_KEYS: [&str; 7] = [
    "fields",
    "id",
    "partition",
    "seqNo",
    "source",
    "sparseVectors",
    "vectors",
];

/// A response: status, headers and JSON body (`null` when not JSON).
#[derive(Debug)]
struct Reply {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Value,
}

impl Reply {
    /// The value of header `name`.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// Asserts the Connect success code (`200`) and returns the body.
    fn expect_ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    /// The error envelope of a failed RPC, for the code and reason assertions.
    fn expect_error(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        assert!(
            self.body.get("code").and_then(Value::as_str).is_some(),
            "a Connect error names its code: {}",
            self.body
        );
        self.body
    }

    /// The human-readable message of a failed RPC. The `ErrorInfo` has no
    /// `message` field (reason, metadata, hint), so the prose lives in the
    /// Connect envelope's `message`.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a Connect error carries a message: {}", self.body))
    }

    /// The `loams-consistency-token` header, or the assertion that says so.
    fn token(&self) -> String {
        self.header(TOKEN)
            .unwrap_or_else(|| panic!("the answer carries a {TOKEN} header: {:?}", self.headers))
            .to_string()
    }
}

/// An in-process server on an ephemeral port, with fast background work.
struct Running {
    server: Server,
    base: String,
    http: reqwest::Client,
    _dir: TempDir,
}

impl Running {
    async fn start() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.link.batch_interval = Duration::ZERO;
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            http: reqwest::Client::new(),
            _dir: dir,
        }
    }

    /// A Connect unary call: `POST` with a JSON body, which is what `curl`
    /// sends (design §44 §4).
    async fn connect(&self, rpc: &str, body: &Value) -> Reply {
        self.connect_with(rpc, &[], body).await
    }

    /// [`Self::connect`], plus request metadata. `loams-hot` is a request
    /// header here for the same reason `loams-backpressure` is on the document
    /// RPCs: it is a transport-level switch with no message-level meaning,
    /// and the answer's half of it is the `loams-hot-used` response header.
    async fn connect_with(&self, rpc: &str, headers: &[(&str, &str)], body: &Value) -> Reply {
        let mut request = self
            .http
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await.expect("send");
        Self::finish(response).await
    }

    async fn finish(response: reqwest::Response) -> Reply {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    /// A native REST call, for the "the RPC answers the same as the REST
    /// route" half of `routing_and_ranking_match_rest_for_every_fixture` and
    /// for the one REST reader of the hot layer.
    async fn rest(&self, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        Self::finish(request.send().await.expect("send")).await
    }

    /// `POST /v1/namespaces/{ns}/query` — the whole of `api::query::search`.
    async fn rest_query(&self, ns: &str, body: &Value) -> Reply {
        self.rest(
            Method::POST,
            &format!("/v1/namespaces/{ns}/query"),
            Some(body.clone()),
        )
        .await
    }

    async fn shutdown(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

// ----- Wire-shape helpers -----

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("an object, not {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// proto3 JSON omits every field at its default, so an answer may carry
/// **fewer** keys than the REST one did, but never a key the message does not
/// document. This checks the "never" half.
fn assert_documented_keys(value: &Value, documented: &[&str], what: &str) {
    let present: BTreeSet<&str> = keys(value).into_iter().collect();
    for key in &present {
        assert!(
            documented.contains(key),
            "{what} answers an undocumented key `{key}`: {value}"
        );
    }
}

/// A proto3 JSON field read the way the wire carries it: a field at its
/// default is **absent**, not `0`/`false`/`""`. A number is compared through
/// [`int64`], because every 64-bit integer on the wire is a decimal string.
fn absent_or(value: &Value, key: &str, expected: Value) -> bool {
    match value.get(key) {
        None => {
            matches!(
                expected,
                Value::Null
                    | Value::Bool(false)
                    | Value::String(_)
                    | Value::Array(_)
                    | Value::Object(_)
            ) || expected.as_u64() == Some(0)
                || expected.as_i64() == Some(0)
        }
        Some(got) if expected.is_number() => int64(got) == int64(&expected),
        Some(got) => *got == expected,
    }
}

/// A proto3 JSON 64-bit integer: `int64`/`uint64` answer a **decimal
/// string**. An unquoted number is taken too, because proto3 JSON requires a
/// parser to accept it.
fn int64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// A JSON number inside a value carried as a `google.protobuf.Struct` (or a
/// `Value`): `Struct` holds every number in a `double`, so a whole number
/// answers as `3.0`.
fn struct_number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// Recursively asserts `got == want`, reading every number as a `Struct`
/// number so a whole number answers as `3.0` on one side and `3` on the other.
fn assert_struct_eq(got: &Value, want: &Value, what: &str) {
    match (got, want) {
        (Value::Object(got), Value::Object(want)) => {
            assert_eq!(got.len(), want.len(), "{what}: the keys of {got:?}");
            for (key, expected) in want {
                let value = got
                    .get(key)
                    .unwrap_or_else(|| panic!("{what}: no key `{key}` in {got:?}"));
                assert_struct_eq(value, expected, &format!("{what}.{key}"));
            }
        }
        (Value::Array(got), Value::Array(want)) => {
            assert_eq!(got.len(), want.len(), "{what}: the length of {got:?}");
            for (i, expected) in want.iter().enumerate() {
                assert_struct_eq(&got[i], expected, &format!("{what}[{i}]"));
            }
        }
        _ => {
            if want.is_number() {
                assert_eq!(struct_number(got), struct_number(want), "{what}: {got}");
            } else {
                assert_eq!(got, want, "{what}");
            }
        }
    }
}

/// A proto3 JSON enum answers its **proto name** in `UPPER_SNAKE`
/// (`TOTAL_RELATION_EQ`), where the REST route answered `snake_case` (`eq`),
/// and a value that is the enum's zero variant is omitted like any other
/// default — so an absent key answers `zero`. The assertion is on the value,
/// not on the enum's spelling or its numbering.
fn enum_is(value: &Value, zero: &str, expected: &str) -> bool {
    if value.is_null() {
        return zero.eq_ignore_ascii_case(expected);
    }
    let want = expected.to_ascii_uppercase();
    value.as_str().is_some_and(|name| {
        name.eq_ignore_ascii_case(expected)
            || name.to_ascii_uppercase().ends_with(&format!("_{want}"))
    })
}

/// The `loams.errors.v1.ErrorInfo` in a Connect error's `details`, decoded the
/// way the protocol base64s it.
fn error_info(body: &Value) -> ErrorInfo {
    let details = body["details"]
        .as_array()
        .unwrap_or_else(|| panic!("a Connect error carries details: {body}"));
    let detail = details
        .first()
        .unwrap_or_else(|| panic!("no ErrorInfo detail in {body}"));
    assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{body}");
    let bytes = STANDARD_NO_PAD
        .decode(
            detail["value"]
                .as_str()
                .unwrap_or_else(|| panic!("an encoded ErrorInfo in {body}")),
        )
        .expect("the Connect protocol base64: unpadded standard");
    ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
}

/// Asserts the `reason` of a Connect error is `expected`.
fn assert_reason(body: &Value, expected: &str) {
    assert_eq!(error_info(body).reason, expected, "{body}");
}

/// The `v1:s<stream>/p<partition>@<offset>[,…]` items of a consistency token.
fn token_offsets(token: &str) -> Vec<(u64, u32, u64)> {
    let body = token
        .strip_prefix("v1:")
        .unwrap_or_else(|| panic!("a `v1:` consistency token: {token}"));
    if body.is_empty() {
        return Vec::new();
    }
    body.split(',')
        .map(|item| {
            let (stream, rest) = item
                .split_once("/p")
                .unwrap_or_else(|| panic!("a token item: {item}"));
            let (partition, offset) = rest
                .split_once('@')
                .unwrap_or_else(|| panic!("a token item: {item}"));
            (
                stream
                    .strip_prefix('s')
                    .unwrap_or_else(|| panic!("a token item: {item}"))
                    .parse()
                    .expect("a stream id"),
                partition.parse().expect("a partition"),
                offset.parse().expect("an offset"),
            )
        })
        .collect()
}

/// Whether `newer` is at least as far along as `older` on every partition
/// `older` names — the machine-checkable meaning of `at_least`.
fn token_covers(newer: &str, older: &str) -> bool {
    let newer = token_offsets(newer);
    token_offsets(older)
        .iter()
        .all(|&(stream, partition, offset)| {
            newer
                .iter()
                .filter(|&&(s, p, _)| (s, p) == (stream, partition))
                .map(|&(_, _, offset)| offset)
                .max()
                .unwrap_or(0)
                >= offset
        })
}

// ----- `DocumentId`, the oneof the wire spells `{"uint": "1"}` -----

fn id_uint(n: u64) -> Value {
    json!({"uint": n.to_string()})
}

fn id_str(text: &str) -> Value {
    json!({"string": text})
}

fn id_uuid(uuid: &str) -> Value {
    json!({"uuid": uuid})
}

/// A `DocumentId` (a hit's `pk`, a scroll's `next`) read back as the REST
/// route's spelling, so the assertions read like the tests they were ported
/// from.
fn pk(value: &Value) -> Value {
    if let Some(inner) = value.get("uint") {
        return json!(int64(inner).unwrap_or_else(|| panic!("a uint id: {value}")));
    }
    if let Some(inner) = value.get("string") {
        return json!(
            inner
                .as_str()
                .unwrap_or_else(|| panic!("a string id: {value}"))
        );
    }
    if let Some(inner) = value.get("uuid") {
        return json!({"uuid": inner.as_str().unwrap_or_else(|| panic!("a uuid id: {value}"))});
    }
    panic!("not a DocumentId: {value}")
}

/// A request with `namespace` and `collection` already set, so a per-RPC
/// fragment need not repeat the names (they are **fields**).
fn request(namespace: &str, collection: &str, extra: &[(&str, Value)]) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("namespace".into(), json!(namespace));
    map.insert("collection".into(), json!(collection));
    for (key, value) in extra {
        map.insert((*key).to_string(), value.clone());
    }
    Value::Object(map)
}

fn kb_request(extra: &[(&str, Value)]) -> Value {
    request("w", "kb", extra)
}

// ----- The fixtures (`crates/loams/tests/it/common/mod.rs`) -----
//
// Duplicated rather than shared: `it/common` is a module of the `it` test
// binary, and Task 9 owns that suite's deletion, so this file must not disturb
// it. The bodies are the fixture's, unchanged, with the ids in the
// `DocumentId` oneof's spelling.

/// The M1.6 fixture's `kb` schema (`body`, `tenant`, `n`, `embedding`).
fn kb_schema() -> Value {
    json!({
        "fields": [
            {"name": "body", "source_path": "body", "kind": {"text": {"analyzer": "standard", "positions": true}}, "indexed": true, "fast": false},
            {"name": "tenant", "source_path": "tenant", "kind": "keyword", "indexed": true, "fast": true},
            {"name": "n", "source_path": "n", "kind": "i64", "indexed": true, "fast": true}
        ],
        "vectors": [{"name": "embedding", "dim": 3, "distance": "cosine"}],
        "sparse_vectors": [],
        "dynamic": "ignore",
        "max_fields": 1000
    })
}

/// The M1.6 fixture's uuid id.
const UUID: &str = "0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e";

/// The M1.6 fixture's six upserts, with no existence reporting.
fn kb_ops() -> Value {
    let upsert = |id: Value, source: Value, embedding: Value| {
        json!({"upsert": {"id": id, "source": source, "vectors": embedding}})
    };
    json!([
        upsert(
            id_uint(1),
            json!({"body": "refund policy", "tenant": "a", "n": 1}),
            json!({"embedding": [1.0, 0.0, 0.0]})
        ),
        upsert(
            id_uint(2),
            json!({"body": "shipping times", "tenant": "a", "n": 2}),
            json!({"embedding": [0.9, 0.1, 0.0]})
        ),
        upsert(
            id_uint(3),
            json!({"body": "refund window", "tenant": "b", "n": 3}),
            json!({"embedding": [0.0, 0.0, 1.0]})
        ),
        upsert(id_uint(u64::MAX), json!({"tenant": "c"}), json!({})),
        upsert(id_str("k-str"), json!({"tenant": "c"}), json!({})),
        upsert(id_uuid(UUID), json!({"tenant": "c"}), json!({})),
    ])
}

/// Creates `kb` in `ns` over `CreateCollection` and writes the fixture's six
/// documents over `WriteDocuments`; returns the write's token.
async fn kb(running: &Running, ns: &str) -> String {
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": ns, "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    let reply = running
        .connect(WRITE_DOCUMENTS, &request(ns, "kb", &[("ops", kb_ops())]))
        .await;
    let token = reply.token();
    reply.expect_ok();
    token
}

/// Fixture step 22's `sp` schema: a dense `e` (dim 2) and a sparse `s`.
fn sp_schema() -> Value {
    json!({
        "fields": [],
        "vectors": [{"name": "e", "dim": 2, "distance": "cosine"}],
        "sparse_vectors": [{"name": "s", "modifier": "none"}],
        "dynamic": "ignore",
        "max_fields": 1000
    })
}

/// Fixture steps 22–23: three documents in `sp`.
fn sp_ops() -> Value {
    let upsert = |id: u64, e: Value, s: Value| {
        json!({"upsert": {"id": id_uint(id), "source": {}, "vectors": {"e": e}, "sparseVectors": {"s": s}}})
    };
    json!([
        upsert(1, json!([1.0, 0.0]), json!({"indices": [5, 1], "values": [2.0, 1.0]})),
        upsert(2, json!([0.0, 1.0]), json!({"indices": [5], "values": [0.5]})),
        upsert(3, json!([0.8, 0.6]), json!({"indices": [7], "values": [3.0]})),
    ])
}

/// Creates `sp` in `ns` and writes its three documents; returns the token.
async fn sp(running: &Running, ns: &str) -> String {
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": ns, "name": "sp", "schema": sp_schema()}),
        )
        .await
        .expect_ok();
    let reply = running
        .connect(WRITE_DOCUMENTS, &request(ns, "sp", &[("ops", sp_ops())]))
        .await;
    let token = reply.token();
    reply.expect_ok();
    token
}

/// The `{"term": {"field": "tenant", "value": …}}` filter.
fn tenant(value: &str) -> Value {
    json!({"term": {"field": "tenant", "value": value}})
}

/// **WIRE**: a read at `token`: `consistency.at_least` is `atLeast`.
fn at_least(token: &str) -> Value {
    json!({"atLeast": token})
}

// ----- The `snake_case` → `lowerCamelCase` mapping -----

/// Every REST key of the search IR that has a different proto3 JSON spelling,
/// from the module's mapping table. A key **not** in this table that contains
/// a `_` fails [`camel`], so the table and the fixtures cannot drift apart.
const CAMEL: [(&str, &str); 32] = [
    // `SearchRequest`
    ("search_after", "searchAfter"),
    ("score_threshold", "scoreThreshold"),
    ("track_total_hits", "trackTotalHits"),
    ("group_by", "groupBy"),
    // retrievers
    ("refine_factor", "refineFactor"),
    ("idf_corpus", "idfCorpus"),
    // `Query`
    ("minimum_should_match", "minimumShouldMatch"),
    ("tie_breaker", "tieBreaker"),
    ("must_not", "mustNot"),
    ("default_fields", "defaultFields"),
    ("default_operator", "defaultOperator"),
    // `Fusion`, `SortKey`, `TrackTotalHits`
    ("weighted_sum", "weightedSum"),
    ("up_to", "upTo"),
    ("group_size", "groupSize"),
    // `Highlight`, `HighlightField`
    ("pre_tag", "preTag"),
    ("post_tag", "postTag"),
    ("fragment_size", "fragmentSize"),
    ("number_of_fragments", "numberOfFragments"),
    // `SearchResponse`, `Hit`
    ("read_token", "readToken"),
    ("hot_used", "hotUsed"),
    ("sort_values", "sortValues"),
    ("sparse_vectors", "sparseVectors"),
    // the arms of `Query`, and of `Fusion`
    ("match_all", "matchAll"),
    ("match_none", "matchNone"),
    ("match_phrase", "matchPhrase"),
    ("multi_match", "multiMatch"),
    ("values_count", "valuesCount"),
    ("is_null", "isNull"),
    ("is_empty", "isEmpty"),
    ("query_string", "queryString"),
    ("constant_score", "constantScore"),
    // Task 3's document messages, whose names share this table's shape
    ("page_token", "pageToken"),
];

/// A unit variant of a serde externally-tagged enum is a **bare string** in
/// the REST IR (`"filter": "match_all"`, `"fusion": "dbsf"`), and a proto
/// oneof arm has no string form, so the unit arm is `{"matchAll": {}}`.
fn unit_arm(key: &str, name: &str) -> Option<Value> {
    let arm = match (key, name) {
        ("filter", "match_all") => Some("matchAll"),
        ("filter", "match_none") => Some("matchNone"),
        ("fusion", "dbsf") => Some("dbsf"),
        _ => None,
    }?;
    Some(json!({ arm: {} }))
}

/// The RPC spelling of a REST search body: every key lowerCamel, every `null`
/// dropped (proto3 JSON has no `null` for a scalar or a message field), and
/// every unit variant of `Query`/`Fusion` written as its oneof arm.
fn camel(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if value.is_null() {
                    continue;
                }
                if let Some(arm) = value.as_str().and_then(|name| unit_arm(key, name)) {
                    out.insert(key.clone(), arm);
                    continue;
                }
                let name = CAMEL
                    .iter()
                    .find(|(rest, _)| *rest == key.as_str())
                    .map(|(_, camel)| (*camel).to_string())
                    .unwrap_or_else(|| {
                        assert!(
                            !key.contains('_'),
                            "the mapping table has no entry for the REST key `{key}`"
                        );
                        key.clone()
                    });
                out.insert(name, camel(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(camel).collect()),
        other => other.clone(),
    }
}

/// The same body as [`camel`], with `namespace` added: names are **fields**
/// (plan ruling 4), so the RPC needs the name the REST path carried.
fn ir_rpc(namespace: &str, rest: &Value) -> Value {
    let mut value = camel(rest);
    value
        .as_object_mut()
        .expect("a search body is an object")
        .insert("namespace".to_string(), json!(namespace));
    value
}

// ----- The M1.6 hybrid fixture, and the two surfaces' reading of it -----

/// M1.6 fixture step 12, the REST spelling, exactly as
/// `it/native_query.rs::hybrid_request` sends it — including the explicit
/// `null` defaults, which [`camel`] drops for the RPC.
fn hybrid_request() -> Value {
    json!({
        "collection": "kb",
        "retrievers": [
            {"vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 10,
                        "params": {"exact": false, "nprobes": null, "refine_factor": null, "ef": null, "oversampling": null, "distance": null},
                        "filter": null}},
            {"text": {"query": {"match": {"field": "body", "text": "refund", "operator": "or", "minimum_should_match": null, "fuzziness": null, "analyzer": null}}, "k": 10}}
        ],
        "fusion": {"rrf": {"k": 60}},
        "limit": 3
    })
}

fn hybrid_rpc(namespace: &str) -> Value {
    ir_rpc(namespace, &hybrid_request())
}

/// The hits of a `SearchResponse`, or none at all (proto3 JSON omits an empty
/// `repeated`).
fn hits(body: &Value) -> Vec<Value> {
    body["hits"].as_array().cloned().unwrap_or_default()
}

/// The hits' ids, in the REST route's spelling, whichever surface answered.
///
/// A REST answer spells a primary key as a bare JSON value (a number, a
/// string) and an RPC answer as the `DocumentId` oneof, so a hit's `pk` is
/// read through `pk()` whenever it is an object — which is every RPC answer and
/// the REST `{"uuid": …}` spelling alike.
fn pks(body: &Value) -> Vec<Value> {
    hits(body)
        .iter()
        .map(|hit| {
            let key = &hit["pk"];
            if key.is_object() {
                pk(key)
            } else {
                key.clone()
            }
        })
        .collect()
}

/// A hit's score. proto3 JSON omits a float at its default, so an absent
/// `score` is `0.0` — the same number REST writes.
fn score(hit: &Value) -> f64 {
    hit["score"].as_f64().unwrap_or(0.0)
}

fn scores(body: &Value) -> Vec<f64> {
    hits(body).iter().map(score).collect()
}

/// A canonical order for a set of ids, so two answers can be compared as sets
/// where the ranking is not what is under test.
fn sorted(mut values: Vec<Value>) -> Vec<Value> {
    values.sort_by_key(|value| value.to_string());
    values
}

/// The `total` of a `SearchResponse`, as `(value, exact)` — whether the count
/// is exact rather than a lower bound.
///
/// The relation is read **by value**: REST answers `eq` and the RPC its proto
/// name, and an absent relation is `eq` too, because a count of exactly the
/// value is the default reading of a total.
fn total(body: &Value) -> Option<(u64, bool)> {
    let total = body.get("total").filter(|total| !total.is_null())?;
    assert_documented_keys(total, &TOTAL_KEYS, "TotalHits");
    let relation = &total["relation"];
    Some((
        int64(&total["value"]).unwrap_or_else(|| panic!("a total's value: {body}")),
        relation.is_null() || enum_is(relation, "unspecified", "eq"),
    ))
}

/// One fixture of `routing_and_ranking_match_rest_for_every_fixture`: the same
/// request on both surfaces.
struct Fixture {
    name: &'static str,
    /// The collection it reads.
    collection: &'static str,
    /// The body `POST /v1/namespaces/{ns}/query` takes.
    rest: Value,
    /// The same request in proto3 JSON.
    rpc: Value,
}

/// A fixture whose RPC spelling is the mechanical [`ir_rpc`] of its REST one.
fn ir_fixture(name: &'static str, collection: &'static str, rest: Value) -> Fixture {
    Fixture {
        name,
        collection,
        rpc: ir_rpc("w", &rest),
        rest,
    }
}

/// A fixture whose two spellings differ by more than the mapping table, so
/// both are written out (the §05 §4 body, `Query::Ids`).
fn explicit_fixture(name: &'static str, collection: &'static str, rest: Value, rpc: Value) -> Fixture {
    Fixture {
        name,
        collection,
        rest,
        rpc,
    }
}

/// Design §05 §4's example body, the two spellings side by side (see the
/// module's table for the three keys that have to be reshaped).
fn section_05_body() -> Fixture {
    let rest = json!({
        "from": "collections.kb",
        "consistency": "strong",
        "retrieve": [
            {"vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 10}},
            {"text": {"field": "body", "query": "refund", "k": 10}}
        ],
        "fuse": {"method": "rrf", "k": 60},
        "select": ["id", "_score", "body"],
        "limit": 3
    });
    let rpc = json!({
        "namespace": "w",
        "from": "collections.kb",
        "consistency": {"freshness": "FRESHNESS_STRONG"},
        "retrieve": [
            {"vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 10}},
            {"text": {"query": {"match": {"field": "body", "text": "refund"}}, "k": 10}}
        ],
        "fuse": {"method": "rrf", "k": 60},
        "select": {"source": {"include": ["body"]}},
        "limit": 3
    });
    explicit_fixture("the §05 §4 body", "kb", rest, rpc)
}

/// Every fixture of the equality test. Each one exercises a different part of
/// the IR, so a field that is accepted and then ignored cannot pass here.
fn fixtures() -> Vec<Fixture> {
    vec![
        ir_fixture(
            "the M1.6 hybrid (fixture step 12)",
            "kb",
            hybrid_request(),
        ),
        ir_fixture(
            "filter only (fixture step 13)",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "fusion": null,
                "filter": tenant("b"), "limit": 10
            }),
        ),
        section_05_body(),
        ir_fixture(
            "text only",
            "kb",
            json!({
                "collection": "kb",
                "retrievers": [{"text": {"query": {"match": {"field": "body", "text": "refund"}}, "k": 10}}],
                "limit": 10
            }),
        ),
        ir_fixture(
            "dense only",
            "kb",
            json!({
                "collection": "kb",
                "retrievers": [{"vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 10}}],
                "limit": 2
            }),
        ),
        ir_fixture(
            "sparse only (fixture step 24)",
            "sp",
            json!({
                "collection": "sp",
                "retrievers": [{"sparse": {"field": "s", "query": {"indices": [5], "values": [1.0]}, "k": 10,
                               "filter": null, "params": {"idf_corpus": null}}}],
                "limit": 10
            }),
        ),
        ir_fixture(
            "dense + sparse, RRF (fixture step 25)",
            "sp",
            json!({
                "collection": "sp",
                "retrievers": [
                    {"sparse": {"field": "s", "query": {"indices": [5], "values": [1.0]}, "k": 10}},
                    {"vector": {"field": "e", "query": [1.0, 0.0], "k": 10}}
                ],
                "fusion": {"rrf": {"k": 60}},
                "limit": 3
            }),
        ),
        ir_fixture(
            "weighted sum over two retrievers",
            "kb",
            json!({
                "collection": "kb",
                "retrievers": [
                    {"vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 10}},
                    {"text": {"query": {"match": {"field": "body", "text": "refund"}}, "k": 10}}
                ],
                "fusion": {"weighted_sum": {"weights": [0.5, 0.5]}},
                "limit": 3
            }),
        ),
        ir_fixture(
            "match_all with a sort on the primary key",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "filter": "match_all",
                "sort": [{"pk": {}}], "limit": 10
            }),
        ),
        ir_fixture(
            "a score threshold above every score",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "filter": "match_all",
                "score_threshold": 0.5, "limit": 10
            }),
        ),
        ir_fixture(
            "track_total_hits exact",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "filter": tenant("c"),
                "track_total_hits": "exact", "limit": 10
            }),
        ),
        ir_fixture(
            "a bool filter with must_not",
            "kb",
            json!({
                "collection": "kb", "retrievers": [],
                "filter": {"bool": {"must_not": [tenant("c")]}}, "limit": 10
            }),
        ),
        ir_fixture(
            "match_none",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "filter": "match_none", "limit": 10
            }),
        ),
        ir_fixture(
            "an offset over the hybrid ranking",
            "kb",
            json!({
                "collection": "kb",
                "retrievers": hybrid_request()["retrievers"].clone(),
                "fusion": {"rrf": {"k": 60}},
                "offset": 1,
                "limit": 2
            }),
        ),
        explicit_fixture(
            "an ids filter",
            "kb",
            json!({
                "collection": "kb", "retrievers": [],
                "filter": {"ids": [1, {"uuid": UUID}]}, "limit": 10
            }),
            json!({
                "namespace": "w", "collection": "kb", "retrievers": [],
                "filter": {"ids": [id_uint(1), id_uuid(UUID)]}, "limit": 10
            }),
        ),
        ir_fixture(
            "a filter no document matches",
            "kb",
            json!({
                "collection": "kb", "retrievers": [], "filter": tenant("zzz"), "limit": 10
            }),
        ),
    ]
}

// ----- The ports -----

/// Port of `it/native_query.rs::hybrid_search_over_http_returns_the_fixture_ranking`
/// — the whole test, with the fixture's **same body** on the RPC.
///
/// The ranking is the fixture's: RRF over a dense vector retriever and a text
/// retriever, `limit` 3.
#[tokio::test]
async fn hybrid_search_over_http_returns_the_fixture_ranking_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;

    let reply = running.connect(SEARCH, &hybrid_rpc("w")).await;
    let header = reply.token();
    let body = reply.expect_ok();
    assert_documented_keys(&body, &SEARCH_KEYS, "SearchResponse");
    assert_eq!(pks(&body), [json!(1u64), json!(3u64), json!(2u64)], "{body}");
    assert_eq!(body["readToken"], header.as_str(), "{body}");
    let scores = scores(&body);
    assert_eq!(scores.len(), 3, "{scores:?}");
    assert!(
        scores.windows(2).all(|pair| pair[0] >= pair[1]),
        "{scores:?}"
    );
    // The two answers REST leaves out by default: no hot structure served the
    // read and no total was counted.
    assert!(body.get("hotUsed").is_none(), "{body}");
    assert!(body.get("total").is_none(), "{body}");

    // The same fixture over REST, read at the same instant, answers the same
    // three ids in the same order — the port is of a *behaviour*, not of a
    // spelling.
    let rest = running
        .rest_query("w", &hybrid_request())
        .await
        .expect_ok();
    assert_eq!(pks(&rest), [json!(1u64), json!(3u64), json!(2u64)], "{rest}");
    assert_eq!(pks(&body), pks(&rest), "{body} vs {rest}");

    running.shutdown().await;
}

/// Port of `it/native_query.rs::a_filter_only_query_over_http` — fixture step
/// 13: no retriever at all, only a filter.
#[tokio::test]
async fn a_filter_only_query_over_http_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;

    let body = running
        .connect(
            SEARCH,
            &ir_rpc(
                "w",
                &json!({
                    "collection": "kb", "retrievers": [], "fusion": null,
                    "filter": tenant("b"), "limit": 10
                }),
            ),
        )
        .await
        .expect_ok();
    assert_documented_keys(&body, &SEARCH_KEYS, "SearchResponse");
    assert_eq!(pks(&body), [json!(3u64)], "{body}");
    assert_eq!(hits(&body).len(), 1, "no second hit: {body}");
    // No retriever means no score worth reporting: the filter's answer is the
    // documents, and a missing `score` is the `0.0` REST writes.
    assert_eq!(scores(&body), [0.0], "{body}");

    running.shutdown().await;
}

/// Port of `it/native_query.rs::sparse_and_hybrid_queries_over_http` — steps
/// 22–26: a sparse collection, a sparse-only search, a dense + sparse RRF, and
/// a sparse vector read back sorted by index.
#[tokio::test]
async fn sparse_and_hybrid_queries_over_http_rpc() {
    let running = Running::start().await;
    let written = sp(&running, "w").await;

    // Step 24: sparse only. Exact scores, so the exact numbers.
    let sparse = json!({"sparse": {"field": "s", "query": {"indices": [5], "values": [1.0]}, "k": 10,
                                  "filter": null, "params": {"idf_corpus": null}}});
    let body = running
        .connect(
            SEARCH,
            &ir_rpc(
                "w",
                &json!({"collection": "sp", "retrievers": [sparse.clone()], "limit": 10}),
            ),
        )
        .await
        .expect_ok();
    assert_documented_keys(&body, &SEARCH_KEYS, "SearchResponse");
    assert_eq!(pks(&body), [json!(1u64), json!(2u64)], "{body}");
    assert_eq!(score(&hits(&body)[0]), 2.0, "{body}");
    assert_eq!(score(&hits(&body)[1]), 0.5, "{body}");
    assert!(hits(&body).get(2).is_none(), "no third hit: {body}");

    // Step 25: dense + sparse, RRF.
    let body = running
        .connect(
            SEARCH,
            &ir_rpc(
                "w",
                &json!({"collection": "sp", "retrievers": [
                    sparse,
                    {"vector": {"field": "e", "query": [1.0, 0.0], "k": 10}}
                ], "fusion": {"rrf": {"k": 60}}, "limit": 3}),
            ),
        )
        .await
        .expect_ok();
    assert_eq!(pks(&body), [json!(1u64), json!(2u64), json!(3u64)], "{body}");

    // Step 26: the sparse vector of a document, read back sorted by index.
    let reply = running
        .connect(
            GET_DOCUMENTS,
            &request(
                "w",
                "sp",
                &[
                    ("ids", json!([id_uint(1)])),
                    (
                        "select",
                        json!({"source": "none", "vectors": ["s"], "fields": []}),
                    ),
                    ("consistency", at_least(&written)),
                ],
            ),
        )
        .await;
    let read = reply.token();
    let body = reply.expect_ok();
    assert!(
        token_covers(&read, &written),
        "the read is at least the write: {read} vs {written}"
    );
    let document = &body["documents"][0];
    assert_documented_keys(document, &DOC_KEYS, "Document");
    assert_eq!(pk(&document["id"]), json!(1u64), "{body}");
    assert_struct_eq(
        &document["sparseVectors"]["s"],
        &json!({"indices": [1, 5], "values": [1.0, 2.0]}),
        "the sparse vector, sorted by index",
    );

    running.shutdown().await;
}

/// Port of `it/native_query.rs::the_section_05_body_is_accepted` — design §05
/// §4's own example body, on the RPC, and the two M3 stages still refused.
#[tokio::test]
async fn the_section_05_body_is_accepted_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;
    let fixture = section_05_body();

    let body = running.connect(SEARCH, &fixture.rpc).await.expect_ok();
    assert_documented_keys(&body, &SEARCH_KEYS, "SearchResponse");
    assert_eq!(pks(&body), [json!(1u64), json!(3u64), json!(2u64)], "{body}");
    assert_struct_eq(
        &hits(&body)[0]["source"],
        &json!({"body": "refund policy"}),
        "`select` named `body` and nothing else",
    );

    // The same body over REST answers the same ranking, which is the point of
    // the two spellings being one request.
    let rest = running.rest_query("w", &fixture.rest).await.expect_ok();
    assert_eq!(pks(&rest), [json!(1u64), json!(3u64), json!(2u64)], "{rest}");
    assert_eq!(pks(&body), pks(&rest), "{body} vs {rest}");

    // `rerank` arrives in M3: `invalid_argument`, with the same prose, on both
    // surfaces. This is the assertion that fails if the hybrid body is decoded
    // **after** the typed message, because proto3 JSON ignores an unknown
    // field and the request would silently drop the stage.
    let mut rpc = fixture.rpc.clone();
    rpc.as_object_mut()
        .expect("an object")
        .insert("rerank".into(), json!({"model": "x"}));
    let error = running.connect(SEARCH, &rpc).await;
    let message = error.message().to_string();
    let error = error.expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    assert!(message.contains("rerank arrives in M3"), "{message}");

    let mut rest = fixture.rest.clone();
    rest.as_object_mut()
        .expect("an object")
        .insert("rerank".into(), json!({"model": "x"}));
    let reply = running.rest_query("w", &rest).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);

    running.shutdown().await;
}

/// Port of `it/native_query.rs::loams_hot_used_is_none_without_a_hot_tier_and_off_is_honoured`
/// — Ruling 11. The `Loams-Hot` switch is a request header and the structures
/// the read used are the `loams-hot-used` response header on both surfaces, so
/// this is a port of the header behaviour, not of the body.
///
/// Note the structural half: `api::router` merges the Connect routes **after**
/// `.layer(hot_layer)`, so today an RPC answer carries no `loams-hot-used`
/// and `Loams-Hot: maybe` on an RPC path is not refused. Merging the Connect
/// router inside the layer is part of this task (see the module's last note).
#[tokio::test]
async fn loams_hot_used_is_none_without_a_hot_tier_and_off_is_honoured_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;
    let body = hybrid_rpc("w");

    for headers in [&[][..], &[("Loams-Hot", "off")][..], &[("Loams-Hot", "ON")][..]] {
        let reply = running.connect_with(SEARCH, headers, &body).await;
        assert_eq!(
            reply.header("loams-hot-used"),
            Some("none"),
            "{headers:?}: no hot structure served the read"
        );
        let body = reply.expect_ok();
        assert!(body.get("hotUsed").is_none(), "{body}");
        assert_eq!(pks(&body), [json!(1u64), json!(3u64), json!(2u64)], "{body}");
    }

    // Every route is inside the hot layer: the REST reader of the same header
    // answers it too.
    let reply = running
        .rest(Method::GET, "/v1/namespaces/w/collections/kb", None)
        .await;
    assert_eq!(reply.header("loams-hot-used"), Some("none"));

    running.shutdown().await;
}

/// Port of `it/native_query.rs::an_invalid_loams_hot_header_is_400` — an
/// unusable `Loams-Hot` value is `invalid_argument` on the RPC too, with the
/// registry reason and the same prose.
#[tokio::test]
async fn an_invalid_loams_hot_header_is_400_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;

    let reply = running
        .connect_with(SEARCH, &[("Loams-Hot", "maybe")], &hybrid_rpc("w"))
        .await;
    let message = reply.message().to_string();
    let error = reply.expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    assert!(
        message.contains("invalid Loams-Hot header: maybe"),
        "{message}"
    );

    running.shutdown().await;
}

// ----- The plan's own test -----

/// The plan's own test: `scroll_streams_pages_with_cursor`.
///
/// `ScrollDocuments` is unary and stays unary (see the module's `page_token`
/// note). What this test pins is the **cursor**: the RPC streams pages of a
/// collection in primary-key order and names where to continue, and the
/// AIP-158 spelling `page_token` is the same cursor as the shipped `after` —
/// interchangeable, not a second pagination.
///
/// So the whole collection is walked twice, once with `page_token` and once
/// with `after`, and the two walks are asserted to be the same documents in
/// the same pages, every one exactly once, ending with a page that has no
/// `next`.
#[tokio::test]
async fn scroll_streams_pages_with_cursor() {
    let running = Running::start().await;
    let seeded = kb(&running, "w").await;

    let by_page_token = scroll_all(&running, "pageToken").await;
    let by_after = scroll_all(&running, "after").await;

    assert_eq!(
        by_page_token, by_after,
        "`page_token` and `after` are the same cursor pagination"
    );

    let walked = by_page_token.iter().flatten().cloned().collect::<Vec<Value>>();
    let every = [
        json!(1u64),
        json!(2u64),
        json!(3u64),
        json!(u64::MAX),
        json!("k-str"),
        json!({"uuid": UUID}),
    ];
    assert_eq!(
        sorted(walked.clone()),
        sorted(every.to_vec()),
        "the whole collection, each document exactly once"
    );
    // Primary-key order: the two lowest unsigned ids open the first page.
    assert_eq!(
        by_page_token[0],
        [json!(1u64), json!(2u64)],
        "the first page is the lowest two ids"
    );
    assert!(
        by_page_token.len() > 1,
        "the walk took more than one page: {by_page_token:?}"
    );
    assert!(
        by_page_token.iter().all(|page| page.len() <= 2),
        "no page is larger than the limit: {by_page_token:?}"
    );
    assert!(
        by_page_token.last().is_some_and(|page| !page.is_empty()),
        "the last page is not empty: {by_page_token:?}"
    );

    // A cursor past the last document is an **empty page with no `next`**, not
    // an error and not a repeat.
    let last = &walked[walked.len() - 1];
    let reply = running
        .connect(
            SCROLL_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("limit", json!(2)),
                    ("select", json!({"source": "none"})),
                    ("pageToken", pk_cursor(last)),
                ],
            ),
        )
        .await;
    let read = reply.token();
    let page = reply.expect_ok();
    assert_documented_keys(&page, &SCROLL_KEYS, "ScrollDocumentsResponse");
    assert!(absent_or(&page, "documents", json!([])), "{page}");
    assert!(
        page.get("next").is_none() || page["next"].is_null(),
        "one empty page is the last page: {page}"
    );
    assert_eq!(page["readToken"], read.as_str(), "{page}");
    assert!(
        token_covers(&read, &seeded),
        "the scroll is at least the seeded write: {read} vs {seeded}"
    );

    // A cursor the wire cannot spell is `invalid_argument`, as every other
    // `DocumentId` is.
    let error = running
        .connect(
            SCROLL_DOCUMENTS,
            &kb_request(&[("pageToken", json!({"uint": "not a number"}))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");

    running.shutdown().await;
}

// ----- The invariant: the RPC answers what REST answers -----

/// **The central assertion of this task.** Every fixture's **same request**,
/// once through `POST /v1/namespaces/{ns}/query` and once through
/// `QueryService/Search`, answers the same ranking: the same ids in the same
/// order, the same scores, the same sources and the same total.
///
/// Nothing here is a hardcoded expectation. The comparison is between the two
/// answers, so the test says *where* they disagree instead of pinning a number
/// that a legitimate ranking change would move — and, more to the point, it
/// fails for the failure this task is about: a field the RPC accepts and then
/// ignores. `filter` that degrades to "match everything", a `score_threshold`
/// that is dropped, a `sort` that is not applied, a fusion that silently
/// becomes RRF — each answers a *different* ranking on one surface, and this
/// is the test that notices.
///
/// The `sp` fixtures need the sparse collection, so both are seeded over RPCs.
#[tokio::test]
async fn routing_and_ranking_match_rest_for_every_fixture() {
    let running = Running::start().await;
    kb(&running, "w").await;
    sp(&running, "w").await;

    let fixtures = fixtures();
    assert!(
        fixtures.len() >= 10,
        "the fixture table is smaller than the IR: {} fixtures",
        fixtures.len()
    );
    for fixture in &fixtures {
        // Both spellings read the collection the fixture is about, by name:
        // the REST body in the path's body, the RPC body in a field or in the
        // §05 §4 alias.
        assert_eq!(
            fixture.rest["collection"].as_str(),
            Some(fixture.collection),
            "{}: the REST fixture reads the collection it says",
            fixture.name
        );
        assert!(
            match fixture.rpc.get("collection").and_then(Value::as_str) {
                Some(name) => name == fixture.collection,
                None => fixture.rpc["from"].as_str().is_some_and(|from| {
                    from == format!("collections.{}", fixture.collection)
                        || from == fixture.collection
                }),
            },
            "{}: the RPC names the collection in a field",
            fixture.name
        );
        let rest = running
            .rest_query("w", &fixture.rest)
            .await
            .expect_ok();
        let rest_ids = pks(&rest);
        let rpc_reply = running.connect(SEARCH, &fixture.rpc).await;
        let rpc_token = rpc_reply.token();
        let rpc = rpc_reply.expect_ok();

        assert_eq!(
            rest_ids,
            pks(&rpc),
            "{}: the ranking and its order",
            fixture.name
        );
        assert_eq!(
            rest_ids.len(),
            hits(&rpc).len(),
            "{}: how many hits",
            fixture.name
        );
        for (index, (rest_hit, rpc_hit)) in
            hits(&rest).iter().zip(hits(&rpc).iter()).enumerate()
        {
            assert_documented_keys(rpc_hit, &HIT_KEYS, "Hit");
            let (rest_score, rpc_score) = (score(rest_hit), score(rpc_hit));
            assert!(
                (rest_score - rpc_score).abs() <= f64::from(f32::EPSILON) * 4.0,
                "{}: hit {index} scores {rest_score} on REST and {rpc_score} on the RPC",
                fixture.name
            );
            assert_struct_eq(
                &rpc_hit["source"],
                &rest_hit["source"],
                &format!("{}: hit {index}'s source", fixture.name),
            );
        }
        assert_eq!(total(&rest), total(&rpc), "{}: the total", fixture.name);
        assert_eq!(
            rpc["readToken"], rpc_token.as_str(),
            "{}: the token is the header and the body",
            fixture.name
        );
        assert!(
            !rpc["readToken"].as_str().unwrap_or_default().is_empty(),
            "{}: a search answers the state it read at",
            fixture.name
        );
        // A total is only answered when the request asked for one.
        if fixture.rest.get("track_total_hits").is_none() {
            assert!(
                total(&rest).is_none() && total(&rpc).is_none(),
                "{}: no total was asked for, so none is answered",
                fixture.name
            );
        }
    }

    running.shutdown().await;
}

/// Every filter form the REST route takes, on both surfaces, with the ids it
/// must answer — so **no filter form can silently degrade to "match all"**.
///
/// This is the test most likely to catch a real bug. A handler that reads a
/// filter it does not recognise and falls back to "no filter" does not fail
/// loudly: it answers `200` with *more* hits than the caller asked for, and
/// every other assertion in this file still passes. So each form is asserted
/// against the ids it must match — the ones `it/native_query.rs` and the M1.2
/// filter suite pin — and each form is additionally asserted **not** to be the
/// whole collection, so "the filter was ignored" is a failure and not a
/// pass.
///
/// The forms are the IR's own (`term`, `terms`, `range`, `exists`, `match`,
/// `ids`, `bool`, `match_all`, `match_none`) and the §05 §4 filter shorthands
/// (`and`, `or`, `not`, and a `term` keyed by its field), which is the whole
/// set `parse_query_body` accepts under `filter`.
#[tokio::test]
async fn filter_ir_is_accepted_in_every_form_the_rest_route_takes() {
    let running = Running::start().await;
    kb(&running, "w").await;

    let every = vec![
        json!(1u64),
        json!(2u64),
        json!(3u64),
        json!(u64::MAX),
        json!("k-str"),
        json!({"uuid": UUID}),
    ];

    // (name, the REST filter, the RPC filter, the ids it must match).
    let forms: Vec<(&str, Value, Value, Vec<Value>)> = vec![
        (
            "match_all",
            json!("match_all"),
            json!({"matchAll": {}}),
            every.clone(),
        ),
        (
            "match_none",
            json!("match_none"),
            json!({"matchNone": {}}),
            Vec::new(),
        ),
        (
            "term",
            tenant("b"),
            tenant("b"),
            vec![json!(3u64)],
        ),
        (
            "terms",
            json!({"terms": {"field": "tenant", "values": ["b"]}}),
            json!({"terms": {"field": "tenant", "values": ["b"]}}),
            vec![json!(3u64)],
        ),
        (
            "range",
            json!({"range": {"field": "n", "gte": 2}}),
            json!({"range": {"field": "n", "gte": 2}}),
            vec![json!(2u64), json!(3u64)],
        ),
        (
            "exists",
            json!({"exists": {"field": "body"}}),
            json!({"exists": {"field": "body"}}),
            vec![json!(1u64), json!(2u64), json!(3u64)],
        ),
        (
            "match",
            json!({"match": {"field": "body", "text": "refund"}}),
            json!({"match": {"field": "body", "text": "refund"}}),
            vec![json!(1u64), json!(3u64)],
        ),
        (
            "ids",
            json!({"ids": [1, {"uuid": UUID}]}),
            json!({"ids": [id_uint(1), id_uuid(UUID)]}),
            vec![json!(1u64), json!({"uuid": UUID})],
        ),
        (
            "bool must_not",
            json!({"bool": {"must_not": [tenant("c")]}}),
            json!({"bool": {"mustNot": [tenant("c")]}}),
            vec![json!(1u64), json!(2u64), json!(3u64)],
        ),
        (
            "§05 §4 and",
            json!({"and": [{"term": {"tenant": "b"}}]}),
            json!({"bool": {"must": [tenant("b")]}}),
            vec![json!(3u64)],
        ),
        (
            "§05 §4 not",
            json!({"not": {"term": {"tenant": "c"}}}),
            json!({"bool": {"mustNot": [tenant("c")]}}),
            vec![json!(1u64), json!(2u64), json!(3u64)],
        ),
        (
            "§05 §4 or",
            json!({"or": [{"term": {"tenant": "b"}}, {"term": {"tenant": "c"}}]}),
            json!({"bool": {"should": [tenant("b"), tenant("c")]}}),
            vec![
                json!(3u64),
                json!(u64::MAX),
                json!("k-str"),
                json!({"uuid": UUID}),
            ],
        ),
    ];

    for (name, rest_filter, rpc_filter, expected) in &forms {
        let limit = every.len() + 1;
        let rest = running
            .rest_query(
                "w",
                &json!({"collection": "kb", "retrievers": [], "filter": rest_filter, "limit": limit}),
            )
            .await
            .expect_ok();
        let rpc = running
            .connect(
                SEARCH,
                &json!({"namespace": "w", "collection": "kb", "retrievers": [],
                        "filter": rpc_filter, "limit": limit}),
            )
            .await
            .expect_ok();

        let expected = sorted(expected.clone());
        assert_eq!(
            sorted(pks(&rest)),
            expected,
            "{name}: the REST route answers exactly these ids"
        );
        assert_eq!(
            sorted(pks(&rpc)),
            expected,
            "{name}: the RPC answers exactly these ids (a filter it did not \
             understand would answer {expected:?} or every id)"
        );
        // The one form that *is* "match everything" is the only one allowed to
        // be it.
        if *name != "match_all" {
            assert_ne!(
                sorted(pks(&rpc)),
                sorted(every.clone()),
                "{name}: the filter was ignored"
            );
        }
    }

    // A filter inside a **retriever** narrows the candidates the same way, so
    // the retriever-scoped filter cannot be dropped while the top-level one is
    // honoured. The query vector is document 3's own embedding, so the one
    // candidate the filter leaves is also the nearest one.
    let body = running
        .connect(
            SEARCH,
            &json!({"namespace": "w", "collection": "kb", "limit": 10, "retrievers": [
                {"vector": {"field": "embedding", "query": [0.0, 0.0, 1.0], "k": 10,
                            "filter": tenant("b")}}
            ]}),
        )
        .await
        .expect_ok();
    assert_eq!(
        sorted(pks(&body)),
        vec![json!(3u64)],
        "the retriever's own filter narrows the candidates: {body}"
    );

    // The two M3 stages are still refused, on both surfaces, and saying so is
    // the half of this that a typed decode would lose.
    for stage in [
        ("expand", json!({"graph": "kg", "from_field": "entity_id", "hops": 2})),
        ("rerank", json!({"model": "x"})),
    ] {
        let mut rpc = section_05_body().rpc.clone();
        rpc.as_object_mut()
            .expect("an object")
            .insert(stage.0.to_string(), stage.1.clone());
        let reply = running.connect(SEARCH, &rpc).await;
        let message = reply.message().to_string();
        let error = reply.expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{}: {error}", stage.0);
        assert!(message.contains(stage.0), "{message}");
    }

    // And the fusion forms: DBSF and a weighted sum are accepted and are not
    // silently an RRF. They are only compared against REST, because their
    // ranking is not the fixture's.
    for (name, rest_fusion, rpc_fusion) in [
        (
            "dbsf",
            json!("dbsf"),
            json!({"dbsf": {}}),
        ),
        (
            "weighted_sum",
            json!({"weighted_sum": {"weights": [1.0, 0.0]}}),
            json!({"weightedSum": {"weights": [1.0, 0.0]}}),
        ),
    ] {
        let rest = running
            .rest_query(
                "w",
                &json!({"collection": "kb", "retrievers": hybrid_request()["retrievers"].clone(),
                        "fusion": rest_fusion, "limit": 3}),
            )
            .await
            .expect_ok();
        let rpc = running
            .connect(
                SEARCH,
                &json!({"namespace": "w", "collection": "kb",
                        "retrievers": hybrid_rpc("w")["retrievers"].clone(),
                        "fusion": rpc_fusion, "limit": 3}),
            )
            .await
            .expect_ok();
        assert_eq!(
            pks(&rpc),
            pks(&rest),
            "{name}: the fusion the caller asked for, not an RRF"
        );
        assert!(
            !hits(&rest).is_empty(),
            "{name}: the fixture has something to rank"
        );
    }

    running.shutdown().await;
}

/// An empty result is an empty answer, never an error: a filter that matches
/// nothing, a total of zero, a scroll of a collection nobody wrote to, and a
/// page past the end all answer `200` with the empties **absent** (proto3
/// JSON's spelling of an empty repeated field) rather than `null` or a
/// failure. A caller that reads before it writes must get an answer.
#[tokio::test]
async fn an_empty_result_set_is_an_empty_page_not_an_error() {
    let running = Running::start().await;
    kb(&running, "w").await;

    // A filter no document matches: no hits, no total, a token.
    let reply = running.connect(SEARCH, &hybrid_rpc("w")).await;
    let header = reply.token();
    let body = reply.expect_ok();
    assert!(!hits(&body).is_empty(), "the fixture is not empty: {body}");
    assert_eq!(body["readToken"], header.as_str(), "{body}");

    let body = running
        .connect(
            SEARCH,
            &json!({"namespace": "w", "collection": "kb", "retrievers": [],
                    "filter": tenant("zzz"), "limit": 10}),
        )
        .await
        .expect_ok();
    assert_documented_keys(&body, &SEARCH_KEYS, "SearchResponse");
    assert!(absent_or(&body, "hits", json!([])), "{body}");
    assert!(body.get("total").is_none(), "no total was asked for: {body}");
    assert!(
        !body["readToken"].as_str().unwrap_or_default().is_empty(),
        "an empty answer still names the state it read at: {body}"
    );

    // …and the same filter with `track_total_hits` counts **zero**, which is a
    // count and not an absent field: `count` of `0` must not read as
    // "unanswered", so it is an `optional uint64` (Task 3's `count`) that is
    // always set.
    let body = running
        .connect(
            SEARCH,
            &json!({"namespace": "w", "collection": "kb", "retrievers": [],
                    "filter": tenant("zzz"), "trackTotalHits": {"exact": {}}, "limit": 10}),
        )
        .await
        .expect_ok();
    let (value, exact) = total(&body).unwrap_or_else(|| panic!("a total of zero: {body}"));
    assert_eq!(value, 0, "{body}");
    assert!(exact, "{body}");

    // A `match_none` filter is the same empty answer, by name this time.
    let body = running
        .connect(
            SEARCH,
            &json!({"namespace": "w", "collection": "kb", "retrievers": [],
                    "filter": {"matchNone": {}}, "limit": 10}),
        )
        .await
        .expect_ok();
    assert!(absent_or(&body, "hits", json!([])), "{body}");

    // A collection nobody wrote to: the search and the scroll are both an
    // empty answer.
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "fresh", "schema": kb_schema()}),
        )
        .await
        .expect_ok();
    let body = running
        .connect(
            SEARCH,
            &json!({"namespace": "w", "collection": "fresh", "retrievers": [],
                    "filter": {"matchAll": {}}, "limit": 10}),
        )
        .await
        .expect_ok();
    assert!(absent_or(&body, "hits", json!([])), "{body}");
    assert!(body.get("total").is_none(), "{body}");

    let reply = running
        .connect(
            SCROLL_DOCUMENTS,
            &request("w", "fresh", &[("limit", json!(10))]),
        )
        .await;
    let read = reply.token();
    let page = reply.expect_ok();
    assert_documented_keys(&page, &SCROLL_KEYS, "ScrollDocumentsResponse");
    assert!(absent_or(&page, "documents", json!([])), "{page}");
    assert!(
        page.get("next").is_none() || page["next"].is_null(),
        "one empty page is the last page: {page}"
    );
    assert_eq!(page["readToken"], read.as_str(), "{page}");

    // And the REST route says the same thing about the same collection, which
    // is what makes the RPC's empty answer the REST answer.
    let rest = running
        .rest_query(
            "w",
            &json!({"collection": "fresh", "retrievers": [], "filter": "match_all", "limit": 10}),
        )
        .await
        .expect_ok();
    assert!(hits(&rest).is_empty(), "{rest}");

    running.shutdown().await;
}

// ----- The scroll walk `scroll_streams_pages_with_cursor` drives -----

/// The `DocumentId` spelling of an id already in the REST spelling, so the
/// cursor of a `next` can be handed straight back as a `page_token`.
fn pk_cursor(pk: &Value) -> Value {
    match pk {
        Value::String(text) => id_str(text),
        Value::Object(_) => pk.clone(),
        other => id_uint(other.as_u64().expect("an unsigned id")),
    }
}

/// Walks the whole of `kb` in pages of two with `cursor` (`"pageToken"` or
/// `"after"`), and returns the ids of each page, in order.
///
/// The loop ends when a page carries no `next`; a walk that does not terminate
/// in eight pages fails rather than looping, because a cursor the server does
/// not honour would answer the same page forever.
async fn scroll_all(running: &Running, cursor: &str) -> Vec<Vec<Value>> {
    let mut pages = Vec::new();
    let mut after: Option<Value> = None;
    for page_number in 0..8 {
        let mut extra: Vec<(&str, Value)> = vec![
            ("limit", json!(2)),
            ("select", json!({"source": "none"})),
        ];
        if let Some(value) = &after {
            extra.push((cursor, value.clone()));
        }
        let reply = running
            .connect(SCROLL_DOCUMENTS, &kb_request(&extra))
            .await;
        let read = reply.token();
        let page = reply.expect_ok();
        assert_documented_keys(&page, &SCROLL_KEYS, "ScrollDocumentsResponse");
        assert_eq!(page["readToken"], read.as_str(), "{page}");

        let mut ids = Vec::new();
        for document in page["documents"].as_array().expect("documents[]") {
            assert_documented_keys(document, &DOC_KEYS, "Document");
            // `source: "none"` leaves the source out, not null.
            assert!(
                document.get("source").is_none(),
                "no source is projected: {document}"
            );
            ids.push(pk(&document["id"]));
        }
        pages.push(ids);

        match page.get("next") {
            None | Some(Value::Null) => return pages,
            Some(next) => {
                after = Some(next.clone());
                assert!(page_number < 7, "the scroll did not terminate: {page}");
            }
        }
    }
    unreachable!("the loop returns or fails above")
}