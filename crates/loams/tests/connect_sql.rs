//! `loams.sql.v1` on the main port (API2 Task 1, SDKP P0b): the REST SQL
//! route's tests ported to `SqlService/Query`, rows equal to REST, `$n`
//! parameters, the Arrow IPC answer and its cap, and the cacheable GET.

use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use buffa::Message as _;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;

const TOKEN: &str = "loams-consistency-token";
const QUERY: &str = "/loams.sql.v1.SqlService/Query";
const QUERY_ARROW: &str = "/loams.sql.v1.SqlService/QueryArrow";
const CREATE_COLLECTION: &str = "/loams.collection.v1.CollectionService/CreateCollection";
const WRITE_DOCUMENTS: &str = "/loams.collection.v1.DocumentService/WriteDocuments";

#[derive(Debug)]
struct Reply {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Value,
}

impl Reply {
    fn expect_ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    fn expect_error(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        self.body
    }

    fn token(&self) -> String {
        self.headers
            .get(TOKEN)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| panic!("the answer carries a {TOKEN} header: {:?}", self.headers))
            .to_string()
    }
}

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

    async fn start_with(tune: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.link.batch_interval = Duration::ZERO;
        tune(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            http: reqwest::Client::new(),
            _dir: dir,
        }
    }

    async fn connect(&self, rpc: &str, body: &Value) -> Reply {
        self.connect_with(rpc, &[], body).await
    }

    async fn connect_with(&self, rpc: &str, headers: &[(&str, &str)], body: &Value) -> Reply {
        let mut request = self
            .http
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        Self::finish(request.send().await.expect("send")).await
    }

    async fn rest(&self, path: &str, body: Value) -> Reply {
        let request = self.http.post(format!("{}{path}", self.base)).json(&body);
        Self::finish(request.send().await.expect("send")).await
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

    async fn shutdown(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

fn error_info(body: &Value) -> ErrorInfo {
    let detail = &body["details"][0];
    assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{body}");
    let bytes = STANDARD_NO_PAD
        .decode(detail["value"].as_str().expect("an encoded ErrorInfo"))
        .expect("unpadded base64");
    ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
}

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

/// The M1.6 fixture's documents that carry `n`, plus one without.
fn kb_ops() -> Value {
    let upsert = |id: u64, source: Value| json!({"upsert": {"id": {"uint": id.to_string()}, "source": source}});
    json!([
        upsert(1, json!({"body": "refund policy", "tenant": "a", "n": 1})),
        upsert(2, json!({"body": "shipping times", "tenant": "a", "n": 2})),
        upsert(3, json!({"body": "refund window", "tenant": "b", "n": 3})),
        upsert(4, json!({"tenant": "c"})),
    ])
}

/// Creates `kb` in `ns` and writes the fixture; answers the write's token.
async fn kb(running: &Running, ns: &str) -> String {
    running
        .connect(
            CREATE_COLLECTION,
            &json!({"namespace": ns, "name": "kb", "schema": kb_schema(), "partitions": 2}),
        )
        .await
        .expect_ok();
    let reply = running
        .connect(
            WRITE_DOCUMENTS,
            &json!({"namespace": ns, "collection": "kb", "ops": kb_ops()}),
        )
        .await;
    let token = reply.token();
    reply.expect_ok();
    token
}

/// A row's cells as plain JSON (a `ListValue` in proto3 JSON is a JSON array),
/// every number as an `f64`: a `google.protobuf.Value` holds a double, and REST
/// writes an integer as a JSON integer.
fn rows(body: &Value) -> Value {
    floats(body.get("rows").cloned().unwrap_or(json!([])))
}

fn floats(value: Value) -> Value {
    match value {
        Value::Number(n) => json!(n.as_f64().expect("a finite number")),
        Value::Array(items) => Value::Array(items.into_iter().map(floats).collect()),
        other => other,
    }
}

// ----- Ported from `it/native_sql.rs` -----

#[tokio::test]
async fn sql_over_http_returns_columns_and_rows_rpc() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    let body = running
        .connect_with(
            QUERY,
            &[(TOKEN, &token)],
            &json!({"namespace": "w", "sql": "SELECT count(*) AS n FROM kb"}),
        )
        .await
        .expect_ok();
    assert_eq!(body["columns"], json!([{"name": "n", "type": "Int64"}]));
    assert_eq!(rows(&body), json!([[4.0]]));
    assert!(body.get("truncated").is_none(), "{body}");

    let body = running
        .connect_with(
            QUERY,
            &[(TOKEN, &token)],
            &json!({
                "namespace": "w",
                "sql": "SELECT n, tenant FROM kb WHERE n IS NOT NULL ORDER BY n",
                "consistency": {"freshness": "FRESHNESS_EVENTUAL"},
            }),
        )
        .await
        .expect_ok();
    assert_eq!(body["columns"][0]["name"], "n");
    assert_eq!(rows(&body), json!([[1.0, "a"], [2.0, "a"], [3.0, "b"]]));
    // The read waited for the header's token, and says so.
    assert_eq!(body["consistencyToken"], json!(token));
    running.shutdown().await;
}

#[tokio::test]
async fn sql_ddl_is_400_rpc() {
    let running = Running::start().await;
    kb(&running, "w").await;
    for statement in [
        "CREATE TABLE t (x INT)",
        "DROP TABLE kb",
        "INSERT INTO kb (n) VALUES (1)",
    ] {
        let body = running
            .connect(QUERY, &json!({"namespace": "w", "sql": statement}))
            .await
            .expect_error(StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_argument", "{statement}: {body}");
        assert_eq!(error_info(&body).reason, "invalid_argument", "{statement}");
    }
    let body = running
        .connect(QUERY, &json!({"sql": "SELECT 1"}))
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error_info(&body).metadata["field"], "namespace");
    running.shutdown().await;
}

#[tokio::test]
async fn query_rows_match_rest() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    for sql in [
        "SELECT n, tenant FROM kb WHERE n IS NOT NULL ORDER BY n",
        "SELECT tenant, count(*) AS c FROM kb GROUP BY tenant ORDER BY tenant",
    ] {
        let rpc = running
            .connect_with(
                QUERY,
                &[(TOKEN, &token)],
                &json!({"namespace": "w", "sql": sql}),
            )
            .await
            .expect_ok();
        let rest = running
            .rest("/v1/namespaces/w/sql", json!({"query": sql}))
            .await
            .expect_ok();
        assert_eq!(rpc["columns"], rest["columns"], "{sql}");
        assert_eq!(rows(&rpc), floats(rest["rows"].clone()), "{sql}");
    }
    running.shutdown().await;
}

