//! The Connect API on the main port (design §44 §4, D600; API1 plan Task 1),
//! against the real `loams` server over its `--listen` address.
//!
//! These are the five tests the plan names for Task 1. Everything else about
//! the API is Task 2 onwards: no application RPC is served yet, so the only
//! service that answers is `loams.instance.v1.InstanceService`, and the only
//! one that refuses is `loams.live.v1.LiveService`.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use buffa::Message as _;
use loams::api::connect::VARIANT;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use serde_json::Value;
use tempfile::TempDir;

/// An in-process server on an ephemeral port, with fast background work.
struct Running {
    server: Server,
    base: String,
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
        edit(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            _dir: dir,
        }
    }

    /// A Connect unary call: `POST` with a JSON body, which is what `curl`
    /// sends (design §44 §4).
    async fn connect(&self, rpc: &str, body: &str) -> (reqwest::StatusCode, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .expect("connect call");
        let status = response.status();
        let body = response.json().await.expect("a JSON body");
        (status, body)
    }
}

/// Design §44 §4: a Connect unary call is an HTTP `POST` with a JSON body, so
/// `curl` reaches every RPC of the API without a bespoke REST layer. The
/// shape is the point: the path is `/<package>.<Service>/<Method>`, the request
/// is `content-type: application/json` and the answer is a JSON object.
#[tokio::test]
async fn connect_json_unary_via_curl_shape() {
    let running = Running::start().await;
    let (status, info) = running
        .connect("/loams.instance.v1.InstanceService/GetInstance", "{}")
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{info}");

    // proto3 JSON: `api_versions` is `apiVersions`, an enum is its proto name.
    assert_eq!(info["edition"], "EDITION_OSS", "{info}");
    assert!(
        info["apiVersions"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == "loams.instance.v1")),
        "{info}"
    );
    // `services[]` is the long form: what is served *and* what is not, so an
    // SDK feature-detects instead of calling and reading the error.
    let services = info["services"].as_array().expect("services[]");
    let entry = |package: &str| {
        services
            .iter()
            .find(|entry| entry["package"] == package)
            .unwrap_or_else(|| panic!("{package} is missing from {info}"))
    };
    let served = entry("loams.instance.v1");
    assert_eq!(served["available"], true, "{served}");
    assert_eq!(served["version"], "v1", "{served}");
    assert_eq!(
        served["services"],
        serde_json::json!(["loams.instance.v1.InstanceService"]),
        "{served}"
    );
    let absent = entry("loams.live.v1");
    // proto3 omits a false bool, so "not served" is absent-or-false.
    assert!(
        absent
            .get("available")
            .is_none_or(|v| v == &Value::Bool(false)),
        "{absent}"
    );
    // A package this binary does not serve is not in `apiVersions`.
    assert!(
        !info["apiVersions"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == "loams.live.v1")),
        "{info}"
    );

    running.server.shutdown().await.expect("shutdown");
}

/// Design §44 §4: connect-rust answers the Connect protocol, gRPC and
/// gRPC-Web from one port, so `grpcurl`, a browser and `curl` all reach the
/// same RPC on the same address. gRPC needs HTTP/2 (prior knowledge here: the
/// listener is plaintext, so there is no ALPN to negotiate), gRPC-Web is
/// HTTP/1.1, and both frame the message the same way.
#[tokio::test]
async fn grpc_and_grpc_web_on_same_port() {
    const RPC: &str = "/loams.instance.v1.InstanceService/GetInstance";
    let running = Running::start().await;

    // An empty `GetInstanceRequest`: one length-prefixed frame, compression
    // flag 0 and a zero length.
    let frame = [0u8, 0, 0, 0, 0];
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
            .post(format!("{}{RPC}", running.base))
            .header("content-type", content_type)
            .header("te", "trailers")
            .body(frame.to_vec())
            .send()
            .await
            .unwrap_or_else(|err| panic!("{content_type}: {err}"));
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{content_type}");
        assert!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with(content_type)),
            "{content_type}: {:?}",
            response.headers()
        );
        let body = response.bytes().await.expect("a framed body");
        let frames = frames(&body);
        assert!(!frames.is_empty(), "{content_type}: {body:?}");
        assert_eq!(frames[0].0, 0, "{content_type}: the frame is uncompressed");
        assert!(!frames[0].1.is_empty(), "{content_type}: a framed answer");
        if prior_knowledge {
            // gRPC carries `grpc-status` in HTTP/2 trailers.
            assert_eq!(frames.len(), 1, "{content_type}: {body:?}");
        } else {
            // gRPC-Web has no trailers to put it in, so it appends a frame
            // whose flag byte has 0x80 set.
            assert_eq!(
                frames.last().map(|frame| frame.0 & 0x80),
                Some(0x80),
                "{content_type}: no trailer frame in {body:?}"
            );
        }
    }

    running.server.shutdown().await.expect("shutdown");
}

