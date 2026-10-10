//! The desktop Graph page's contract (design §48 §18.2, GR1 Task 7).
//!
//! `conformance/graph/desktop/*.json` pin every call the page makes and the answer it reads. The
//! server (this crate's `GraphAdmin`, seeded with `movies.gql`) and the seeded mock
//! (`loams-apps-mock`, over Connect JSON as the page's transport speaks it) must both answer each
//! one as the fixture says; the page's own tests (GR1 Task 8) replay the same files. The
//! `GetInstance` fixture is the server's `crates/loams/tests/connect_graph.rs`'s to check, since
//! this crate has no instance service.
//!
//! The answers are generated from the server: `UPDATE_GOLDEN=1 cargo test -p loams-graph --test
//! desktop_contract` rewrites them, keeping each fixture's values at its ignored paths.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use connectrpc::{ConnectError, ErrorCode};
use futures::StreamExt as _;
use loams_apps_mock::graph::contract::{self, Answer, ErrorAnswer};
use loams_common::meta::MetaStore;
use loams_graph::Engine;
use loams_graph::catalog::{GraphCatalog, GraphMode, NewGraph};
use loams_graph::service::admin::GraphAdmin;
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::graph::v1 as pb;
use loams_store::Store;
use serde_json::Value;
use tempfile::TempDir;

/// A single-node metastore, a bucket and a data directory.
struct Fixture {
    _node: MetaNode,
    meta: Arc<dyn MetaStore>,
    store: Store,
    data_dir: TempDir,
    _meta_dir: TempDir,
}

impl Fixture {
    async fn start() -> Self {
        let meta_dir = TempDir::new().expect("temp dir");
        let clock = Arc::new(SystemClock);
        let mut config = MetaConfig::new(1, meta_dir.path(), Store::in_memory());
        config.clock = clock.clone();
        let node = MetaNode::start(config, &Router::new())
            .await
            .expect("start meta");
        node.initialize([1]).await.expect("initialize");
        node.wait_for_leader(Duration::from_secs(30))
            .await
            .expect("leader");
        let client = MetaClient::new(node.clone(), vec![], clock, MetaClientConfig::default());
        Self {
            _node: node,
            meta: Arc::new(client),
            store: Store::in_memory(),
            data_dir: TempDir::new().expect("data dir"),
            _meta_dir: meta_dir,
        }
    }

    fn admin(&self) -> GraphAdmin {
        GraphAdmin::new(
            Arc::new(Engine::with_data_dir(self.data_dir.path())),
            GraphCatalog::new(self.meta.clone(), self.store.clone()),
        )
    }
}

/// The `ErrorInfo` of an error detail's unpadded base64 value.
fn decode_info(value: &str) -> ErrorInfo {
    use buffa::Message as _;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value.trim_end_matches('='))
        .expect("unpadded base64");
    ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
}

fn info(err: &ConnectError) -> ErrorInfo {
    let detail = err
        .details
        .iter()
        .find(|d| d.type_url == "loams.errors.v1.ErrorInfo")
        .unwrap_or_else(|| panic!("no ErrorInfo in {err:?}"));
    decode_info(detail.value.as_deref().unwrap_or_default())
}

/// A failed call as a fixture writes it.
fn error_answer(code: &str, message: String, info: ErrorInfo) -> Answer {
    Answer::Error(ErrorAnswer {
        code: code.to_string(),
        message,
        reason: info.reason,
        metadata: info.metadata.into_iter().collect(),
    })
}

/// The server's answer, as JSON.
fn answer<T: serde::Serialize>(result: Result<T, ConnectError>) -> Answer {
    match result {
        Ok(message) => Answer::Response(serde_json::to_value(message).expect("serializes")),
        Err(err) => {
            let info = info(&err);
            error_answer(err.code.as_str(), err.message.unwrap_or_default(), info)
        }
    }
}

fn request<T: serde::de::DeserializeOwned>(method: &str, request: &Value) -> T {
    serde_json::from_value(request.clone()).unwrap_or_else(|err| panic!("{method}: {err}"))
}

/// One fixture call on the server.
async fn call_server(admin: &GraphAdmin, method: &str, req: &Value) -> Answer {
    match method {
        "loams.graph.v1.GraphAdminService/ListGraphs" => {
            answer(admin.list_graphs(request(method, req)).await)
        }
        "loams.graph.v1.GraphAdminService/GetSchema" => {
            answer(admin.get_schema(request(method, req)).await)
        }
        "loams.graph.v1.GraphService/Execute" => answer(admin.execute(request(method, req)).await),
        "loams.graph.v1.GraphService/Explain" => answer(admin.explain(request(method, req)).await),
        "loams.graph.v1.GraphService/ExecuteStream" => {
            match admin.execute_stream(request(method, req)).await {
                Ok(stream) => {
                    let chunks: Vec<pb::ResultChunk> =
                        stream.map(|chunk| chunk.expect("a chunk")).collect().await;
                    Answer::Chunks(
                        chunks
                            .iter()
                            .map(|c| serde_json::to_value(c).expect("serializes"))
                            .collect(),
                    )
                }
                Err(err) => answer::<()>(Err(err)),
            }
        }
        other => panic!("no server call for {other}"),
    }
}

