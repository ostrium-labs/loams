//! `loams.stream.v1` on the main port (API2 Task 2, SDKP P0c): create,
//! describe, list, produce and fetch over Connect, the idempotent produce, the
//! long poll, CloudEvents, and the wire compatibility with the tonic-era
//! messages.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use buffa::Message as _;
use loams::{Server, ServerConfig};
use loams_proto::loams::errors::v1::ErrorInfo;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;

const CREATE_NAMESPACE: &str = "/loams.collection.v1.NamespaceService/CreateNamespace";
const CREATE: &str = "/loams.stream.v1.StreamService/CreateStream";
const DESCRIBE: &str = "/loams.stream.v1.StreamService/DescribeStream";
const LIST: &str = "/loams.stream.v1.StreamService/ListStreams";
const PRODUCE: &str = "/loams.stream.v1.StreamService/Produce";
const FETCH: &str = "/loams.stream.v1.StreamService/Fetch";
const PRODUCE_EVENTS: &str = "/loams.stream.v1.StreamService/ProduceCloudEvents";
const FETCH_EVENTS: &str = "/loams.stream.v1.StreamService/FetchCloudEvents";

#[derive(Debug)]
struct Reply {
    status: StatusCode,
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
}

struct Running {
    server: Server,
    base: String,
    http: reqwest::Client,
    _dir: TempDir,
}

impl Running {
    async fn start() -> Self {
        Self::start_with(Duration::from_millis(20)).await
    }

    /// With this WAL flush interval: how long an append waits for its flush.
    async fn start_with(flush_interval: Duration) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.log.flush_interval = flush_interval;
        config.worker_poll_interval = Duration::from_millis(50);
        config.link.batch_interval = Duration::ZERO;
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        let running = Self {
            server,
            base,
            http: reqwest::Client::new(),
            _dir: dir,
        };
        running
            .connect(CREATE_NAMESPACE, &json!({"namespace": "w"}))
            .await
            .expect_ok();
        running
    }

    async fn connect(&self, rpc: &str, body: &Value) -> Reply {
        let response = self
            .http
            .post(format!("{}{rpc}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("send");
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply { status, body }
    }

    async fn shutdown(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

fn error_info(body: &Value) -> ErrorInfo {
    let bytes = STANDARD_NO_PAD
        .decode(body["details"][0]["value"].as_str().expect("an ErrorInfo"))
        .expect("base64");
    ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
}

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

fn text(value: &Value) -> String {
    String::from_utf8(
        STANDARD
            .decode(value.as_str().expect("bytes"))
            .expect("base64"),
    )
    .expect("utf-8")
}

/// A proto3 JSON `uint64` (a decimal string), absent as 0.
fn u64_of(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::String(s) => s.parse().expect("a decimal"),
        Value::Number(n) => n.as_u64().expect("a u64"),
        other => panic!("not a uint64: {other}"),
    }
}

async fn create(running: &Running, name: &str, partitions: u32) -> Value {
    running
        .connect(
            CREATE,
            &json!({"namespace": "w", "name": name, "partitions": partitions}),
        )
        .await
        .expect_ok()
}

fn records(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| json!({"value": b64(v)})).collect())
}

#[tokio::test]
async fn create_stream_is_idempotent_by_name() {
    let running = Running::start().await;
    let first = create(&running, "events", 2).await;
    assert_eq!(first["created"], json!(true));
    assert_eq!(first["stream"]["partitions"], json!(2));
    let again = create(&running, "events", 2).await;
    assert!(again.get("created").is_none(), "{again}");
    assert_eq!(again["stream"]["id"], first["stream"]["id"]);
    let body = running
        .connect(
            CREATE,
            &json!({"namespace": "w", "name": "events", "partitions": 3}),
        )
        .await
        .expect_error(StatusCode::CONFLICT);
    assert_eq!(error_info(&body).reason, "already_exists");
    let body = running
        .connect(
            CREATE,
            &json!({"namespace": "w", "name": "none", "partitions": 0}),
        )
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error_info(&body).metadata["field"], "partitions");
    running.shutdown().await;
}

