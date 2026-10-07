//! `loams.document.v1` on the main port (design §44 §4 and §5.1, ruling 4;
//! API1 Task 3), against the real `loams` server over its `--listen` address.
//!
//! Every test here is a port of a REST test in `crates/loams/tests/it/`, by
//! its name with an `_rpc` suffix, or a test the API1 plan names outright
//! (`write_idempotency_key_replays_same_token`, `consistency_at_least_waits`).
//! The behaviour is the REST behaviour: the handlers call the same
//! `CollectionService` traits (`write`, `get_with_token`, `scroll_with_token`,
//! `count_with_token`, `delete_by_filter`, `patch_by_filter`), only the shape
//! of the call changed.
//!
//! ## How these tests reach the RPCs
//!
//! `loams.document.v1` does not exist yet, so nothing here may name a
//! generated Rust type: a compile error is not a red test. Every RPC is a
//! Connect unary `POST` of JSON to `/<package>.<Service>/<Method>` over
//! `reqwest`, the shape `curl` sends (design §44 §4), and every answer is read
//! as `serde_json::Value`. When the package lands the tests keep working: the
//! wire format does not change. Until then every RPC path here is unrouted,
//! so every test fails at its first `DocumentService` call.
//!
//! ## Seeding data
//!
//! Collections and namespaces exist as RPCs (`loams.collection.v1`, Task 2),
//! so they are created here over `CreateCollection`. Documents can only be
//! written by `DocumentService/WriteDocuments`, so every document in this file
//! is written by the RPC under test — deliberately none over the native REST
//! route, which would let a test pass while `WriteDocuments` is broken. The
//! REST routes survive until Task 9; this is the round trip Task 2 could not
//! port, and here both halves of it are the RPC.
//!
//! ## The wire shapes this file pins
//!
//! Design §44 §5.1 moves resource names out of the URL and into the request
//! message, and proto3 JSON renames `snake_case` to `lowerCamelCase`, so a
//! REST key becomes a camelCase key here (`report_existence` is
//! `reportExistence`, `max_rows` is `maxRows`, `rows_remaining` is
//! `rowsRemaining`). Every field name this file sends or expects is in the
//! module's report to the implementer; the decisions a reader could have made
//! differently are marked **WIRE** below.
//!
//! Three REST assertions have **no** proto3 JSON spelling, and are dropped or
//! weakened rather than faked:
//!
//! - **Unknown fields are ignored, not refused.** `{"patch": {}}` on a delete,
//!   `{"delete": {"id": …, "source": …}}` and `{"partitons": 2}` were `400` on
//!   REST because the handlers are `#[serde(deny_unknown_fields)]`. proto3
//!   JSON says to ignore them, so they are replaced by the refusals that *do*
//!   survive it: a body that is not JSON, and a known field of the wrong JSON
//!   type.
//! - **`null` for a missing document.** proto3 JSON has no `null` for an
//!   element of a `repeated` field of messages. A missing id answers an **entry
//!   with no `id`** (**WIRE**), which is the same information: the entry is
//!   there, in the requested id's position, and carries no `id`.
//! - **Field insertion order.** `documents[0]`'s `id` stayed first and the
//!   source's keys came back in the order they were written because of a serde
//!   insertion-order rule (M1.3 row E58). proto3 JSON orders a message's
//!   fields by field number and a `Struct` by its map iteration order, so the
//!   *ordering* assertion is dropped; the *content* assertion — every key the
//!   write put in the source is in the answer — is kept and is exact.
//!
//! ## **WIRE** — the shapes a reader could have made differently
//!
//! These are decisions, not accidents. An implementer who changes one of them
//! changes this file, not the other way round.
//!
//! - **A document id is a typed oneof, not a `google.protobuf.Value`.** A
//!   `Value` holds numbers in a `double`, so the fixture's `u64::MAX` id
//!   (`18446744073709551615`) would come back as `18446744073709552000`. So
//!   `DocumentId` is a oneof of `uint` / `string` / `uuid`, whose proto3 JSON is
//!   `{"uint": "1"}` / `{"string": "k-str"}` / `{"uuid": "0190f5c4-…"}`. The
//!   `uuid` arm is the REST spelling unchanged; the other two are named
//!   because a bare JSON `1` has no oneof spelling.
//! - **A document's `source` is a `google.protobuf.Struct`.** A document's
//!   source is arbitrary caller JSON with a new key per document, so it is the
//!   same case as a collection's schema (ruling 2.1): a `Struct` keeps the
//!   REST source JSON exactly. **Dense and sparse vectors follow the same
//!   rule for the same reason**: `map<string, google.protobuf.Value>` answers
//!   `{"embedding": [1.0, 0.0, 0.0]}` byte for byte, and a patch's
//!   `{"embedding": null}` — which removes the vector — is representable at
//!   all only because the value is a `Value` rather than a `ListValue`. A
//!   typed `Document` would renumber every one of these and break every caller
//!   that posts the REST shape today.
//! - **`filter` is a `google.protobuf.Value`, not the typed IR.** The typed
//!   filter IR is **Task 4**; until it lands, `filter` is the native query
//!   JSON verbatim (`{"term": {"field": "tenant", "value": "c"}}`,
//!   `"match_all"`).
//! - **`select` is a `google.protobuf.Struct`**, carrying the REST projection
//!   JSON verbatim (`{"source": "all" | "none" | {"include": [], "exclude":
//!   []}, "vectors": [], "fields": []}`). `source` has a JSON object form that
//!   a `string` cannot hold.
//! - **`Patch.mode` is a `string` carrying the REST spelling** (`merge_deep`,
//!   `merge_top`, `replace`; absent is `merge_deep`), following the precedent
//!   `loams.collection.v1` set for `ScanColumn.role` and
//!   `ScanColumn.distance`. It is only ever *sent*, never read, so a string
//!   costs nothing and keeps a switchable spelling verbatim.
//! - **An answer-side enum is asserted by value, not by spelling.** The write
//!   `results` and the backpressure `state` are proto enums, and `enum_is`
//!   accepts any spelling that ends in the value's `UPPER_SNAKE` name, so the
//!   enum's numbering and its prefix stay the implementer's choice.
//! - **Consistency travels as a request field** (`consistency.atLeast`,
//!   `consistency.freshness`, `consistency.pin`) and comes back in **both** the
//!   `loams-consistency-token` response header and the body (`token` on a
//!   write, `readToken` on a read).
//! - **Backpressure override stays a request header** (`loams-backpressure:
//!   off`) and the backlog stays response headers (`loams-unapplied-records`,
//!   `loams-unapplied-bytes`), because that is the REST behaviour and the
//!   response half is headers by rule.
//! - **`index`, `matched`, `limit`, `retry_after_ms`, `kind` and `name` ride
//!   in `ErrorInfo.metadata`**, which `loams.errors.v1` types as
//!   `map<string, string>` and which `docs/api/reasons.md` already promises
//!   for `resource_exhausted`, `invalid_argument` and `not_found`. Every
//!   reason asserted here is already a registry row; this task adds none.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use buffa::Message as _;
use loams::api::{BACKPRESSURE_HEADER, UNAPPLIED_BYTES_HEADER, UNAPPLIED_RECORDS_HEADER};
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The consistency-token header (the REST `Loams-Consistency-Token`).
const TOKEN: &str = "loams-consistency-token";

// The RPC paths of design §44 §5.1, row for row of `docs/api/route-map.md`.
const WRITE_DOCUMENTS: &str = "/loams.document.v1.DocumentService/WriteDocuments";
const GET_DOCUMENTS: &str = "/loams.document.v1.DocumentService/GetDocuments";
const SCROLL_DOCUMENTS: &str = "/loams.document.v1.DocumentService/ScrollDocuments";
const COUNT_DOCUMENTS: &str = "/loams.document.v1.DocumentService/CountDocuments";
const DELETE_BY_FILTER: &str = "/loams.document.v1.DocumentService/DeleteByFilter";
const PATCH_BY_FILTER: &str = "/loams.document.v1.DocumentService/PatchByFilter";

/// The two Task 2 RPCs these tests lean on.
const CREATE_COLLECTION: &str = "/loams.collection.v1.CollectionService/CreateCollection";
const GET_COLLECTION: &str = "/loams.collection.v1.CollectionService/GetCollection";
const UPDATE_ALIASES: &str = "/loams.collection.v1.CollectionService/UpdateAliases";

