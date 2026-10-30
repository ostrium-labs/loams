//! The transfer and run HTTP API (§85 §6, §11, D2422): an axum router the
//! engine mounts under its main router, and `loams-transfer serve` serves
//! alone. The router is the same over HTTP/3 and HTTP/2 (D2437).
//!
//! Errors are JSON `{code, message, ...}` with the stable codes of
//! [`Error::code`](crate::Error::code).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use futures::StreamExt;
use loams_common::NamespaceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::Error;
use crate::run::{MAX_LOG_APPEND, RunOutcome, Runs};
use crate::transfer::{CreateTransfer, Transfers};

/// The chunk hash header (§85 §6).
pub const H_CHUNK_SHA: &str = "loams-chunk-sha256";
/// The lease epoch header (§85 §6, D2430).
pub const H_EPOCH: &str = "loams-lease-epoch";
/// The committed log length when a log read started.
pub const H_LOG_COMMITTED: &str = "loams-log-committed";
/// The run state when a log read started.
pub const H_RUN_STATE: &str = "loams-run-state";

/// What a route needs on the namespace (D2443).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
    Runner,
}

/// Why a request was not authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// No or an unknown credential.
    Unauthenticated,
    /// The credential lacks the access.
    Forbidden,
    /// The namespace does not exist (or the caller may not know it does).
    NoNamespace,
}

/// Resolves a namespace name and checks the caller's access to it. It sees
/// the request's headers and extensions, so an embedding server can use a
/// principal its own authentication layer put there (the engine does).
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn authorize(
        &self,
        headers: &HeaderMap,
        extensions: &http::Extensions,
        ns: &str,
        need: Access,
    ) -> Result<NamespaceId, Denied>;
}

/// Maps namespace names to ids.
#[async_trait]
pub trait Namespaces: Send + Sync + 'static {
    async fn resolve(&self, name: &str) -> Option<NamespaceId>;
}

/// Every name is a namespace, with ids handed out in order. For tests and
/// the in-memory dev server.
#[derive(Debug, Default)]
pub struct MemNamespaces(Mutex<HashMap<String, NamespaceId>>);

#[async_trait]
impl Namespaces for MemNamespaces {
    async fn resolve(&self, name: &str) -> Option<NamespaceId> {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let next = NamespaceId(m.len() as u64 + 1);
        Some(*m.entry(name.to_string()).or_insert(next))
    }
}

/// Bearer tokens with fixed access, for `loams-transfer serve` and tests.
/// The engine has its own [`Authorizer`] on its principals.
pub struct StaticTokens {
    /// SHA-256 of each token → its access.
    tokens: Vec<([u8; 32], Vec<Access>)>,
    namespaces: Arc<dyn Namespaces>,
}

impl std::fmt::Debug for StaticTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticTokens")
            .field("tokens", &self.tokens.len())
            .finish_non_exhaustive()
    }
}

impl StaticTokens {
    pub fn new(namespaces: Arc<dyn Namespaces>) -> Self {
        StaticTokens {
            tokens: Vec::new(),
            namespaces,
        }
    }

    /// Adds a token with its access.
    pub fn with(mut self, token: &str, access: &[Access]) -> Self {
        self.tokens
            .push((Sha256::digest(token.as_bytes()).into(), access.to_vec()));
        self
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

#[async_trait]
impl Authorizer for StaticTokens {
    async fn authorize(
        &self,
        headers: &HeaderMap,
        _extensions: &http::Extensions,
        ns: &str,
        need: Access,
    ) -> Result<NamespaceId, Denied> {
        let token = bearer(headers).ok_or(Denied::Unauthenticated)?;
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        // Compare every entry, in constant time per entry.
        let mut found: Option<&Vec<Access>> = None;
        for (d, access) in &self.tokens {
            let diff = d
                .iter()
                .zip(digest.iter())
                .fold(0u8, |a, (x, y)| a | (x ^ y));
            if diff == 0 {
                found = Some(access);
            }
        }
        let access = found.ok_or(Denied::Unauthenticated)?;
        if !access.contains(&need) {
            return Err(Denied::Forbidden);
        }
        self.namespaces.resolve(ns).await.ok_or(Denied::NoNamespace)
    }
}

/// The router's state.
#[derive(Clone)]
pub struct Api {
    pub transfers: Arc<Transfers>,
    pub runs: Arc<Runs>,
    pub auth: Arc<dyn Authorizer>,
    /// How often a followed log read polls for more.
    pub tail_poll: Duration,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api").finish_non_exhaustive()
    }
}

/// The JSON error body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<u64>,
}

