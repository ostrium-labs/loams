//! `loams.collection.v1` on the main port (design §44 §4 and §5.1, ruling
//! 4; API1 Task 2), against the real `loams` server over its `--listen`
//! address.
//!
//! Every test here is a port of a REST test in `crates/loams/tests/it/`, by
//! its name with an `_rpc` suffix, or a test the API1 plan names outright
//! (`create_collection_repeat_is_safe`, `scan_returns_pin_token`). The
//! behaviour is the REST behaviour: the handlers call the same
//! `CollectionService` traits, only the shape of the call changed.
//!
//! ## How these tests reach the RPCs
//!
//! `loams.collection.v1` does not exist yet, so nothing here may name a
//! generated Rust type: a compile error is not a red test. Every RPC is a
//! Connect unary `POST` of JSON to `/<package>.<Service>/<Method>` over
//! `reqwest`, the shape `curl` sends (design §44 §4), and every answer is
//! read as `serde_json::Value`. When the package lands, the tests keep
//! working: the wire format does not change.
//!
//! ## The three shapes a port had to decide
//!
//! Design §44 §5.1 moves resource names out of the URL and into the request
//! message, and proto3 JSON renames `snake_case` to `lowerCamelCase`, so a
//! REST key becomes a camelCase key here (`collection_id` is
//! `collectionId`, `manifest_version` is `manifestVersion`).
//!
//! Three REST assertions have **no** proto3 JSON spelling, and are dropped
//! rather than faked. They are listed in the module's report to the
//! implementer:
//!
//! - **Unknown fields are ignored, not refused.** `{"partitons": 2}` and
//!   `{"when": "current"}` were `400` on REST because the handlers
//!   `#[serde(deny_unknown_fields)]`. proto3 JSON says to ignore them, so
//!   they are replaced here by the refusals that *do* survive it: a body
//!   that is not JSON, and a known field of the wrong JSON type.
//! - **`"current"` as a scan point.** `at` is `{"manifestVersion": n}` here;
//!   the live manifest is the absent `at`, which needs no spelling.
//! - **`{"at": {"tag": "x"}}`.** A proto has no `tag` field to refuse; the
//!   refusal that survives is a wrong-typed `at`, which is asserted.
//!
//! ## Seeding data
//!
//! The RPC surface under test has no `DocumentService`, so the documents are
//! written over the **native REST route**. That is not a workaround: the
//! plan's rule is behaviour first, deletion last, and the REST routes and
//! the Connect API coexist until Task 9.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

// The RPC paths of design §44 §5.1, row for row of `docs/api/route-map.md`.
const CREATE_NAMESPACE: &str = "/loams.collection.v1.NamespaceService/CreateNamespace";
const CREATE_COLLECTION: &str = "/loams.collection.v1.CollectionService/CreateCollection";
const LIST_COLLECTIONS: &str = "/loams.collection.v1.CollectionService/ListCollections";
const GET_COLLECTION: &str = "/loams.collection.v1.CollectionService/GetCollection";
const DROP_COLLECTION: &str = "/loams.collection.v1.CollectionService/DropCollection";
const ADD_FIELDS: &str = "/loams.collection.v1.CollectionService/AddFields";
const LIST_VERSIONS: &str = "/loams.collection.v1.CollectionService/ListVersions";
const SCAN: &str = "/loams.collection.v1.CollectionService/Scan";
const UPDATE_ALIASES: &str = "/loams.collection.v1.CollectionService/UpdateAliases";
const SET_HOT: &str = "/loams.collection.v1.CollectionService/SetHot";
const WARM: &str = "/loams.collection.v1.CollectionService/WarmCollection";

/// How long a test waits for the server's own background work (the link, the
/// hot tier). Nothing here sleeps to paper over a race: every wait polls
/// with a deadline, as `it/native_scan.rs` does.
const WAIT: Duration = Duration::from_secs(30);

// `CollectionInfo`'s documented fields, in proto3 JSON.
const INFO_KEYS: [&str; 15] = [
    "aliases",
    "backpressure",
    "createdAtMs",
    "hot",
    "id",
    "linkLagRecords",
    "liveDocCount",
    "manifestVersion",
    "name",
    "namespace",
    "partitions",
    "schema",
    "sizeBytes",
    "stream",
    "unappliedBytes",
];

/// `BackpressureStatus` (M1.3 Task 15, D86), in proto3 JSON.
const BACKPRESSURE_KEYS: [&str; 5] = [
    "maxUnappliedBytes",
    "maxUnappliedRecords",
    "state",
    "unappliedBytes",
    "unappliedRecords",
];

/// `ScanPlan`'s documented fields (M1.2 Task 14, D53), in proto3 JSON.
const PLAN_KEYS: [&str; 17] = [
    "collection",
    "collectionId",
    "columns",
    "durableToken",
    "expiresAtMs",
    "fragments",
    "lance",
    "liveRows",
    "manifestVersion",
    "namespace",
    "offsets",
    "pin",
    "pkEncoding",
    "plannedAtMs",
    "schemaVersion",
    "tail",
    "tailRecords",
];

