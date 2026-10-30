//! The upcall listener: the endpoints Neon's components call on
//! `pg-control` (design §46 §4.3, §28 §5.2; PG2 Task 8).
//!
//! | Caller | Route | Auth | Answer |
//! |---|---|---|---|
//! | `compute_ctl` | `GET /compute/api/v2/computes/{id}/spec` | the compute's upcall JWT ([`ComputeKeys`]) | `{status: "empty"}` for an unbound compute; `{status: "attached", spec, compute_ctl_config}` for a bound one |
//! | storage controller | `PUT /notify-attach` | a static bearer token ([`UpcallConfig::notify_token`]) | 200 when recorded and every running compute took the new spec; 423 when recorded but not applied; 429 to slow it down |
//! | pageserver | `POST /upcall/v1/timeline_import_status` | its `control_plane_api_token` ([`UpcallConfig::pageserver_token`]) | 200 when the import's status is recorded (NF2 Task 13) |
//! | pageserver | `GET /upcall/v1/timeline_import_status` | likewise | the recorded `ShardImportStatus`, or `{"InProgress": null}` for an import not reported yet |
//! | pageserver | `POST /upcall/v1/validate` | likewise | every tenant shard's generation valid (NF2 Task 13: one pageserver per shard, attached at its last known generation) |
//!
//! `notify-safekeepers` is not served: the storage controller manages no
//! safekeepers (§46 §9.1). Nor is `notify-attach` when no token is
//! configured (single-node mode runs no storage controller).
//!
//! **Its own listener.** [`UpcallServer`] is the only way to serve these
//! routes (the router is private), and [`UpcallServer::bind`] refuses the
//! API listener's address ([`UpcallError::SharesApiListener`]). It is
//! internal: computes and the storage controller reach it, tenants never
//! (§46 §15's NetworkPolicy).
//!
//! **Bounds.** Connections are capped ([`UpcallLimits::max_connections`];
//! one over is closed at once) and must send their headers within
//! [`UpcallLimits::header_read_timeout`]. A body is at most
//! [`UpcallLimits::max_body_bytes`], read only after the caller is
//! authenticated. Per peer IP: spec requests take from one bucket
//! ([`UpcallLimits::spec_rate`]; empty is 503, which `compute_ctl` retries)
//! and refused credentials from another ([`UpcallLimits::auth_failure_rate`];
//! empty is 429). At most [`UpcallLimits::max_notify_in_flight`]
//! notifications run at once (the controller's own concurrency); one more
//! is 429, which the controller retries after 5 s.
//!
//! **Secrets.** No token, header or spec is logged or put in an answer's
//! error; errors are short fixed texts.
//!
//! The router cannot be mounted elsewhere:
//!
//! ```compile_fail
//! let _ = loams_pg_control::upcall::router;
//! ```

mod keys;
mod limit;
pub(crate) mod spec;
mod tenants;

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures::StreamExt;
use hyper_util::rt::{TokioIo, TokioTimer};
use loams_postgres::compute_ctl::{
    ComputeCtlClient, ComputeCtlConfig, ControlPlaneComputeStatus, ControlPlaneConfigResponse,
};
use loams_postgres::spec::ComputeSpec;
use loams_postgres::storcon::NotifyAttachRequest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower::Service;

pub use keys::{
    COMPUTE_TOKEN_TTL, CTL_TOKEN_TTL, ComputeKeys, KeyError, SIGNING_KEY_REF, SigningKeyError,
    TokenError, UPCALL_AUDIENCE, UpcallClaims, load_or_create_signing_key,
};
pub use limit::Rate;
pub use spec::Sources as SpecSources;

/// What specs are built from, beyond the records (Task 16's endpoint
/// storage).
pub mod spec_sources {
    pub use super::spec::{EndpointStorage, LFC_OFFLOAD_INTERVAL_S};
}

use crate::ids::ComputeId;
use crate::model::{
    BranchPrefix, BranchRec, ComputeKey, ComputeRec, ComputeStatus, EndpointPrefix, EndpointRec,
    ImportState, ImportStatusKey, ImportStatusRec, ProjectKey, ProjectRec, ShardAttachment,
};
use crate::secrets::Secret;
use crate::service::Clock;
use crate::store::{Batch, BatchError, KvControlStore, PgControlStore, StoreError};
use limit::PeerLimiter;
use spec::{Sources, SpecError};
use tenants::{Lookup, Tenants};

/// The upcall listener's address in single-node mode
/// (`--pg-upcall-listen`).
pub const DEFAULT_UPCALL_LISTEN: &str = "127.0.0.1:7690";

/// How many times recording a shard map is redone after a concurrent write.
const ATTEMPTS: usize = 5;

/// The most shards a tenant has (`ShardCount` is a `u8`).
const MAX_SHARDS: usize = 255;