#[tokio::test]
async fn describe_and_list_streams_paginate() {
    let running = Running::start().await;
    for name in ["c", "a", "b"] {
        create(&running, name, 1).await;
    }
    running
        .connect(
            PRODUCE,
            &json!({"namespace": "w", "stream": "a", "records": records(&["x", "y"])}),
        )
        .await
        .expect_ok();
    let described = running
        .connect(DESCRIBE, &json!({"namespace": "w", "stream": "a"}))
        .await
        .expect_ok();
    assert_eq!(
        u64_of(&described["stream"]["offsets"][0]["highWatermark"]),
        2
    );

    let mut names = Vec::new();
    let mut token = String::new();
    loop {
        let page = running
            .connect(
                LIST,
                &json!({"namespace": "w", "pageSize": 2, "pageToken": token}),
            )
            .await
            .expect_ok();
        for stream in page["streams"].as_array().expect("streams") {
            names.push(stream["name"].as_str().expect("a name").to_owned());
        }
        match page.get("nextPageToken").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => token = next.to_owned(),
            _ => break,
        }
    }
    assert_eq!(names, ["a", "b", "c"]);
    let body = running
        .connect(DESCRIBE, &json!({"namespace": "w", "stream": "missing"}))
        .await
        .expect_error(StatusCode::NOT_FOUND);
    assert_eq!(error_info(&body).reason, "not_found");
    running.shutdown().await;
}

#[tokio::test]
async fn fetch_paginates_by_offset() {
    let running = Running::start().await;
    create(&running, "events", 1).await;
    let produced = running
        .connect(
            PRODUCE,
            &json!({"namespace": "w", "stream": "events", "records": records(&["a", "b", "c"])}),
        )
        .await
        .expect_ok();
    assert_eq!(u64_of(&produced["baseOffset"]), 0);
    assert_eq!(u64_of(&produced["lastOffset"]), 2);
    assert!(
        produced["consistencyToken"]
            .as_str()
            .expect("a token")
            .starts_with("v1:s"),
        "{produced}"
    );

    let mut seen = Vec::new();
    let mut offset = 0;
    for _ in 0..5 {
        let page = running
            .connect(
                FETCH,
                &json!({"namespace": "w", "stream": "events", "offset": offset, "maxRecords": 2}),
            )
            .await
            .expect_ok();
        let fetched = page["records"].as_array().cloned().unwrap_or_default();
        if fetched.is_empty() {
            break;
        }
        assert!(fetched.len() <= 2);
        for record in &fetched {
            seen.push(text(&record["value"]));
        }
        offset = u64_of(&page["nextOffset"]);
    }
    assert_eq!(seen, ["a", "b", "c"]);
    assert_eq!(offset, 3);
    running.shutdown().await;
}

#[tokio::test]
async fn produce_without_key_is_not_retried_by_server() {
    let running = Running::start().await;
    create(&running, "events", 1).await;
    let body = json!({"namespace": "w", "stream": "events", "records": records(&["once"])});
    let first = running.connect(PRODUCE, &body).await.expect_ok();
    let second = running.connect(PRODUCE, &body).await.expect_ok();
    assert_eq!(u64_of(&first["baseOffset"]), 0);
    assert_eq!(
        u64_of(&second["baseOffset"]),
        1,
        "an unkeyed produce appends every time"
    );
    assert!(second.get("duplicate").is_none());
    running.shutdown().await;
}