const LANCE_KEYS: [&str; 5] = [
    "manifestPath",
    "stableRowIds",
    "storageFormat",
    "uri",
    "version",
];

const FRAGMENT_KEYS: [&str; 7] = [
    "deletedRows",
    "deletionFile",
    "files",
    "id",
    "lance",
    "liveRows",
    "physicalRows",
];

/// `ListCollections` is AIP-158 paginated (design §44 §5.1's note), so a list
/// response may also carry `nextPageToken`. No test here pages — every
/// fixture fits one page — so the key is allowed and nothing else is.
const PAGE_KEY: &str = "nextPageToken";

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

    /// The HTTP status, for the failure message.
    fn status(&self) -> StatusCode {
        self.status
    }

    /// Asserts the Connect success code (`200`) and returns the body.
    ///
    /// The Connect protocol has no `201`: a successful unary RPC is always
    /// `200`, so the REST suite's "a retry-safe repeat is `201`" becomes
    /// "`200`, and it is `create_collection_repeat_is_safe` that pins that
    /// the repeat is a success at all rather than a conflict.
    fn expect_ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    /// The error envelope of a failed RPC, for the reason assertions.
    fn expect_error(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        assert!(
            self.body.get("code").and_then(Value::as_str).is_some(),
            "a Connect error names its code: {}",
            self.body
        );
        self.body
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
        Self::start_with(|_| {}).await
    }

    /// [`Self::start`], with the hot tuning `it/hot_http.rs` uses.
    async fn start_hot() -> Self {
        Self::start_with(|config| {
            config.hot.reconcile_interval = Duration::from_millis(50);
            config.hot_build.poll_interval = Duration::from_millis(100);
            config.hot_build.rebuild_max_staleness = Duration::from_millis(500);
            config.hnsw_engine = Some(Arc::new(loams_hnsw::FlatEngine));
            config.collection.index_poll_interval = Duration::from_millis(100);
        })
        .await
    }

    /// [`Self::start`], with `edit` applied to the config.
    async fn start_with(edit: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.link.batch_interval = Duration::ZERO;
        edit(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            http: reqwest::Client::new(),
            _dir: dir,
        }
    }

    /// The server's data directory (the Lance bucket lives under it).
    fn data_dir(&self) -> &Path {
        self._dir.path()
    }

    /// A Connect unary call: `POST` with a JSON body, which is what `curl`
    /// sends (design §44 §4).
    async fn connect(&self, rpc: &str, body: &Value) -> Reply {
        self.connect_raw(rpc, &body.to_string()).await
    }

    /// [`Self::connect`], for a body that is not a JSON value (a malformed
    /// request, which the Connect codec answers `400 invalid_argument`).
    async fn connect_raw(&self, rpc: &str, body: &str) -> Reply {
        let response = self
            .http
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap_or_else(|err| panic!("{rpc}: {err}"));
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    /// A native REST call, for seeding documents (`DocumentService` is
    /// Task 3) and for the "the REST routes still answer" half of
    /// `collection_names_are_fields_not_paths`.
    async fn rest(&self, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("send");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    /// Writes documents over the native REST route: `DocumentService` is
    /// Task 3 and does not exist yet.
    async fn write_documents(&self, ns: &str, collection: &str, ops: Value) -> Reply {
        self.rest(
            Method::POST,
            &format!("/v1/namespaces/{ns}/collections/{collection}/documents"),
            Some(ops),
        )
        .await
    }

    async fn shutdown(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

/// `GetCollection` for a name or an alias.
async fn collection(running: &Running, ns: &str, name: &str) -> Value {
    running
        .connect(
            GET_COLLECTION,
            &json!({"namespace": ns, "collection": name}),
        )
        .await
        .expect_ok()
}

/// Polls `GetCollection` until `check` holds; returns that answer.
async fn until(
    running: &Running,
    ns: &str,
    name: &str,
    what: &str,
    check: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let info = collection(running, ns, name).await;
        if check(&info) {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {what}: {info}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

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

/// The REST test asserted `keys(body) == INFO_KEYS`: the answer speaks the
/// documented JSON and nothing else. proto3 JSON omits every field at its
/// default, so the answer may carry **fewer** keys than the REST one did (a
/// `0`, a `false`, an empty string, an empty list is absent, not `0`/`false`/
/// `""`/`[]`), but never a key the message does not document.
///
/// That is the whole of the adaptation, and it is checked in both directions
/// per message: every key present is documented, and each documented key the
/// test goes on to assert by value must therefore be present.
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
/// default is **absent**, not `0`/`false`/`""`. So a documented default is
/// accepted as either.
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
        Some(got) => *got == expected,
    }
}

/// A proto3 JSON 64-bit integer.
///
/// proto3 JSON spells `int64`/`uint64` as a **decimal string**, not a number:
/// that is what keeps a 64-bit value lossless in JavaScript, and this
/// repository says so itself in `proto/loams/live/v1/value.proto` ("In the
/// Connect JSON encoding an int64 is a string"). So `collectionId`,
/// `manifestVersion`, `liveDocCount`, `liveRows`, `lance.version`,
/// `owner.nodeId`, `CollectionInfo.id` and `namespaceId` all answer `"6"`, and
/// every read of one goes through here. An unquoted number is taken too,
/// because proto3 JSON requires a parser to accept it.
fn int64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// A JSON number inside a document carried as a `google.protobuf.Struct`.
///
/// `Struct` holds every JSON number in `Value.number_value`, which is a
/// `double`, so proto3 JSON writes a whole number as `3.0`. The value the
/// caller sent is the value it reads; only the spelling differs, and this is
/// the one place it can (`loams.collection.v1`'s header says why a collection's
/// schema is a `Struct`).
fn struct_number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// A proto3 JSON enum answers its **proto name** in `UPPER_SNAKE`
/// (`BACKPRESSURE_STATE_OPEN`), where the REST route answered `snake_case`
/// (`open`), and a value that is the enum's zero variant is omitted like any
/// other default — so an absent key answers `zero`. The assertion is on the
/// value, not on the enum's spelling or its numbering.
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

/// The `loams.errors.v1.ErrorInfo` in a Connect error's `details`
/// (design §44 §7.4, D611), decoded the way the protocol base64s it.
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

// ----- The M1.6 fixture (`crates/loams/tests/it/common/mod.rs`) -----
//
// Duplicated rather than shared: `it/common` is a module of the `it` test
// binary, and Task 9 owns that suite's deletion, so this file must not
// disturb it. The bodies are the fixture's, unchanged.
//
// **`schema` is the one wire-form decision in this file.** It is the REST
// schema JSON, carried verbatim: `source_path`, `dynamic: "ignore"`,
// `distance: "cosine"`, `max_fields`. If the proto models the schema as
// typed messages instead, the fix is this one helper — `sourcePath`,
// `maxFields`, `sparseVectors`, and the enums as proto names — and nothing
// else in the file moves.

/// The M1.6 fixture's `kb` schema: `body` (text), `tenant` (keyword, fast),
/// `n` (i64, fast) and `embedding` (dim 3, cosine). Dynamic mapping is
/// `ignore`, not the encoder default `strict`: step 14 patches in `meta.x`,
/// which a strict schema refuses.
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

/// The M1.6 fixture step 10 upserts.
fn kb_docs() -> Value {
    let upsert = |id: Value, source: Value, embedding: Value| json!({"upsert": {"id": id, "source": source, "vectors": embedding}});
    json!({
        "ops": [
            upsert(json!(1), json!({"body": "refund policy", "tenant": "a", "n": 1}), json!({"embedding": [1.0, 0.0, 0.0]})),
            upsert(json!(2), json!({"body": "shipping times", "tenant": "a", "n": 2}), json!({"embedding": [0.9, 0.1, 0.0]})),
            upsert(json!(3), json!({"body": "refund window", "tenant": "b", "n": 3}), json!({"embedding": [0.0, 0.0, 1.0]})),
            upsert(json!(18446744073709551615u64), json!({"tenant": "c"}), json!({})),
            upsert(json!("k-str"), json!({"tenant": "c"}), json!({})),
            upsert(json!({"uuid": UUID}), json!({"tenant": "c"}), json!({})),
        ],
        "report_existence": false
    })
}

/// Creates `name` in `ns` over `CreateCollection` and writes `n` documents
/// over the native REST route. Returns the collection's answer.
async fn seed(running: &Running, ns: &str, name: &str, n: u64) -> Value {
    let info = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": ns, "name": name, "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    let ops: Vec<Value> = (0..n)
        .map(|i| {
            let a = i as f64 * 0.37;
            json!({"upsert": {
                "id": i,
                "source": {"body": format!("word{} common", i % 5), "tenant": format!("t{}", i % 3)},
                "vectors": {"embedding": [a.cos(), a.sin(), 0.1 + (i % 7) as f64 * 0.05]}
            }})
        })
        .collect();
    running
        .write_documents(ns, name, json!({ "ops": ops }))
        .await
        .expect_ok();
    info
}

/// The hot tier's per-structure state, as `it/hot_http.rs` reads it.
fn hot_state(body: &Value, structure: &str) -> Value {
    body["hot"][structure]["state"].clone()
}

// ----- The ports -----

/// Port of `it/native_collections.rs::collection_routes_speak_the_documented_json`
/// (plan M1.2 Task 11, rule 1).
///
/// Every row of §44 §5.1 for collections: create, list, get, drop, fields,
/// versions and aliases. What changed is the shape of the call: the
/// namespace and the collection are **fields**, the answers are proto3 JSON,
/// and an error is a Connect envelope with a stable `reason` rather than a
/// status code with an `error` string.
#[tokio::test]
async fn collection_routes_speak_the_documented_json_rpc() {
    let running = Running::start().await;
    let create = json!({"namespace": "w", "name": "kb", "schema": kb_schema(), "partitions": 2});
    let first = running
        .connect(CREATE_COLLECTION, &create)
        .await
        .expect_ok();
    assert_documented_keys(&first, &INFO_KEYS, "CollectionInfo");
    assert_documented_keys(
        &first["backpressure"],
        &BACKPRESSURE_KEYS,
        "BackpressureStatus",
    );
    // A plain write is admitted on a fresh collection (M1.3 Task 15, D86).
    assert!(
        enum_is(&first["backpressure"]["state"], "open", "open"),
        "{first}"
    );
    assert!(absent_or(&first, "unappliedBytes", json!(0)), "{first}");
    assert_eq!(first["name"], "kb", "{first}");
    assert_eq!(first["namespace"], "w", "{first}");
    assert_eq!(first["partitions"], 2, "{first}");
    assert_eq!(
        struct_number(&first["schema"]["version"]),
        Some(1.0),
        "{first}"
    );
    // 0 before the first commit, which proto3 omits.
    assert!(absent_or(&first, "manifestVersion", json!(0)), "{first}");
    assert!(absent_or(&first, "aliases", json!([])), "{first}");
    assert_eq!(
        struct_number(&first["schema"]["vectors"][0]["dim"]),
        Some(3.0),
        "{first}"
    );

    // A retry-safe repeat succeeds with the same id, and a different schema
    // under the name is `already_exists`.
    let again = running
        .connect(CREATE_COLLECTION, &create)
        .await
        .expect_ok();
    assert_eq!(again["id"], first["id"], "{again}");
    let mut other = kb_schema();
    other["vectors"][0]["dim"] = json!(4);
    let error = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": other}),
        )
        .await
        .expect_error(StatusCode::CONFLICT);
    assert_eq!(error["code"], "already_exists", "{error}");
    assert_eq!(error_info(&error).reason, "already_exists", "{error}");

    // Malformed JSON and a known field of the wrong type are `invalid_argument`.
    // (The REST test's unknown key, `{"partitons": 2}`, has no proto3 JSON
    // spelling: proto3 JSON ignores unknown fields.)
    let error = running
        .connect_raw(CREATE_COLLECTION, "{\"namespace\": ")
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    let error = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "x", "schema": kb_schema(), "partitions": "two"}),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");

    // A second collection, to check the sort.
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "alpha", "schema": kb_schema()}),
        )
        .await
        .expect_ok();
    let list = running
        .connect(LIST_COLLECTIONS, &json!({"namespace": "w"}))
        .await
        .expect_ok();
    assert_documented_keys(&list, &["collections", PAGE_KEY], "ListCollectionsResponse");
    let names: Vec<&str> = list["collections"]
        .as_array()
        .expect("collections[]")
        .iter()
        .map(|c| c["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["alpha", "kb"], "{list}");
    assert_documented_keys(
        &list["collections"][1],
        &INFO_KEYS,
        "ListCollectionsResponse.collections[]",
    );

    // Aliases, and a get by alias.
    let body = running
        .connect(
            UPDATE_ALIASES,
            &json!({"namespace": "w", "actions": [{"create": {"alias": "kb_live", "collection": "kb"}}]}),
        )
        .await
        .expect_ok();
    assert_eq!(body, json!({}), "UpdateAliases answers an empty message");
    let by_alias = collection(&running, "w", "kb_live").await;
    assert_documented_keys(&by_alias, &INFO_KEYS, "CollectionInfo");
    assert_eq!(by_alias["id"], first["id"], "{by_alias}");
    assert_eq!(by_alias["name"], "kb", "{by_alias}");
    assert_eq!(by_alias["aliases"], json!(["kb_live"]), "{by_alias}");

    // Fields: the new schema. (`kind` is the REST spelling here; see the
    // note on `kb_schema` about the wire form.)
    let body = running
        .connect(
            ADD_FIELDS,
            &json!({
                "namespace": "w",
                "collection": "kb",
                "fields": [{"name": "color", "kind": "keyword"}],
                "annotations": {"loams.team": "x"}
            }),
        )
        .await
        .expect_ok();
    assert_documented_keys(&body, &["schema"], "AddFieldsResponse");
    assert_eq!(
        struct_number(&body["schema"]["version"]),
        Some(2.0),
        "{body}"
    );
    assert_eq!(body["schema"]["fields"][3]["name"], "color", "{body}");
    assert_eq!(body["schema"]["annotations"]["loams.team"], "x", "{body}");

    // Versions: none before the first commit.
    let body = running
        .connect(
            LIST_VERSIONS,
            &json!({"namespace": "w", "collection": "kb"}),
        )
        .await
        .expect_ok();
    assert_documented_keys(&body, &["versions", PAGE_KEY], "ListVersionsResponse");
    assert!(absent_or(&body, "versions", json!([])), "{body}");

    // A missing collection is `not_found` with its kind and name.
    let error = running
        .connect(
            GET_COLLECTION,
            &json!({"namespace": "w", "collection": "nope"}),
        )
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "not_found", "{error}");
    assert_eq!(error_info(&error).reason, "not_found", "{error}");

    // Drop twice: the second drop is a success that dropped nothing.
    let body = running
        .connect(
            DROP_COLLECTION,
            &json!({"namespace": "w", "collection": "kb"}),
        )
        .await
        .expect_ok();
    assert!(absent_or(&body, "dropped", json!(true)), "{body}");
    let body = running
        .connect(
            DROP_COLLECTION,
            &json!({"namespace": "w", "collection": "kb"}),
        )
        .await
        .expect_ok();
    // `dropped: false` is a proto3 default, so it is absent.
    assert!(absent_or(&body, "dropped", json!(false)), "{body}");

    running.shutdown().await;
}

