//! `loams.graph.v1` on the main port (GR1 Task 5, design §48 §8, D741), against the real `loams`
//! server. Built twice: without the `graph` feature every RPC answers `feature_not_in_variant`;
//! with it the package is served (`cargo test -p loams --features graph --test connect_graph`).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use buffa::Message as _;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use prost::Message as _;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Every RPC of both graph services, with its streaming shape.
const RPCS: &[(&str, bool)] = &[
    ("/loams.graph.v1.GraphAdminService/GetEngineInfo", false),
    ("/loams.graph.v1.GraphAdminService/CreateGraph", false),
    ("/loams.graph.v1.GraphAdminService/GetGraph", false),
    ("/loams.graph.v1.GraphAdminService/ListGraphs", false),
    ("/loams.graph.v1.GraphAdminService/UpdateGraph", false),
    ("/loams.graph.v1.GraphAdminService/DeleteGraph", false),
    ("/loams.graph.v1.GraphAdminService/GetSchema", false),
    ("/loams.graph.v1.GraphAdminService/RestoreGraph", false),
    ("/loams.graph.v1.GraphAdminService/ExportGraph", false),
    ("/loams.graph.v1.GraphAdminService/ImportGraph", false),
    ("/loams.graph.v1.GraphService/Execute", false),
    ("/loams.graph.v1.GraphService/ExecuteBatch", false),
    ("/loams.graph.v1.GraphService/ExecuteStream", true),
    ("/loams.graph.v1.GraphService/Explain", false),
];

struct Running {
    server: Server,
    base: String,
    _dir: TempDir,
}

