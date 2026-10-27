//! Shared by the `connect_*` tests: a stand-in engine serving the part of
//! `loams.stream.v1` that connect calls (Connect JSON over a Unix socket, as
//! the engine's `--connect-socket` does), with the engine's dedup by
//! `source` + `id`; and helpers to start a role over it.
//!
//! The real engine's side of the same calls is tested in
//! `crates/loams/tests/connect_socket.rs`; `connect_engine.rs` here runs
//! against a real engine when `LOAMS_CONNECT_ENGINE_URL` is set.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use loams_connector_sdk::testing::reference::ReferenceConnector;
use loams_connector_sdk::{ConnectorFactory, Scrubber, Secret};
use loams_fabric::connect::engine::{EngineClient, EngineUrl};
use loams_fabric::connect::instances::{self, Catalog, Factories, Instance};
use loams_fabric::connect::state::StateStore;
use loams_fabric::connect::{Connect, Parts};
use loams_flow::secrets::file::FileSecretStore;
use loams_store::Store;
use serde_json::{Value, json};

pub const KEY: &str = "lk_test_connect_key_0123456789";

#[derive(Debug, Default)]
struct Stream {
    partitions: Vec<Vec<Value>>,
    seen: HashSet<(String, String)>,
}

#[derive(Debug, Default)]
pub struct EngineState {
    streams: BTreeMap<(String, String), Stream>,
    pub appended: u64,
    pub duplicates: u64,
    pub produce_calls: u64,
    pub fetch_calls: u64,
    fail_next: u32,
}

/// The stand-in engine. Dropping it stops serving.
pub struct FakeEngine {
    pub socket: PathBuf,
    pub state: Arc<Mutex<EngineState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeEngine {
    pub async fn start(dir: &Path) -> Self {
        let socket = dir.join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind the socket");
        let state: Arc<Mutex<EngineState>> = Arc::default();
        let st = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((io, _)) = listener.accept().await else {
                    continue;
                };
                let st = st.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req| {
                        let st = st.clone();
                        async move { Ok::<_, Infallible>(handle(req, &st).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(io), svc)
                        .await;
                });
            }
        });
        Self {
            socket,
            state,
            task,
        }
    }

    pub fn url(&self) -> EngineUrl {
        EngineUrl::Unix(self.socket.clone())
    }

    pub fn client(&self) -> EngineClient {
        EngineClient::new(self.url(), Some(Secret::new(KEY)))
    }

    /// The events of a stream, per partition, in log order.
    pub fn events(&self, ns: &str, stream: &str) -> Vec<Vec<Value>> {
        let s = self.state.lock().unwrap();
        s.streams
            .get(&(ns.to_owned(), stream.to_owned()))
            .map(|s| s.partitions.clone())
            .unwrap_or_default()
    }

    pub fn ids(&self, ns: &str, stream: &str) -> Vec<String> {
        self.events(ns, stream)
            .into_iter()
            .flatten()
            .map(|e| e["id"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    pub fn duplicates(&self) -> u64 {
        self.state.lock().unwrap().duplicates
    }

    pub fn fetch_calls(&self) -> u64 {
        self.state.lock().unwrap().fetch_calls
    }

    pub fn fail_next(&self, n: u32) {
        self.state.lock().unwrap().fail_next = n;
    }

    /// Append events straight to a stream (a producer other than connect).
    pub fn push(&self, ns: &str, stream: &str, events: Vec<Value>) {
        let mut s = self.state.lock().unwrap();
        let st = s
            .streams
            .entry((ns.to_owned(), stream.to_owned()))
            .or_insert_with(|| Stream {
                partitions: vec![Vec::new()],
                ..Stream::default()
            });
        st.partitions[0].extend(events);
    }
}

fn reply(status: StatusCode, v: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(v.to_string())))
        .unwrap()
}