/// Port of `it/native_collections.rs::write_get_scroll_count_round_trip_over_http`,
/// **scoped to what Task 2 owns**.
///
/// The REST test is one long write/get/scroll/count round trip. Those four
/// routes become `loams.collection.v1.DocumentService` in **Task 3** and do
/// not exist yet, so a faithful port of the whole test is not possible and
/// half of it would be a test of APIs that do not exist. What is ported is
/// the part Task 2 owns: the collection the documents go into, and the reads
/// of that collection afterwards. The rest of the round trip is Task 3's,
/// and `it/native_collections.rs` keeps pinning it until then.
#[tokio::test]
async fn write_get_scroll_count_round_trip_over_http_rpc() {
    let running = Running::start().await;
    let info = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    assert_documented_keys(&info, &INFO_KEYS, "CollectionInfo");

    // Step 10 over the native REST route (no `WriteDocuments` yet).
    let written = running.write_documents("w", "kb", kb_docs()).await;
    let header = written
        .header(TOKEN)
        .expect("the write's consistency-token header")
        .to_string();
    let body = written.expect_ok();
    assert_eq!(body["token"], header.as_str(), "{body}");
    assert_eq!(body["results"], json!(vec!["accepted"; 6]), "{body}");

    // The collection the RPC created is the collection REST wrote to: it is
    // in the list, under the id the create returned.
    let list = running
        .connect(LIST_COLLECTIONS, &json!({"namespace": "w"}))
        .await
        .expect_ok();
    assert_eq!(
        list["collections"].as_array().map(Vec::len),
        Some(1),
        "{list}"
    );
    assert_eq!(list["collections"][0]["id"], info["id"], "{list}");
    assert_eq!(list["collections"][0]["name"], "kb", "{list}");

    // And `GetCollection` reports what the link applied: six live rows at a
    // manifest version above 0, with no lag left.
    let applied = until(
        &running,
        "w",
        "kb",
        "the six documents are applied",
        |info| {
            int64(&info["liveDocCount"]) == Some(6)
                && absent_or(info, "linkLagRecords", json!(0))
                && int64(&info["manifestVersion"]).is_some_and(|version| version > 0)
        },
    )
    .await;
    assert_eq!(applied["id"], info["id"], "{applied}");
    assert_eq!(applied["namespace"], "w", "{applied}");
    assert_eq!(int64(&applied["liveDocCount"]), Some(6), "{applied}");

    running.shutdown().await;
}