/// The frames of a gRPC or gRPC-Web body: each is a compression flag byte, a
/// big-endian message length and that many bytes. Panics on a partial one.
fn frames(body: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 5 <= body.len() {
        let len =
            u32::from_be_bytes([body[at + 1], body[at + 2], body[at + 3], body[at + 4]]) as usize;
        let end = at + 5 + len;
        assert!(end <= body.len(), "a frame runs past the body: {body:?}");
        out.push((body[at], &body[at + 5..end]));
        at = end;
    }
    assert_eq!(at, body.len(), "a trailing partial frame: {body:?}");
    out
}

/// Design §44 §4: `grpc.health.v1.Health` is served on the main port, so
/// `grpc_health_probe`, Kubernetes' gRPC liveness probe and a service mesh
/// can check this node without a bespoke endpoint. An empty service name is
/// the whole process.
#[tokio::test]
async fn health_rpc_ok() {
    let running = Running::start().await;
    let (status, health) = running.connect("/grpc.health.v1.Health/Check", "{}").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{health}");
    assert_eq!(health["status"], "SERVING", "{health}");

    // The catalogue's served services are registered too, so a probe that
    // names one gets an answer instead of `not_found`.
    let (status, named) = running
        .connect(
            "/grpc.health.v1.Health/Check",
            r#"{"service":"loams.instance.v1.InstanceService"}"#,
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{named}");
    assert_eq!(named["status"], "SERVING", "{named}");

    running.server.shutdown().await.expect("shutdown");
}

/// Design §44 §4, D600: a service whose engine is not in the build variant
/// answers `unimplemented` with `ErrorInfo.reason = feature_not_in_variant`
/// and the variant in `metadata`, so a caller can tell "not in this build"
/// from "not written yet". `loams.live.v1` is a `full`-variant engine
/// (§30 §8.2) and no variant this binary is has it.
#[tokio::test]
async fn unavailable_service_reports_reason() {
    let running = Running::start().await;
    let (status, error) = running
        .connect("/loams.live.v1.LiveService/Query", "{}")
        .await;
    assert_eq!(status, reqwest::StatusCode::NOT_IMPLEMENTED, "{error}");
    assert_eq!(error["code"], "unimplemented", "{error}");

    let details = error["details"].as_array().expect("details[]");
    let detail = details
        .first()
        .unwrap_or_else(|| panic!("no ErrorInfo detail in {error}"));
    assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{error}");
    let bytes = STANDARD_NO_PAD
        .decode(
            detail["value"]
                .as_str()
                .unwrap_or_else(|| panic!("an encoded ErrorInfo in {error}")),
        )
        .expect("the Connect protocol base64: unpadded standard");
    let info = ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo");
    assert_eq!(info.reason, "feature_not_in_variant");
    assert_eq!(
        info.metadata.get("variant").map(String::as_str),
        Some(VARIANT)
    );

    running.server.shutdown().await.expect("shutdown");
}

/// Design §44 §4, Q603: reflection publishes the schema of the API to anyone
/// who can reach the port, so it is off by default and `loams dev` turns it
/// on (`ServerConfig.reflection`, which `main.rs`'s `Dev` arm sets).
///
/// What is pinned here is the whole round trip: with reflection on, a client
/// that speaks the reflection protocol — what `grpcurl` does — gets the list
/// of services the server was built from, off `loams-proto`'s descriptor set;
/// with it off, the same path falls through to the native `404` like any other
/// unknown path.
///
/// The request is gRPC-Web because reflection is a bidirectional stream, which
/// the Connect unary protocol cannot carry.
#[tokio::test]
async fn reflection_lists_the_served_services_where_it_is_asked_for() {
    use connectrpc_reflection::wire::v1::ServerReflectionRequest;
    use connectrpc_reflection::wire::v1::ServerReflectionResponse;
    use connectrpc_reflection::wire::v1::server_reflection_request::MessageRequest;
    use connectrpc_reflection::wire::v1::server_reflection_response::MessageResponse;

    let request = ServerReflectionRequest {
        message_request: Some(MessageRequest::ListServices(String::new())),
        ..Default::default()
    };
    let payload = ServerReflectionRequest::encode_to_vec(&request);
    let mut frame = vec![0u8];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);

    let running = Running::start_with(|config| config.reflection = true).await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
            running.base
        ))
        .header("content-type", "application/grpc-web+proto")
        .body(frame)
        .send()
        .await
        .expect("reflection request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let answer = response.bytes().await.expect("a framed answer");
    let frames = frames(&answer);
    let answer =
        ServerReflectionResponse::decode_from_slice(frames[0].1).expect("a reflection answer");
    let Some(response) = answer.message_response else {
        panic!("{answer:?}");
    };
    let MessageResponse::ListServicesResponse(list) = response else {
        panic!("{response:?}");
    };
    assert!(
        list.service
            .iter()
            .any(|service| service.name == "loams.instance.v1.InstanceService"),
        "{list:?}"
    );
    running.server.shutdown().await.expect("shutdown");

    let running = Running::start().await;
    let (status, body) = running
        .connect(
            "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
            "{}",
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "not_found", "{body}");
    running.server.shutdown().await.expect("shutdown");
}