/// The listener's bounds. See the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct UpcallLimits {
    pub max_connections: usize,
    pub header_read_timeout: Duration,
    /// How long a spec request, or a notification's store work, may take.
    pub request_timeout: Duration,
    pub max_body_bytes: usize,
    /// The longest `Authorization` header read.
    pub max_token_bytes: usize,
    pub spec_rate: Rate,
    pub auth_failure_rate: Rate,
    /// How many peers the buckets remember.
    pub max_peers: usize,
    pub max_notify_in_flight: usize,
    /// How long a notification waits for its computes to take the new spec
    /// before answering 423. Below the controller's 10 s request timeout.
    pub apply_timeout: Duration,
    /// How many computes one notification reconfigures at once.
    pub max_configure_in_flight: usize,
}

impl Default for UpcallLimits {
    fn default() -> Self {
        UpcallLimits {
            max_connections: 256,
            header_read_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(5),
            max_body_bytes: 64 * 1024,
            max_token_bytes: 4096,
            spec_rate: Rate {
                per_second: 5.0,
                burst: 20,
            },
            auth_failure_rate: Rate {
                per_second: 1.0,
                burst: 10,
            },
            max_peers: 4096,
            max_notify_in_flight: 32,
            apply_timeout: Duration::from_secs(8),
            max_configure_in_flight: 16,
        }
    }
}

/// How the listener runs.
#[derive(Clone)]
pub struct UpcallConfig {
    /// `--pg-upcall-listen`.
    pub listen: SocketAddr,
    /// The API listener's address, which this one must not share.
    pub api_listen: Option<SocketAddr>,
    /// The storage controller's bearer token (its `--control-plane-jwt-token`).
    /// `None`: `notify-attach` is not served.
    pub notify_token: Option<Secret<String>>,
    /// The pageservers' bearer token (their `control_plane_api_token`), for
    /// the import status upcall (NF2 Task 13): a pageserver's
    /// `control_plane_api` points at `http://<this listener>/upcall/v1/`.
    /// `None`: the import status routes are not served.
    pub pageserver_token: Option<Secret<String>>,
    /// Pageserver node id → libpq URL, for the spec.
    pub pageservers: BTreeMap<u64, String>,
    /// The node of a branch no `notify-attach` named (single-node mode).
    pub default_pageserver: Option<u64>,
    /// `loams-wal` pool name → its connection strings.
    pub wal_pools: BTreeMap<String, Vec<String>>,
    /// Endpoint storage, for the computes' file cache state (Task 16).
    pub endpoint_storage: Option<spec::EndpointStorage>,
    /// Roles' password verifiers for the specs (Task 24); `None`: roles
    /// get no password.
    pub verifiers: Option<Arc<crate::scram::RoleVerifiers>>,
    pub limits: UpcallLimits,
    pub clock: Clock,
}

impl UpcallConfig {
    /// A listener on `listen` with no storage and the default limits.
    pub fn defaults(listen: SocketAddr) -> Self {
        UpcallConfig {
            listen,
            api_listen: None,
            notify_token: None,
            pageserver_token: None,
            pageservers: BTreeMap::new(),
            default_pageserver: None,
            wal_pools: BTreeMap::new(),
            endpoint_storage: None,
            verifiers: None,
            limits: UpcallLimits::default(),
            clock: Clock::system(),
        }
    }
}

impl fmt::Debug for UpcallConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpcallConfig")
            .field("listen", &self.listen)
            .field("api_listen", &self.api_listen)
            .field("notify_token", &self.notify_token)
            .field("pageserver_token", &self.pageserver_token)
            .field("pageservers", &self.pageservers)
            .field("default_pageserver", &self.default_pageserver)
            .field("wal_pools", &self.wal_pools)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Why the listener did not start or stopped.
#[derive(Debug, thiserror::Error)]
pub enum UpcallError {
    #[error("the upcall listener {listen} would share the API listener {api}")]
    SharesApiListener { listen: SocketAddr, api: SocketAddr },
    #[error("binding {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("watching the projects: {0}")]
    Store(#[from] StoreError),
}

/// A compute that did not take a new spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigureError(pub String);

/// Hands a running compute a new spec: `compute_ctl`'s `/configure`
/// ([`ComputeCtlConfigurer`]; tests fake it).
#[async_trait]
pub trait ComputeConfigurer: Send + Sync + 'static {
    async fn configure(
        &self,
        compute: &ComputeRec,
        spec: &ComputeSpec,
        config: &ComputeCtlConfig,
    ) -> Result<(), ConfigureError>;
}

/// [`ComputeConfigurer`] over `compute_ctl`'s HTTP API at the record's
/// `ctl_url`, with a [`CTL_TOKEN_TTL`] token per call.
#[derive(Debug)]
pub struct ComputeCtlConfigurer {
    keys: Arc<ComputeKeys>,
    clock: Clock,
}

impl ComputeCtlConfigurer {
    pub fn new(keys: Arc<ComputeKeys>, clock: Clock) -> Self {
        ComputeCtlConfigurer { keys, clock }
    }
}