/// Port of `it/native_scan.rs::scan_route_speaks_the_documented_json`
/// (plan M1.2 Task 14, D53): the plan an external reader needs to read one
/// state of a collection.
#[tokio::test]
async fn scan_route_speaks_the_documented_json_rpc() {
    let running = Running::start().await;
    seed(&running, "w", "kb", 6).await;
    running
        .connect(
            UPDATE_ALIASES,
            &json!({"namespace": "w", "actions": [{"create": {"alias": "kb-live", "collection": "kb"}}]}),
        )
        .await
        .expect_ok();
    // Wait until the link applied every write.
    until(&running, "w", "kb", "the link caught up", |info| {
        absent_or(info, "linkLagRecords", json!(0))
            && int64(&info["manifestVersion"]).is_some_and(|version| version > 0)
    })
    .await;

    let reply = running
        .connect(SCAN, &json!({"namespace": "w", "collection": "kb-live"}))
        .await;
    let header = reply.header(TOKEN).map(str::to_string);
    let plan = reply.expect_ok();
    assert_documented_keys(&plan, &PLAN_KEYS, "ScanPlan");
    assert_eq!(plan["namespace"], "w", "{plan}");
    // The plan reports the collection's name, never the alias it was asked by.
    assert_eq!(plan["collection"], "kb", "{plan}");
    assert_eq!(plan["pkEncoding"], "loams_canonical_v1", "{plan}");
    assert!(absent_or(&plan, "tail", json!(false)), "{plan}");
    assert_eq!(int64(&plan["liveRows"]), Some(6), "{plan}");
    assert_eq!(plan["durableToken"], plan["pin"]["token"], "{plan}");
    assert_eq!(header.as_deref(), plan["pin"]["token"].as_str(), "{plan}");
    assert_eq!(plan["offsets"].as_array().map(Vec::len), Some(2), "{plan}");
    let lance = &plan["lance"];
    assert_documented_keys(lance, &LANCE_KEYS, "ScanPlan.lance");
    assert!(int64(&lance["version"]).is_some(), "{lance}");
    let bucket = url::Url::from_directory_path(
        running
            .data_dir()
            .join("bucket")
            .canonicalize()
            .expect("the bucket directory"),
    )
    .expect("a file URL")
    .to_string();
    let uri = lance["uri"].as_str().expect("a uri");
    assert!(uri.starts_with(&bucket), "{uri} under {bucket}");
    assert!(
        uri.ends_with(&format!(
            "/collections/{}/lance",
            int64(&plan["collectionId"]).expect("the plan's collection id")
        )),
        "{uri}"
    );
    let fragment = &plan["fragments"][0];
    assert_documented_keys(fragment, &FRAGMENT_KEYS, "ScanPlan.fragments[]");
    assert_eq!(plan["columns"][0]["role"], "pk", "{plan}");
    assert_eq!(plan["columns"][4]["vector"], "embedding", "{plan}");

    // A manifest version plans exactly that state, and the live manifest
    // answers the same Lance version.
    let version = plan["manifestVersion"].clone();
    let pinned = running
        .connect(
            SCAN,
            &json!({"namespace": "w", "collection": "kb", "at": {"manifestVersion": version}}),
        )
        .await
        .expect_ok();
    assert_eq!(pinned["fragments"], plan["fragments"], "{pinned}");
    let current = running
        .connect(SCAN, &json!({"namespace": "w", "collection": "kb"}))
        .await
        .expect_ok();
    assert_eq!(current["lance"], plan["lance"], "{current}");

    // A scan point that is not a scan point is `invalid_argument`. (The
    // REST test's `{"at": {"tag": "x"}}` and `{"at": {"manifest": 1}}` have
    // no proto3 JSON spelling: a proto has no `tag` field, and proto3 JSON
    // ignores an unknown one. These three are the refusals that survive.)
    // The code is asserted, not the `reason`: whether the refusal comes from
    // the codec (a decode error, which carries no `ErrorInfo`) or from the
    // handler depends on how `at` is typed.
    for at in [json!({"manifestVersion": "seven"}), json!(7), json!([])] {
        let error = running
            .connect(
                SCAN,
                &json!({"namespace": "w", "collection": "kb", "at": at}),
            )
            .await
            .expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{error}");
    }

    // An unknown collection and a gone manifest are `not_found`.
    let error = running
        .connect(SCAN, &json!({"namespace": "w", "collection": "nope"}))
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "not_found", "{error}");
    let error = running
        .connect(
            SCAN,
            &json!({"namespace": "w", "collection": "kb", "at": {"manifestVersion": 1_000_000}}),
        )
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error_info(&error).reason, "not_found", "{error}");

    running.shutdown().await;
}