#[tokio::test]
async fn produce_with_producer_seq_dedupes() {
    let running = Running::start().await;
    create(&running, "events", 1).await;
    let body = |seq: u64, values: &[&str]| {
        json!({
            "namespace": "w", "stream": "events", "records": records(values),
            "producerId": "app-1", "producerSeq": seq,
        })
    };
    let first = running
        .connect(PRODUCE, &body(1, &["a", "b", "c"]))
        .await
        .expect_ok();
    let repeat = running
        .connect(PRODUCE, &body(1, &["a", "b", "c"]))
        .await
        .expect_ok();
    assert_eq!(repeat["duplicate"], json!(true));
    assert_eq!(repeat["baseOffset"], first["baseOffset"]);
    assert_eq!(repeat["lastOffset"], first["lastOffset"]);
    let next = running.connect(PRODUCE, &body(2, &["d"])).await.expect_ok();
    assert_eq!(u64_of(&next["baseOffset"]), 3);
    let fetched = running
        .connect(
            FETCH,
            &json!({"namespace": "w", "stream": "events", "offset": 0}),
        )
        .await
        .expect_ok();
    assert_eq!(fetched["records"].as_array().map(Vec::len), Some(4));

    let half = json!({"namespace": "w", "stream": "events", "records": records(&["x"]), "producerId": "p"});
    let body = running
        .connect(PRODUCE, &half)
        .await
        .expect_error(StatusCode::BAD_REQUEST);
    assert_eq!(error_info(&body).metadata["field"], "producer_seq");
    running.shutdown().await;
}