#[async_trait]
impl ComputeConfigurer for ComputeCtlConfigurer {
    async fn configure(
        &self,
        compute: &ComputeRec,
        spec: &ComputeSpec,
        config: &ComputeCtlConfig,
    ) -> Result<(), ConfigureError> {
        let fail = |what: &str| ConfigureError(what.to_owned());
        let url = compute
            .ctl_url
            .as_deref()
            .ok_or_else(|| fail("the compute has no compute_ctl address"))?;
        let url = url::Url::parse(url).map_err(|_| fail("the compute_ctl address is not a URL"))?;
        let id: ComputeId = compute
            .id
            .parse()
            .map_err(|_| fail("the compute id is malformed"))?;
        let token = self
            .keys
            .issue_compute_ctl_token(&id, self.clock.now_ms(), CTL_TOKEN_TTL)
            .map_err(|e| ConfigureError(e.to_string()))?;
        let client =
            ComputeCtlClient::new(url, token).map_err(|e| ConfigureError(e.to_string()))?;
        client
            .configure(spec, config)
            .await
            .map(|_| ())
            .map_err(|e| ConfigureError(e.to_string()))
    }
}

/// The peer's IP, which the connection loop puts on every request.
#[derive(Debug, Clone, Copy)]
struct Peer(IpAddr);

struct Shared {
    store: KvControlStore,
    keys: Arc<ComputeKeys>,
    configurer: Arc<dyn ComputeConfigurer>,
    sources: Sources,
    notify_digest: Option<[u8; 32]>,
    pageserver_digest: Option<[u8; 32]>,
    tenants: Option<Arc<Tenants>>,
    limits: UpcallLimits,
    clock: Clock,
    spec_peers: PeerLimiter,
    failure_peers: PeerLimiter,
    notify_slots: Semaphore,
}

/// The upcall listener, bound. [`serve`](Self::serve) runs it.
pub struct UpcallServer {
    listener: TcpListener,
    router: Router,
    max_connections: usize,
    header_read_timeout: Duration,
    tenants_task: Option<tokio::task::JoinHandle<()>>,
}

impl fmt::Debug for UpcallServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpcallServer")
            .field("listen", &self.listener.local_addr().ok())
            .finish_non_exhaustive()
    }
}

/// Whether two listen addresses can take the same connections: the same
/// port (not 0) and the same IP, or either IP unspecified.
fn overlaps(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() != 0
        && a.port() == b.port()
        && (a.ip() == b.ip() || a.ip().is_unspecified() || b.ip().is_unspecified())
}

impl UpcallServer {
    /// Binds `config.listen`. With a `notify_token`, also starts the tenant
    /// directory (a watch on the projects).
    ///
    /// # Errors
    ///
    /// [`UpcallError::SharesApiListener`] when `listen` overlaps
    /// `api_listen`; [`UpcallError::Bind`].
    pub async fn bind(
        store: KvControlStore,
        keys: Arc<ComputeKeys>,
        configurer: Arc<dyn ComputeConfigurer>,
        config: UpcallConfig,
    ) -> Result<Self, UpcallError> {
        if let Some(api) = config.api_listen
            && overlaps(config.listen, api)
        {
            return Err(UpcallError::SharesApiListener {
                listen: config.listen,
                api,
            });
        }
        let listener =
            TcpListener::bind(config.listen)
                .await
                .map_err(|source| UpcallError::Bind {
                    addr: config.listen,
                    source,
                })?;
        let (tenants, tenants_task) = match &config.notify_token {
            Some(_) => {
                let (t, task) = Tenants::follow(&store)?;
                (Some(t), Some(task))
            }
            None => (None, None),
        };
        let limits = config.limits.clone();
        let shared = Arc::new(Shared {
            store,
            keys,
            configurer,
            sources: Sources {
                pageservers: config.pageservers,
                default_pageserver: config.default_pageserver,
                wal_pools: config.wal_pools,
                endpoint_storage: config.endpoint_storage,
                verifiers: config.verifiers,
            },
            notify_digest: config
                .notify_token
                .as_ref()
                .map(|t| Sha256::digest(t.expose().as_bytes()).into()),
            pageserver_digest: config
                .pageserver_token
                .as_ref()
                .map(|t| Sha256::digest(t.expose().as_bytes()).into()),
            tenants,
            spec_peers: PeerLimiter::new(limits.spec_rate, limits.max_peers),
            failure_peers: PeerLimiter::new(limits.auth_failure_rate, limits.max_peers),
            notify_slots: Semaphore::new(limits.max_notify_in_flight),
            limits: limits.clone(),
            clock: config.clock,
        });
        Ok(UpcallServer {
            listener,
            router: router(shared),
            max_connections: limits.max_connections,
            header_read_timeout: limits.header_read_timeout,
            tenants_task,
        })
    }