/// Port of `it/hot_http.rs::put_hot_sets_the_catalog_and_returns_status`
/// (plan M1.3 Task 8 rule 1): `SetHot` writes the catalog configuration and
/// answers the status the local hot tier then builds towards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_hot_sets_the_catalog_and_returns_status_rpc() {
    let running = Running::start_hot().await;
    seed(&running, "acme", "docs", 50).await;
    let body = running
        .connect(
            SET_HOT,
            &json!({"namespace": "acme", "collection": "docs", "hot": {"vectors": true}}),
        )
        .await
        .expect_ok();
    let status = hot_status(&body);
    assert_eq!(status["config"]["vectors"], true, "{status}");
    assert_eq!(status["enabled"], true, "{status}");
    assert_eq!(int64(&status["owner"]["nodeId"]), Some(1), "{status}");
    assert_eq!(status["owner"]["local"], true, "{status}");
    assert!(
        enum_is(&status["vectors"]["state"], "off", "building"),
        "{status}"
    );

    let ready = until(&running, "acme", "docs", "vectors are ready", |info| {
        enum_is(&hot_state(info, "vectors"), "off", "ready")
    })
    .await;
    assert!(enum_is(&hot_state(&ready, "text"), "off", "off"), "{ready}");
    assert!(
        enum_is(&hot_state(&ready, "fragments"), "off", "off"),
        "{ready}"
    );
    assert!(
        enum_is(
            &ready["hot"]["vectors"]["columns"]["embedding"]["state"],
            "off",
            "ready"
        ),
        "the embedding column is ready: {ready}"
    );

    running.shutdown().await;
}

