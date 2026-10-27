//! Mock Walrus aggregators and publishers on loopback (no network).

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, put};
use loams_walrus::{BlobId, WalrusConfig};
use url::Url;

/// How a mock aggregator answers.
#[derive(Clone, Debug)]
pub struct Behaviour {
    pub status: StatusCode,
    /// The body of any path not in `paths`.
    pub body: Bytes,
    /// Bodies by exact request path.
    pub paths: HashMap<String, Bytes>,
    pub delay: Duration,
    /// Answer 404 to this many requests first.
    pub not_found_first: usize,
}

impl Behaviour {
    pub fn serve(body: impl Into<Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            body: body.into(),
            paths: HashMap::new(),
            delay: Duration::ZERO,
            not_found_first: 0,
        }
    }

    pub fn status(status: StatusCode) -> Self {
        Self {
            status,
            ..Self::serve(&b"nope"[..])
        }
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn path(mut self, path: impl Into<String>, body: impl Into<Bytes>) -> Self {
        self.paths.insert(path.into(), body.into());
        self
    }

    pub fn not_found_first(mut self, n: usize) -> Self {
        self.not_found_first = n;
        self
    }
}

#[derive(Debug)]
struct AggState {
    behaviour: Behaviour,
    hits: AtomicUsize,
    completed: AtomicUsize,
    paths: Mutex<Vec<String>>,
}

/// A running mock aggregator.
#[derive(Clone, Debug)]
pub struct MockAggregator {
    pub url: Url,
    state: Arc<AggState>,
}

impl MockAggregator {
    pub async fn start(behaviour: Behaviour) -> Self {
        let state = Arc::new(AggState {
            behaviour,
            hits: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            paths: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .fallback(any(aggregate))
            .with_state(state.clone());
        let url = serve(app).await;
        Self { url, state }
    }

    /// Requests received.
    pub fn hits(&self) -> usize {
        self.state.hits.load(Ordering::SeqCst)
    }

    /// Requests answered (a cancelled hedge never finishes its delay).
    pub fn completed(&self) -> usize {
        self.state.completed.load(Ordering::SeqCst)
    }

    /// The paths requested, in order.
    pub fn paths(&self) -> Vec<String> {
        self.state.paths.lock().unwrap().clone()
    }
}

async fn aggregate(State(state): State<Arc<AggState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    let n = state.hits.fetch_add(1, Ordering::SeqCst);
    state.paths.lock().unwrap().push(path.clone());
    tokio::time::sleep(state.behaviour.delay).await;
    state.completed.fetch_add(1, Ordering::SeqCst);
    let b = &state.behaviour;
    if n < b.not_found_first {
        return (StatusCode::NOT_FOUND, "not yet").into_response();
    }
    if !b.status.is_success() {
        return (b.status, b.body.clone()).into_response();
    }
    let body = b.paths.get(&path).unwrap_or(&b.body).clone();
    ranged(&headers, body)
}

fn ranged(headers: &HeaderMap, body: Bytes) -> Response {
    if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok())
        && let Some(spec) = range.strip_prefix("bytes=")
        && let Some((a, z)) = spec.split_once('-')
    {
        let len = body.len();
        let a: usize = a.parse().unwrap();
        let z: usize = z.parse::<usize>().unwrap().min(len.saturating_sub(1));
        if a >= len {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        return (
            StatusCode::PARTIAL_CONTENT,
            [(header::CONTENT_RANGE, format!("bytes {a}-{z}/{len}"))],
            body.slice(a..=z),
        )
            .into_response();
    }
    (StatusCode::OK, body).into_response()
}

/// A request a mock publisher received.
#[derive(Clone, Debug)]
pub struct PutSeen {
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub auth: Option<String>,
    pub body: Bytes,
}

#[derive(Debug)]
struct PubState {
    status: StatusCode,
    response: String,
    seen: Mutex<Vec<PutSeen>>,
}

/// A running mock publisher answering every store with `response`.
#[derive(Clone, Debug)]
pub struct MockPublisher {
    pub url: Url,
    state: Arc<PubState>,
}

impl MockPublisher {
    pub async fn start(status: StatusCode, response: impl Into<String>) -> Self {
        let state = Arc::new(PubState {
            status,
            response: response.into(),
            seen: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/v1/blobs", put(store))
            .route("/v1/quilts", put(store))
            .with_state(state.clone());
        let url = serve(app).await;
        Self { url, state }
    }

    pub fn seen(&self) -> Vec<PutSeen> {
        self.state.seen.lock().unwrap().clone()
    }
}

async fn store(
    State(state): State<Arc<PubState>>,
    Query(query): Query<BTreeMap<String, String>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header_of = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    state.seen.lock().unwrap().push(PutSeen {
        path: uri.path().to_string(),
        query,
        content_type: header_of(header::CONTENT_TYPE),
        auth: header_of(header::AUTHORIZATION),
        body,
    });
    (
        state.status,
        [(header::CONTENT_TYPE, "application/json")],
        state.response.clone(),
    )
        .into_response()
}

async fn serve(app: Router) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Url::parse(&format!("http://{addr}/")).unwrap()
}

/// An enabled config over `aggregators`, no cache, no publisher of its own
/// (on Mainnet, so no public default either), a short hedge.
pub fn config(aggregators: &[&MockAggregator]) -> WalrusConfig {
    WalrusConfig {
        enabled: true,
        network: loams_walrus::Network::Mainnet,
        aggregators: aggregators.iter().map(|a| a.url.clone()).collect(),
        hedge_after_ms: 50,
        request_timeout_ms: 10_000,
        ..WalrusConfig::default()
    }
}

pub fn blob_id(n: u8) -> BlobId {
    BlobId::from_bytes([n; 32])
}