fn u64_of(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

async fn handle(req: Request<Incoming>, st: &Mutex<EngineState>) -> Response<Full<Bytes>> {
    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {KEY}"));
    if !authorized {
        return reply(
            StatusCode::UNAUTHORIZED,
            &json!({"code": "unauthenticated", "message": "no API key"}),
        );
    }
    let path = req.uri().path().to_owned();
    let body = req.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let method = path.rsplit('/').next().unwrap_or_default().to_owned();
    let ns = v["namespace"].as_str().unwrap_or_default().to_owned();
    if method == "FetchCloudEvents" {
        // The long poll: an empty partition holds the call a little.
        let empty = {
            let s = st.lock().unwrap();
            let stream = v["stream"].as_str().unwrap_or_default().to_owned();
            let p = v["partition"].as_u64().unwrap_or(0) as usize;
            let off = u64_of(&v["offset"]) as usize;
            s.streams
                .get(&(ns.clone(), stream))
                .and_then(|x| x.partitions.get(p))
                .is_none_or(|x| x.len() <= off)
        };
        let wait = v["maxWaitMs"].as_u64().unwrap_or(0).min(100);
        if empty && wait > 0 {
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
    }
    let mut s = st.lock().unwrap();
    if s.fail_next > 0 {
        s.fail_next -= 1;
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"code": "unavailable", "message": "injected"}),
        );
    }
    match method.as_str() {
        "CreateStream" => {
            let name = v["name"].as_str().unwrap_or_default().to_owned();
            let n = v["partitions"].as_u64().unwrap_or(1).max(1) as usize;
            let created = !s.streams.contains_key(&(ns.clone(), name.clone()));
            let st = s.streams.entry((ns, name)).or_insert_with(|| Stream {
                partitions: vec![Vec::new(); n],
                ..Stream::default()
            });
            reply(
                StatusCode::OK,
                &json!({"stream": {"partitions": st.partitions.len()}, "created": created}),
            )
        }
        "DescribeStream" => {
            let name = v["stream"].as_str().unwrap_or_default().to_owned();
            match s.streams.get(&(ns, name)) {
                Some(st) => reply(
                    StatusCode::OK,
                    &json!({"stream": {"partitions": st.partitions.len()}}),
                ),
                None => reply(
                    StatusCode::NOT_FOUND,
                    &json!({"code": "not_found", "message": "no stream"}),
                ),
            }
        }
        "ProduceCloudEvents" => {
            s.produce_calls += 1;
            let name = v["stream"].as_str().unwrap_or_default().to_owned();
            let raw = STANDARD
                .decode(v["jsonBatch"].as_str().unwrap_or_default())
                .unwrap_or_default();
            let events: Vec<Value> = serde_json::from_slice(&raw).unwrap_or_default();
            let Some(stream) = s.streams.get_mut(&(ns, name)) else {
                return reply(
                    StatusCode::NOT_FOUND,
                    &json!({"code": "not_found", "message": "no stream"}),
                );
            };
            let n = stream.partitions.len();
            let mut results = Vec::new();
            let (mut appended, mut dups) = (0, 0);
            for ev in events {
                let key = (
                    ev["source"].as_str().unwrap_or_default().to_owned(),
                    ev["id"].as_str().unwrap_or_default().to_owned(),
                );
                if !stream.seen.insert(key.clone()) {
                    dups += 1;
                    results.push(json!({"status": "EVENT_STATUS_DUPLICATE"}));
                    continue;
                }
                let p = match v["partition"].as_u64() {
                    Some(p) => p as usize,
                    None => {
                        let subject = ev["subject"].as_str().unwrap_or(&key.1);
                        subject.bytes().map(usize::from).sum::<usize>() % n
                    }
                };
                stream.partitions[p].push(ev);
                appended += 1;
                results.push(json!({"status": "EVENT_STATUS_APPENDED", "partition": p}));
            }
            s.appended += appended;
            s.duplicates += dups;
            reply(StatusCode::OK, &json!({"results": results}))
        }
        "FetchCloudEvents" => {
            s.fetch_calls += 1;
            let name = v["stream"].as_str().unwrap_or_default().to_owned();
            let p = v["partition"].as_u64().unwrap_or(0) as usize;
            let off = u64_of(&v["offset"]) as usize;
            let Some(stream) = s.streams.get(&(ns, name)) else {
                return reply(
                    StatusCode::NOT_FOUND,
                    &json!({"code": "not_found", "message": "no stream"}),
                );
            };
            let log = &stream.partitions[p];
            let out: Vec<Value> = log.iter().skip(off).take(500).cloned().collect();
            let next = off + out.len();
            reply(
                StatusCode::OK,
                &json!({
                    "jsonBatch": STANDARD.encode(Value::Array(out.clone()).to_string()),
                    "count": out.len(),
                    "nextOffset": next.to_string(),
                    "highWatermark": log.len().to_string(),
                }),
            )
        }
        other => reply(
            StatusCode::NOT_FOUND,
            &json!({"code": "unimplemented", "message": other}),
        ),
    }
}

/// The repository's `connectors/` directory.
pub fn connectors_dir() -> PathBuf {
    loams_flow::manifest::repository_root().join("connectors")
}

/// The catalog, with the SDK's reference connector allowed.
pub fn catalog() -> Catalog {
    Catalog::load(&connectors_dir())
        .expect("the registry loads")
        .allow_unregistered("reference")
}

/// Factories with the reference connector (its handles shared with `rc`).
pub fn factories(rc: &ReferenceConnector) -> Factories {
    let mut f = Factories::new();
    let rc = rc.clone();
    f.register("reference", None, move |_, _| {
        Ok(Arc::new(rc.clone()) as Arc<dyn ConnectorFactory>)
    });
    f
}

pub fn instance(yaml: &str, catalog: &Catalog, factories: &Factories) -> Instance {
    instances::parse("test.yaml", yaml, catalog, factories).unwrap_or_else(|e| panic!("{e}"))
}

/// A role over the stand-in engine and `store`.
pub async fn start(
    engine: &FakeEngine,
    store: &Store,
    secrets_root: &Path,
    scrubber: Arc<Scrubber>,
    instances: Vec<Instance>,
) -> Connect {
    Connect::start_with(Parts {
        instances,
        engine: engine.client(),
        state: StateStore::new(store.clone(), "_meta/connect/"),
        secrets: Arc::new(FileSecretStore::new(secrets_root)),
        scrubber,
        admin_listen: "127.0.0.1:0".parse().unwrap(),
        admin_token: Some(Secret::new("admin-token-for-tests")),
        fence_every: Duration::from_millis(200),
    })
    .await
    .unwrap_or_else(|e| panic!("start: {}", e.message))
}

/// Poll `cond` every 20 ms for up to `secs`.
pub async fn eventually(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !cond() {
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One admin API call: `(status, body)`.
pub async fn admin(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let io = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .unwrap();
    tokio::spawn(conn);
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "connect");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = req
        .body(Full::new(Bytes::from(
            body.map(|b| b.to_string()).unwrap_or_default(),
        )))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