/// Port of `it/hot_http.rs::hot_routes_accept_aliases` (plan M1.3 Task 8
/// rules 1, 2 and 4): a collection is reached by its alias everywhere, and
/// the alias resolves to the collection's own hot configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hot_routes_accept_aliases_rpc() {
    let running = Running::start_hot().await;
    seed(&running, "acme", "docs", 20).await;
    running
        .connect(
            UPDATE_ALIASES,
            &json!({"namespace": "acme", "actions": [{"create": {"alias": "live", "collection": "docs"}}]}),
        )
        .await
        .expect_ok();

    let body = running
        .connect(
            SET_HOT,
            &json!({"namespace": "acme", "collection": "live", "hot": {"text": true}}),
        )
        .await
        .expect_ok();
    assert_eq!(hot_status(&body)["config"]["text"], true, "{body}");
    assert_eq!(
        collection(&running, "acme", "docs").await["hot"]["config"]["text"],
        true
    );

    let body = running
        .connect(WARM, &json!({"namespace": "acme", "collection": "live"}))
        .await
        .expect_ok();
    assert_eq!(body, json!({}), "WarmCollection answers an empty message");

    assert_eq!(
        collection(&running, "acme", "live").await["hot"]["config"]["text"],
        true
    );

    running.shutdown().await;
}