#[tokio::test]
async fn fetch_long_poll_returns_on_append() {
    let running = Running::start().await;
    create(&running, "events", 1).await;
    let base = running.base.clone();
    let http = running.http.clone();
    let started = Instant::now();
    let poll = tokio::spawn(async move {
        let response = http
            .post(format!("{base}{FETCH}"))
            .header("content-type", "application/json")
            .body(
                json!({"namespace": "w", "stream": "events", "offset": 0, "maxWaitMs": 10_000})
                    .to_string(),
            )
            .send()
            .await
            .expect("send");
        response.json::<Value>().await.expect("json")
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    running
        .connect(
            PRODUCE,
            &json!({"namespace": "w", "stream": "events", "records": records(&["late"])}),
        )
        .await
        .expect_ok();
    let page = poll.await.expect("poll");
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "the poll waited out its deadline"
    );
    assert_eq!(text(&page["records"][0]["value"]), "late");

    // An empty poll answers at its deadline with nothing.
    let started = Instant::now();
    let empty = running
        .connect(
            FETCH,
            &json!({"namespace": "w", "stream": "events", "offset": 1, "maxWaitMs": 200}),
        )
        .await
        .expect_ok();
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert!(empty.get("records").is_none(), "{empty}");
    running.shutdown().await;
}

#[tokio::test]
async fn cloudevents_produce_and_fetch_rpc() {
    let running = Running::start().await;
    create(&running, "orders", 1).await;
    let batch = json!([
        {"specversion": "1.0", "id": "e1", "source": "/orders", "type": "order.created", "data": {"n": 1}},
        {"specversion": "1.0", "id": "e2", "source": "/orders", "type": "order.created", "data": {"n": 2}},
    ]);
    let body = json!({"namespace": "w", "stream": "orders", "partition": 0, "jsonBatch": b64(&batch.to_string())});
    let first = running.connect(PRODUCE_EVENTS, &body).await.expect_ok();
    let statuses: Vec<&str> = first["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(statuses, ["EVENT_STATUS_APPENDED", "EVENT_STATUS_APPENDED"]);
    let again = running.connect(PRODUCE_EVENTS, &body).await.expect_ok();
    assert_eq!(again["results"][0]["status"], "EVENT_STATUS_DUPLICATE");

    let fetched = running
        .connect(
            FETCH_EVENTS,
            &json!({"namespace": "w", "stream": "orders", "offset": 0}),
        )
        .await
        .expect_ok();
    assert_eq!(fetched["count"], json!(2));
    let events: Value = serde_json::from_str(&text(&fetched["jsonBatch"])).expect("a JSON batch");
    assert_eq!(events[0]["id"], "e1");
    assert_eq!(events[1]["data"], json!({"n": 2}));
    running.shutdown().await;
}

/// A client that goes away mid-request leaves no open claims behind: the
/// ingest finishes on its own task, so re-sending the batch is answered
/// `DUPLICATE` at once and the stream holds each event once. Before, the
/// dropped handler left the appended events' claims open: the retry was
/// answered `IN_FLIGHT` until they lapsed (120 s) and then appended the events
/// again (a `loams-fabric connect` source killed with -9, 2026-10-28).
#[tokio::test]
async fn cloudevents_produce_survives_a_dropped_client() {
    // A long WAL flush holds the append, so the client gives up while the
    // engine is between claiming the ids and completing them.
    let running = Running::start_with(Duration::from_millis(1500)).await;
    create(&running, "orders", 1).await;
    let batch = json!([
        {"specversion": "1.0", "id": "e1", "source": "/orders", "type": "order.created", "data": {"n": 1}},
        {"specversion": "1.0", "id": "e2", "source": "/orders", "type": "order.created", "data": {"n": 2}},
    ]);
    let body = json!({"namespace": "w", "stream": "orders", "partition": 0, "jsonBatch": b64(&batch.to_string())});
    let dropped = running
        .http
        .post(format!("{}{PRODUCE_EVENTS}", running.base))
        .header("content-type", "application/json")
        .body(body.to_string())
        .timeout(Duration::from_millis(300))
        .send()
        .await;
    assert!(
        dropped.as_ref().is_err_and(reqwest::Error::is_timeout),
        "the client should give up before the flush: {dropped:?}"
    );
    // Past the flush, far inside the claims' 120 s.
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let again = running.connect(PRODUCE_EVENTS, &body).await.expect_ok();
    let statuses: Vec<&str> = again["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(
        statuses,
        ["EVENT_STATUS_DUPLICATE", "EVENT_STATUS_DUPLICATE"]
    );
    let fetched = running
        .connect(
            FETCH_EVENTS,
            &json!({"namespace": "w", "stream": "orders", "offset": 0, "maxWaitMs": 0}),
        )
        .await
        .expect_ok();
    assert_eq!(fetched["count"], json!(2));
    running.shutdown().await;
}

/// A tonic-era `ProduceRequest` and `ProduceCloudEventsRequest`, encoded by
/// `loams-stream-grpc`'s prost types, decode on the Connect service, and its
/// answer decodes with the old `ProduceResponse`.
#[cfg(feature = "stream-grpc")]
#[tokio::test]
async fn proto_wire_compatible_with_tonic_era() {
    use loams_stream_grpc::events::cloudevents::{CloudEvent, CloudEventBatch, cloud_event::Data};
    use loams_stream_grpc::proto::{
        ProduceCloudEventsRequest, ProduceCloudEventsResponse, ProduceRequest, ProduceResponse,
        Record, produce_cloud_events_request::Events,
    };
    use prost::Message as _;

    let running = Running::start().await;
    create(&running, "events", 1).await;
    let post = |rpc: &'static str, body: Vec<u8>| {
        let http = running.http.clone();
        let url = format!("{}{rpc}", running.base);
        async move {
            let response = http
                .post(url)
                .header("content-type", "application/proto")
                .body(body)
                .send()
                .await
                .expect("send");
            assert_eq!(response.status(), StatusCode::OK);
            response.bytes().await.expect("bytes")
        }
    };
    let old = ProduceRequest {
        namespace: "w".into(),
        stream: "events".into(),
        partition: 0,
        records: vec![Record {
            value: Some(b"old".to_vec()),
            ..Default::default()
        }],
    };
    let answer = ProduceResponse::decode(post(PRODUCE, old.encode_to_vec()).await).expect("decode");
    assert_eq!(answer.base_offset, 0);
    assert_eq!(answer.last_offset, 0);

    let batch = CloudEventBatch {
        events: vec![CloudEvent {
            id: "w1".into(),
            source: "/old".into(),
            spec_version: "1.0".into(),
            r#type: "t".into(),
            data: Some(Data::TextData("hi".into())),
            ..Default::default()
        }],
    };
    let old = ProduceCloudEventsRequest {
        namespace: "w".into(),
        stream: "events".into(),
        partition: Some(0),
        events: Some(Events::Batch(batch)),
    };
    let answer =
        ProduceCloudEventsResponse::decode(post(PRODUCE_EVENTS, old.encode_to_vec()).await)
            .expect("decode");
    assert_eq!(answer.results.len(), 1);
    running.shutdown().await;
}