/// `movies` (OWNED, from `movies.gql`) and `kg` (LINKED) in `default`, as the fixtures expect.
///
/// `kg` is written to the catalog directly: `CreateGraph` refuses LINKED until GR1 Task 17, and
/// the page only lists it.
async fn seeded(fixture: &Fixture) -> GraphAdmin {
    let admin = fixture.admin();
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: contract::NAMESPACE.to_string(),
            name: "movies".to_string(),
            ..Default::default()
        })
        .await
        .expect("create movies");
    for statement in contract::movies_statements() {
        admin
            .execute(pb::ExecuteRequest {
                namespace: contract::NAMESPACE.to_string(),
                graph: "movies".to_string(),
                statement: statement.to_string(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|err| panic!("{statement}: {err:?}"));
    }
    admin
        .catalog()
        .create(
            contract::NAMESPACE,
            "kg",
            NewGraph {
                mode: GraphMode::Linked,
                ..Default::default()
            },
        )
        .await
        .expect("create kg");
    admin
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/graph/desktop")
        .join(name)
}

/// The real service, seeded with `movies`, answers every fixture call as the fixture says
/// (`elapsedNanos`, ids and times excepted, per each fixture's `ignore`).
#[tokio::test]
async fn server_answers_match_desktop_fixtures() {
    let fixture = Fixture::start().await;
    let admin = seeded(&fixture).await;
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, mut document) in contract::fixtures() {
        for exchange in &mut document.exchanges {
            if !exchange.method.starts_with("loams.graph.v1.") {
                continue;
            }
            let actual = call_server(&admin, &exchange.method, &exchange.request).await;
            checked += 1;
            if update {
                exchange.record(actual);
            } else if let Err(diff) = exchange.check(&actual) {
                failures.push(format!("{name}: {diff}"));
            }
        }
        if update {
            std::fs::write(fixture_path(name), contract::render(&document)).expect("writes");
        }
    }
    assert!(checked >= 9, "only {checked} graph calls in the fixtures");
    assert!(
        failures.is_empty(),
        "the server's answers differ from conformance/graph/desktop (regenerate with \
         UPDATE_GOLDEN=1 if the change is intended):\n{}",
        failures.join("\n\n")
    );
}

/// One fixture call on the mock, over Connect JSON: a unary POST, or an enveloped server stream.
async fn call_mock(base: &str, exchange: &contract::Exchange) -> Answer {
    let client = reqwest::Client::new();
    let url = format!("{base}/{}", exchange.method);
    if exchange.chunks.is_some() {
        let payload = exchange.request.to_string().into_bytes();
        let mut framed = vec![0u8];
        framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
        framed.extend_from_slice(&payload);
        let response = client
            .post(url)
            .header("content-type", "application/connect+json")
            .body(framed)
            .send()
            .await
            .expect("a stream call");
        assert!(response.status().is_success(), "{}", exchange.method);
        let body = response.bytes().await.expect("a body");
        let mut chunks = Vec::new();
        let mut at = 0;
        while at + 5 <= body.len() {
            let len = u32::from_be_bytes([body[at + 1], body[at + 2], body[at + 3], body[at + 4]])
                as usize;
            let message: Value =
                serde_json::from_slice(&body[at + 5..at + 5 + len]).expect("a JSON frame");
            if body[at] & 0x02 != 0 {
                if let Some(error) = message.get("error") {
                    return mock_error(error);
                }
            } else {
                chunks.push(message);
            }
            at += 5 + len;
        }
        return Answer::Chunks(chunks);
    }
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .body(exchange.request.to_string())
        .send()
        .await
        .expect("a unary call");
    let ok = response.status().is_success();
    let body: Value = response.json().await.expect("a JSON body");
    if ok {
        Answer::Response(body)
    } else {
        mock_error(&body)
    }
}

/// A Connect JSON error (`{code, message, details: [{type, value}]}`) as a fixture writes it.
fn mock_error(error: &Value) -> Answer {
    let detail = error["details"]
        .as_array()
        .and_then(|details| {
            details
                .iter()
                .find(|d| d["type"] == "loams.errors.v1.ErrorInfo")
        })
        .unwrap_or_else(|| panic!("no ErrorInfo in {error}"));
    error_answer(
        error["code"].as_str().expect("a code"),
        error["message"].as_str().unwrap_or_default().to_string(),
        decode_info(detail["value"].as_str().expect("a detail value")),
    )
}