/// The plan's own test: `CreateCollection` is idempotent by name (design §44
/// §5.1). A client that retries after a lost answer must get the collection
/// it already made, not a conflict — so the repeat succeeds and answers the
/// **same** id, and the namespace still holds one collection.
#[tokio::test]
async fn create_collection_repeat_is_safe() {
    let running = Running::start().await;
    let request = json!({
        "namespace": "w",
        "name": "kb",
        "schema": kb_schema(),
        "partitions": 2
    });
    let first = running
        .connect(CREATE_COLLECTION, &request)
        .await
        .expect_ok();
    assert!(int64(&first["id"]).is_some(), "{first}");

    // The retry: the same request, three times over.
    for attempt in 0..3 {
        let again = running.connect(CREATE_COLLECTION, &request).await;
        assert_eq!(
            again.status(),
            StatusCode::OK,
            "attempt {attempt}: a repeat is a success, not a conflict: {}",
            again.body
        );
        assert_eq!(again.body["id"], first["id"], "attempt {attempt}");
        assert_eq!(again.body["name"], first["name"], "attempt {attempt}");
        assert_eq!(
            again.body["namespace"], first["namespace"],
            "attempt {attempt}"
        );
    }

    // One collection, not four: the repeat created nothing.
    let list = running
        .connect(LIST_COLLECTIONS, &json!({"namespace": "w"}))
        .await
        .expect_ok();
    assert_eq!(
        list["collections"].as_array().map(Vec::len),
        Some(1),
        "{list}"
    );
    assert_eq!(list["collections"][0]["id"], first["id"], "{list}");

    // And the collection is the one every other RPC resolves.
    assert_eq!(collection(&running, "w", "kb").await["id"], first["id"]);
    let versions = running
        .connect(
            LIST_VERSIONS,
            &json!({"namespace": "w", "collection": "kb"}),
        )
        .await
        .expect_ok();
    assert!(absent_or(&versions, "versions", json!([])), "{versions}");

    running.shutdown().await;
}

/// The plan's own test: `Scan` answers the plan **and** the pin's token, in
/// the `loams-consistency-token` response header (design §44 §5.1; M1.2 rule
/// 6). The header and the body's pin are the same token, and the token is
/// what a reader sends to read exactly the planned state — so it is also
/// accepted back as a scan point.
#[tokio::test]
async fn scan_returns_pin_token() {
    let running = Running::start().await;
    seed(&running, "w", "kb", 6).await;
    until(&running, "w", "kb", "the link caught up", |info| {
        absent_or(info, "linkLagRecords", json!(0))
            && int64(&info["manifestVersion"]).is_some_and(|version| version > 0)
    })
    .await;

    let reply = running
        .connect(SCAN, &json!({"namespace": "w", "collection": "kb"}))
        .await;
    let token = reply
        .header(TOKEN)
        .unwrap_or_else(|| panic!("Scan answers the pin's token in {TOKEN}: {}", reply.body))
        .to_string();
    let plan = reply.expect_ok();

    // The header is the plan's pin, and the plan's durable state.
    assert_eq!(plan["pin"]["token"], token.as_str(), "{plan}");
    assert_eq!(plan["durableToken"], token.as_str(), "{plan}");
    assert!(token.starts_with("v1:"), "{token}");
    assert_eq!(
        plan["pin"]["manifestVersion"], plan["manifestVersion"],
        "{plan}"
    );

    // The token reads the pinned state: planning at it plans the same plan.
    let repinned = running
        .connect(
            SCAN,
            &json!({"namespace": "w", "collection": "kb", "at": {"token": token}}),
        )
        .await
        .expect_ok();
    assert_eq!(repinned["fragments"], plan["fragments"], "{repinned}");
    assert_eq!(
        repinned["manifestVersion"], plan["manifestVersion"],
        "{repinned}"
    );

    running.shutdown().await;
}