/// How long a test waits for the server's own background work. Nothing here
/// sleeps to paper over a race: every wait polls with a deadline, as
/// `it/native_scan.rs` does.
const WAIT: Duration = Duration::from_secs(30);

/// `WriteDocumentsResponse`'s documented fields, in proto3 JSON. The two
/// backlog counters are *allowed* but not required: every test here asserts
/// the backlog in the headers, which is where the REST behaviour puts it, so an
/// implementation that reports it only in the headers passes.
const WRITE_KEYS: [&str; 5] = [
    "positions",
    "results",
    "token",
    "unappliedBytes",
    "unappliedRecords",
];

/// `OpPosition`'s documented fields, in proto3 JSON (`seq_no` is `seqNo`).
const POSITION_KEYS: [&str; 2] = ["partition", "seqNo"];

/// `Document`'s documented fields, in proto3 JSON.
const DOC_KEYS: [&str; 7] = [
    "fields",
    "id",
    "partition",
    "seqNo",
    "source",
    "sparseVectors",
    "vectors",
];

const GET_KEYS: [&str; 2] = ["documents", "readToken"];
const SCROLL_KEYS: [&str; 3] = ["documents", "next", "readToken"];
const COUNT_KEYS: [&str; 2] = ["count", "readToken"];

/// The filter-write answer's documented fields, in proto3 JSON. `retryAfterMs`
/// is set only when a call stopped at its deadline on a refused batch, so no
/// test asserts it.
const FILTER_WRITE_KEYS: [&str; 9] = [
    "affected",
    "batches",
    "cursor",
    "matched",
    "pin",
    "retryAfterMs",
    "rowsRemaining",
    "token",
    "written",
];