/// An error response.
#[derive(Debug)]
pub struct ApiError(StatusCode, ErrorBody);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let status = match &e {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::LeaseHeld { .. }
            | Error::LeaseLost
            | Error::Conflict { .. }
            | Error::Offset { .. }
            | Error::State(_) => StatusCode::CONFLICT,
            Error::Integrity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Store(_) | Error::Meta(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let mut body = ErrorBody {
            code: e.code().to_string(),
            message: e.to_string(),
            ..ErrorBody::default()
        };
        match &e {
            Error::LeaseHeld {
                holder,
                deadline_ms,
            } => {
                body.holder = Some(holder.clone());
                body.deadline_ms = Some(*deadline_ms);
            }
            Error::Offset { committed } => body.committed = Some(*committed),
            Error::Store(_) | Error::Meta(_) | Error::Corrupt(_) => {
                tracing::warn!(error = %e, "transfer API error");
            }
            _ => {}
        }
        ApiError(status, body)
    }
}

impl From<Denied> for ApiError {
    fn from(d: Denied) -> Self {
        let (status, code, message) = match d {
            Denied::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "a valid bearer token is required",
            ),
            Denied::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "the credential lacks access to this namespace",
            ),
            Denied::NoNamespace => (StatusCode::NOT_FOUND, "not_found", "namespace not found"),
        };
        ApiError(
            status,
            ErrorBody {
                code: code.into(),
                message: message.into(),
                ..ErrorBody::default()
            },
        )
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(msg: &str) -> ApiError {
    Error::BadRequest(msg.to_string()).into()
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> ApiResult<&'a str> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad(&format!("missing header {name}")))
}

fn epoch_header(h: &HeaderMap) -> ApiResult<u64> {
    header(h, H_EPOCH)?
        .parse()
        .map_err(|_| bad("the lease epoch must be a number"))
}

/// The caller as the authorizer sees it: headers and extensions.
#[derive(Debug)]
pub struct Caller {
    headers: HeaderMap,
    extensions: http::Extensions,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Caller {
            headers: parts.headers.clone(),
            extensions: parts.extensions.clone(),
        })
    }
}

impl Api {
    async fn ns(&self, c: &Caller, ns: &str, need: Access) -> ApiResult<NamespaceId> {
        Ok(self
            .auth
            .authorize(&c.headers, &c.extensions, ns, need)
            .await?)
    }
}

/// The routes, with paths relative to the server root.
pub fn router(api: Api) -> Router {
    let t = "/v1/namespaces/{ns}/transfers";
    let r = "/v1/namespaces/{ns}/runs";
    Router::new()
        .route(t, get(list_transfers))
        .route(
            &format!("{t}/{{tid}}"),
            put(create_transfer).get(view_transfer),
        )
        .route(&format!("{t}/{{tid}}/lease"), post(lease_transfer))
        .route(
            &format!("{t}/{{tid}}/chunks/{{i}}"),
            put(put_chunk).get(get_chunk),
        )
        .route(&format!("{t}/{{tid}}/complete"), post(complete_transfer))
        .route(&format!("{t}/{{tid}}/abandon"), post(abandon_transfer))
        .route(r, get(list_runs))
        .route("/v1/namespaces/{ns}/runner/claim", post(claim_run))
        .route(&format!("{r}/{{rid}}"), put(create_run).get(view_run))
        .route(&format!("{r}/{{rid}}/cancel"), post(cancel_run))
        .route(&format!("{r}/{{rid}}/renew"), post(renew_run))
        .route(&format!("{r}/{{rid}}/log"), put(append_log).get(read_log))
        .route(&format!("{r}/{{rid}}/artifacts"), post(add_artifact))
        .route(&format!("{r}/{{rid}}/finish"), post(finish_run))
        .layer(DefaultBodyLimit::max(MAX_LOG_APPEND + (64 << 10)))
        .with_state(api)
}