/// The seeded mock answers every fixture call, `GetInstance` included, as the fixture says, so
/// the page behaves the same against `loams-apps-mock` and `loams dev`.
#[tokio::test]
async fn mock_answers_match_desktop_fixtures() {
    let mock = loams_apps_mock::serve(loams_apps_mock::MockConfig {
        listen: "127.0.0.1:0".parse().expect("an address"),
        ..loams_apps_mock::MockConfig::default()
    })
    .await
    .expect("the mock starts");
    let mut failures = Vec::new();
    for (name, document) in contract::fixtures() {
        for exchange in &document.exchanges {
            let actual = call_mock(&mock.url(), exchange).await;
            if let Err(diff) = exchange.check(&actual) {
                failures.push(format!("{name}: {diff}"));
            }
        }
    }
    // `values.json` decodes with the mock's generated `Value` and encodes back unchanged.
    let values: Value = serde_json::from_str(contract::VALUES_JSON).expect("values.json");
    for case in values["cases"].as_array().expect("cases") {
        let decoded: loams_apps_mock::proto::loams::graph::v1::Value =
            serde_json::from_value(case["value"].clone())
                .unwrap_or_else(|err| panic!("{}: {err}", case["name"]));
        let encoded = serde_json::to_value(&decoded).expect("encodes");
        if contract::sorted(encoded.clone()) != contract::sorted(case["value"].clone()) {
            failures.push(format!("values.json {}: {encoded}", case["name"]));
        }
    }
    mock.stop().await;
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A broken statement's error says where it broke, so the editor can underline it, and carries the
/// engine's GQLSTATUS (§48 §8.3).
#[tokio::test]
async fn syntax_error_has_position() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: "default".to_string(),
            name: "movies".to_string(),
            ..Default::default()
        })
        .await
        .expect("create");
    // Line 2, column 16: the `RETURN` where the node pattern's `)` belongs.
    let statement = "MATCH (m:Movie)\nMATCH (p:Person RETURN p";
    for explain in [false, true] {
        let err = if explain {
            admin
                .explain(pb::ExplainRequest {
                    namespace: "default".to_string(),
                    graph: "movies".to_string(),
                    statement: statement.to_string(),
                    ..Default::default()
                })
                .await
                .expect_err("a syntax error")
        } else {
            admin
                .execute(pb::ExecuteRequest {
                    namespace: "default".to_string(),
                    graph: "movies".to_string(),
                    statement: statement.to_string(),
                    read_only: true,
                    ..Default::default()
                })
                .await
                .expect_err("a syntax error")
        };
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
        let info = info(&err);
        assert_eq!(info.reason, "gql_syntax_error", "{info:?}");
        let get = |key: &str| {
            info.metadata
                .get(key)
                .unwrap_or_else(|| panic!("no {key} in {info:?}"))
                .clone()
        };
        assert_eq!(get("gqlstatus"), "42001", "{info:?}");
        assert_eq!(get("line"), "2", "{info:?}");
        assert_eq!(get("column"), "17", "{info:?}");
        assert_eq!(get("length"), "6", "{info:?}");
    }
    // A non-atomic batch reports the failed statement's position in `StatementError.info`.
    let batch = admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "default".to_string(),
            graph: "movies".to_string(),
            statements: vec![pb::Statement {
                statement: statement.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .expect("a non-atomic batch answers its failure");
    let failed = batch.error.as_option().expect("a statement error");
    let info = failed.info.as_option().expect("an ErrorInfo");
    assert_eq!(info.reason, "gql_syntax_error", "{info:?}");
    for (key, want) in [("gqlstatus", "42001"), ("line", "2"), ("column", "17")] {
        assert_eq!(
            info.metadata.get(key).map(String::as_str),
            Some(want),
            "{info:?}"
        );
    }
}

/// Canary (Grafeo 0.5.43): in a result with ORDER BY, a relationship variable is answered as
/// `INT64 0` instead of the relationship, so `execute_graph.json` has no ORDER BY. When this fails,
/// the engine answers relationships in ordered results: add `ORDER BY a.name` to that fixture's
/// statement, regenerate it, and delete this test.
#[tokio::test]
async fn canary_order_by_answers_a_relationship_as_zero() {
    use pb::__buffa::oneof::value::Kind;
    let fixture = Fixture::start().await;
    let admin = seeded(&fixture).await;
    let run = |statement: &str| {
        admin.execute(pb::ExecuteRequest {
            namespace: contract::NAMESPACE.to_string(),
            graph: "movies".to_string(),
            statement: statement.to_string(),
            read_only: true,
            ..Default::default()
        })
    };
    let kinds = |response: pb::ExecuteResponse| -> Vec<Option<Kind>> {
        let rows = response.rows.as_option().expect("rows").rows.clone();
        rows.into_iter()
            .map(|row| row.values[0].kind.clone())
            .collect()
    };
    let unordered = kinds(
        run("MATCH (a:Person)-[r:ACTED_IN]->(m:Movie) RETURN r")
            .await
            .expect("unordered"),
    );
    assert_eq!(unordered.len(), 3);
    assert!(
        unordered
            .iter()
            .all(|kind| matches!(kind, Some(Kind::Relationship(_)))),
        "{unordered:?}"
    );
    let ordered = kinds(
        run("MATCH (a:Person)-[r:ACTED_IN]->(m:Movie) RETURN r ORDER BY a.name")
            .await
            .expect("ordered"),
    );
    assert_eq!(ordered.len(), 3);
    assert!(
        ordered
            .iter()
            .all(|kind| matches!(kind, Some(Kind::Int64(0)))),
        "Grafeo now answers relationships in ordered results; see this test's doc: {ordered:?}"
    );
}