/// `FilterWritePin`'s documented fields, in proto3 JSON.
const PIN_KEYS: [&str; 2] = ["manifestVersion", "token"];
/// `FilterWriteCursor`'s documented fields, in proto3 JSON.
const CURSOR_KEYS: [&str; 3] = ["after", "manifestVersion", "token"];

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
    /// `200`, so the REST suite's "a retry-safe create is `201`" becomes "`200`,
    /// and a repeat is a success at all rather than a conflict".
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

    /// The human-readable message of a failed RPC.
    ///
    /// The `loams.errors.v1.ErrorInfo` has no `message` field (reason,
    /// metadata, hint), so the prose lives in the Connect envelope's `message`
    /// — the key the REST body put it under too.
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
        Self::start_with(|_| {}).await
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

    /// A Connect unary call: `POST` with a JSON body, which is what `curl`
    /// sends (design §44 §4).
    async fn connect(&self, rpc: &str, body: &Value) -> Reply {
        self.connect_raw(rpc, &body.to_string()).await
    }

    /// [`Self::connect`], plus request metadata. `loams-backpressure: off` is
    /// the one header a **request** still carries (see the **WIRE** note).
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

    /// A native REST call, for the "the REST routes still answer" half of
    /// `document_names_are_fields_not_paths`.
    async fn rest(&self, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        Self::finish(request.send().await.expect("send")).await
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

/// The REST test asserted `keys(body) == KEYS`: the answer speaks the
/// documented JSON and nothing else. proto3 JSON omits every field at its
/// default, so the answer may carry **fewer** keys than the REST one did (a
/// `0`, a `false`, an empty string, an empty list, and a `null` source are
/// absent, not `0`/`false`/`""`/`[]`/`null`), but never a key the message does
/// not document.
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
/// proto3 JSON spells `int64`/`uint64` as a **decimal string**: that is what
/// keeps a 64-bit value lossless in JavaScript, and this repository says so
/// itself in `proto/loams/live/v1/value.proto`. So `count`, `matched`,
/// `affected`, `written`, `batches`, `seqNo`, `manifestVersion`,
/// `unappliedRecords` and `unappliedBytes` all answer `"6"`, and every read of
/// one goes through here. An unquoted number is taken too, because proto3 JSON
/// requires a parser to accept it.
fn int64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// A JSON number inside a value carried as a `google.protobuf.Struct` (or a
/// `Value`).
///
/// `Struct` holds every JSON number in `Value.number_value`, which is a
/// `double`, so proto3 JSON writes a whole number as `3.0`. The value the
/// caller sent is the value it reads; only the spelling differs.
fn struct_number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// Recursively asserts `got == want`, reading every number as a `Struct`
/// number so a whole number answers as `3.0` on one side and `3` on the other.
fn assert_struct_eq(got: &Value, want: &Value, what: &str) {
    match (got, want) {
        (Value::Object(got), Value::Object(want)) => {
            assert_eq!(got.len(), want.len(), "{what}: the keys of {got}");
            for (key, expected) in want {
                let value = got
                    .get(key)
                    .unwrap_or_else(|| panic!("{what}: no key `{key}` in {got}"));
                assert_struct_eq(value, expected, &format!("{what}.{key}"));
            }
        }
        (Value::Array(got), Value::Array(want)) => {
            assert_eq!(got.len(), want.len(), "{what}: the length of {got}");
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
/// (`OP_RESULT_ACCEPTED`, `BACKPRESSURE_STATE_THROTTLING`), where the REST
/// route answered `snake_case` (`accepted`, `throttling`), and a value that is
/// the enum's zero variant is omitted like any other default — so an absent key
/// answers `zero`. The assertion is on the value, not on the enum's spelling or
/// its numbering.
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

/// Asserts the `reason` of a Connect error is `expected`.
fn assert_reason(body: &Value, expected: &str) {
    assert_eq!(error_info(body).reason, expected, "{body}");
}

/// A `metadata` entry of a Connect error, as the REST body spelled it.
fn metadata(body: &Value, key: &str) -> Option<String> {
    error_info(body).metadata.get(key).cloned()
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

/// **WIRE**: an id of `PrimaryKey::U64`.
fn id_uint(n: u64) -> Value {
    json!({"uint": n.to_string()})
}

/// **WIRE**: an id of `PrimaryKey::Str`.
fn id_str(text: &str) -> Value {
    json!({"string": text})
}

/// **WIRE**: an id of `PrimaryKey::Uuid`, spelled exactly as REST spells it.
fn id_uuid(uuid: &str) -> Value {
    json!({"uuid": uuid})
}

/// A `DocumentId` answer read back as the REST route's spelling, so the
/// assertions read like the tests they were ported from.
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

/// **WIRE**: a missing document — an entry of `documents[]` that carries no
/// `id`. proto3 JSON has no `null` for an element of a `repeated` field of
/// messages, and an absent `id` is exactly the REST `null`.
fn is_missing(document: &Value) -> bool {
    document.is_null() || document.get("id").is_none()
}

/// A `DocumentService` request with `namespace` and `collection` already set,
/// so a per-RPC fragment need not repeat the names (they are **fields**).
fn request(namespace: &str, collection: &str, extra: &[(&str, Value)]) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("namespace".into(), json!(namespace));
    map.insert("collection".into(), json!(collection));
    for (key, value) in extra {
        map.insert((*key).to_string(), value.clone());
    }
    Value::Object(map)
}

/// `request("w", "kb", …)`.
fn kb_request(extra: &[(&str, Value)]) -> Value {
    request("w", "kb", extra)
}

/// [`request`], with the extra fields read off a JSON object, so a table of
/// bad requests can be spelled as objects.
fn request_from(namespace: &str, collection: &str, extra: &Value) -> Value {
    let fields: Vec<(&str, Value)> = match extra.as_object() {
        Some(object) => object
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect(),
        None => Vec::new(),
    };
    request(namespace, collection, &fields)
}

// ----- The fixture (`crates/loams/tests/it/common/mod.rs`) -----
//
// Duplicated rather than shared: `it/common` is a module of the `it` test
// binary, and Task 9 owns that suite's deletion, so this file must not disturb
// it. The bodies are the fixture's, unchanged, with the ids in the
// `DocumentId` oneof's spelling.

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

/// The M1.6 fixture's six upserts, with no existence reporting.
fn kb_ops() -> Value {
    let upsert = |id: Value, source: Value, embedding: Value| json!({"upsert": {"id": id, "source": source, "vectors": embedding}});
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
/// documents over `WriteDocuments`. Returns the collection's answer and the
/// write's token — what `it/common`'s `kb()` returns, plus the collection.
async fn kb(running: &Running, ns: &str) -> (Value, String) {
    let info = running
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
    (info, token)
}

/// **WIRE**: a read at `token`: `consistency.at_least` is `atLeast`.
fn at_least(token: &str) -> Value {
    json!({"atLeast": token})
}

/// `CountDocuments` at `token`, and the count it answers.
async fn count(running: &Running, ns: &str, collection: &str, filter: Value, token: &str) -> u64 {
    let body = running
        .connect(
            COUNT_DOCUMENTS,
            &request(
                ns,
                collection,
                &[("filter", filter), ("consistency", at_least(token))],
            ),
        )
        .await
        .expect_ok();
    int64(&body["count"]).unwrap_or_else(|| panic!("a count: {body}"))
}

/// The source of document `id`, as `it/filter_write_http.rs::source` reads it.
async fn source(running: &Running, ns: &str, collection: &str, id: Value) -> Value {
    let body = running
        .connect(
            GET_DOCUMENTS,
            &request(ns, collection, &[("ids", json!([id]))]),
        )
        .await
        .expect_ok();
    assert!(!is_missing(&body["documents"][0]), "{body}");
    body["documents"][0]["source"].clone()
}

/// The `{"term": {"field": "tenant", "value": …}}` filter.
fn tenant(value: &str) -> Value {
    json!({"term": {"field": "tenant", "value": value}})
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
        let info = running
            .connect(GET_COLLECTION, &request(ns, name, &[]))
            .await
            .expect_ok();
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

// ----- The ports -----

/// Port of `it/native_collections.rs::write_get_scroll_count_round_trip_over_http`
/// — **the whole test**, which Task 2 deliberately scoped down because these
/// four routes were Task 3's.
///
/// The REST test is one long write/get/scroll/count round trip over a
/// collection `CreateCollection` made. Here every one of those four routes is
/// a `loams.document.v1.DocumentService` call, and the namespace and the
/// collection are **fields**: one path serves every collection.
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

    // Step 10: the write. `report_existence` is `reportExistence`, and every op
    // without existence reporting answers `accepted`.
    let reply = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[("ops", kb_ops()), ("reportExistence", json!(false))],
            ),
        )
        .await;
    let header = reply.token();
    let body = reply.expect_ok();
    assert_documented_keys(&body, &WRITE_KEYS, "WriteDocumentsResponse");
    assert_eq!(body["token"], header.as_str(), "{body}");
    let results = body["results"]
        .as_array()
        .unwrap_or_else(|| panic!("results[]: {body}"));
    assert_eq!(results.len(), 6, "{body}");
    for result in results {
        assert!(enum_is(result, "unspecified", "accepted"), "{body}");
    }
    let positions = body["positions"]
        .as_array()
        .unwrap_or_else(|| panic!("positions[]: {body}"));
    assert_eq!(positions.len(), 6, "{body}");
    for position in positions {
        assert_documented_keys(position, &POSITION_KEYS, "OpPosition");
        assert!(
            int64(&position["seqNo"]).is_some(),
            "every op was placed: {position}"
        );
    }

    // Step 11: request order, with the missing id (999) answered as an entry
    // with no `id`.
    let reply = running
        .connect(
            GET_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    (
                        "ids",
                        json!([
                            id_uint(1),
                            id_uint(u64::MAX),
                            id_str("k-str"),
                            id_uuid(UUID),
                            id_uint(999)
                        ]),
                    ),
                    (
                        "select",
                        json!({"source": "all", "vectors": ["embedding"], "fields": []}),
                    ),
                    ("consistency", at_least(&header)),
                ],
            ),
        )
        .await;
    let read_header = reply.token();
    let docs = reply.expect_ok();
    assert_documented_keys(&docs, &GET_KEYS, "GetDocumentsResponse");
    assert_eq!(docs["readToken"], read_header.as_str(), "{docs}");
    assert!(
        token_covers(&read_header, &header),
        "the read is at least the write: {read_header} vs {header}"
    );
    let documents = docs["documents"]
        .as_array()
        .unwrap_or_else(|| panic!("documents[]: {docs}"));
    assert_eq!(documents.len(), 5, "{docs}");
    assert_documented_keys(&documents[0], &DOC_KEYS, "Document");
    // The `u64::MAX` id survives the round trip exactly, which is why a
    // document id is a typed oneof and not a `Value`.
    assert_eq!(pk(&documents[0]["id"]), json!(1u64), "{docs}");
    assert_eq!(documents[0]["source"]["body"], "refund policy", "{docs}");
    assert_eq!(
        documents[0]["vectors"]["embedding"],
        json!([1.0, 0.0, 0.0]),
        "{docs}"
    );
    assert_eq!(pk(&documents[1]["id"]), json!(u64::MAX), "{docs}");
    assert_eq!(pk(&documents[2]["id"]), json!("k-str"), "{docs}");
    assert_eq!(pk(&documents[3]["id"]), json!({"uuid": UUID}), "{docs}");
    assert!(is_missing(&documents[4]), "999 is not there: {docs}");

    // Steps 14–16: a patch and a delete, visible to the next get with no
    // consistency field (strong by default).
    let body = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    (
                        "ops",
                        json!([{
                            "patch": {
                                "id": id_uint(1),
                                "mode": "merge_deep",
                                "source": {"meta": {"x": 1}},
                                "deleteKeys": ["tenant"]
                            }
                        }]),
                    ),
                    ("reportExistence", json!(true)),
                ],
            ),
        )
        .await;
    let body = body.expect_ok();
    assert!(
        enum_is(&body["results"][0], "unspecified", "updated"),
        "{body}"
    );
    let reply = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    (
                        "ops",
                        json!([
                            {"delete": {"id": id_uint(2)}},
                            {"delete": {"id": id_uint(424242)}}
                        ]),
                    ),
                    ("reportExistence", json!(true)),
                ],
            ),
        )
        .await;
    let token = reply.token();
    let body = reply.expect_ok();
    assert!(
        enum_is(&body["results"][0], "unspecified", "deleted"),
        "{body}"
    );
    assert!(
        enum_is(&body["results"][1], "unspecified", "not_found"),
        "{body}"
    );
    assert!(int64(&body["positions"][0]["seqNo"]).is_some(), "{body}");
    let docs = running
        .connect(
            GET_DOCUMENTS,
            &request("w", "kb", &[("ids", json!([id_uint(1), id_uint(2)]))]),
        )
        .await;
    let read = docs.token();
    let docs = docs.expect_ok();
    // A number inside a `source` `Struct` answers as a double (`1.0`).
    assert_eq!(
        struct_number(&docs["documents"][0]["source"]["meta"]["x"]),
        Some(1.0),
        "{docs}"
    );
    assert!(
        docs["documents"][0]["source"].get("tenant").is_none(),
        "`deleteKeys` removed it: {docs}"
    );
    assert!(is_missing(&docs["documents"][1]), "2 was deleted: {docs}");
    assert!(token_covers(&read, &token), "{read} vs {token}");

    // Scroll in pages of two until `next` is absent.
    let mut seen: Vec<Value> = Vec::new();
    let mut after: Option<Value> = None;
    for page_number in 0..8 {
        let mut extra: Vec<(&str, Value)> = vec![
            ("limit", json!(2)),
            (
                "select",
                json!({"source": "none", "vectors": [], "fields": []}),
            ),
        ];
        if let Some(cursor) = &after {
            extra.push(("after", cursor.clone()));
        }
        let reply = running
            .connect(SCROLL_DOCUMENTS, &request("w", "kb", &extra))
            .await;
        let read = reply.token();
        let page = reply.expect_ok();
        assert_documented_keys(&page, &SCROLL_KEYS, "ScrollDocumentsResponse");
        assert_eq!(page["readToken"], read.as_str(), "{page}");
        for document in page["documents"].as_array().expect("documents[]") {
            // `source: "none"` leaves the source out, not null.
            assert!(
                document.get("source").is_none(),
                "no source is projected: {document}"
            );
            seen.push(pk(&document["id"]));
        }
        if page.get("next").is_none() || page["next"].is_null() {
            break;
        }
        after = Some(page["next"].clone());
        assert!(page_number < 7, "the scroll did not terminate: {page}");
    }
    assert_eq!(
        seen,
        [
            json!(1u64),
            json!(3u64),
            json!(u64::MAX),
            json!({"uuid": UUID}),
            json!("k-str")
        ]
    );

    // Count, with and without a filter. `count` is a `uint64`, so it answers a
    // decimal string.
    let reply = running
        .connect(COUNT_DOCUMENTS, &request("w", "kb", &[]))
        .await;
    let read = reply.token();
    let body = reply.expect_ok();
    assert_documented_keys(&body, &COUNT_KEYS, "CountDocumentsResponse");
    assert_eq!(int64(&body["count"]), Some(5), "{body}");
    assert_eq!(body["readToken"], read.as_str(), "{body}");
    let reply = running
        .connect(
            COUNT_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("filter", tenant("c")),
                    ("consistency", json!({"freshness": "FRESHNESS_EVENTUAL"})),
                ],
            ),
        )
        .await;
    let read = reply.token();
    let body = reply.expect_ok();
    assert_eq!(int64(&body["count"]), Some(3), "{body}");
    assert_eq!(body["readToken"], read.as_str(), "{body}");

    // The collection is the one the RPC created and the RPC wrote to.
    assert_eq!(info["name"], "kb", "{info}");
    let applied = until(&running, "w", "kb", "the writes are applied", |info| {
        int64(&info["liveDocCount"]) == Some(5)
    })
    .await;
    assert_eq!(applied["id"], info["id"], "{applied}");

    running.shutdown().await;
}