// ---------------------------------------------------------------- transfers

async fn list_transfers(
    State(api): State<Api>,
    Path(ns): Path<String>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    Ok(Json(api.transfers.list(id).await?).into_response())
}

async fn create_transfer(
    State(api): State<Api>,
    Path((ns, tid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<CreateTransfer>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    Ok(Json(api.transfers.create(id, &tid, req).await?).into_response())
}

#[derive(Debug, Deserialize)]
struct ViewQuery {
    #[serde(default)]
    verify: Option<String>,
}

async fn view_transfer(
    State(api): State<Api>,
    Path((ns, tid)): Path<(String, String)>,
    Query(q): Query<ViewQuery>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    if q.verify.as_deref() == Some("full") {
        api.transfers.verify_full(id, &tid).await?;
    }
    Ok(Json(api.transfers.view(id, &tid).await?).into_response())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub holder: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LeaseReply {
    pub epoch: u64,
    pub deadline_ms: u64,
}

async fn lease_transfer(
    State(api): State<Api>,
    Path((ns, tid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<LeaseRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    let g = api.transfers.lease(id, &tid, &req.holder).await?;
    Ok(Json(LeaseReply {
        epoch: g.epoch,
        deadline_ms: g.deadline_ms,
    })
    .into_response())
}

async fn put_chunk(
    State(api): State<Api>,
    Path((ns, tid, i)): Path<(String, String, u32)>,
    c: Caller,
    body: Body,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    let sha = header(&c.headers, H_CHUNK_SHA)?.to_string();
    let epoch = epoch_header(&c.headers)?;
    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(io::Error::other))
        .boxed();
    let out = api
        .transfers
        .put_chunk(id, &tid, i, &sha, epoch, stream)
        .await?;
    Ok(Json(out).into_response())
}

async fn get_chunk(
    State(api): State<Api>,
    Path((ns, tid, i)): Path<(String, String, u32)>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    let (sha, len, stream) = api.transfers.open_chunk(id, &tid, i).await?;
    let mut resp = Body::from_stream(stream.map(|r| r.map_err(io::Error::other))).into_response();
    let hs = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&sha) {
        hs.insert(H_CHUNK_SHA, v);
    }
    hs.insert(http::header::CONTENT_LENGTH, HeaderValue::from(len));
    hs.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok(resp)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub file_sha256: String,
    pub epoch: u64,
}

async fn complete_transfer(
    State(api): State<Api>,
    Path((ns, tid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<CompleteRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    let v = api
        .transfers
        .complete(id, &tid, req.epoch, &req.file_sha256)
        .await?;
    Ok(Json(v).into_response())
}

async fn abandon_transfer(
    State(api): State<Api>,
    Path((ns, tid)): Path<(String, String)>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    Ok(Json(api.transfers.abandon(id, &tid).await?).into_response())
}

// ---------------------------------------------------------------- runs

#[derive(Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub rid: String,
    pub state: String,
    pub job: String,
}

async fn list_runs(
    State(api): State<Api>,
    Path(ns): Path<String>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    let out: Vec<RunSummary> = api
        .runs
        .list(id)
        .await?
        .into_iter()
        .map(|(rid, state, job)| RunSummary { rid, state, job })
        .collect();
    Ok(Json(out).into_response())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRun {
    pub job: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

async fn create_run(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<CreateRun>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    Ok(Json(api.runs.create(id, &rid, &req.job, req.params).await?).into_response())
}

async fn view_run(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    Ok(Json(api.runs.view(id, &rid).await?).into_response())
}

async fn cancel_run(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Write).await?;
    Ok(Json(api.runs.cancel(id, &rid).await?).into_response())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub runner: String,
    pub jobs: Vec<String>,
}

async fn claim_run(
    State(api): State<Api>,
    Path(ns): Path<String>,
    c: Caller,
    Json(req): Json<ClaimRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Runner).await?;
    Ok(Json(api.runs.claim(id, &req.runner, &req.jobs).await?).into_response())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenewRequest {
    pub runner: String,
    pub epoch: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenewReply {
    pub cancel_requested: bool,
}

async fn renew_run(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<RenewRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Runner).await?;
    let cancel = api.runs.renew(id, &rid, &req.runner, req.epoch).await?;
    Ok(Json(RenewReply {
        cancel_requested: cancel,
    })
    .into_response())
}

#[derive(Debug, Deserialize)]
struct AppendQuery {
    offset: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppendReply {
    pub committed: u64,
}

async fn append_log(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    Query(q): Query<AppendQuery>,
    c: Caller,
    data: Bytes,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Runner).await?;
    let epoch = epoch_header(&c.headers)?;
    let committed = api.runs.append_log(id, &rid, epoch, q.offset, data).await?;
    Ok(Json(AppendReply { committed }).into_response())
}

#[derive(Debug, Deserialize)]
struct ReadQuery {
    #[serde(default)]
    from: u64,
    #[serde(default)]
    follow: bool,
}

async fn read_log(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    Query(q): Query<ReadQuery>,
    c: Caller,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Read).await?;
    let first = api.runs.read_log(id, &rid, q.from).await?;
    let (committed, state) = (first.committed, first.state.clone());
    let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(8);
    let runs = Arc::clone(&api.runs);
    let poll = api.tail_poll;
    let follow = q.follow;
    // The read streams from a task, so a follower's connection carries the
    // log as it grows until the run ends; a closed connection ends the task.
    tokio::spawn(async move {
        let mut read = first;
        loop {
            let (end, done) = (read.committed, read.done);
            while let Some(b) = read.bytes.next().await {
                let item = b.map_err(io::Error::other);
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    return;
                }
            }
            if done || !follow {
                return;
            }
            loop {
                tokio::time::sleep(poll).await;
                if tx.is_closed() {
                    return;
                }
                match runs.read_log(id, &rid, end).await {
                    Ok(r) if r.committed > end || r.done => {
                        read = r;
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = tx.send(Err(io::Error::other(e.to_string()))).await;
                        return;
                    }
                }
            }
        }
    });
    let mut resp = Body::from_stream(ReceiverStream::new(rx)).into_response();
    let hs = resp.headers_mut();
    hs.insert(H_LOG_COMMITTED, HeaderValue::from(committed));
    if let Ok(v) = HeaderValue::from_str(&state) {
        hs.insert(H_RUN_STATE, v);
    }
    hs.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok(resp)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactRequest {
    pub name: String,
    pub tid: String,
    pub size: u64,
    pub epoch: u64,
}

async fn add_artifact(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<ArtifactRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Runner).await?;
    api.runs
        .add_artifact(id, &rid, req.epoch, &req.name, &req.tid, req.size)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FinishRequest {
    pub outcome: RunOutcome,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub message: Option<String>,
    pub epoch: u64,
}

async fn finish_run(
    State(api): State<Api>,
    Path((ns, rid)): Path<(String, String)>,
    c: Caller,
    Json(req): Json<FinishRequest>,
) -> ApiResult<Response> {
    let id = api.ns(&c, &ns, Access::Runner).await?;
    let v = api
        .runs
        .finish(id, &rid, req.epoch, req.outcome, req.exit_code, req.message)
        .await?;
    Ok(Json(v).into_response())
}