    /// The bound address (port 0 resolved).
    ///
    /// # Panics
    ///
    /// Never: the socket is bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("a bound listener has an address")
    }

    /// Serves until `shutdown` resolves; open connections are dropped then.
    pub async fn serve(self, shutdown: impl Future<Output = ()> + Send) {
        let slots = Arc::new(Semaphore::new(self.max_connections));
        let (stop, _) = tokio::sync::watch::channel(());
        tokio::pin!(shutdown);
        loop {
            let accepted = tokio::select! {
                () = &mut shutdown => break,
                a = self.listener.accept() => a,
            };
            let (socket, peer) = match accepted {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(error = %e, "upcall listener: accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                tracing::warn!(peer = %peer.ip(), "upcall listener: connection limit reached");
                drop(socket);
                continue;
            };
            let router = self.router.clone();
            let header_read_timeout = self.header_read_timeout;
            let mut stopped = stop.subscribe();
            tokio::spawn(async move {
                let _permit = permit;
                let ip = Peer(peer.ip());
                let service = hyper::service::service_fn(move |mut req: hyper::Request<_>| {
                    req.extensions_mut().insert(ip);
                    let mut router = router.clone();
                    async move {
                        Ok::<_, Infallible>(router.call(req).await.unwrap_or_else(|e| match e {}))
                    }
                });
                let mut builder = hyper::server::conn::http1::Builder::new();
                builder
                    .timer(TokioTimer::new())
                    .header_read_timeout(header_read_timeout);
                tokio::select! {
                    _ = stopped.changed() => {}
                    _ = builder.serve_connection(TokioIo::new(socket), service) => {}
                }
            });
        }
        drop(stop);
        if let Some(task) = self.tenants_task {
            task.abort();
        }
    }
}

fn router(shared: Arc<Shared>) -> Router {
    let mut r = Router::new().route("/compute/api/v2/computes/{id}/spec", get(spec_upcall));
    if shared.notify_digest.is_some() {
        r = r.route("/notify-attach", put(notify_attach));
    }
    if shared.pageserver_digest.is_some() {
        r = r
            .route(
                "/upcall/v1/timeline_import_status",
                post(put_import_status).get(get_import_status),
            )
            .route("/upcall/v1/validate", post(validate_generations));
    }
    r.with_state(shared)
}

fn error(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
}

fn retry_after(status: StatusCode, msg: &str) -> Response {
    let mut resp = error(status, msg);
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    resp
}

/// The bearer token of `headers`, if it is one of at most `max` bytes.
fn bearer(headers: &HeaderMap, max: usize) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?;
    if value.len() > max {
        return None;
    }
    let text = value.to_str().ok()?;
    let (scheme, token) = text.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

impl Shared {
    /// A refused credential: 401, or 429 once the peer's failures run out.
    fn refuse(&self, peer: Peer, status: StatusCode, msg: &str) -> Response {
        if self.failure_peers.take(peer.0, Instant::now()) {
            error(status, msg)
        } else {
            retry_after(StatusCode::TOO_MANY_REQUESTS, "too many refused requests")
        }
    }

    fn ctl_config(&self) -> ComputeCtlConfig {
        ComputeCtlConfig {
            jwks: self.keys.jwks(),
            tls: None,
        }
    }
}

fn json(body: &ControlPlaneConfigResponse) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            bytes,
        )
            .into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "the spec did not encode"),
    }
}

fn spec_error(compute: &str, e: SpecError) -> Response {
    match e {
        SpecError::NotCurrent(why) => {
            tracing::info!(compute, why, "spec upcall: refused");
            error(StatusCode::CONFLICT, why)
        }
        SpecError::NotReady(why) => {
            tracing::info!(compute, why = %why, "spec upcall: not ready");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "the compute's spec is not ready",
            )
        }
        SpecError::Store(e @ StoreError::Corrupt(_)) => {
            tracing::error!(compute, error = %e, "spec upcall: a record does not decode");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
        SpecError::Store(e) => {
            tracing::warn!(compute, error = %e, "spec upcall: store unavailable");
            error(StatusCode::SERVICE_UNAVAILABLE, "the store is unavailable")
        }
    }
}

/// `GET /compute/api/v2/computes/{id}/spec`.
async fn spec_upcall(
    State(s): State<Arc<Shared>>,
    Extension(peer): Extension<Peer>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !s.spec_peers.take(peer.0, Instant::now()) {
        return retry_after(StatusCode::SERVICE_UNAVAILABLE, "too many spec requests");
    }
    let Some(token) = bearer(&headers, s.limits.max_token_bytes) else {
        return s.refuse(
            peer,
            StatusCode::UNAUTHORIZED,
            "a compute token is required",
        );
    };
    let claims = match s.keys.verify_upcall_token(token, s.clock.now_ms()) {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(peer = %peer.0, error = %e, "spec upcall: token refused");
            return s.refuse(
                peer,
                StatusCode::UNAUTHORIZED,
                "the compute token is invalid",
            );
        }
    };
    if claims.compute_id != id {
        tracing::info!(peer = %peer.0, "spec upcall: token for another compute");
        return s.refuse(
            peer,
            StatusCode::FORBIDDEN,
            "the token is for another compute",
        );
    }
    match tokio::time::timeout(s.limits.request_timeout, spec_answer(&s, &id, &claims.kid)).await {
        Ok(resp) => resp,
        Err(_) => {
            tracing::warn!(compute = %id, "spec upcall: timed out");
            error(StatusCode::SERVICE_UNAVAILABLE, "timed out")
        }
    }
}