/// Port of `it/native_collections.rs::a_rejected_atomic_write_is_400_with_the_op_index`:
/// a write is atomic (Ruling 16) and a rejected op names its index.
#[tokio::test]
async fn a_rejected_atomic_write_is_400_with_the_op_index_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;

    // A wrong dimension in op 1: nothing of the request is written.
    let error = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[(
                    "ops",
                    json!([
                        {"upsert": {"id": id_uint(7), "source": {"tenant": "d"}, "vectors": {"embedding": [1.0, 0.0, 0.0]}}},
                        {"upsert": {"id": id_uint(8), "source": {}, "vectors": {"embedding": [1.0, 2.0]}}}
                    ]),
                )],
            ),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    assert_eq!(metadata(&error, "index").as_deref(), Some("1"), "{error}");
    let docs = running
        .connect(
            GET_DOCUMENTS,
            &request("w", "kb", &[("ids", json!([id_uint(7), id_uint(8)]))]),
        )
        .await;
    let docs = docs.expect_ok();
    assert!(
        is_missing(&docs["documents"][0]) && is_missing(&docs["documents"][1]),
        "nothing of the rejected write was written: {docs}"
    );

    // An op the API cannot parse names its index too, and its message says
    // which op: the validation is unchanged, so the prose is unchanged.
    let reply = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[(
                    "ops",
                    json!([
                        {"delete": {"id": id_uint(1)}},
                        {"upsert": {"id": id_uint(9), "sparseVectors": {"s": {"indices": [1, 1], "values": [1.0, 2.0]}}}}
                    ]),
                )],
            ),
        )
        .await;
    let message = reply.message().to_string();
    let error = reply.expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    assert_eq!(metadata(&error, "index").as_deref(), Some("1"), "{error}");
    assert!(message.contains("op 1: sparse vector s: "), "{message}");

    // An op whose *id* the codec cannot decode is refused too. The code is
    // asserted and not the index: whether an id of the wrong JSON type is
    // refused by the codec (which never learns the op's index) or by the
    // handler (which does) depends on how `DocumentId` is typed, and either is
    // the REST refusal.
    let error = running
        .connect(
            WRITE_DOCUMENTS,
            &request("w", "kb", &[("ops", json!([{"upsert": {"id": true}}]))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");

    running.shutdown().await;
}

/// Port of `it/native_collections.rs::write_responses_carry_the_consistency_token_header`:
/// a write's token is the `loams-consistency-token` header and the same token
/// in the body, and a read naming it sees the write and answers a token of its
/// own.
#[tokio::test]
async fn write_responses_carry_the_consistency_token_header_rpc() {
    let running = Running::start().await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    let reply = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[(
                    "ops",
                    json!([{"upsert": {"id": id_uint(1), "source": {"tenant": "a"}}}]),
                )],
            ),
        )
        .await;
    let header = reply.token();
    let body = reply.expect_ok();
    assert!(header.starts_with("v1:s"), "{header}");
    assert_eq!(body["token"], header.as_str(), "{body}");

    // A read naming the write's token as its `at_least` sees the write, and
    // answers its own read token in the header and in the body.
    let reply = running
        .connect(
            GET_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("ids", json!([id_uint(1)])),
                    ("consistency", at_least(&header)),
                ],
            ),
        )
        .await;
    let read = reply.token();
    let body = reply.expect_ok();
    assert_eq!(body["readToken"], read.as_str(), "{body}");
    assert!(read.starts_with("v1:s"), "{read}");
    assert!(
        token_covers(&read, &header),
        "the read's token covers the write's: {read} vs {header}"
    );
    assert_eq!(body["documents"][0]["source"]["tenant"], "a", "{body}");

    // An unparseable token is `invalid_argument`.
    let error = running
        .connect(
            GET_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("ids", json!([id_uint(1)])),
                    ("consistency", at_least("c1:nope")),
                ],
            ),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");

    running.shutdown().await;
}

/// Port of `it/filter_write_http.rs::delete_by_filter_route_speaks_the_documented_json`:
/// a partial call, the cursor that finishes it at the same pin, and reads at
/// the token that see the deletions.
#[tokio::test]
async fn delete_by_filter_route_speaks_the_documented_json_rpc() {
    let running = Running::start().await;
    let (_, seeded) = kb(&running, "w").await;

    // A partial call: one of the three `c` documents, and a cursor.
    let reply = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[
                ("filter", tenant("c")),
                ("maxRows", json!(1)),
                ("allowPartial", json!(true)),
            ]),
        )
        .await;
    let header = reply.token();
    let first = reply.expect_ok();
    assert_documented_keys(&first, &FILTER_WRITE_KEYS, "FilterWriteResponse");
    assert_eq!(first["token"], header.as_str(), "{first}");
    assert_eq!(int64(&first["matched"]), Some(3), "{first}");
    assert_eq!(int64(&first["affected"]), Some(1), "{first}");
    assert_eq!(int64(&first["batches"]), Some(1), "{first}");
    assert_eq!(first["rowsRemaining"], true, "{first}");
    assert!(int64(&first["written"]).is_some(), "{first}");
    assert_documented_keys(&first["pin"], &PIN_KEYS, "FilterWritePin");
    assert_documented_keys(&first["cursor"], &CURSOR_KEYS, "FilterWriteCursor");
    assert_eq!(first["cursor"]["token"], first["pin"]["token"], "{first}");

    // The cursor finishes the write at the same pin.
    let reply = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[("filter", tenant("c")), ("cursor", first["cursor"].clone())]),
        )
        .await;
    let token = reply.token();
    let rest = reply.expect_ok();
    assert_eq!(int64(&rest["matched"]), Some(3), "{rest}");
    assert_eq!(int64(&rest["affected"]), Some(2), "{rest}");
    assert!(absent_or(&rest, "rowsRemaining", json!(false)), "{rest}");
    assert!(
        rest.get("cursor").is_none() || rest["cursor"].is_null(),
        "the last call has no cursor: {rest}"
    );
    assert_eq!(rest["pin"], first["pin"], "{rest}");
    assert!(
        token_covers(&token, &seeded),
        "the answer is at least the seeded write: {token} vs {seeded}"
    );
    assert_eq!(count(&running, "w", "kb", tenant("c"), &token).await, 0);
    assert_eq!(
        count(&running, "w", "kb", json!("match_all"), &token).await,
        3
    );

    // A single-target alias names its collection.
    running
        .connect(
            UPDATE_ALIASES,
            &json!({"namespace": "w", "actions": [{"create": {"alias": "kb_live", "collection": "kb"}}]}),
        )
        .await
        .expect_ok();
    let body = running
        .connect(
            DELETE_BY_FILTER,
            &request("w", "kb_live", &[("filter", tenant("b"))]),
        )
        .await;
    let alias_token = body.token();
    let body = body.expect_ok();
    assert_eq!(int64(&body["affected"]), Some(1), "{body}");
    assert_eq!(body["token"], alias_token.as_str(), "{body}");

    // A wrong-typed field and a body that is not JSON are the refusals that
    // survive proto3 JSON, which ignores an unknown field (the REST test's
    // `{"patch": {}}` on a delete has no spelling here).
    let error = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[("filter", tenant("a")), ("maxRows", json!("two"))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    let error = running
        .connect_raw(DELETE_BY_FILTER, "{\"namespace\": ")
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");

    running.shutdown().await;
}