/// Design §44 §5.1 and the plan's ruling 4: **no resource name in a URL
/// path, names are message fields.** The same logical operation is reachable
/// under one path whatever the namespace and the collection are, and a path
/// that *does* carry the names is not a route at all.
#[tokio::test]
async fn collection_names_are_fields_not_paths() {
    let running = Running::start().await;
    // A namespace is a field of `CreateNamespace`, not a path segment.
    let created = running
        .connect(CREATE_NAMESPACE, &json!({"namespace": "w"}))
        .await
        .expect_ok();
    assert_eq!(created["namespace"], "w", "{created}");
    assert!(int64(&created["namespaceId"]).is_some(), "{created}");

    // Two collections in it, then every read by field under the *same* path.
    let kb = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema()}),
        )
        .await
        .expect_ok();
    let kb2 = running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb2", "schema": kb_schema()}),
        )
        .await
        .expect_ok();
    assert_ne!(kb["id"], kb2["id"], "{kb} and {kb2} are two collections");
    assert_eq!(collection(&running, "w", "kb").await["id"], kb["id"]);
    assert_eq!(collection(&running, "w", "kb2").await["id"], kb2["id"]);
    assert_documented_keys(
        &collection(&running, "w", "kb").await,
        &INFO_KEYS,
        "CollectionInfo",
    );
    let list = running
        .connect(LIST_COLLECTIONS, &json!({"namespace": "w"}))
        .await
        .expect_ok();
    assert_eq!(
        list["collections"].as_array().map(Vec::len),
        Some(2),
        "{list}"
    );

    // A name in the path is not a route: the REST shape under the Connect
    // package, and the Connect shape under `/v1`, are both unrouted. The
    // native `404` fallback answers them, which is what
    // `it/http.rs::framework_rejections_use_the_json_error_body` pins.
    for path in [
        "/loams.collection.v1.CollectionService/GetCollection/w/collections/kb",
        "/v1/loams.collection.v1.CollectionService/GetCollection",
        "/loams.collection.v1/namespaces/w/collections/kb",
    ] {
        let reply = running
            .rest(
                Method::POST,
                path,
                Some(json!({"namespace": "w", "collection": "kb"})),
            )
            .await;
        assert!(
            reply.status().is_client_error(),
            "{path} must not be a route: {}",
            reply.body
        );
    }
    // No `/v1` segment is needed to reach the operation at all.
    let reach = running
        .connect_raw(GET_COLLECTION, r#"{"namespace":"w","collection":"kb"}"#)
        .await;
    assert_eq!(reach.status(), StatusCode::OK, "{}", reach.body);
    assert!(
        !GET_COLLECTION.contains("/v1") && !GET_COLLECTION.contains("kb"),
        "a Connect path names no resource: {GET_COLLECTION}"
    );

    running.shutdown().await;
}

/// Design §44 §7.4, D611: a caller branches on `reason`, not on `code`, so a
/// failure the server raises must carry `loams.errors.v1.ErrorInfo` in the
/// Connect error's details with a reason that is in `docs/api/reasons.md`.
/// The kind and the name of what was missing ride in `metadata` (M1.2 rule
/// 3), so a caller can tell a missing collection from a missing namespace
/// without parsing the message.
#[tokio::test]
async fn unknown_collection_reports_a_reason() {
    let running = Running::start().await;
    seed(&running, "w", "kb", 1).await;

    let error = running
        .connect(
            GET_COLLECTION,
            &json!({"namespace": "w", "collection": "nope"}),
        )
        .await
        .expect_error(StatusCode::NOT_FOUND);
    let info = error_info(&error);
    assert_eq!(info.reason, "not_found", "{error}");
    assert_eq!(
        info.metadata.get("kind").map(String::as_str),
        Some("collection"),
        "{error}"
    );
    assert_eq!(
        info.metadata.get("name").map(String::as_str),
        Some("nope"),
        "{error}"
    );

    // Every RPC that resolves a collection answers the same way. (`Drop`
    // is not among them: dropping what is not there is a success that
    // dropped nothing, as it is on REST.)
    for (rpc, request) in [
        (
            ADD_FIELDS,
            json!({"namespace": "w", "collection": "nope", "fields": [{"name": "color", "kind": "keyword"}]}),
        ),
        (
            LIST_VERSIONS,
            json!({"namespace": "w", "collection": "nope"}),
        ),
        (SCAN, json!({"namespace": "w", "collection": "nope"})),
        (
            SET_HOT,
            json!({"namespace": "w", "collection": "nope", "hot": {"vectors": true}}),
        ),
        (WARM, json!({"namespace": "w", "collection": "nope"})),
    ] {
        let error = running.connect(rpc, &request).await;
        assert_eq!(
            error.status(),
            StatusCode::NOT_FOUND,
            "{rpc}: {}",
            error.body
        );
        let reason = error_info(&error.body).reason;
        assert_eq!(reason, "not_found", "{rpc}: {}", error.body);
    }

    running.shutdown().await;
}

/// The hot status of a `SetHot` answer. `SetHot` answers the hot status; a
/// handler that nests it under a `hot` key is equally acceptable, so both
/// shapes are read (the REST route nested it: `{"hot": status}`).
fn hot_status(body: &Value) -> &Value {
    body.get("hot").unwrap_or(body)
}