async fn spec_answer(s: &Shared, id: &str, kid: &str) -> Response {
    if id.parse::<ComputeId>().is_err() {
        return error(StatusCode::NOT_FOUND, "unknown compute");
    }
    let compute = match s
        .store
        .get::<ComputeRec>(&ComputeKey { id: id.to_owned() })
        .await
    {
        Ok(Some(c)) => c.record,
        Ok(None) => return error(StatusCode::NOT_FOUND, "unknown compute"),
        Err(e) => return spec_error(id, SpecError::Store(e)),
    };
    if compute.jwt_key_id.as_deref() != Some(kid) {
        tracing::info!(
            compute = id,
            "spec upcall: token signed with another key than the record's"
        );
        return error(StatusCode::UNAUTHORIZED, "the compute token is invalid");
    }
    if matches!(
        compute.status,
        ComputeStatus::Stopping | ComputeStatus::Stopped | ComputeStatus::Failed
    ) {
        return spec_error(id, SpecError::NotCurrent("the compute is stopped"));
    }
    let Some(binding) = &compute.binding else {
        tracing::debug!(compute = id, "spec upcall: empty");
        return json(&ControlPlaneConfigResponse {
            spec: None,
            status: ControlPlaneComputeStatus::Empty,
            compute_ctl_config: s.ctl_config(),
        });
    };
    match spec::attached(&s.store, &s.sources, &compute, binding).await {
        Ok(spec) => {
            tracing::debug!(compute = id, "spec upcall: attached");
            json(&ControlPlaneConfigResponse {
                spec: Some(spec),
                status: ControlPlaneComputeStatus::Attached,
                compute_ctl_config: s.ctl_config(),
            })
        }
        Err(e) => spec_error(id, e),
    }
}

/// `PUT /notify-attach`.
/// Whether `headers` carry the bearer token whose digest is `want`.
fn token_matches(headers: &HeaderMap, max: usize, want: Option<&[u8; 32]>) -> bool {
    match (bearer(headers, max), want) {
        (Some(token), Some(want)) => {
            let got: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            bool::from(got.ct_eq(want))
        }
        _ => false,
    }
}

/// A pageserver's `PutTimelineImportStatusRequest`. The generation is not
/// kept: Loams runs one pageserver per tenant shard, in emergency mode.
#[derive(Deserialize)]
struct PutImportStatus {
    tenant_shard_id: String,
    timeline_id: String,
    status: serde_json::Value,
}

/// A pageserver's `ValidateRequest`: the tenant shards and generations its
/// deletion queue wants to act on.
#[derive(Deserialize)]
struct ValidateRequest {
    tenants: Vec<ValidateTenant>,
}

/// One shard of a `ValidateRequest` (its generation, `gen`, is not read).
#[derive(Deserialize)]
struct ValidateTenant {
    id: String,
}

/// A pageserver's `TimelineImportStatusRequest`.
#[derive(Deserialize)]
struct GetImportStatus {
    tenant_shard_id: String,
    timeline_id: String,
}

/// The state a `ShardImportStatus` names: `"Done"`, `{"Error": "..."}` or
/// `{"InProgress": ...}`; `None` for anything else.
pub fn import_state(status: &serde_json::Value) -> Option<ImportState> {
    match status {
        serde_json::Value::String(s) if s == "Done" => Some(ImportState::Done),
        serde_json::Value::Object(m) if m.len() == 1 => {
            if m.contains_key("InProgress") {
                Some(ImportState::InProgress)
            } else {
                m.get("Error").map(|e| {
                    ImportState::Error(e.as_str().map_or_else(|| e.to_string(), str::to_string))
                })
            }
        }
        _ => None,
    }
}