/// Port of `it/filter_write_http.rs::patch_by_filter_route_applies_the_patch`.
#[tokio::test]
async fn patch_by_filter_route_applies_the_patch_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;
    let body = running
        .connect(
            PATCH_BY_FILTER,
            &kb_request(&[
                ("filter", tenant("a")),
                (
                    "patch",
                    json!({
                        "mode": "merge_deep",
                        "source": {"extra": {"x": 1}},
                        "deleteKeys": ["n"]
                    }),
                ),
            ]),
        )
        .await;
    let token = body.token();
    let body = body.expect_ok();
    assert_eq!(int64(&body["matched"]), Some(2), "{body}");
    assert_eq!(int64(&body["affected"]), Some(2), "{body}");
    assert_eq!(body["token"], token.as_str(), "{body}");
    let patched = source(&running, "w", "kb", id_uint(1)).await;
    assert_struct_eq(
        &patched,
        &json!({"body": "refund policy", "tenant": "a", "extra": {"x": 1}}),
        "the patched source",
    );
    let untouched = source(&running, "w", "kb", id_uint(3)).await;
    assert_struct_eq(
        &untouched,
        &json!({"body": "refund window", "tenant": "b", "n": 3}),
        "the document the filter did not match",
    );

    // `null` removes a vector.
    let body = running
        .connect(
            PATCH_BY_FILTER,
            &kb_request(&[
                ("filter", tenant("b")),
                ("patch", json!({"vectors": {"embedding": null}})),
            ]),
        )
        .await;
    let token = body.token();
    let body = body.expect_ok();
    assert_eq!(int64(&body["affected"]), Some(1), "{body}");
    let docs = running
        .connect(
            GET_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("ids", json!([id_uint(3)])),
                    ("consistency", at_least(&token)),
                ],
            ),
        )
        .await;
    let docs = docs.expect_ok();
    assert!(
        docs["documents"][0].get("vectors").is_none(),
        "the vector is gone: {docs}"
    );

    // An unknown mode, a patch without its object, and a patch that would
    // create a document are refused.
    for bad in [
        json!({"filter": tenant("a"), "patch": {"mode": "other"}}),
        json!({"filter": tenant("a")}),
        json!({"filter": tenant("a"), "patch": {"upsert": {}}}),
    ] {
        let error = running
            .connect(PATCH_BY_FILTER, &request_from("w", "kb", &bad))
            .await
            .expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{bad}: {error}");
    }

    running.shutdown().await;
}

/// Port of `it/filter_write_http.rs::over_the_limit_is_400_with_matched_and_limit`:
/// a call over its limit is refused with the numbers it refused on, and nothing
/// is deleted.
#[tokio::test]
async fn over_the_limit_is_400_with_matched_and_limit_rpc() {
    let running = Running::start().await;
    let (_, token) = kb(&running, "w").await;
    let error = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[("filter", tenant("c")), ("maxRows", json!(2))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    assert_eq!(metadata(&error, "matched").as_deref(), Some("3"), "{error}");
    assert_eq!(metadata(&error, "limit").as_deref(), Some("2"), "{error}");
    assert_eq!(
        count(&running, "w", "kb", tenant("c"), &token).await,
        3,
        "the refused call deleted nothing"
    );

    // `maxRows: 0` is refused as well, without the extras.
    let error = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[
                ("filter", tenant("c")),
                ("maxRows", json!(0)),
                ("allowPartial", json!(true)),
            ]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert!(
        metadata(&error, "matched").is_none(),
        "a request error carries no matched: {error}"
    );

    running.shutdown().await;
}

/// Port of `it/filter_write_http.rs::the_token_header_covers_the_filter_write`:
/// a filter write honours the token it is given, answers its own, and a read
/// at that token sees the deletions and every earlier write.
#[tokio::test]
async fn the_token_header_covers_the_filter_write_rpc() {
    let running = Running::start().await;
    let (_, written) = kb(&running, "w").await;
    let reply = running
        .connect_with(
            DELETE_BY_FILTER,
            &[(TOKEN, written.as_str()), (BACKPRESSURE_HEADER, "off")],
            &kb_request(&[("filter", tenant("a"))]),
        )
        .await;
    let token = reply.token();
    let body = reply.expect_ok();
    assert_eq!(int64(&body["affected"]), Some(2), "{body}");
    assert_eq!(body["token"], token.as_str(), "{body}");
    assert!(
        token_covers(&token, &written),
        "the answer covers the token it was given: {token} vs {written}"
    );
    // A read at the token sees the deletions, and every earlier write.
    assert_eq!(count(&running, "w", "kb", tenant("a"), &token).await, 0);
    assert_eq!(
        count(&running, "w", "kb", json!("match_all"), &token).await,
        4
    );

    // An unusable consistency token is `invalid_argument`.
    let error = running
        .connect(
            DELETE_BY_FILTER,
            &kb_request(&[
                ("filter", tenant("b")),
                ("consistency", at_least("not a token")),
            ]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");

    running.shutdown().await;
}

// ----- The four write-backpressure ports (`it/backpressure.rs`) -----

/// A server whose link never commits within a test (a small batch waits an
/// hour) and whose budget is `records` unapplied records, measured afresh
/// before every admission. The collection is created over the RPC.
async fn paused(records: u64) -> Running {
    let running = Running::start_with(move |config| {
        config.link.batch_interval = Duration::from_secs(3600);
        config.link.batch_records = 1_000_000;
        config.query.backpressure.max_unapplied_records = records;
        config.query.backpressure.refresh_interval = Duration::ZERO;
    })
    .await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({
                "namespace": "bp",
                "name": "docs",
                "schema": {
                    "fields": [{"name": "t", "source_path": "t", "kind": "keyword", "indexed": true, "fast": false}],
                    "vectors": [],
                    "sparse_vectors": [],
                    "dynamic": "ignore",
                    "max_fields": 1000
                },
                "partitions": 2
            }),
        )
        .await
        .expect_ok();
    running
}

/// `it/backpressure.rs::ops`: one keyword upsert per key, as a whole
/// `WriteDocuments` request for the `bp/docs` collection.
fn bp_ops(range: std::ops::Range<u64>) -> Value {
    let ops: Vec<Value> = range
        .map(|i| json!({"upsert": {"id": id_uint(i), "source": {"t": format!("k{i}")}}}))
        .collect();
    request("bp", "docs", &[("ops", json!(ops))])
}

/// The `loams-unapplied-records` / `loams-unapplied-bytes` answer.
fn backlog(reply: &Reply) -> (u64, u64) {
    let number = |name: &str| -> u64 {
        reply
            .header(name)
            .unwrap_or_else(|| panic!("no {name} header in {:?}", reply.headers))
            .parse()
            .expect("a number")
    };
    (
        number(UNAPPLIED_RECORDS_HEADER),
        number(UNAPPLIED_BYTES_HEADER),
    )
}

/// How long a throttled write says to wait: the `Retry-After` header if the
/// answer carries one, `ErrorInfo.metadata.retry_after_ms` otherwise (which is
/// what `docs/api/reasons.md` registers for `resource_exhausted`), and the two
/// must agree when both are there.
fn check_retry_after(header: Option<&str>, body: &Value) {
    let header = header.map(|value| {
        value
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("a Retry-After in seconds: {value}"))
    });
    let recorded = metadata(body, "retry_after_ms").map(|value| {
        value
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("a retry_after_ms: {value}"))
    });
    if let (Some(header), Some(recorded)) = (header, recorded) {
        assert_eq!(
            header,
            recorded.div_ceil(1000).max(1),
            "the Retry-After and the retry_after_ms agree"
        );
    }
    let wait = header
        .or(recorded)
        .unwrap_or_else(|| panic!("a throttled write says how long to wait: {body}"));
    assert!((1..=30).contains(&wait), "the wait is 1..=30s: {wait}");
}