/// Design §44 §7.4, D611: `docs/api/reasons.md` is the registry of every
/// `reason` a caller may branch on. Within an API major version a reason is
/// never renamed and never removed, so the page may only grow: the test fails
/// on a duplicate, on a name outside `snake_case`, and on a registry that has
/// lost one of the reasons it promised.
#[test]
fn reasons_are_snake_case_and_unique() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/api/reasons.md");
    let text = std::fs::read_to_string(&path).expect("read docs/api/reasons.md");
    let mut reasons = BTreeSet::new();
    for line in text.lines() {
        // A registry row: `| `reason` | code | raised by | … |`.
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let Some(reason) = cells[1].strip_prefix('`').and_then(|r| r.strip_suffix('`')) else {
            continue;
        };
        assert!(
            reason.starts_with(|c: char| c.is_ascii_lowercase()),
            "a reason is lower_snake_case: {reason}"
        );
        assert!(
            reason
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "a reason is [a-z0-9_] only: {reason}"
        );
        assert!(
            reasons.insert(reason.to_owned()),
            "duplicate reason: {reason}"
        );
    }
    // The reasons the API promises today. Removing one of these is a breaking
    // change, and this line is what says so.
    for promised in [
        // AP0's, before this task.
        "approval_expired",
        "device_revoked",
        "not_implemented",
        // This task's.
        "feature_not_in_variant",
        // HS1 Task 7's: `loams.house.v1` with no House front configured.
        "house_not_configured",
        // The code-to-class mapping every RPC error goes through (D611).
        "invalid_argument",
        "not_found",
        "unavailable",
    ] {
        assert!(
            reasons.contains(promised),
            "{promised} is missing from {path:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `loams.house.v1` through the main port (HS1 Task 7, design §49 §18.1, D778):
// the engine proxies `/loams.house.v1.*` to `[house] endpoint` (a
// `loams-fabric house` front), and lists the package only while the endpoint's
// health check passes.
// ---------------------------------------------------------------------------

mod house {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::{Request, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get};
    use bytes::Bytes;
    use futures::StreamExt as _;
    use tokio::sync::{mpsc, oneshot};

    use super::*;

    const EXECUTE: &str = "/loams.house.v1.HouseService/ExecuteQuery";

    /// What the stand-in House front saw of the one proxied request.
    #[derive(Debug, Default)]
    struct Seen {
        authorization: Option<String>,
        path_and_query: String,
        body: Vec<u8>,
        hop_by_hop: Vec<String>,
    }

    /// A stand-in for `loams-fabric house`: `/ping` answers while `healthy`, and
    /// the RPC path records what it got, tells the test when the first request
    /// chunk arrived, sends one response chunk, and sends the second only when
    /// the test says so.
    struct Front {
        healthy: AtomicBool,
        seen: Mutex<Seen>,
        first_chunk: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }

    async fn ping(State(front): State<Arc<Front>>) -> Response {
        if front.healthy.load(Ordering::SeqCst) {
            (StatusCode::OK, "Ok.\n").into_response()
        } else {
            (StatusCode::SERVICE_UNAVAILABLE, "down\n").into_response()
        }
    }

    async fn rpc(State(front): State<Arc<Front>>, request: Request) -> Response {
        let headers: &HeaderMap = request.headers();
        {
            let mut seen = front.seen.lock().expect("seen");
            seen.authorization = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            seen.path_and_query = request
                .uri()
                .path_and_query()
                .map(|p| p.as_str().to_owned())
                .unwrap_or_default();
            for name in ["connection", "keep-alive", "upgrade", "proxy-authorization"] {
                if headers.contains_key(name) {
                    seen.hop_by_hop.push(name.to_owned());
                }
            }
        }
        let mut body = request.into_body().into_data_stream();
        while let Some(data) = body.next().await {
            let data = data.expect("a request chunk");
            front
                .seen
                .lock()
                .expect("seen")
                .body
                .extend_from_slice(&data);
            if let Some(tell) = front.first_chunk.lock().expect("first").take() {
                let _ = tell.send(());
            }
        }
        let release = front.release.lock().expect("release").take();
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(2);
        tokio::spawn(async move {
            let _ = tx.send(Ok(Bytes::from_static(b"first;"))).await;
            if let Some(release) = release {
                let _ = release.await;
            }
            let _ = tx.send(Ok(Bytes::from_static(b"second"))).await;
        });
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/connect+json")
            .header("x-front", "house")
            .body(Body::from_stream(stream))
            .expect("a response")
    }

    /// Starts the stand-in front on an ephemeral loopback port.
    async fn front(healthy: bool) -> (Arc<Front>, String) {
        let front = Arc::new(Front {
            healthy: AtomicBool::new(healthy),
            seen: Mutex::new(Seen::default()),
            first_chunk: Mutex::new(None),
            release: Mutex::new(None),
        });
        let app = Router::new()
            .route("/ping", get(ping))
            .route("/{*rest}", any(rpc))
            .with_state(front.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (front, format!("http://{addr}"))
    }

    async fn with_house(endpoint: Option<&str>) -> Running {
        let endpoint = endpoint.map(|e| e.parse::<url::Url>().expect("a URL"));
        Running::start_with(move |config| {
            config.house.endpoint = endpoint;
            config.house.health_interval = Duration::from_millis(50);
        })
        .await
    }

    /// `GetInstance`'s answer for `loams.house.v1`: (in `services[]` as
    /// available, in `apiVersions`).
    async fn listed(running: &Running) -> (bool, bool) {
        let (status, info) = running
            .connect("/loams.instance.v1.InstanceService/GetInstance", "{}")
            .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{info}");
        let entry = info["services"]
            .as_array()
            .and_then(|list| list.iter().find(|e| e["package"] == "loams.house.v1"))
            .unwrap_or_else(|| panic!("loams.house.v1 is missing from {info}"))
            .clone();
        assert_eq!(entry["unstable"], true, "{entry}");
        assert_eq!(
            entry["services"],
            serde_json::json!(["loams.house.v1.HouseService"]),
            "{entry}"
        );
        let available = entry.get("available") == Some(&Value::Bool(true));
        let versioned = info["apiVersions"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == "loams.house.v1"));
        (available, versioned)
    }

    async fn becomes(running: &Running, want: (bool, bool)) -> bool {
        for _ in 0..100 {
            if listed(running).await == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// The `ErrorInfo` of a Connect JSON error.
    fn reason_of(error: &Value) -> ErrorInfo {
        let detail = error["details"]
            .as_array()
            .and_then(|d| d.first())
            .unwrap_or_else(|| panic!("no ErrorInfo detail in {error}"));
        assert_eq!(detail["type"], "loams.errors.v1.ErrorInfo", "{error}");
        let bytes = STANDARD_NO_PAD
            .decode(detail["value"].as_str().expect("an encoded ErrorInfo"))
            .expect("unpadded base64");
        ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
    }

    /// §49 §18.1: the main port forwards `/loams.house.v1.*` with the caller's
    /// `Authorization`, streaming the request to the front as it arrives and the
    /// answer back as it is produced (a server-streaming `ExecuteQuery` must not
    /// be buffered whole).
    #[tokio::test]
    async fn house_proxy_streams_and_preserves_auth() {
        let (front, endpoint) = front(true).await;
        let (first_tx, first_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *front.first_chunk.lock().expect("first") = Some(first_tx);
        *front.release.lock().expect("release") = Some(release_rx);
        let running = with_house(Some(&endpoint)).await;

        let (body_tx, body_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(2);
        let call = reqwest::Client::new()
            .post(format!("{}{EXECUTE}?trace=1", running.base))
            .header("content-type", "application/connect+json")
            .header("authorization", "Bearer lk_test.secret-token")
            .header("connection", "keep-alive")
            .body(reqwest::Body::wrap_stream(
                tokio_stream::wrappers::ReceiverStream::new(body_rx),
            ))
            .send();
        let call = tokio::spawn(call);

        // Request streaming: the front sees the first chunk while the client has
        // not finished its body.
        body_tx
            .send(Ok(Bytes::from_static(b"{\"sql\":")))
            .await
            .expect("send");
        tokio::time::timeout(Duration::from_secs(10), first_rx)
            .await
            .expect("the first request chunk reached the front before the body ended")
            .expect("told");
        body_tx
            .send(Ok(Bytes::from_static(b"\"SELECT 1\"}")))
            .await
            .expect("send");
        drop(body_tx);

        let mut response = call.await.expect("joined").expect("the proxied call");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers()["x-front"], "house");
        assert_eq!(
            response.headers()["content-type"],
            "application/connect+json"
        );
        // Response streaming: the first chunk arrives while the front still
        // holds the second.
        let first = tokio::time::timeout(Duration::from_secs(10), response.chunk())
            .await
            .expect("the first response chunk arrived before the front finished")
            .expect("a chunk")
            .expect("not the end");
        assert_eq!(&first[..], b"first;");
        release_tx.send(()).expect("release");
        let mut rest = Vec::new();
        while let Some(chunk) = response.chunk().await.expect("a chunk") {
            rest.extend_from_slice(&chunk);
        }
        assert_eq!(rest, b"second");

        let seen = std::mem::take(&mut *front.seen.lock().expect("seen"));
        assert_eq!(
            seen.authorization.as_deref(),
            Some("Bearer lk_test.secret-token"),
            "the caller's Authorization goes through unchanged"
        );
        assert_eq!(seen.path_and_query, format!("{EXECUTE}?trace=1"));
        assert_eq!(seen.body, b"{\"sql\":\"SELECT 1\"}");
        assert!(
            seen.hop_by_hop.is_empty(),
            "hop-by-hop headers stay on their hop: {:?}",
            seen.hop_by_hop
        );
        running.server.shutdown().await.expect("shutdown");
    }

    /// §49 §18.1: `GetInstance` lists `loams.house.v1` as available (and in
    /// `apiVersions`) only while an endpoint is configured and its health check
    /// passes; it is always in `services[]`, `unstable: true` until GA.
    #[tokio::test]
    async fn catalogue_lists_house_only_when_healthy() {
        let running = with_house(None).await;
        assert_eq!(listed(&running).await, (false, false), "not configured");
        running.server.shutdown().await.expect("shutdown");

        let (front, endpoint) = front(false).await;
        let running = with_house(Some(&endpoint)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            listed(&running).await,
            (false, false),
            "configured, unhealthy"
        );
        front.healthy.store(true, Ordering::SeqCst);
        assert!(becomes(&running, (true, true)).await, "healthy: listed");
        front.healthy.store(false, Ordering::SeqCst);
        assert!(becomes(&running, (false, false)).await, "unhealthy again");
        running.server.shutdown().await.expect("shutdown");

        // An endpoint nothing listens on is never listed.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = closed.local_addr().expect("addr");
        drop(closed);
        let running = with_house(Some(&format!("http://{addr}"))).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(listed(&running).await, (false, false), "unreachable");
        // And a call to it is `unavailable`, not a hang or a bare 502.
        let (status, error) = running.connect(EXECUTE, "{}").await;
        assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{error}");
        assert_eq!(error["code"], "unavailable", "{error}");
        assert_eq!(reason_of(&error).reason, "unavailable");
        running.server.shutdown().await.expect("shutdown");
    }

    /// Without `[house] endpoint`, every `loams.house.v1` RPC answers
    /// `unimplemented` with the reason `house_not_configured`, in the shape of
    /// the protocol the caller speaks: a Connect unary error, a Connect
    /// end-of-stream message, or gRPC-Web trailers.
    #[tokio::test]
    async fn house_absent_answers_feature_not_configured() {
        let running = with_house(None).await;

        let (status, error) = running.connect(EXECUTE, "{}").await;
        assert_eq!(status, reqwest::StatusCode::NOT_IMPLEMENTED, "{error}");
        assert_eq!(error["code"], "unimplemented", "{error}");
        assert_eq!(reason_of(&error).reason, "house_not_configured");
        let (_, other) = running
            .connect("/loams.house.v1.HouseService/ListTables", "{}")
            .await;
        assert_eq!(reason_of(&other).reason, "house_not_configured");

        // Connect streaming: HTTP 200 and one end-of-stream envelope.
        let response = reqwest::Client::new()
            .post(format!("{}{EXECUTE}", running.base))
            .header("content-type", "application/connect+json")
            .body(vec![0u8, 0, 0, 0, 2, b'{', b'}'])
            .send()
            .await
            .expect("a streaming call");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/connect+json"
        );
        let body = response.bytes().await.expect("body");
        let envelopes = frames(&body);
        assert_eq!(envelopes.len(), 1, "{body:?}");
        assert_eq!(envelopes[0].0, 0b10, "the end-of-stream flag");
        let end: Value = serde_json::from_slice(envelopes[0].1).expect("JSON");
        assert_eq!(end["error"]["code"], "unimplemented", "{end}");
        assert_eq!(reason_of(&end["error"]).reason, "house_not_configured");

        // gRPC-Web: HTTP 200, trailers in a 0x80 frame, status 12.
        let response = reqwest::Client::new()
            .post(format!("{}{EXECUTE}", running.base))
            .header("content-type", "application/grpc-web+proto")
            .body(vec![0u8, 0, 0, 0, 0])
            .send()
            .await
            .expect("a gRPC-Web call");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = response.bytes().await.expect("body");
        let web = frames(&body);
        assert_eq!(web.len(), 1, "{body:?}");
        assert_eq!(web[0].0, 0x80, "a trailers frame");
        let trailers = String::from_utf8_lossy(web[0].1).to_lowercase();
        assert!(trailers.contains("grpc-status: 12\r\n"), "{trailers}");
        assert!(trailers.contains("grpc-status-details-bin: "), "{trailers}");

        running.server.shutdown().await.expect("shutdown");
    }
}