/// `POST /upcall/v1/timeline_import_status`: records a pageserver's report
/// on an import (NF2 Task 13). The last report wins; a repeated one is a
/// no-op, as the pageserver's at-least-once delivery needs.
async fn put_import_status(
    State(s): State<Arc<Shared>>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !token_matches(
        &headers,
        s.limits.max_token_bytes,
        s.pageserver_digest.as_ref(),
    ) {
        tracing::info!(peer = %peer.0, "timeline_import_status: token refused");
        return s.refuse(peer, StatusCode::UNAUTHORIZED, "the token is invalid");
    }
    let Ok(bytes) = axum::body::to_bytes(body, s.limits.max_body_bytes).await else {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large or unreadable",
        );
    };
    let Ok(req) = serde_json::from_slice::<PutImportStatus>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "the body is not an import status");
    };
    let Some(state) = import_state(&req.status) else {
        return error(
            StatusCode::BAD_REQUEST,
            "the status is not a ShardImportStatus",
        );
    };
    let key = ImportStatusKey {
        tenant_shard_id: req.tenant_shard_id.clone(),
        timeline_id: req.timeline_id.clone(),
    };
    let rec = ImportStatusRec {
        tenant_shard_id: req.tenant_shard_id,
        timeline_id: req.timeline_id,
        state,
        status_json: req.status.to_string(),
        updated_at_ms: s.clock.now_ms(),
    };
    let api = s.store.api_writer();
    for _ in 0..3 {
        let expected = match s.store.get::<ImportStatusRec>(&key).await {
            Ok(Some(v))
                if v.record.state == rec.state && v.record.status_json == rec.status_json =>
            {
                return StatusCode::OK.into_response();
            }
            Ok(Some(v)) => Some(v.version),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "timeline_import_status: store unavailable");
                return error(StatusCode::SERVICE_UNAVAILABLE, "the store is unavailable");
            }
        };
        match api.put(&rec, expected).await {
            Ok(_) => {
                tracing::info!(
                    tenant = %rec.tenant_shard_id,
                    timeline = %rec.timeline_id,
                    state = ?rec.state,
                    "timeline import status recorded"
                );
                return StatusCode::OK.into_response();
            }
            Err(StoreError::Conflict { .. }) => continue,
            Err(e) => {
                tracing::warn!(error = %e, "timeline_import_status: store unavailable");
                return error(StatusCode::SERVICE_UNAVAILABLE, "the store is unavailable");
            }
        }
    }
    error(StatusCode::SERVICE_UNAVAILABLE, "the status kept changing")
}

/// `POST /upcall/v1/validate`: every generation a pageserver asks about is
/// valid. The storage controller answers this to fence a pageserver that
/// lost its attachment; Loams runs one pageserver per tenant shard, in
/// emergency mode at its last known generation, so there is nothing to
/// fence. Without an answer the pageserver's deletion queue never deletes a
/// layer and never advances its remote consistent LSN (NF2 Task 13).
async fn validate_generations(
    State(s): State<Arc<Shared>>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !token_matches(
        &headers,
        s.limits.max_token_bytes,
        s.pageserver_digest.as_ref(),
    ) {
        tracing::info!(peer = %peer.0, "validate: token refused");
        return s.refuse(peer, StatusCode::UNAUTHORIZED, "the token is invalid");
    }
    let Ok(bytes) = axum::body::to_bytes(body, s.limits.max_body_bytes).await else {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large or unreadable",
        );
    };
    let Ok(req) = serde_json::from_slice::<ValidateRequest>(&bytes) else {
        return error(
            StatusCode::BAD_REQUEST,
            "the body is not a validate request",
        );
    };
    let tenants: Vec<serde_json::Value> = req
        .tenants
        .into_iter()
        .map(|t| serde_json::json!({"id": t.id, "valid": true}))
        .collect();
    axum::Json(serde_json::json!({ "tenants": tenants })).into_response()
}

/// `GET /upcall/v1/timeline_import_status`: the last recorded report, or
/// in progress for an import not reported yet (a pageserver asks when it
/// restarts an import).
async fn get_import_status(
    State(s): State<Arc<Shared>>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !token_matches(
        &headers,
        s.limits.max_token_bytes,
        s.pageserver_digest.as_ref(),
    ) {
        tracing::info!(peer = %peer.0, "timeline_import_status: token refused");
        return s.refuse(peer, StatusCode::UNAUTHORIZED, "the token is invalid");
    }
    let Ok(bytes) = axum::body::to_bytes(body, s.limits.max_body_bytes).await else {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large or unreadable",
        );
    };
    let Ok(req) = serde_json::from_slice::<GetImportStatus>(&bytes) else {
        return error(
            StatusCode::BAD_REQUEST,
            "the body is not an import status request",
        );
    };
    let key = ImportStatusKey {
        tenant_shard_id: req.tenant_shard_id,
        timeline_id: req.timeline_id,
    };
    let text = match s.store.get::<ImportStatusRec>(&key).await {
        Ok(Some(v)) => v.record.status_json,
        Ok(None) => r#"{"InProgress":null}"#.to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "timeline_import_status: store unavailable");
            return error(StatusCode::SERVICE_UNAVAILABLE, "the store is unavailable");
        }
    };
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        text,
    )
        .into_response()
}