/// Port of `it/backpressure.rs::a_throttled_write_gets_429_retry_after_and_backlog_headers`.
#[tokio::test]
async fn a_throttled_write_gets_429_retry_after_and_backlog_headers_rpc() {
    let running = paused(3).await;
    running
        .connect(WRITE_DOCUMENTS, &bp_ops(0..3))
        .await
        .expect_ok();
    let reply = running.connect(WRITE_DOCUMENTS, &bp_ops(3..4)).await;
    let (records, bytes) = backlog(&reply);
    let header = reply.header("retry-after").map(str::to_string);
    let error = reply.expect_error(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error["code"], "resource_exhausted", "{error}");
    assert_reason(&error, "resource_exhausted");
    check_retry_after(header.as_deref(), &error);
    assert_eq!(records, 3);
    assert!(bytes > 0);
    let info = running
        .connect(GET_COLLECTION, &request("bp", "docs", &[]))
        .await
        .expect_ok();
    assert!(
        enum_is(&info["backpressure"]["state"], "unspecified", "throttling"),
        "{info}"
    );
    assert_eq!(int64(&info["unappliedBytes"]), Some(bytes), "{info}");
    running.shutdown().await;
}

/// Port of `it/backpressure.rs::every_write_response_carries_the_backlog_headers`:
/// the backlog is measured at admission, before this write's own records.
#[tokio::test]
async fn every_write_response_carries_the_backlog_headers_rpc() {
    let running = paused(100).await;
    let first = running.connect(WRITE_DOCUMENTS, &bp_ops(0..4)).await;
    assert_eq!(first.status(), StatusCode::OK, "{}", first.body);
    assert_eq!(backlog(&first), (0, 0), "measured before its own records");
    let second = running.connect(WRITE_DOCUMENTS, &bp_ops(4..5)).await;
    assert_eq!(second.status(), StatusCode::OK, "{}", second.body);
    let (records, bytes) = backlog(&second);
    assert_eq!(records, 4);
    assert!(bytes > 0);
    running.shutdown().await;
}

/// Port of `it/backpressure.rs::loams_backpressure_off_admits_a_bulk_write`.
#[tokio::test]
async fn loams_backpressure_off_admits_a_bulk_write_rpc() {
    let running = paused(2).await;
    running
        .connect(WRITE_DOCUMENTS, &bp_ops(0..2))
        .await
        .expect_ok();
    let reply = running.connect(WRITE_DOCUMENTS, &bp_ops(2..3)).await;
    assert_eq!(
        reply.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        reply.body
    );
    // Backlogs 2, 4 and 6 are under 4 × 2; 8 is not.
    for i in 0u64..3 {
        let reply = running
            .connect_with(
                WRITE_DOCUMENTS,
                &[(BACKPRESSURE_HEADER, "off")],
                &bp_ops(10 + 2 * i..12 + 2 * i),
            )
            .await;
        assert_eq!(reply.status(), StatusCode::OK, "{}", reply.body);
        assert_eq!(backlog(&reply).0, 2 + 2 * i, "{}", reply.body);
    }
    let reply = running
        .connect_with(
            WRITE_DOCUMENTS,
            &[(BACKPRESSURE_HEADER, "off")],
            &bp_ops(20..22),
        )
        .await;
    assert_eq!(
        reply.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        reply.body
    );
    running.shutdown().await;
}