impl Running {
    async fn start_with(edit: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.reflection = true;
        edit(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            _dir: dir,
        }
    }

    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// A Connect unary call with a JSON body.
    async fn connect(&self, rpc: &str, body: &Value) -> (reqwest::StatusCode, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("connect call");
        let status = response.status();
        let text = response.text().await.expect("a body");
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// A Connect server-streaming call with a JSON body: one enveloped request message, and the
    /// end-of-stream message's `error`, if any.
    async fn connect_stream(
        &self,
        rpc: &str,
        body: &Value,
    ) -> (reqwest::StatusCode, Vec<(u8, Value)>) {
        let payload = body.to_string().into_bytes();
        let mut framed = vec![0u8];
        framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
        framed.extend_from_slice(&payload);
        let response = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/connect+json")
            .body(framed)
            .send()
            .await
            .expect("connect stream call");
        let status = response.status();
        let bytes = response.bytes().await.expect("a body");
        let messages = frames(&bytes)
            .into_iter()
            .map(|(flags, data)| (flags, serde_json::from_slice(data).unwrap_or(Value::Null)))
            .collect();
        (status, messages)
    }

    async fn instance(&self) -> Value {
        let (status, info) = self
            .connect("/loams.instance.v1.InstanceService/GetInstance", &json!({}))
            .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{info}");
        info
    }
}

/// The length-prefixed frames of a body.
fn frames(body: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 5 <= body.len() {
        let len =
            u32::from_be_bytes([body[at + 1], body[at + 2], body[at + 3], body[at + 4]]) as usize;
        let end = at + 5 + len;
        assert!(end <= body.len(), "a frame runs past the body");
        out.push((body[at], &body[at + 5..end]));
        at = end;
    }
    out
}

/// The `ErrorInfo.reason` of a Connect JSON error body.
fn reason(error: &Value) -> String {
    let detail = &error["details"][0];
    assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{error}");
    let bytes = STANDARD_NO_PAD
        .decode(detail["value"].as_str().expect("an encoded detail"))
        .expect("base64");
    ErrorInfo::decode_from_slice(&bytes)
        .expect("ErrorInfo")
        .reason
}

/// The graph row of `GetInstance.services[]`.
fn graph_status(info: &Value) -> Value {
    info["services"]
        .as_array()
        .expect("services")
        .iter()
        .find(|s| s["package"] == "loams.graph.v1")
        .cloned()
        .unwrap_or_else(|| panic!("no loams.graph.v1 row: {info}"))
}

/// Reflection's service list, through a gRPC-Web `ListServices`.
async fn reflected_services(running: &Running) -> BTreeSet<String> {
    // ServerReflectionRequest { list_services: "" } = field 7, empty string.
    let request = [0x3a, 0x00];
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(request.len()).expect("small").to_be_bytes());
    framed.extend_from_slice(&request);
    let response = reqwest::Client::new()
        .post(format!(
            "{}/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
            running.base
        ))
        .header("content-type", "application/grpc-web+proto")
        .body(framed)
        .send()
        .await
        .expect("reflection");
    let body = response.bytes().await.expect("body");
    let message = frames(&body)
        .into_iter()
        .find(|(flags, _)| flags & 0x80 == 0)
        .expect("a reflection answer")
        .1
        .to_vec();
    // ServerReflectionResponse.list_services_response = field 6 { repeated ServiceResponse
    // service = 1 { string name = 1 } }.
    #[derive(Clone, PartialEq, prost::Message)]
    struct ServiceResponse {
        #[prost(string, tag = "1")]
        name: String,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct ListServiceResponse {
        #[prost(message, repeated, tag = "1")]
        service: Vec<ServiceResponse>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct Answer {
        #[prost(message, optional, tag = "6")]
        list_services_response: Option<ListServiceResponse>,
    }
    Answer::decode(message.as_slice())
        .expect("decodes")
        .list_services_response
        .expect("a list")
        .service
        .into_iter()
        .map(|s| s.name)
        .collect()
}

/// Every method of every service in `loams_proto::FILE_DESCRIPTOR_SET`, by service.
fn descriptor_methods() -> Vec<(String, Vec<(String, bool)>)> {
    let set = prost_types::FileDescriptorSet::decode(loams_proto::FILE_DESCRIPTOR_SET)
        .expect("descriptor set");
    let mut out = Vec::new();
    for file in &set.file {
        for service in &file.service {
            let name = format!("{}.{}", file.package(), service.name());
            let methods = service
                .method
                .iter()
                .map(|m| (m.name().to_string(), m.server_streaming()))
                .collect();
            out.push((name, methods));
        }
    }
    out
}

/// Packages `loams-proto` compiles only because a served package imports them; reflection
/// lists their services, which no route answers yet (GR1 Task 5, R2.3).
const IMPORT_ONLY: &[&str] = &["loams.operations.v1"];

/// `reflection_lists_only_served_or_stubbed_services`, for whichever build this is: every
/// service reflection lists answers every one of its methods with something other than a
/// missing route, unless its package is import-only.
#[tokio::test]
async fn reflection_lists_only_served_or_stubbed_services() {
    let running = Running::start().await;
    let listed = reflected_services(&running).await;
    assert!(listed.contains("loams.graph.v1.GraphService"), "{listed:?}");
    let methods = descriptor_methods();
    let mut checked = 0;
    for service in &listed {
        if service.starts_with("grpc.")
            || IMPORT_ONLY
                .iter()
                .any(|p| service.starts_with(&format!("{p}.")))
        {
            continue;
        }
        let Some((_, service_methods)) = methods.iter().find(|(name, _)| name == service) else {
            continue;
        };
        for (method, streaming) in service_methods {
            let rpc = format!("/{service}/{method}");
            // A Connect answer, not merely "not 404": a unary error is a JSON body with a `code`;
            // a stream is a 200 with an end-of-stream message, whose `error` has one.
            if *streaming {
                let (status, messages) = running.connect_stream(&rpc, &json!({})).await;
                assert_eq!(status, reqwest::StatusCode::OK, "{rpc}");
                let end = messages
                    .iter()
                    .find(|(flags, _)| flags & 0x02 != 0)
                    .unwrap_or_else(|| panic!("{rpc}: no end-of-stream message"));
                if let Some(error) = end.1.get("error") {
                    assert!(error["code"].is_string(), "{rpc}: {error}");
                }
            } else {
                let (status, body) = running.connect(&rpc, &json!({})).await;
                assert!(body.is_object(), "{rpc}: {status} {body}");
                if !status.is_success() {
                    assert!(body["code"].is_string(), "{rpc}: {status} {body}");
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 14, "only {checked} methods checked");
    running.server.shutdown().await.expect("shutdown");
}

// ---------------------------------------------------------------------------------------------
// Without the feature
// ---------------------------------------------------------------------------------------------

#[cfg(not(feature = "graph"))]
#[tokio::test]
async fn instance_advertises_graph_unavailable_when_off() {
    let running = Running::start().await;
    let info = running.instance().await;
    let graph = graph_status(&info);
    assert_ne!(graph["available"], true, "{graph}");
    assert_eq!(graph["unstable"], true, "{graph}");
    assert!(
        info["apiVersions"]
            .as_array()
            .is_none_or(|list| !list.iter().any(|p| p == "loams.graph.v1")),
        "{info}"
    );
    running.server.shutdown().await.expect("shutdown");
}

#[cfg(not(feature = "graph"))]
#[tokio::test]
async fn graph_rpcs_answer_not_in_variant_without_feature() {
    let running = Running::start().await;
    assert_graph_absent(&running).await;
    running.server.shutdown().await.expect("shutdown");
}

/// Every graph RPC answers `unimplemented`/`feature_not_in_variant` (`GraphAbsent`).
async fn assert_graph_absent(running: &Running) {
    for (rpc, streaming) in RPCS {
        if *streaming {
            let (status, messages) = running.connect_stream(rpc, &json!({})).await;
            assert_eq!(status, reqwest::StatusCode::OK, "{rpc}");
            let end = messages
                .iter()
                .find(|(flags, _)| flags & 0x02 != 0)
                .unwrap_or_else(|| panic!("{rpc}: no end-of-stream message"));
            let error = &end.1["error"];
            assert_eq!(error["code"], "unimplemented", "{rpc}: {error}");
            assert_eq!(reason(error), "feature_not_in_variant", "{rpc}");
        } else {
            let (status, error) = running.connect(rpc, &json!({})).await;
            assert_eq!(
                status,
                reqwest::StatusCode::NOT_IMPLEMENTED,
                "{rpc}: {error}"
            );
            assert_eq!(error["code"], "unimplemented", "{rpc}");
            assert_eq!(reason(&error), "feature_not_in_variant", "{rpc}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// With the feature
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "graph")]
#[tokio::test]
async fn instance_advertises_graph_when_feature_on() {
    let running = Running::start().await;
    let info = running.instance().await;
    let graph = graph_status(&info);
    assert_eq!(graph["available"], true, "{graph}");
    assert_eq!(graph["unstable"], true, "{graph}");
    let services: BTreeSet<&str> = graph["services"]
        .as_array()
        .expect("services")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        services,
        BTreeSet::from([
            "loams.graph.v1.GraphAdminService",
            "loams.graph.v1.GraphService"
        ])
    );
    assert!(
        info["apiVersions"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == "loams.graph.v1")),
        "{info}"
    );
    running.server.shutdown().await.expect("shutdown");
}

/// One statement over each protocol the main port speaks: Connect JSON, gRPC (HTTP/2) and
/// gRPC-Web (HTTP/1.1).
#[cfg(feature = "graph")]
#[tokio::test]
async fn connect_json_grpc_and_grpc_web_reach_execute() {
    use loams_proto::loams::graph::v1 as pb;
    let running = Running::start().await;
    let (status, graph) = running
        .connect(
            "/loams.graph.v1.GraphAdminService/CreateGraph",
            &json!({"namespace": "acme", "name": "kg", "idempotencyKey": "k1"}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{graph}");
    assert!(
        graph["id"].as_str().is_some_and(|id| id.starts_with("gr_")),
        "{graph}"
    );

    // Connect JSON.
    let (status, answer) = running
        .connect(
            "/loams.graph.v1.GraphService/Execute",
            &json!({"namespace": "acme", "graph": "kg", "statement": "RETURN 41 + 1 AS x"}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{answer}");
    assert_eq!(
        answer["rows"]["rows"][0]["values"][0]["int64"], "42",
        "{answer}"
    );

    // gRPC and gRPC-Web, with a binary ExecuteRequest.
    let request = pb::ExecuteRequest {
        namespace: "acme".to_string(),
        graph: "kg".to_string(),
        statement: "RETURN 7 AS x".to_string(),
        ..Default::default()
    };
    let payload = request.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
    framed.extend_from_slice(&payload);
    for (content_type, prior_knowledge) in [
        ("application/grpc+proto", true),
        ("application/grpc-web+proto", false),
    ] {
        let mut builder = reqwest::Client::builder();
        if prior_knowledge {
            builder = builder.http2_prior_knowledge();
        }
        let response = builder
            .build()
            .expect("client")
            .post(format!(
                "{}/loams.graph.v1.GraphService/Execute",
                running.base
            ))
            .header("content-type", content_type)
            .header("te", "trailers")
            .body(framed.clone())
            .send()
            .await
            .unwrap_or_else(|err| panic!("{content_type}: {err}"));
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{content_type}");
        let body = response.bytes().await.expect("body");
        let message = frames(&body)
            .into_iter()
            .find(|(flags, _)| flags & 0x80 == 0)
            .unwrap_or_else(|| panic!("{content_type}: no message"))
            .1
            .to_vec();
        let answer = pb::ExecuteResponse::decode_from_slice(&message).expect("an ExecuteResponse");
        let value = &answer.rows.as_option().expect("rows").rows[0].values[0];
        assert!(
            matches!(value.kind, Some(pb::__buffa::oneof::value::Kind::Int64(7))),
            "{content_type}: {value:?}"
        );
    }
    running.server.shutdown().await.expect("shutdown");
}

/// A graph needs persistent storage: without a graph data directory the server refuses to
/// start, unless graphs are explicitly ephemeral (dev or in-memory mode; Task 3 review I2).
#[cfg(feature = "graph")]
#[tokio::test]
async fn graph_requires_a_data_dir() {
    let dir = TempDir::new().expect("temp dir");
    let mut config = ServerConfig::new(dir.path());
    config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
    config.graph.data_dir = None;
    let err = Server::start(config).await.expect_err("refused");
    assert!(err.to_string().contains("graph"), "{err}");
    assert!(err.to_string().contains("data"), "{err}");

    let running = Running::start_with(|config| {
        config.graph.data_dir = None;
        config.graph.ephemeral = true;
    })
    .await;
    let (status, _) = running
        .connect(
            "/loams.graph.v1.GraphAdminService/CreateGraph",
            &json!({"namespace": "acme", "name": "kg"}),
        )
        .await;
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "an ephemeral graph server serves"
    );
    running.server.shutdown().await.expect("shutdown");
}

/// Graph RPCs have no authorizer until MT1 (D750): serving them on a non-loopback address is
/// refused with a clear message.
#[cfg(feature = "graph")]
#[tokio::test]
async fn non_loopback_listen_without_authorizer_refused() {
    // `::ffff:127.0.0.1` too: `Ipv6Addr::is_loopback` is `::1` only, and refusing the mapped
    // form is the safe side.
    for listen in ["0.0.0.0:0", "[::]:0", "[::ffff:127.0.0.1]:0"] {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = listen.parse().expect("an address");
        let err = Server::start(config).await.expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("loopback"), "{listen}: {text}");
        assert!(text.contains("graph"), "{listen}: {text}");
    }
}

/// In a `graph` build, `--no-graph` serves `GraphAbsent`: every graph RPC answers
/// `feature_not_in_variant` and `GetInstance` reports the package unavailable.
#[cfg(feature = "graph")]
#[tokio::test]
async fn no_graph_answers_graph_absent() {
    let running = Running::start_with(|config| config.graph.enabled = false).await;
    assert!(running.server.graph_data_dir().is_none());
    assert_ne!(graph_status(&running.instance().await)["available"], true);
    assert_graph_absent(&running).await;
    running.server.shutdown().await.expect("shutdown");
}

/// In a `graph` build, a cluster node does not serve Loams Graph before it has the `graph` role
/// (R5.2, Task 12), so it answers `GraphAbsent` (the node's start path takes `serves_graph`; a
/// cluster node cannot start in process here).
#[cfg(feature = "graph")]
#[test]
fn a_cluster_node_does_not_serve_graph() {
    let dir = TempDir::new().expect("temp dir");
    let mut config = ServerConfig::new(dir.path());
    assert!(config.serves_graph());
    config.cluster = Some(loams::ClusterConfig::new(
        1,
        loams_hot::Roles::all(),
        "127.0.0.1:1",
        std::collections::BTreeMap::from([(1, "127.0.0.1:1".to_string())]),
    ));
    assert!(!config.serves_graph());
    config.cluster = None;
    config.graph.enabled = false;
    assert!(!config.serves_graph());
}

/// The scheduled catalog sweep (R4.9) runs on its interval, without anyone calling
/// `sweep_documents`. (What it deletes is `loams-graph`'s tests: outside `test-hooks` its grace
/// has a 5-minute floor.)
#[cfg(feature = "graph")]
#[tokio::test]
async fn catalog_sweep_runs_on_schedule() {
    let running = Running::start_with(|config| {
        config.graph.maintenance_every = Duration::from_millis(50);
    })
    .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while running.server.graph_sweeps().expect("graph is served") < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the sweep never ran twice"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    running.server.shutdown().await.expect("shutdown");
}

/// `maintenance_every` must be greater than zero (a zero interval would panic the task).
#[cfg(feature = "graph")]
#[tokio::test]
async fn zero_maintenance_interval_refused() {
    let dir = TempDir::new().expect("temp dir");
    let mut config = ServerConfig::new(dir.path());
    config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
    config.graph.maintenance_every = Duration::ZERO;
    let err = Server::start(config).await.expect_err("refused");
    assert!(err.to_string().contains("maintenance_every"), "{err}");
}

/// The scheduled purge (Task 4 R4.9): a deleted graph's storage goes once the retention hold has
/// passed, without anyone calling `purge_expired`.
#[cfg(feature = "graph")]
#[tokio::test]
async fn deleted_graphs_are_purged_on_schedule() {
    let running = Running::start_with(|config| {
        config.graph.retention_hold = Duration::ZERO;
        config.graph.maintenance_every = Duration::from_millis(100);
    })
    .await;
    let (_, graph) = running
        .connect(
            "/loams.graph.v1.GraphAdminService/CreateGraph",
            &json!({"namespace": "acme", "name": "kg"}),
        )
        .await;
    let id = graph["id"].as_str().expect("an id").to_string();
    let (status, _) = running
        .connect(
            "/loams.graph.v1.GraphService/Execute",
            &json!({"namespace": "acme", "graph": "kg", "statement": "INSERT (:X)"}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let dir = running
        .server
        .graph_data_dir()
        .expect("a graph data dir")
        .join("graphs")
        .join(&id);
    assert!(dir.exists(), "{}", dir.display());
    let (status, _) = running
        .connect(
            "/loams.graph.v1.GraphAdminService/DeleteGraph",
            &json!({"namespace": "acme", "name": "kg"}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while dir.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "not purged: {}",
            dir.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    running.server.shutdown().await.expect("shutdown");
}