async fn notify_attach(
    State(s): State<Arc<Shared>>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let authorized = match (bearer(&headers, s.limits.max_token_bytes), &s.notify_digest) {
        (Some(token), Some(want)) => {
            let got: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            bool::from(got.ct_eq(want))
        }
        _ => false,
    };
    if !authorized {
        tracing::info!(peer = %peer.0, "notify-attach: token refused");
        return s.refuse(peer, StatusCode::UNAUTHORIZED, "the token is invalid");
    }
    let Ok(_slot) = s.notify_slots.try_acquire() else {
        return retry_after(
            StatusCode::TOO_MANY_REQUESTS,
            "too many notifications at once",
        );
    };
    let Ok(bytes) = axum::body::to_bytes(body, s.limits.max_body_bytes).await else {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large or unreadable",
        );
    };
    let Ok(req) = serde_json::from_slice::<NotifyAttachRequest>(&bytes) else {
        return error(
            StatusCode::BAD_REQUEST,
            "the body is not a notify-attach request",
        );
    };
    let shards = match shard_map(&req) {
        Ok(m) => m,
        Err(why) => return error(StatusCode::BAD_REQUEST, why),
    };
    let recorded = tokio::time::timeout(s.limits.request_timeout, record(&s, &req, &shards)).await;
    let (key, project) = match recorded {
        Ok(Ok(found)) => found,
        Ok(Err(resp)) => return resp,
        Err(_) => {
            tracing::warn!(tenant = %req.tenant_id, "notify-attach: recording timed out");
            return error(StatusCode::SERVICE_UNAVAILABLE, "timed out");
        }
    };
    let (applied, failed) = push(&s, &key, &project).await;
    tracing::info!(
        tenant = %req.tenant_id,
        project = %project.id,
        shards = shards.len(),
        applied,
        failed,
        "notify-attach: recorded"
    );
    if failed == 0 {
        (StatusCode::OK, axum::Json(serde_json::json!({}))).into_response()
    } else {
        error(
            StatusCode::LOCKED,
            "recorded, but not every compute took it",
        )
    }
}

/// The shard map a well-formed request names, by shard number.
fn shard_map(req: &NotifyAttachRequest) -> Result<Vec<ShardAttachment>, &'static str> {
    if req.shards.is_empty() || req.shards.len() > MAX_SHARDS {
        return Err("a tenant has 1 to 255 shards");
    }
    let mut shards: Vec<ShardAttachment> = req
        .shards
        .iter()
        .map(|s| ShardAttachment {
            shard_number: s.shard_number,
            node_id: s.node_id,
        })
        .collect();
    shards.sort_by_key(|s| s.shard_number);
    let contiguous = shards
        .iter()
        .enumerate()
        .all(|(i, s)| usize::from(s.shard_number) == i);
    if !contiguous {
        return Err("shard numbers must be 0..count, once each");
    }
    match req.stripe_size {
        None if shards.len() != 1 => Err("an unsharded tenant has one shard"),
        Some(0) => Err("a stripe size is not 0"),
        _ => Ok(shards),
    }
}

/// The project of `req`'s tenant, with the new shard map written on every
/// branch.
async fn record(
    s: &Shared,
    req: &NotifyAttachRequest,
    shards: &[ShardAttachment],
) -> Result<(ProjectKey, ProjectRec), Response> {
    let Some(tenants) = &s.tenants else {
        return Err(error(StatusCode::NOT_FOUND, "not served"));
    };
    let tenant = req.tenant_id.0;
    let mut lookup = tenants.find(&tenant);
    if lookup == Lookup::Unknown {
        // A project created a moment ago may not be in the last scan yet.
        tokio::time::sleep(s.store.options().poll * 2).await;
        lookup = tenants.find(&tenant);
    }
    let key = match lookup {
        Lookup::Found(k) => k,
        Lookup::NotSynced => {
            return Err(retry_after(StatusCode::SERVICE_UNAVAILABLE, "starting"));
        }
        Lookup::Unknown => return Err(error(StatusCode::NOT_FOUND, "unknown tenant")),
    };
    let unavailable = |e: &StoreError| {
        tracing::warn!(tenant = %req.tenant_id, error = %e, "notify-attach: store unavailable");
        error(StatusCode::SERVICE_UNAVAILABLE, "the store is unavailable")
    };
    let project = match s.store.get::<ProjectRec>(&key).await {
        Ok(Some(p)) if p.record.tenant_id == tenant => p.record,
        Ok(_) => return Err(error(StatusCode::NOT_FOUND, "unknown tenant")),
        Err(e) => return Err(unavailable(&e)),
    };
    let prefix = BranchPrefix {
        project_id: project.id.clone(),
    };
    for _ in 0..ATTEMPTS {
        let branches = spec::list_all::<_, BranchRec>(&s.store, &prefix)
            .await
            .map_err(|e| unavailable(&e))?;
        let mut batch = Batch::new();
        for b in branches {
            if b.record.shards == shards && b.record.stripe_size == req.stripe_size {
                continue;
            }
            let mut rec = b.record;
            rec.shards = shards.to_vec();
            rec.stripe_size = req.stripe_size;
            rec.updated_at_ms = s.clock.now_ms();
            batch
                .put(&rec, Some(b.version))
                .map_err(|e| unavailable(&e))?;
        }
        if batch.is_empty() {
            return Ok((key, project));
        }
        match s.store.api_writer().commit(batch).await {
            Ok(_) => return Ok((key, project)),
            Err(BatchError {
                error: StoreError::Conflict { .. } | StoreError::NotFound,
                ..
            }) => {}
            Err(e) => return Err(unavailable(&e.error)),
        }
    }
    Err(unavailable(&StoreError::Unavailable(
        "the branches kept changing".into(),
    )))
}