/// Port of `it/backpressure.rs::an_unknown_backpressure_value_is_400`.
#[tokio::test]
async fn an_unknown_backpressure_value_is_400_rpc() {
    let running = paused(2).await;
    let error = running
        .connect_with(
            WRITE_DOCUMENTS,
            &[(BACKPRESSURE_HEADER, "maybe")],
            &bp_ops(0..1),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");
    running.shutdown().await;
}

// ----- The two tests the plan names -----

/// The plan's own test: `WriteDocuments` is the first RPC that is not
/// naturally idempotent (ruling 2.4), so it carries an `idempotency_key`.
///
/// A client that retries after a lost answer must not write twice: the retry
/// replays the **same** answer — the same token, the same per-op results and
/// the same positions — and the collection holds one copy of each document.
/// The REST write had no key and no dedupe ledger at all, so "the dedupe
/// window equals the REST one" is pinned here as the immediate retry: a key
/// inside the window replays, a *different* key is not deduplicated, and no
/// key at all is never deduplicated.
#[tokio::test]
async fn write_idempotency_key_replays_same_token() {
    let running = Running::start().await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    let ops = json!([
        {"upsert": {"id": id_uint(1), "source": {"body": "refund policy", "tenant": "a"}}},
        {"upsert": {"id": id_uint(2), "source": {"body": "shipping times", "tenant": "a"}}}
    ]);
    let with_key = request(
        "w",
        "kb",
        &[
            ("idempotencyKey", json!("retry-1")),
            ("ops", ops.clone()),
            ("reportExistence", json!(true)),
        ],
    );

    let first = running.connect(WRITE_DOCUMENTS, &with_key).await;
    let first_token = first.token();
    let first = first.expect_ok();
    assert!(int64(&first["positions"][0]["seqNo"]).is_some(), "{first}");
    assert!(
        enum_is(&first["results"][0], "unspecified", "created"),
        "{first}"
    );

    // The retry: the same request, the same key, three times over.
    for attempt in 0..3 {
        let again = running.connect(WRITE_DOCUMENTS, &with_key).await;
        let again_token = again.token();
        let again = again.expect_ok();
        assert_eq!(again["token"], first["token"], "attempt {attempt}");
        assert_eq!(again_token, first_token, "attempt {attempt}: the header");
        assert_eq!(again["results"], first["results"], "attempt {attempt}");
        assert_eq!(
            again["positions"], first["positions"],
            "attempt {attempt}: the same positions, so the ops were not written again"
        );
    }

    // And the collection holds one copy of each, at one place in the tail.
    let info = until(&running, "w", "kb", "the write is applied", |info| {
        int64(&info["liveDocCount"]) == Some(2) && absent_or(info, "linkLagRecords", json!(0))
    })
    .await;
    assert!(int64(&info["liveDocCount"]).is_some(), "{info}");
    assert_eq!(
        count(&running, "w", "kb", json!("match_all"), &first_token).await,
        2
    );

    // A different key is a different write: each document is upserted again,
    // so it is an `updated` at a new position — and the collection still holds
    // two documents.
    let other = running
        .connect(
            WRITE_DOCUMENTS,
            &request(
                "w",
                "kb",
                &[
                    ("idempotencyKey", json!("retry-2")),
                    ("ops", ops.clone()),
                    ("reportExistence", json!(true)),
                ],
            ),
        )
        .await;
    let other_token = other.token();
    let other = other.expect_ok();
    assert_ne!(other["token"], first["token"], "{other}");
    assert!(
        enum_is(&other["results"][0], "unspecified", "updated"),
        "{other}"
    );
    assert_ne!(
        other["positions"][0]["seqNo"], first["positions"][0]["seqNo"],
        "{other}"
    );
    assert_eq!(
        count(&running, "w", "kb", json!("match_all"), &other_token).await,
        2
    );

    // No key at all is no dedupe either.
    let none = running
        .connect(
            WRITE_DOCUMENTS,
            &request("w", "kb", &[("ops", ops), ("reportExistence", json!(true))]),
        )
        .await;
    let none = none.expect_ok();
    assert!(
        enum_is(&none["results"][0], "unspecified", "updated"),
        "{none}"
    );

    running.shutdown().await;
}

/// The plan's own test: `at_least` **waits** for the token it was given rather
/// than answering from whatever state it finds.
///
/// An `at_least(T)` read syncs the tail to `T`'s offsets and reads exactly the
/// range up to them, so a read at an older token cannot see a write
/// acknowledged after it, while a read at a newer token must. That is the whole
/// difference between `at_least` and `eventual`, and it is what makes the field
/// a wait and not a hint. Every wait here is the server's own; no test sleeps
/// to paper over one.
#[tokio::test]
async fn consistency_at_least_waits() {
    let running = Running::start().await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();

    // The first state: `n` is 1, and its token is `T1`.
    let first = running
        .connect(
            WRITE_DOCUMENTS,
            &kb_request(&[(
                "ops",
                json!([{"upsert": {"id": id_uint(1), "source": {"body": "refund policy", "tenant": "a", "n": 1}}}]),
            )]),
        )
        .await;
    let t1 = first.token();
    first.expect_ok();

    // A second state, acknowledged after `T1`: `n` is 7, and its token `T2` is
    // strictly ahead of `T1`.
    let second = running
        .connect(
            WRITE_DOCUMENTS,
            &kb_request(&[(
                "ops",
                json!([{"upsert": {"id": id_uint(1), "source": {"body": "refund policy", "tenant": "a", "n": 7}}}]),
            )]),
        )
        .await;
    let t2 = second.token();
    second.expect_ok();
    assert!(
        !token_covers(&t1, &t2),
        "T2 is strictly ahead of T1: {t2} and {t1}"
    );

    // A read at `T1` waits for `T1` and answers exactly that state: it does
    // not see the write acknowledged after it.
    let reply = running
        .connect(
            GET_DOCUMENTS,
            &kb_request(&[("ids", json!([id_uint(1)])), ("consistency", at_least(&t1))]),
        )
        .await;
    let read1 = reply.token();
    let body = reply.expect_ok();
    assert_eq!(
        struct_number(&body["documents"][0]["source"]["n"]),
        Some(1.0),
        "{body}"
    );
    assert_eq!(body["readToken"], read1.as_str(), "{body}");
    assert!(
        token_covers(&read1, &t1),
        "the answer is at least T1: {read1} vs {t1}"
    );

    // A read at `T2` waits for `T2` and sees the newer write. Same request,
    // one token different: the answer changed because the read waited, not
    // because the caller retried a write.
    let reply = running
        .connect(
            GET_DOCUMENTS,
            &kb_request(&[("ids", json!([id_uint(1)])), ("consistency", at_least(&t2))]),
        )
        .await;
    let read2 = reply.token();
    let body = reply.expect_ok();
    assert_eq!(
        struct_number(&body["documents"][0]["source"]["n"]),
        Some(7.0),
        "{body}"
    );
    assert!(
        token_covers(&read2, &t2),
        "the answer is at least T2: {read2} vs {t2}"
    );

    // The scroll and the count carry a token the same way.
    let reply = running
        .connect(
            SCROLL_DOCUMENTS,
            &kb_request(&[("limit", json!(10)), ("consistency", at_least(&t1))]),
        )
        .await;
    let scroll_token = reply.token();
    let page = reply.expect_ok();
    assert_eq!(
        struct_number(&page["documents"][0]["source"]["n"]),
        Some(1.0),
        "{page}"
    );
    assert!(token_covers(&scroll_token, &t1), "{scroll_token} vs {t1}");
    assert_eq!(
        count(&running, "w", "kb", json!("match_all"), &t2).await,
        1,
        "one document at either token"
    );

    // A filter write honours a token the same way: the count at the token the
    // delete answered sees the deletion. (The REST test that pinned it is
    // `the_token_header_covers_the_filter_write_rpc`.)
    let deleted = running
        .connect(DELETE_BY_FILTER, &kb_request(&[("filter", tenant("a"))]))
        .await;
    let deleted_token = deleted.token();
    deleted.expect_ok();
    assert_eq!(
        count(&running, "w", "kb", tenant("a"), &deleted_token).await,
        0
    );

    // A token that is not a token is refused rather than ignored.
    for bad in ["not a token", "c1:nope", "v1:s1/p-1@0"] {
        let error = running
            .connect(
                COUNT_DOCUMENTS,
                &kb_request(&[("consistency", at_least(bad))]),
            )
            .await
            .expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{bad}: {error}");
    }

    running.shutdown().await;
}

// ----- Beyond the ports -----

/// Design §44 §5.1 and ruling 4: **no resource name in a URL path, names are
/// message fields.** The same logical operation is reachable under one path
/// whatever the namespace and the collection are, and a path that *does* carry
/// the names is not a route at all.
#[tokio::test]
async fn document_names_are_fields_not_paths() {
    let running = Running::start().await;
    for (namespace, name) in [("w", "kb"), ("w", "kb2"), ("other", "kb")] {
        running
            .connect(
                CREATE_COLLECTION,
                &json!({"namespace": namespace, "name": name, "schema": kb_schema()}),
            )
            .await
            .expect_ok();
    }

    // Every collection's documents are written and counted under the *same*
    // paths.
    for (namespace, name) in [("w", "kb"), ("w", "kb2"), ("other", "kb")] {
        running
            .connect(
                WRITE_DOCUMENTS,
                &request(
                    namespace,
                    name,
                    &[(
                        "ops",
                        json!([{"upsert": {"id": id_uint(1), "source": {"tenant": "a"}}}]),
                    )],
                ),
            )
            .await
            .expect_ok();
        let body = running
            .connect(COUNT_DOCUMENTS, &request(namespace, name, &[]))
            .await
            .expect_ok();
        assert_eq!(int64(&body["count"]), Some(1), "{namespace}/{name}: {body}");
    }

    // A name in the path is not a route: the REST shape under the Connect
    // package, and the Connect shape under `/v1`, are both unrouted, and the
    // native `404` fallback answers them.
    for path in [
        "/loams.document.v1.DocumentService/WriteDocuments/w/collections/kb",
        "/v1/loams.document.v1.DocumentService/WriteDocuments",
        "/loams.document.v1/namespaces/w/collections/kb/documents",
    ] {
        let reply = running
            .rest(
                Method::POST,
                path,
                Some(request("w", "kb", &[("ops", json!([]))])),
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
        .connect_raw(
            WRITE_DOCUMENTS,
            r#"{"namespace":"w","collection":"kb","ops":[]}"#,
        )
        .await;
    assert_eq!(reach.status(), StatusCode::OK, "{}", reach.body);
    for rpc in [
        WRITE_DOCUMENTS,
        GET_DOCUMENTS,
        SCROLL_DOCUMENTS,
        COUNT_DOCUMENTS,
    ] {
        assert!(
            !rpc.contains("/v1") && !rpc.contains("kb"),
            "a Connect path names no resource: {rpc}"
        );
    }

    running.shutdown().await;
}

/// Design §44 §7.4, D611: every `DocumentService` RPC that resolves a
/// collection answers a collection that is not there the same way — a Connect
/// `not_found` whose `reason` is a registry row, with the kind and the name in
/// `metadata` so a caller can tell a missing collection from a missing
/// namespace without parsing prose.
#[tokio::test]
async fn unknown_collection_reports_a_reason() {
    let running = Running::start().await;
    kb(&running, "w").await;

    let error = running
        .connect(COUNT_DOCUMENTS, &request("w", "nope", &[]))
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "not_found", "{error}");
    assert_reason(&error, "not_found");
    assert_eq!(
        metadata(&error, "kind").as_deref(),
        Some("collection"),
        "{error}"
    );
    assert_eq!(metadata(&error, "name").as_deref(), Some("nope"), "{error}");

    // Every other RPC that resolves a collection answers the same way.
    for (rpc, extra) in [
        (
            WRITE_DOCUMENTS,
            json!({"ops": [{"upsert": {"id": id_uint(1), "source": {"tenant": "a"}}}]}),
        ),
        (GET_DOCUMENTS, json!({"ids": [id_uint(1)]})),
        (SCROLL_DOCUMENTS, json!({"limit": 1})),
        (DELETE_BY_FILTER, json!({"filter": tenant("a")})),
        (
            PATCH_BY_FILTER,
            json!({"filter": tenant("a"), "patch": {"source": {"x": 1}}}),
        ),
    ] {
        let error = running
            .connect(rpc, &request_from("w", "nope", &extra))
            .await
            .expect_error(StatusCode::NOT_FOUND);
        assert_eq!(error["code"], "not_found", "{rpc}: {error}");
        assert_reason(&error, "not_found");
        assert_eq!(
            metadata(&error, "name").as_deref(),
            Some("nope"),
            "{rpc}: {error}"
        );
    }

    // A missing namespace is the same refusal.
    let error = running
        .connect(COUNT_DOCUMENTS, &request("nowhere", "kb", &[]))
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "not_found", "{error}");
    assert_reason(&error, "not_found");

    running.shutdown().await;
}

/// The empty states: a collection nobody wrote to counts zero, scrolls to one
/// empty page and answers no document. A caller that reads before it writes
/// must get an answer and not an error, and proto3 JSON omits the empties.
#[tokio::test]
async fn count_and_scroll_of_an_empty_collection_answer_zero_rpc() {
    let running = Running::start().await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema()}),
        )
        .await
        .expect_ok();

    let reply = running
        .connect(COUNT_DOCUMENTS, &request("w", "kb", &[]))
        .await;
    let token = reply.token();
    let body = reply.expect_ok();
    assert_documented_keys(&body, &COUNT_KEYS, "CountDocumentsResponse");
    assert!(absent_or(&body, "count", json!(0)), "{body}");
    assert_eq!(body["readToken"], token.as_str(), "{body}");

    let reply = running
        .connect(SCROLL_DOCUMENTS, &kb_request(&[("limit", json!(10))]))
        .await;
    let token = reply.token();
    let page = reply.expect_ok();
    assert_documented_keys(&page, &SCROLL_KEYS, "ScrollDocumentsResponse");
    assert!(absent_or(&page, "documents", json!([])), "{page}");
    assert!(
        page.get("next").is_none() || page["next"].is_null(),
        "one page is the last page: {page}"
    );
    assert_eq!(page["readToken"], token.as_str(), "{page}");

    // A filter that matches nothing counts zero too.
    let body = running
        .connect(COUNT_DOCUMENTS, &kb_request(&[("filter", tenant("a"))]))
        .await;
    let body = body.expect_ok();
    assert!(absent_or(&body, "count", json!(0)), "{body}");

    // And a get of a document nobody wrote answers an entry with no `id`.
    let body = running
        .connect(GET_DOCUMENTS, &kb_request(&[("ids", json!([id_uint(1)]))]))
        .await;
    let body = body.expect_ok();
    assert!(is_missing(&body["documents"][0]), "{body}");

    // An empty write is a success that wrote nothing.
    let reply = running
        .connect(WRITE_DOCUMENTS, &kb_request(&[("ops", json!([]))]))
        .await;
    let token = reply.token();
    let body = reply.expect_ok();
    assert!(absent_or(&body, "results", json!([])), "{body}");
    assert!(absent_or(&body, "positions", json!([])), "{body}");
    assert_eq!(body["token"], token.as_str(), "{body}");

    running.shutdown().await;
}