// ----- Parameters (SDKP P0b) -----

#[tokio::test]
async fn query_params_bind_rpc() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    let body = running
        .connect_with(
            QUERY,
            &[(TOKEN, &token)],
            &json!({
                "namespace": "w",
                "sql": "SELECT tenant, count(*) AS c FROM kb WHERE n > $1 GROUP BY tenant ORDER BY tenant",
                "params": [1],
            }),
        )
        .await
        .expect_ok();
    assert_eq!(rows(&body), json!([["a", 1.0], ["b", 1.0]]));

    let body = running
        .connect_with(
            QUERY,
            &[(TOKEN, &token)],
            &json!({
                "namespace": "w",
                "sql": "SELECT n FROM kb WHERE tenant = $1 AND n >= $2 ORDER BY n",
                "params": ["a", 1],
            }),
        )
        .await
        .expect_ok();
    assert_eq!(rows(&body), json!([[1.0], [2.0]]));
    running.shutdown().await;
}

#[tokio::test]
async fn query_param_count_mismatch_refused_rpc() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    for (sql, params, expected) in [
        ("SELECT n FROM kb WHERE n > $1", json!([]), 1),
        ("SELECT n FROM kb WHERE n > $1", json!([1, 2]), 1),
        ("SELECT n FROM kb", json!([1]), 0),
    ] {
        let body = running
            .connect_with(
                QUERY,
                &[(TOKEN, &token)],
                &json!({"namespace": "w", "sql": sql, "params": params}),
            )
            .await
            .expect_error(StatusCode::BAD_REQUEST);
        let info = error_info(&body);
        assert_eq!(info.reason, "sql_param_count", "{sql}: {body}");
        assert_eq!(info.metadata["expected"], expected.to_string(), "{sql}");
        assert_eq!(info.metadata["field"], "params");
    }
    // A list is not a scalar.
    let body = running
        .connect(
            QUERY,
            &json!({"namespace": "w", "sql": "SELECT n FROM kb WHERE n > $1", "params": [[1]]}),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error_info(&body).reason, "invalid_argument");
    running.shutdown().await;
}

#[tokio::test]
async fn query_max_rows_truncates_rpc() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    let body = running
        .connect_with(
            QUERY,
            &[(TOKEN, &token)],
            &json!({"namespace": "w", "sql": "SELECT n FROM kb ORDER BY n", "maxRows": 2}),
        )
        .await
        .expect_ok();
    assert_eq!(rows(&body).as_array().map(Vec::len), Some(2));
    assert_eq!(body["truncated"], json!(true));
    running.shutdown().await;
}

// ----- Arrow (Q608) -----

#[tokio::test]
async fn query_arrow_roundtrips() {
    let running = Running::start().await;
    let token = kb(&running, "w").await;
    let body = running
        .connect_with(
            QUERY_ARROW,
            &[(TOKEN, &token)],
            &json!({"namespace": "w", "sql": "SELECT n, tenant FROM kb WHERE n IS NOT NULL ORDER BY n"}),
        )
        .await
        .expect_ok();
    let bytes = STANDARD
        .decode(body["arrowIpc"].as_str().expect("arrow bytes"))
        .expect("base64");
    let reader =
        arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).expect("ipc");
    let schema = reader.schema();
    assert_eq!(schema.field(0).name(), "n");
    assert_eq!(schema.field(1).name(), "tenant");
    let total: usize = reader.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(total, 3);
    running.shutdown().await;
}

#[tokio::test]
async fn query_arrow_over_cap_refused() {
    let running = Running::start_with(|config| config.query.sql.arrow_max_bytes = 64).await;
    let token = kb(&running, "w").await;
    let body = running
        .connect_with(
            QUERY_ARROW,
            &[(TOKEN, &token)],
            &json!({"namespace": "w", "sql": "SELECT * FROM kb"}),
        )
        .await
        .expect_error(StatusCode::TOO_MANY_REQUESTS);
    let info = error_info(&body);
    assert_eq!(info.reason, "result_too_large", "{body}");
    assert_eq!(info.metadata["use"], "flight");
    running.shutdown().await;
}

// ----- NO_SIDE_EFFECTS: a Connect GET -----

#[tokio::test]
async fn query_is_get_cacheable() {
    let running = Running::start().await;
    kb(&running, "w").await;
    let message = json!({"namespace": "w", "sql": "SELECT 1 AS one"}).to_string();
    let encoded: String = message
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let response = running
        .http
        .get(format!(
            "{}{QUERY}?connect=v1&encoding=json&message={encoded}",
            running.base
        ))
        .send()
        .await
        .expect("get");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("json");
    assert_eq!(rows(&body), json!([[1.0]]));
    running.shutdown().await;
}