/// Pushes the new spec to every running compute of `project` that is its
/// endpoint's current compute: (applied, failed). Unfinished at
/// `apply_timeout` counts as failed.
async fn push(s: &Shared, key: &ProjectKey, project: &ProjectRec) -> (usize, usize) {
    let prefix = EndpointPrefix {
        project_id: project.id.clone(),
    };
    let endpoints = match spec::list_all::<_, EndpointRec>(&s.store, &prefix).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(project = %project.id, error = %e, "notify-attach: endpoints unreadable");
            return (0, 1);
        }
    };
    let mut computes = Vec::new();
    for ep in endpoints {
        let Some(id) = ep.record.compute_id else {
            continue;
        };
        match s
            .store
            .get::<ComputeRec>(&ComputeKey { id: id.clone() })
            .await
        {
            Ok(Some(c))
                if c.record.status == ComputeStatus::Running
                    && c.record.binding.as_ref().is_some_and(|b| {
                        b.project_id == key.id && b.endpoint_id == ep.record.id
                    }) =>
            {
                computes.push(c.record);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(compute = %id, error = %e, "notify-attach: compute unreadable");
                return (0, 1);
            }
        }
    }
    let total = computes.len();
    let config = s.ctl_config();
    let config = &config;
    let work = futures::stream::iter(computes)
        .map(|c| async move {
            let Some(binding) = &c.binding else {
                return false;
            };
            let outcome = match spec::attached(&s.store, &s.sources, &c, binding).await {
                Ok(spec) => s
                    .configurer
                    .configure(&c, &spec, config)
                    .await
                    .map_err(|e| e.0),
                Err(e) => Err(format!("{e:?}")),
            };
            match outcome {
                Ok(()) => true,
                Err(why) => {
                    tracing::warn!(compute = %c.id, error = %why, "notify-attach: compute not reconfigured");
                    false
                }
            }
        })
        .buffer_unordered(s.limits.max_configure_in_flight.max(1))
        .fold(0usize, |n, ok| async move { n + usize::from(ok) });
    match tokio::time::timeout(s.limits.apply_timeout, work).await {
        Ok(applied) => (applied, total - applied),
        Err(_) => {
            tracing::warn!(project = %project.id, "notify-attach: computes did not answer in time");
            (0, total.max(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use loams_postgres::TenantId;
    use loams_postgres::storcon::NotifyAttachShard;

    use super::*;

    fn req(stripe: Option<u32>, shards: &[(u8, u64)]) -> NotifyAttachRequest {
        NotifyAttachRequest {
            tenant_id: TenantId([1; 16]),
            preferred_az: None,
            stripe_size: stripe,
            shards: shards
                .iter()
                .map(|&(n, node)| NotifyAttachShard {
                    node_id: node,
                    shard_number: n,
                })
                .collect(),
        }
    }

    #[test]
    fn shard_maps_are_checked() {
        assert!(shard_map(&req(None, &[(0, 1)])).is_ok());
        assert!(shard_map(&req(None, &[])).is_err());
        assert!(shard_map(&req(None, &[(0, 1), (1, 2)])).is_err());
        assert!(shard_map(&req(None, &[(1, 1)])).is_err());
        assert!(shard_map(&req(Some(0), &[(0, 1)])).is_err());
        assert!(shard_map(&req(Some(8), &[(0, 1), (0, 2)])).is_err());
        let m = shard_map(&req(Some(8), &[(1, 2), (0, 1)])).expect("sharded");
        assert_eq!(m[0].node_id, 1);
        assert_eq!(m[1].node_id, 2);
    }

    #[test]
    fn listeners_overlap_on_port_and_ip() {
        let p = |s: &str| s.parse::<SocketAddr>().expect("an address");
        assert!(overlaps(p("127.0.0.1:7690"), p("127.0.0.1:7690")));
        assert!(overlaps(p("0.0.0.0:7690"), p("127.0.0.1:7690")));
        assert!(overlaps(p("127.0.0.1:7690"), p("[::]:7690")));
        assert!(!overlaps(p("127.0.0.1:7690"), p("127.0.0.1:7691")));
        assert!(!overlaps(p("127.0.0.1:7690"), p("10.0.0.1:7690")));
        assert!(!overlaps(p("127.0.0.1:0"), p("127.0.0.1:0")));
    }

    #[test]
    fn bearer_tokens_are_bounded() {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(bearer(&h, 64), Some("abc"));
        assert_eq!(bearer(&h, 4), None);
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer(&h, 64), None);
    }
}