/// A document id the wire cannot spell is `invalid_argument`, and so is a
/// document op that is none of the three, a malformed filter, or an op the
/// handler cannot parse.
///
/// Where the **codec** is what refused — a field of the wrong JSON type — only
/// the Connect `code` is asserted, because a decode failure never reaches the
/// handler and carries no `ErrorInfo`. Where the **handler** is what refused,
/// the `reason` is asserted too, because a caller branches on it
/// (`docs/api/reasons.md`).
#[tokio::test]
async fn a_malformed_document_id_is_invalid_argument_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;

    // A document id the codec refuses: an arm of the wrong JSON type, a value
    // the arm cannot hold, and two arms at once.
    for bad in [
        json!({"uint": "not a number"}),
        json!({"uint": true}),
        json!({"uint": "-1"}),
        json!({"string": 1}),
        json!({"uuid": 7}),
        json!({"uint": "1", "string": "k-str"}),
    ] {
        let error = running
            .connect(GET_DOCUMENTS, &kb_request(&[("ids", json!([bad]))]))
            .await;
        let error = error.expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{bad}: {error}");
    }

    // A `source` that is not an object and a `delete_keys` that is not a list
    // are the same codec refusal inside a write.
    for bad in [
        json!({"upsert": {"id": id_uint(1), "source": "not an object"}}),
        json!({"patch": {"id": id_uint(1), "deleteKeys": "not a list"}}),
    ] {
        let error = running
            .connect(WRITE_DOCUMENTS, &kb_request(&[("ops", json!([bad]))]))
            .await;
        let error = error.expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{bad}: {error}");
    }

    // An id that is a `uuid` arm but not a UUID: the handler parses it.
    let error = running
        .connect(
            GET_DOCUMENTS,
            &kb_request(&[("ids", json!([id_uuid("not-a-uuid")]))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");

    // A write op is exactly one of `upsert`, `delete` and `patch`.
    let error = running
        .connect(
            WRITE_DOCUMENTS,
            &kb_request(&[("ops", json!([{"merge": {"id": id_uint(1)}}]))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");

    // A vector that is not a list of numbers and a patch mode that is not one
    // of the three: both refused by the handler, which names the op's index.
    for bad in [
        json!({"upsert": {"id": id_uint(1), "vectors": {"embedding": "not a list"}}}),
        json!({"patch": {"id": id_uint(1), "mode": "other"}}),
    ] {
        let error = running
            .connect(WRITE_DOCUMENTS, &kb_request(&[("ops", json!([bad]))]))
            .await;
        let error = error.expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_argument", "{bad}: {error}");
        assert_reason(&error, "invalid_argument");
        assert_eq!(metadata(&error, "index").as_deref(), Some("0"), "{bad}");
    }

    // A malformed filter is the same refusal. (`filter` is the native query
    // JSON until Task 4 types it, so this is the REST refusal.)
    let error = running
        .connect(
            COUNT_DOCUMENTS,
            &kb_request(&[("filter", json!({"term": {"field": "tenant"}}))]),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert_reason(&error, "invalid_argument");

    running.shutdown().await;
}

/// **WIRE**: a document's `source` is a `google.protobuf.Struct`, so the JSON a
/// caller writes is the JSON it reads — every kind a document can carry, and
/// keys the schema has never heard of. The only normalization a `Struct` costs
/// is a whole number answering as `3.0` (`Value.number_value` is a `double`),
/// which is what `assert_struct_eq` reads through.
#[tokio::test]
async fn document_source_round_trips_as_json_rpc() {
    let running = Running::start().await;
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": "w", "name": "kb", "schema": kb_schema()}),
        )
        .await
        .expect_ok();
    let source = json!({
        "text": "hello",
        "count": 3,
        "ratio": 0.5,
        "flag": true,
        "nothing": null,
        "list": [1, "two", false, {"deep": 4}],
        "nested": {"a": {"b": {"c": [1.5]}}},
        "tenant": "a",
        "n": 1,
        "a_key_the_schema_has_never_heard_of": {"and": ["its", "value"]}
    });
    running
        .connect(
            WRITE_DOCUMENTS,
            &kb_request(&[(
                "ops",
                json!([{"upsert": {"id": id_uint(1), "source": source}}]),
            )]),
        )
        .await
        .expect_ok();

    let body = running
        .connect(GET_DOCUMENTS, &kb_request(&[("ids", json!([id_uint(1)]))]))
        .await;
    let body = body.expect_ok();
    assert_documented_keys(&body["documents"][0], &DOC_KEYS, "Document");
    assert_struct_eq(&body["documents"][0]["source"], &source, "the source");

    // A projection selects parts of the source…
    let body = running
        .connect(
            GET_DOCUMENTS,
            &kb_request(&[
                ("ids", json!([id_uint(1)])),
                (
                    "select",
                    json!({"source": {"include": ["nested"], "exclude": []}}),
                ),
            ]),
        )
        .await;
    let body = body.expect_ok();
    assert_struct_eq(
        &body["documents"][0]["source"],
        &json!({"nested": {"a": {"b": {"c": [1.5]}}}}),
        "the included part of the source",
    );

    // …and `source: "none"` leaves the source out entirely rather than
    // answering an empty object, while the id and the place in the collection
    // are still there.
    let body = running
        .connect(
            GET_DOCUMENTS,
            &kb_request(&[
                ("ids", json!([id_uint(1)])),
                ("select", json!({"source": "none"})),
            ]),
        )
        .await;
    let body = body.expect_ok();
    assert!(
        body["documents"][0].get("source").is_none(),
        "no source is projected: {body}"
    );
    assert_eq!(pk(&body["documents"][0]["id"]), json!(1u64), "{body}");
    assert!(int64(&body["documents"][0]["seqNo"]).is_some(), "{body}");

    running.shutdown().await;
}
