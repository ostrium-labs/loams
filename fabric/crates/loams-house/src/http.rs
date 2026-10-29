//! The ClickHouse HTTP interface (FL2 Task 2's contract, HS1 Task 3), served on the
//! worker pool through hyper's HTTP/1.1 connection.
//!
//! # The transport (rulings R3.1, R3.8)
//!
//! hyper owns the wire: request heads (at most `max_headers` fields, else `431`;
//! URIs up to hyper's 65 534 bytes, else `414`), `Content-Length` and chunked
//! bodies, conflicting framing (two different `Content-Length`s or a malformed one
//! are `400`; `Content-Length` with `Transfer-Encoding: chunked` is read as chunked
//! and the connection closes after it, RFC 9112 §6.3, so nothing can be smuggled
//! behind it: R3.9), HTTP/1.0, HEAD, pipelining,
//! keep-alive and `Expect: 100-continue` (the `100` goes out only when the body is
//! first read, which is after authentication). The client's silence is bounded per
//! request-body frame (`receive_timeout`) and per head and idle gap (hyper's header
//! timer, at the advertised `keep_alive`); writes have an idle timeout
//! ([`IdleIo`]); connections are capped by a semaphore; a connection ends with a
//! lingering close.
//!
//! # A request
//!
//! `GET /` and `GET /ping` answer `Ok.\n`. Otherwise the statement is `?query=`, or
//! the POST body; with both, the parameter is the statement and the body is its
//! data (an `INSERT … FORMAT <f>`), or the rest of the statement. Credentials,
//! settings and the response headers are FL2 Task 2's; see [`crate::auth`] and
//! [`crate::compress`]. GET (and HEAD), and a read-only user, may only read:
//! anything else is `164 READONLY` before a worker is asked. An `INSERT` takes its
//! worker only once its data has started to arrive, so a slow client cannot hold
//! one idle (review I1).
//!
//! # The response
//!
//! Output is held until `buffer_size` (1 MiB, clamped to 1 B – 16 MiB) has
//! accumulated or the statement ends, so a statement that fails early still gets
//! its own status and `X-ClickHouse-Exception-Code`, and a small result's
//! `X-ClickHouse-Summary` is final. Past it, the head goes out with the summary so
//! far and the body streams. A failure after that is Ruling 9's: the exception
//! text is appended to the body and the connection is closed without the
//! terminating chunk ([`crate::errors::MidStreamBody`]'s rule).
//!
//! With `send_progress_in_http_headers = 1` the head carries **one**
//! `X-ClickHouse-Progress` line, the counters when it went out. ClickHouse writes
//! such lines all through a query, keeping its header block open; hyper writes a
//! head in one piece, so this is a recorded surface deviation (R3.8).
//!
//! # `loams.house.v1`
//!
//! Every path under `/loams.house.v1.` is the House's Connect API (HS1 Task 8,
//! [`crate::api`]): it goes to connect-rust's service, on the same connections,
//! with the same connection cap and timers (a request body silent for
//! `receive_timeout` between frames fails). Connect and gRPC-Web are served;
//! gRPC proper needs HTTP/2, which this listener does not speak (R3.8).
//!
//! `wait_end_of_query = 1` spools the whole result (in memory up to
//! [`SPOOL_IN_MEMORY`], then an unlinked temporary file in `tmp_dir`, at most
//! `wait_end_of_query_max_bytes`, within the front's `spool_budget_bytes`) and
//! answers only when the statement is over.

use std::convert::Infallible;
use std::future::Future;
use std::io::{self, Read as _, Seek, SeekFrom, Write as _};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use connectrpc::{ConnectRpcBody, ConnectRpcService};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body::{Body as HttpBody, Frame as BodyFrame, SizeHint};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use loams_house_ipc::{Execute, Limits, Output, Progress};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Sleep;

use crate::admission::{Event, Outcome, WorkerLease, WorkerPool};
use crate::api::{ApiCatalog, FixtureCatalog, HouseApi, LakeCatalog};
use crate::auth;
use crate::catalog::Lake;
use crate::classify::{Classified, Expect, Stmt, check_text, classify, readonly_error};
use crate::compress::{self, Decoder, Encoder, Encoding};
use crate::config::{self, BUFFER_SIZE_RANGE, DISPLAY_NAME, HouseConfig, SPOOL_IN_MEMORY, UserMap};
use crate::deny;
use crate::errors::{ChError, HouseError};
use crate::gate;
use crate::request::{self, InsertHead, NON_SETTINGS, Scanner};
use crate::session::{SessionGuard, SessionParams, SessionTable};
use crate::watchdog::ExitReason;

/// The most of an unwanted request body read to keep its connection (and to let
/// the client read the answer before the connection closes).
const MAX_DRAIN: u64 = 1024 * 1024;
/// How long a failed body holds its abort after the exception text (Ruling 9, fix
/// round 2 N5). hyper gives a body no signal that its bytes reached the socket, but
/// it flushes whenever the body is pending, so the text goes out within this bound
/// unless the client's socket stops accepting bytes for that long. **The limit:**
/// a client that is not reading may then miss the text; it still gets a body
/// without its terminating chunk, which no client reads as a complete result.
const FLUSH_BOUND: Duration = Duration::from_millis(100);
/// How many body pieces may wait between the statement and the client.
const CHANNEL_DEPTH: usize = 8;
/// How long a closing connection waits for the client's side to finish.
const LINGER: Duration = Duration::from_secs(2);

/// A running HTTP interface. Dropping it stops accepting connections; requests in
/// flight finish.
#[derive(Debug)]
pub struct HouseHandle {
    addr: SocketAddr,
    tls_addr: Option<SocketAddr>,
    admin_addr: Option<SocketAddr>,
    accept: JoinHandle<()>,
    admin: Option<JoinHandle<()>>,
    tls_accept: Option<JoinHandle<()>>,
    /// The native protocol's listeners (HE0, R9.40): plaintext and TLS.
    native_addr: Option<SocketAddr>,
    native_tls_addr: Option<SocketAddr>,
    native_accept: Vec<JoinHandle<()>>,
    compaction: Option<JoinHandle<()>>,
    retention: Option<JoinHandle<()>>,
    pipes: Option<JoinHandle<()>>,
    queues: Option<JoinHandle<()>>,
    views: Option<JoinHandle<()>>,
    storage: Option<JoinHandle<()>>,
    query_log_task: Option<JoinHandle<()>>,
    /// The usage hub and its quotas (HS1 Task 22).
    usage: Arc<crate::usage::Usage>,
    lake: Option<Arc<Lake>>,
    /// The front's metrics and its query log (HS1 Task 23).
    metrics: Arc<crate::metrics::Metrics>,
    query_log: Option<Arc<crate::querylog::QueryLog>>,
}

impl HouseHandle {
    /// Its usage hub, whose quotas both surfaces admit statements under (HS1
    /// Task 22).
    pub fn usage(&self) -> &Arc<crate::usage::Usage> {
        &self.usage
    }

    /// Runs one storage round now, as of `at` (HS1 Task 22): one
    /// `house.storage` per lake table, each namespace's total to its quota.
    pub async fn storage_round(&self, at: std::time::SystemTime) {
        if let Some(lake) = &self.lake {
            crate::usage::StorageReporter::new(Arc::clone(lake), Arc::clone(&self.usage))
                .round(at)
                .await;
        }
    }

    /// Its metrics (HS1 Task 23).
    pub fn metrics(&self) -> &Arc<crate::metrics::Metrics> {
        &self.metrics
    }

    /// Its query log, when it has a catalog (HS1 Task 23).
    pub fn query_log(&self) -> Option<&Arc<crate::querylog::QueryLog>> {
        self.query_log.as_ref()
    }

    /// Writes every namespace's query log rows now (HS1 Task 23), as the
    /// timer does every `query_log_interval`.
    pub async fn flush_query_log(&self) {
        if let Some(log) = &self.query_log {
            log.flush().await;
        }
    }

    /// The address it listens on (the port chosen, for `:0`).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The TLS listener's address, when it has one (HS1 Task 20).
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
    }

    /// The admin listener's address, when it has one (HS1 Task 23).
    pub fn admin_addr(&self) -> Option<SocketAddr> {
        self.admin_addr
    }

    /// The native protocol's plaintext listener, when it has one (R9.40).
    pub fn native_addr(&self) -> Option<SocketAddr> {
        self.native_addr
    }

    /// The native protocol's TLS listener, when it has one (R9.40).
    pub fn native_tls_addr(&self) -> Option<SocketAddr> {
        self.native_tls_addr
    }

    /// Stops accepting connections.
    pub fn shutdown(self) {
        self.abort();
    }

    fn abort(&self) {
        self.accept.abort();
        if let Some(tls) = &self.tls_accept {
            tls.abort();
        }
        for native in &self.native_accept {
            native.abort();
        }
        if let Some(retention) = &self.retention {
            retention.abort();
        }
        if let Some(pipes) = &self.pipes {
            pipes.abort();
        }
        if let Some(queues) = &self.queues {
            queues.abort();
        }
        if let Some(admin) = &self.admin {
            admin.abort();
        }
        if let Some(views) = &self.views {
            views.abort();
        }
        if let Some(compaction) = &self.compaction {
            compaction.abort();
        }
        if let Some(storage) = &self.storage {
            storage.abort();
        }
        if let Some(task) = &self.query_log_task {
            task.abort();
        }
    }
}

impl Drop for HouseHandle {
    fn drop(&mut self) {
        self.abort();
    }
}

#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) config: HouseConfig,
    pub(crate) pool: WorkerPool,
    spool: Arc<SpoolBudget>,
    sessions: Arc<SessionTable>,
    api: ApiService,
    /// The catalog of lake tables (HS1 Task 10), when the House has one.
    lake: Option<Arc<Lake>>,
    /// Namespace queues and node memory (§49 §12, HS1 Task 21).
    admission: Arc<crate::admission::Admission>,
    /// Running statements, every surface: `KILL QUERY` (HS1 Task 21).
    pub(crate) processes: Arc<crate::watchdog::Processes>,
    /// Every statement's end: metrics, operator line, trace, query log (HS1
    /// Task 23).
    pub(crate) observer: Arc<crate::querylog::Observer>,
}

/// connect-rust's service for `loams.house.v1`, which has no `Debug` of its own.
#[derive(Clone)]
struct ApiService(ConnectRpcService<connectrpc::Router>);

impl std::fmt::Debug for ApiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiService(loams.house.v1)")
    }
}

/// Serves the ClickHouse HTTP interface and `loams.house.v1` on `config.listen`,
/// running statements on `pool`. A non-loopback address is refused (D111). The
/// API's browsing RPCs read what a House with no catalog has
/// ([`FixtureCatalog::empty`]).
pub async fn serve(config: HouseConfig, pool: WorkerPool) -> Result<HouseHandle, HouseError> {
    serve_with_catalog(config, pool, Arc::new(FixtureCatalog::empty())).await
}

/// [`serve`], with the API's metadata read from `catalog` (fixtures, in tests).
pub async fn serve_with_catalog(
    config: HouseConfig,
    pool: WorkerPool,
    catalog: Arc<dyn ApiCatalog>,
) -> Result<HouseHandle, HouseError> {
    serve_all(config, pool, catalog, None).await
}

/// [`serve`] with a catalog of lake tables (HS1 Task 10): DDL, `USE`, `DESCRIBE`,
/// `SHOW CREATE` and `EXISTS` of its tables are answered from it, and so are the
/// API's browsing RPCs.
pub async fn serve_with_lake(
    config: HouseConfig,
    pool: WorkerPool,
    lake: Arc<Lake>,
) -> Result<HouseHandle, HouseError> {
    let catalog = Arc::new(LakeCatalog(Arc::clone(&lake)));
    serve_all(config, pool, catalog, Some(lake)).await
}

async fn serve_all(
    config: HouseConfig,
    pool: WorkerPool,
    catalog: Arc<dyn ApiCatalog>,
    lake: Option<Arc<Lake>>,
) -> Result<HouseHandle, HouseError> {
    config::check_listen(config.listen)?;
    let listen = config.listen;
    let network = |what: &str, err: io::Error| {
        HouseError::from(ChError::network_error(format!(
            "house listen on {listen}: {what}: {err}"
        )))
    };
    std::fs::create_dir_all(&config.tmp_dir).map_err(|err| network("the spool directory", err))?;
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| network("bind", err))?;
    let addr = listener
        .local_addr()
        .map_err(|err| network("address", err))?;
    // The TLS listener (HS1 Task 20): any address, never plaintext.
    let (tls, tls_addr) = match &config.tls {
        Some(tls) => {
            config::check_tls_users(tls.listen, &config.users)?;
            let listener = TcpListener::bind(tls.listen)
                .await
                .map_err(|err| network("bind the TLS listener", err))?;
            let bound = listener
                .local_addr()
                .map_err(|err| network("the TLS listener's address", err))?;
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::clone(&tls.config));
            (Some((listener, acceptor)), Some(bound))
        }
        None => (None, None),
    };
    // The admin listener (HS1 Task 23): metrics, loopback only.
    let admin_listener = match config.admin_listen {
        Some(admin) => {
            config::check_listen(admin)?;
            let listener = TcpListener::bind(admin)
                .await
                .map_err(|err| network("bind the admin listener", err))?;
            let bound = listener
                .local_addr()
                .map_err(|err| network("the admin listener's address", err))?;
            Some((listener, bound))
        }
        None => None,
    };
    // The native protocol (HE0, R9.40): plaintext on loopback only, TLS with
    // the TLS listener's certificate. Bound here, so a taken port fails the
    // start as the HTTP listener's does.
    let native_listener = match config.native_listen {
        Some(native) => {
            config::check_listen(native)?;
            match TcpListener::bind(native).await {
                Ok(listener) => Some(listener),
                Err(err) if config.native_listen_optional => {
                    eprintln!(
                        "loams-house: native protocol {native}: {err}; not served (name \
                         another address with --native-listen)"
                    );
                    None
                }
                Err(err) => return Err(network("bind the native listener", err)),
            }
        }
        None => None,
    };
    let native_tls_listener = match (config.native_tls_listen, &config.tls) {
        (Some(native), Some(tls)) => {
            config::check_tls_users(native, &config.users)?;
            let listener = TcpListener::bind(native)
                .await
                .map_err(|err| network("bind the native TLS listener", err))?;
            Some((listener, Arc::clone(&tls.config)))
        }
        (Some(native), None) => {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "native TLS listener {native}: needs the TLS listener's certificate"
            ))));
        }
        (None, _) => None,
    };
    let local = |listener: &TcpListener, what: &str| {
        listener.local_addr().map_err(|err| network(what, err))
    };
    let native_addr = native_listener
        .as_ref()
        .map(|l| local(l, "the native listener's address"))
        .transpose()?;
    let native_tls_addr = native_tls_listener
        .as_ref()
        .map(|(l, _)| local(l, "the native TLS listener's address"))
        .transpose()?;
    let permits = Arc::new(Semaphore::new(config.max_connections.max(1)));
    let tls_permits = Arc::clone(&permits);
    let native_permits = Arc::clone(&permits);
    let unpin_pool = pool.clone();
    let sessions = SessionTable::with_user_caps(
        config.max_live_sessions,
        config.max_sessions_per_user,
        config.max_pinned_workers_per_user,
        Arc::new(move |worker: &str| unpin_pool.unpin(worker)),
    );
    // Sessions idle past their timeout end on a timer too, releasing their pinned
    // workers (HS1 Task 4); the task ends with the table.
    let swept = Arc::downgrade(&sessions);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let Some(sessions) = swept.upgrade() else {
                return;
            };
            sessions.sweep();
        }
    });
    // Quotas and usage (HS1 Task 22): one set of counters for both surfaces,
    // fed by every lease's, insert's and storage round's events.
    let quotas = crate::quota::Quotas::new(Arc::clone(&config.quotas));
    // Observability (HS1 Task 23): one set of metrics for the surfaces, the
    // commits and the compactions; the query log when there is a catalog.
    let metrics = lake
        .as_ref()
        .map_or_else(Arc::default, |lake| Arc::clone(lake.metrics()));
    metrics.set_namespace_labels(config.metrics_namespace_labels);
    let query_log = lake
        .as_ref()
        .map(|lake| crate::querylog::QueryLog::new(Arc::clone(lake), config.query_log_retention));
    let observer = Arc::new(crate::querylog::Observer {
        metrics: Arc::clone(&metrics),
        tracer: Arc::clone(&config.tracer),
        oplog: config.operator_log.clone(),
        log: query_log.clone(),
    });
    let usage = crate::usage::Usage::with_metrics(
        Arc::clone(&config.usage),
        Arc::clone(&quotas),
        Arc::clone(&metrics),
    );
    pool.set_usage(Arc::clone(&usage));
    if let Some(lake) = &lake {
        lake.set_quotas(Arc::clone(&quotas));
    }
    let admission = crate::admission::Admission::with_quotas(config.admission.clone(), quotas);
    let processes = Arc::new(crate::watchdog::Processes::default());
    let api = ApiService(ConnectRpcService::new(
        HouseApi::new(
            config.clone(),
            pool.clone(),
            catalog,
            lake.clone(),
            crate::api::Governance {
                admission: Arc::clone(&admission),
                processes: Arc::clone(&processes),
                observer: Arc::clone(&observer),
            },
        )
        .router(),
    ));
    let shared = Arc::new(Shared {
        spool: Arc::new(SpoolBudget::new(config.spool_budget_bytes)),
        config,
        pool,
        sessions,
        api,
        lake,
        admission,
        processes,
        observer,
    });
    let tls_shared = Arc::clone(&shared);
    let mut native_accept = Vec::new();
    if let Some(listener) = native_listener {
        native_accept.push(crate::native::spawn(
            listener,
            None,
            Arc::clone(&shared),
            Arc::clone(&native_permits),
        ));
    }
    if let Some((listener, tls)) = native_tls_listener {
        native_accept.push(crate::native::spawn(
            listener,
            Some(tokio_rustls::TlsAcceptor::from(native_tls_config(&tls))),
            Arc::clone(&shared),
            native_permits,
        ));
    }
    // Background compaction of the lake's tables (HS1 Task 13).
    let compaction = match (&shared.lake, shared.config.compaction_interval) {
        (Some(lake), every) if !every.is_zero() => Some(
            crate::compact::Scheduler::new(
                Arc::clone(lake),
                shared.pool.clone(),
                shared.config.clone(),
            )
            .spawn(every),
        ),
        _ => None,
    };
    // Snapshot expiry, orphan files and purges (HS1 Task 15).
    let retention = match (&shared.lake, shared.config.retention_interval) {
        (Some(lake), every) if !every.is_zero() => {
            Some(crate::retention::Scheduler::new(Arc::clone(lake)).spawn(every))
        }
        _ => None,
    };
    // The pipes' runner (HS1 Task 16), when the House reaches an engine.
    let pipes = match (
        &shared.lake,
        &shared.config.engine,
        shared.config.pipe_interval,
    ) {
        (Some(lake), Some(link), every) if !every.is_zero() => {
            match crate::pipes::engine_source(link) {
                Ok(source) => Some(
                    crate::pipes::PipeRunner::new(
                        Arc::clone(lake),
                        shared.pool.clone(),
                        shared.config.clone(),
                        link.clone(),
                        source,
                    )
                    .spawn(every),
                ),
                Err(err) => {
                    eprintln!("loams-house: pipes do not run: {}", err.message());
                    None
                }
            }
        }
        _ => None,
    };
    let (admin, admin_addr) = match admin_listener {
        Some((listener, bound)) => (
            Some(crate::metrics::serve(
                listener,
                crate::metrics::Sources {
                    pool: shared.pool.clone(),
                    lake: shared.lake.clone(),
                    admission: Arc::clone(&shared.admission),
                    metrics: Arc::clone(&metrics),
                    cache: shared.config.cache_metrics.clone(),
                },
            )),
            Some(bound),
        ),
        None => (None, None),
    };
    // The `S3Queue` pipes' runner (HS1 Task 18): no engine link needed.
    let queues = match (&shared.lake, shared.config.pipe_interval) {
        (Some(lake), every) if !every.is_zero() => Some(
            crate::pipes::s3queue::QueueRunner::new(
                Arc::clone(lake),
                shared.pool.clone(),
                shared.config.clone(),
            )
            .spawn(every),
        ),
        _ => None,
    };
    // Materialized views' catch-up (HS1 Task 17).
    let views = match (&shared.lake, shared.config.view_interval) {
        (Some(lake), every) if !every.is_zero() => Some(
            crate::mv::Scheduler::new(crate::mv::ViewRunner::new(crate::pipes::ViewWriter {
                lake: Arc::clone(lake),
                pool: shared.pool.clone(),
                limits: shared.config.session_limits,
                max_insert_bytes: shared.config.lake_insert_max_bytes,
            }))
            .spawn(every),
        ),
        _ => None,
    };
    // Each lake table's bytes, daily (HS1 Task 22).
    let storage = match (&shared.lake, shared.config.storage_usage_interval) {
        (Some(lake), every) if !every.is_zero() => Some(
            crate::usage::StorageReporter::new(Arc::clone(lake), Arc::clone(&usage)).spawn(every),
        ),
        _ => None,
    };
    // The query log's group commits and its retention (HS1 Task 23).
    let query_log_task = match (&query_log, shared.config.query_log_interval) {
        (Some(log), every) if !every.is_zero() => Some(log.spawn(every)),
        _ => None,
    };
    let handle_lake = shared.lake.clone();
    let accept = tokio::spawn(async move {
        loop {
            let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                return;
            };
            match listener.accept().await {
                Ok((stream, _)) => {
                    let _ = stream.set_nodelay(true);
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move {
                        connection(stream, shared).await;
                        drop(permit);
                    });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    });
    let tls_accept = match tls {
        Some((listener, acceptor)) => {
            let shared = Arc::clone(&tls_shared);
            let permits = Arc::clone(&tls_permits);
            Some(tokio::spawn(async move {
                loop {
                    let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                        return;
                    };
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let _ = stream.set_nodelay(true);
                            let shared = Arc::clone(&shared);
                            let acceptor = acceptor.clone();
                            tokio::spawn(async move {
                                // A handshake gets what a request body's frame
                                // gets: `receive_timeout`.
                                let handshake = tokio::time::timeout(
                                    shared.config.receive_timeout,
                                    acceptor.accept(stream),
                                )
                                .await;
                                if let Ok(Ok(stream)) = handshake {
                                    connection(stream, shared).await;
                                }
                                drop(permit);
                            });
                        }
                        Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                    }
                }
            }))
        }
        None => None,
    };
    Ok(HouseHandle {
        addr,
        tls_addr,
        admin_addr,
        accept,
        admin,
        tls_accept,
        native_addr,
        native_tls_addr,
        native_accept,
        compaction,
        retention,
        pipes,
        queues,
        views,
        storage,
        query_log_task,
        usage,
        lake: handle_lake,
        metrics,
        query_log,
    })
}

/// The TLS listener's configuration for the native protocol: the same
/// certificate, without HTTP's ALPN (native clients offer none, or their own).
fn native_tls_config(http: &Arc<rustls::ServerConfig>) -> Arc<rustls::ServerConfig> {
    let mut config = rustls::ServerConfig::clone(http);
    config.alpn_protocols.clear();
    Arc::new(config)
}

async fn connection<S>(stream: S, shared: Arc<Shared>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let config = &shared.config;
    let io = TokioIo::new(IdleIo::new(stream, config.send_timeout));
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        // hyper's header timer runs whenever it waits for a head — between
        // requests too — so it is the keep-alive idle limit as well (N4): the
        // value `Keep-Alive: timeout=` advertises is the one enforced.
        .header_read_timeout(head_timeout(config))
        .max_headers(config.max_headers)
        .max_buf_size(config.max_head_bytes.max(8192))
        .half_close(false)
        .keep_alive(!config.keep_alive.is_zero());
    let service_shared = Arc::clone(&shared);
    let service = service_fn(move |request| {
        let shared = Arc::clone(&service_shared);
        async move { Ok::<_, Infallible>(handle(shared, request).await) }
    });
    // A clean end hands the socket back for a lingering close (review M3); a
    // failed one (a body aborted under Ruling 9) has already broken it on purpose.
    if let Ok(parts) = builder
        .serve_connection(io, service)
        .without_shutdown()
        .await
    {
        let mut stream = parts.io.into_inner().stream;
        let _ = tokio::time::timeout(LINGER, async {
            let _ = stream.shutdown().await;
            let mut sink = [0u8; 8192];
            let mut read = 0usize;
            while read < 64 * 1024 {
                match stream.read(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => read += n,
                }
            }
        })
        .await;
    }
}

/// How long a connection may wait for a request head, idle time included: the
/// advertised keep-alive, or `receive_timeout` when keep-alive is off.
fn head_timeout(config: &HouseConfig) -> Duration {
    if config.keep_alive.is_zero() {
        config.receive_timeout
    } else {
        config.keep_alive
    }
}

/// A socket whose writes time out when they stall (review I1): a client that stops
/// reading a response is cut off rather than holding a worker. Reads have no timer
/// here (fix round 2, N1): hyper keeps a read pending for the whole exchange
/// (`half_close(false)` watches for the client going away), so a read timer would
/// fire during any statement longer than it. The client's silence is bounded where
/// it matters instead: each request-body frame ([`RequestBody::next`],
/// `receive_timeout`) and each head and idle gap (hyper's header timer,
/// `keep_alive`).
#[derive(Debug)]
struct IdleIo<S> {
    stream: S,
    write_idle: Duration,
    write_deadline: Option<Pin<Box<Sleep>>>,
}

impl<S> IdleIo<S> {
    fn new(stream: S, write_idle: Duration) -> Self {
        Self {
            stream,
            write_idle,
            write_deadline: None,
        }
    }

    fn timed_out(
        deadline: &mut Option<Pin<Box<Sleep>>>,
        idle: Duration,
        cx: &mut Context<'_>,
    ) -> Poll<io::Error> {
        let sleep = deadline.get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("the client read nothing for {idle:?}"),
            )),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for IdleIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for IdleIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_write(cx, buf) {
            Poll::Ready(result) => {
                this.write_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => match Self::timed_out(&mut this.write_deadline, this.write_idle, cx) {
                Poll::Ready(err) => Poll::Ready(Err(err)),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_flush(cx) {
            Poll::Ready(result) => {
                this.write_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => match Self::timed_out(&mut this.write_deadline, this.write_idle, cx) {
                Poll::Ready(err) => Poll::Ready(Err(err)),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// A response body: whole, or streamed from the statement through a channel.
#[derive(Debug)]
pub struct HouseBody {
    kind: BodyKind,
}

#[derive(Debug)]
enum BodyKind {
    Full(Option<Bytes>),
    Stream {
        rx: mpsc::Receiver<Piece>,
        len: Option<u64>,
        abort: Option<Pin<Box<Sleep>>>,
        /// A `Failed` piece's text went out: abort next.
        failing: bool,
    },
    /// A `loams.house.v1` response, as connect-rust writes it.
    Connect(ConnectBody),
}

/// connect-rust's response body, which has no `Debug` of its own.
struct ConnectBody(ConnectRpcBody);

impl std::fmt::Debug for ConnectBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectBody")
    }
}

/// A piece of a streamed body.
#[derive(Debug)]
enum Piece {
    Data(Bytes),
    /// The statement failed after the head went out: end the body with an error,
    /// so hyper closes the connection without the terminating chunk (Ruling 9).
    Abort,
    /// The statement failed after the head went out (Ruling 9): its exception
    /// text, which HTTP sends before it aborts, and the error, which the native
    /// protocol sends as an `Exception` packet.
    Failed {
        text: Bytes,
        error: HouseError,
    },
}

impl HouseBody {
    fn full(bytes: impl Into<Bytes>) -> Self {
        Self {
            kind: BodyKind::Full(Some(bytes.into())),
        }
    }

    fn connect(body: ConnectRpcBody) -> Self {
        Self {
            kind: BodyKind::Connect(ConnectBody(body)),
        }
    }

    fn stream(rx: mpsc::Receiver<Piece>, len: Option<u64>) -> Self {
        Self {
            kind: BodyKind::Stream {
                rx,
                len,
                abort: None,
                failing: false,
            },
        }
    }
}

/// What a statement's response body holds, piece by piece, for the native
/// protocol (which reads the body rather than sending it).
#[derive(Debug)]
pub(crate) enum BodyPiece {
    /// Result bytes.
    Data(Bytes),
    /// The statement failed after its result started.
    Failed(HouseError),
    /// The body broke off without an error of the statement's (a spool's read).
    Broken,
}

impl HouseBody {
    /// The next piece, `None` at the end.
    pub(crate) async fn next_piece(&mut self) -> Option<BodyPiece> {
        match &mut self.kind {
            BodyKind::Full(bytes) => bytes.take().map(BodyPiece::Data),
            BodyKind::Stream { rx, failing, .. } => {
                if *failing {
                    return None;
                }
                match rx.recv().await? {
                    Piece::Data(bytes) => Some(BodyPiece::Data(bytes)),
                    Piece::Failed { error, .. } => {
                        *failing = true;
                        Some(BodyPiece::Failed(error))
                    }
                    Piece::Abort => {
                        *failing = true;
                        Some(BodyPiece::Broken)
                    }
                }
            }
            BodyKind::Connect(_) => Some(BodyPiece::Broken),
        }
    }
}

/// The statement's error, on a response that answers one: what the native
/// protocol sends as its `Exception` packet (the HTTP body carries its text).
#[derive(Debug, Clone)]
pub(crate) struct StatementError(pub(crate) HouseError);

/// What a statement from the native protocol asks of the shared path: its
/// result as `Native` blocks whatever the statement's `FORMAT` says, since the
/// protocol carries blocks and the client formats them.
#[derive(Debug, Clone)]
pub(crate) struct ForcedFormat(pub(crate) String);

impl HttpBody for HouseBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame<Bytes>, io::Error>>> {
        match &mut self.get_mut().kind {
            BodyKind::Full(bytes) => Poll::Ready(bytes.take().map(|b| Ok(BodyFrame::data(b)))),
            BodyKind::Stream {
                rx, abort, failing, ..
            } => {
                if let Some(sleep) = abort {
                    ready!(sleep.as_mut().poll(cx));
                    return Poll::Ready(Some(Err(io::Error::other(
                        "the statement failed after the response started (Ruling 9)",
                    ))));
                }
                let next = if *failing {
                    Some(Piece::Abort)
                } else {
                    ready!(rx.poll_recv(cx))
                };
                match next {
                    Some(Piece::Data(bytes)) => Poll::Ready(Some(Ok(BodyFrame::data(bytes)))),
                    Some(Piece::Failed { text, .. }) => {
                        *failing = true;
                        Poll::Ready(Some(Ok(BodyFrame::data(text))))
                    }
                    Some(Piece::Abort) => {
                        // A pending poll lets hyper flush the exception text first.
                        let mut sleep = Box::pin(tokio::time::sleep(FLUSH_BOUND));
                        let _ = sleep.as_mut().poll(cx);
                        *abort = Some(sleep);
                        Poll::Pending
                    }
                    None => Poll::Ready(None),
                }
            }
            BodyKind::Connect(body) => Pin::new(&mut body.0)
                .poll_frame(cx)
                .map(|frame| frame.map(|frame| frame.map_err(|never| match never {}))),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            BodyKind::Full(None) => true,
            BodyKind::Connect(body) => body.0.is_end_stream(),
            _ => false,
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            BodyKind::Full(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            BodyKind::Full(None) => SizeHint::with_exact(0),
            BodyKind::Stream { len: Some(len), .. } => SizeHint::with_exact(*len),
            BodyKind::Stream { .. } => SizeHint::default(),
            BodyKind::Connect(body) => body.0.size_hint(),
        }
    }
}

/// A Connect request's body: hyper's, failing when the client sends nothing for
/// `receive_timeout` between frames (as [`RequestBody::next`] does for statements).
#[derive(Debug)]
struct TimedBody {
    body: Incoming,
    idle: Duration,
    timer: Option<Pin<Box<Sleep>>>,
}

impl HttpBody for TimedBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame<Bytes>, io::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.body).poll_frame(cx) {
            Poll::Ready(frame) => {
                this.timer = None;
                Poll::Ready(frame.map(|frame| frame.map_err(io::Error::other)))
            }
            Poll::Pending => {
                let idle = this.idle;
                let timer = this
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
                ready!(timer.as_mut().poll(cx));
                Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the client sent nothing for {idle:?}"),
                ))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// A `loams.house.v1` request, answered by connect-rust over [`crate::api`].
async fn connect(shared: &Arc<Shared>, request: Request<Incoming>) -> Response<HouseBody> {
    let idle = shared.config.receive_timeout;
    let request = request.map(|body| TimedBody {
        body,
        idle,
        timer: None,
    });
    let mut service = shared.api.0.clone();
    match tower::Service::call(&mut service, request).await {
        Ok(response) => response.map(HouseBody::connect),
        Err(never) => match never {},
    }
}

/// The request body, read a data frame at a time.
pub(crate) struct RequestBody {
    body: BodySource,
    done: bool,
    idle: Duration,
}

/// Where a statement's body comes from.
#[derive(Debug)]
enum BodySource {
    /// An HTTP request's.
    Http(Incoming),
    /// The native protocol's `Data` blocks, as a `Native` format body; an error
    /// is a body that broke off.
    Channel(mpsc::Receiver<Result<Bytes, HouseError>>),
}

impl RequestBody {
    fn new(body: Incoming, idle: Duration) -> Self {
        let done = body.is_end_stream();
        Self {
            body: BodySource::Http(body),
            done,
            idle,
        }
    }

    /// A statement with no body.
    pub(crate) fn empty() -> Self {
        let (_, rx) = mpsc::channel(1);
        Self {
            body: BodySource::Channel(rx),
            done: true,
            idle: Duration::from_secs(1),
        }
    }

    /// A body fed through a channel (an `INSERT`'s data over the native
    /// protocol), each piece within `idle` of the last.
    pub(crate) fn channel(rx: mpsc::Receiver<Result<Bytes, HouseError>>, idle: Duration) -> Self {
        Self {
            body: BodySource::Channel(rx),
            done: false,
            idle,
        }
    }

    /// The next data, or `None` at the end. A read that fails (broken framing) or a
    /// client silent for `receive_timeout` between frames is `210`.
    async fn next(&mut self) -> Result<Option<Bytes>, HouseError> {
        while !self.done {
            let body = match &mut self.body {
                BodySource::Http(body) => body,
                BodySource::Channel(rx) => {
                    let Ok(piece) = tokio::time::timeout(self.idle, rx.recv()).await else {
                        self.done = true;
                        return Err(HouseError::from(ChError::network_error(format!(
                            "Cannot read the INSERT's data: the client sent nothing for {:?}",
                            self.idle
                        ))));
                    };
                    match piece {
                        Some(Ok(data)) if !data.is_empty() => return Ok(Some(data)),
                        Some(Ok(_)) => continue,
                        Some(Err(err)) => {
                            self.done = true;
                            return Err(err);
                        }
                        None => {
                            self.done = true;
                            continue;
                        }
                    }
                }
            };
            let Ok(frame) = tokio::time::timeout(self.idle, body.frame()).await else {
                self.done = true;
                return Err(HouseError::from(ChError::network_error(format!(
                    "Cannot read the request body: the client sent nothing for {:?}",
                    self.idle
                ))));
            };
            match frame {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data()
                        && !data.is_empty()
                    {
                        return Ok(Some(data));
                    }
                }
                Some(Err(err)) => {
                    self.done = true;
                    return Err(HouseError::from(ChError::network_error(format!(
                        "Cannot read the request body: {err}"
                    ))));
                }
                None => self.done = true,
            }
        }
        Ok(None)
    }

    /// Reads and discards up to [`MAX_DRAIN`]; whether the body is now fully read.
    async fn drain(&mut self) -> bool {
        let mut drained = 0u64;
        loop {
            match self.next().await {
                Ok(Some(piece)) => {
                    drained += piece.len() as u64;
                    if drained > MAX_DRAIN {
                        return false;
                    }
                }
                Ok(None) => return true,
                Err(_) => return false,
            }
        }
    }
}

/// What every response of a statement carries.
#[derive(Debug)]
struct Meta {
    query_id: String,
    format: String,
    timezone: String,
    encoding: Option<Encoding>,
    keep_alive: Duration,
    version: String,
    send_progress: bool,
    /// `X-Loams-Session-Affinity`, for a request in a session (HS1 Task 4).
    affinity: Option<String>,
    /// `X-Loams-Worker`, once a worker ran the statement (Shared contracts).
    worker: Option<String>,
    /// `X-Loams-Snapshot-Id` after a lake write (`table=id`, HS1 Task 12).
    snapshot: Option<String>,
    /// The statement's audit record (HS1 Task 20), written once, when the
    /// response's head is built.
    audit: Option<Arc<Audit>>,
    /// Its admission and its process-list entry (HS1 Task 21), held until the
    /// response ends.
    held: Held,
    /// Its query log row, metrics, operator line and trace (HS1 Task 23),
    /// ended when the response does.
    observed: Option<Arc<crate::querylog::Statement>>,
}

/// What a statement holds while it runs (HS1 Task 21).
#[derive(Debug, Default)]
struct Held {
    ticket: Option<crate::admission::Ticket>,
    process: Option<crate::watchdog::ProcessGuard>,
}

/// A statement's audit record on its way to the sink.
#[derive(Debug)]
struct Audit {
    sink: Arc<dyn crate::audit::AuditSink>,
    event: std::sync::Mutex<Option<crate::audit::AuditEvent>>,
}

impl Audit {
    fn new(sink: &Arc<dyn crate::audit::AuditSink>, query_id: &str) -> Self {
        Self {
            sink: Arc::clone(sink),
            event: std::sync::Mutex::new(Some(crate::audit::AuditEvent::now("http", query_id))),
        }
    }

    fn update(&self, change: impl FnOnce(&mut crate::audit::AuditEvent)) {
        if let Some(event) = self
            .event
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            change(event);
        }
    }

    fn finish(&self, code: i32) {
        let taken = self.event.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut event) = taken {
            event.code = code;
            self.sink.record(&event);
        }
    }
}

impl Meta {
    /// The head, in ClickHouse's order as far as hyper keeps it: hyper adds `Date`,
    /// `Connection` and the framing itself.
    fn head(
        &self,
        status: StatusCode,
        content_type: &str,
        code: Option<i32>,
        last: &Progress,
        close: bool,
    ) -> http::response::Builder {
        if let Some(audit) = &self.audit {
            audit.finish(code.unwrap_or(0));
        }
        if let Some(observed) = &self.observed {
            observed.progress(last);
            if let Some(code) = code {
                observed.fail(code, "");
            }
        }
        let mut builder = Response::builder()
            .status(status)
            .header("Content-Type", content_type)
            .header("X-ClickHouse-Server-Display-Name", DISPLAY_NAME);
        if let Some(encoding) = self.encoding.filter(|e| e.is_http()) {
            builder = builder.header("Content-Encoding", encoding.as_str());
        }
        builder = builder
            .header("X-ClickHouse-Query-Id", header_value(&self.query_id))
            .header("X-ClickHouse-Format", header_value(&self.format))
            .header("X-ClickHouse-Timezone", header_value(&self.timezone));
        if let Some(code) = code {
            builder = builder.header("X-ClickHouse-Exception-Code", code);
        }
        if let Some(affinity) = &self.affinity {
            builder = builder.header("X-Loams-Session-Affinity", header_value(affinity));
        }
        if let Some(worker) = &self.worker {
            builder = builder.header("X-Loams-Worker", header_value(worker));
        }
        if let Some(snapshot) = &self.snapshot {
            builder = builder.header("X-Loams-Snapshot-Id", header_value(snapshot));
        }
        if close {
            builder = builder.header("Connection", "close");
        } else if !self.keep_alive.is_zero() {
            builder = builder.header(
                "Keep-Alive",
                format!("timeout={}", self.keep_alive.as_secs().max(1)),
            );
        }
        if self.send_progress {
            builder = builder.header("X-ClickHouse-Progress", progress_json(last));
        }
        builder.header("X-ClickHouse-Summary", progress_json(last))
    }

    fn encode_whole(&self, bytes: &[u8]) -> Vec<u8> {
        // An error under `compress=1` goes out as text, as clickhouse-server
        // sends one (clients read it either way).
        match self
            .encoding
            .filter(|e| e.is_http())
            .and_then(|e| Encoder::new(e).ok())
        {
            Some(mut encoder) => {
                let mut out = encoder.feed(bytes).unwrap_or_default();
                out.extend(encoder.finish().unwrap_or_default());
                out
            }
            None => bytes.to_vec(),
        }
    }

    /// A whole error response (nothing of the result went out).
    fn error(&self, error: &HouseError, last: &Progress, close: bool) -> Response<HouseBody> {
        if let Some(observed) = &self.observed {
            observed.fail(error.code(), error.message());
        }
        let body = self.encode_whole(error.render(&self.version).as_bytes());
        let status =
            StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = self
            .head(
                status,
                "text/plain; charset=UTF-8",
                Some(error.code()),
                last,
                close,
            )
            .body(HouseBody::full(body))
            .unwrap_or_else(|_| fallback());
        response
            .extensions_mut()
            .insert(StatementError(error.clone()));
        response
    }
}

/// An empty success (`SET`, `USE`, DDL).
fn empty_ok(meta: &Meta) -> Response<HouseBody> {
    meta.head(
        StatusCode::OK,
        content_type(&meta.format),
        None,
        &Progress::default(),
        false,
    )
    .body(HouseBody::full(Vec::new()))
    .unwrap_or_else(|_| fallback())
}

/// How long `KILL QUERY … SYNC` waits for the statement to be gone.
const KILL_SYNC_WAIT: Duration = Duration::from_secs(10);

/// `KILL QUERY WHERE query_id = '…'` (§49 §12, HS1 Task 21): kills the
/// statement's worker and answers at once with ClickHouse's columns
/// (`kill_status`, `query_id`, `user`, `query`); `SYNC` waits for it to be gone.
/// Only the caller's namespace is seen, and another user's statement needs
/// `house:admin` (`497`). No match is an empty result, as in ClickHouse.
async fn kill_query(
    shared: &Arc<Shared>,
    meta: &Meta,
    user: &UserMap,
    kill: &crate::classify::KillQuery,
) -> Response<HouseBody> {
    use crate::classify::KillMode;
    use crate::watchdog::KillRefusal;
    let Some(layout) = KillLayout::of(&meta.format) else {
        let err = HouseError::from(ChError::not_implemented(format!(
            "KILL QUERY answers in TabSeparated, TSV, CSV or JSONEachRow (with names and \
             types where they have them), not {}",
            meta.format
        )));
        return meta.error(&err, &Progress::default(), false);
    };
    let killed = shared
        .processes
        .kill(&kill.query_id, user, kill.mode == KillMode::Test);
    let rows = match killed {
        Ok(mut killed) => {
            if kill.mode == KillMode::Test {
                // ClickHouse's status for a row it did not try to kill.
                killed.status = "unknown_status";
            } else if kill.mode == KillMode::Sync && killed.status == "waiting" {
                let gone = tokio::time::timeout(KILL_SYNC_WAIT, async {
                    while shared.processes.is_running(&kill.query_id) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
                if gone.is_ok() {
                    killed.status = "finished";
                }
            }
            vec![killed]
        }
        Err(KillRefusal::NotFound) => Vec::new(),
        Err(KillRefusal::Denied) => {
            let err = HouseError::from(ChError::access_denied(format!(
                "{}: Not enough privileges to kill query {}: it is another user's, which needs \
                 house:admin",
                user.user, kill.query_id
            )));
            return meta.error(&err, &Progress::default(), false);
        }
    };
    let body = layout.render(&rows);
    meta.head(
        StatusCode::OK,
        content_type(&meta.format),
        None,
        &Progress::default(),
        false,
    )
    .body(HouseBody::full(meta.encode_whole(&body)))
    .unwrap_or_else(|_| fallback())
}

/// The formats `KILL QUERY` answers in, rendered by the front (no worker is
/// asked: a namespace at its limits must still be able to kill).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KillLayout {
    Tsv { names: bool, types: bool },
    Csv { names: bool },
    JsonEachRow,
}

impl KillLayout {
    const COLUMNS: [&'static str; 4] = ["kill_status", "query_id", "user", "query"];

    fn of(format: &str) -> Option<Self> {
        Some(match format {
            "TabSeparated" | "TSV" => Self::Tsv {
                names: false,
                types: false,
            },
            "TabSeparatedWithNames" | "TSVWithNames" => Self::Tsv {
                names: true,
                types: false,
            },
            "TabSeparatedWithNamesAndTypes" | "TSVWithNamesAndTypes" => Self::Tsv {
                names: true,
                types: true,
            },
            "CSV" => Self::Csv { names: false },
            "CSVWithNames" => Self::Csv { names: true },
            "JSONEachRow" => Self::JsonEachRow,
            _ => return None,
        })
    }

    fn render(self, rows: &[crate::watchdog::Killed]) -> Vec<u8> {
        let cells = |k: &crate::watchdog::Killed| {
            [
                k.status.to_string(),
                k.query_id.clone(),
                k.user.clone(),
                k.query.clone(),
            ]
        };
        let mut out = String::new();
        match self {
            Self::Tsv { names, types } => {
                let line = |out: &mut String, cells: &[String]| {
                    let escaped: Vec<String> = cells.iter().map(|c| tsv_escape(c)).collect();
                    out.push_str(&escaped.join("\t"));
                    out.push('\n');
                };
                if names {
                    line(&mut out, &Self::COLUMNS.map(str::to_string));
                }
                if types {
                    line(&mut out, &["String"; 4].map(str::to_string));
                }
                for row in rows {
                    line(&mut out, &cells(row));
                }
            }
            Self::Csv { names } => {
                let line = |out: &mut String, cells: &[String]| {
                    let quoted: Vec<String> = cells
                        .iter()
                        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
                        .collect();
                    out.push_str(&quoted.join(","));
                    out.push('\n');
                };
                if names {
                    line(&mut out, &Self::COLUMNS.map(str::to_string));
                }
                for row in rows {
                    line(&mut out, &cells(row));
                }
            }
            Self::JsonEachRow => {
                for row in rows {
                    let object: serde_json::Map<String, serde_json::Value> = Self::COLUMNS
                        .iter()
                        .zip(cells(row))
                        .map(|(k, v)| ((*k).to_string(), serde_json::Value::String(v)))
                        .collect();
                    out.push_str(&serde_json::Value::Object(object).to_string());
                    out.push('\n');
                }
            }
        }
        out.into_bytes()
    }
}

/// A `String` as ClickHouse's `TabSeparated` writes it, which is also how a
/// `String` query parameter's value is read (HE1 R14.6).
pub(crate) fn tsv_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\'' => out.push_str("\\'"),
            other => out.push(other),
        }
    }
    out
}

fn fallback() -> Response<HouseBody> {
    let mut response = Response::new(HouseBody::full(Bytes::from_static(b"internal error\n")));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response
}

fn plain(status: StatusCode, text: &str, keep_alive: Duration) -> Response<HouseBody> {
    let mut builder = Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=UTF-8")
        .header("X-ClickHouse-Server-Display-Name", DISPLAY_NAME);
    if !keep_alive.is_zero() {
        builder = builder.header(
            "Keep-Alive",
            format!("timeout={}", keep_alive.as_secs().max(1)),
        );
    }
    builder
        .body(HouseBody::full(text.as_bytes().to_vec()))
        .unwrap_or_else(|_| fallback())
}

/// A header value without what a header cannot hold.
fn header_value(value: &str) -> HeaderValue {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    HeaderValue::from_str(&cleaned).unwrap_or_else(|_| HeaderValue::from_static(""))
}

async fn handle(shared: Arc<Shared>, request: Request<Incoming>) -> Response<HouseBody> {
    let keep = shared.config.keep_alive;
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    if path.starts_with("/loams.house.v1.") {
        return connect(&shared, request).await;
    }
    let query = request.uri().query().unwrap_or("").to_string();
    let params = match request::parse_query(&query) {
        Ok(params) => params,
        Err(err) => {
            let meta = Meta {
                query_id: uuid::Uuid::new_v4().to_string(),
                format: "TabSeparated".to_string(),
                timezone: "UTC".to_string(),
                encoding: None,
                keep_alive: keep,
                version: shared.config.version.clone(),
                send_progress: false,
                affinity: None,
                worker: None,
                snapshot: None,
                audit: None,
                held: Held::default(),
                observed: None,
            };
            return meta.error(&err, &Progress::default(), true);
        }
    };
    let has_query = params.iter().any(|(k, _)| k == "query");
    let reading = method == Method::GET || method == Method::HEAD;
    match path.as_str() {
        "/ping" | "/" if reading && (path == "/ping" || !has_query) => {
            plain(StatusCode::OK, "Ok.\n", keep)
        }
        "/" if reading || method == Method::POST => {
            // The statement's query log row, metrics, operator line and trace
            // (HS1 Task 23); its work runs under its root span.
            let query_id = params
                .iter()
                .find(|(k, v)| k == "query_id" && !v.is_empty())
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let parent = request
                .headers()
                .get("traceparent")
                .and_then(|v| v.to_str().ok())
                .and_then(crate::trace::SpanContext::from_traceparent);
            let observed = shared.observer.begin("http", &query_id, parent);
            let current = observed.current();
            let (parts, body) = request.into_parts();
            let body = RequestBody::new(body, shared.config.receive_timeout);
            crate::trace::scope(
                current,
                run(&shared, parts, body, params, query_id, observed),
            )
            .await
        }
        "/" | "/ping" => plain(StatusCode::METHOD_NOT_ALLOWED, "Use GET or POST.\n", keep),
        _ => plain(
            StatusCode::NOT_FOUND,
            &format!(
                "There is no handle {path}\n\nUse / or /ping for health checks.\nSend queries with POST or GET /?query=...\n"
            ),
            keep,
        ),
    }
}

/// One statement from the native protocol (`crate::native`), on the same path
/// as an HTTP request's: `parts` carries the connection's credentials as
/// `X-ClickHouse-User`/`X-ClickHouse-Key` and `params` the statement, its
/// settings and parameters as HTTP's query string would.
pub(crate) async fn run_native(
    shared: &Arc<Shared>,
    parts: http::request::Parts,
    body: RequestBody,
    params: Vec<(String, String)>,
    query_id: String,
    parent: Option<crate::trace::SpanContext>,
) -> Response<HouseBody> {
    let observed = shared.observer.begin("native", &query_id, parent);
    let current = observed.current();
    crate::trace::scope(
        current,
        run(shared, parts, body, params, query_id, observed),
    )
    .await
}

/// A statement.
async fn run(
    shared: &Arc<Shared>,
    parts: http::request::Parts,
    mut body: RequestBody,
    params: Vec<(String, String)>,
    query_id: String,
    observed: Arc<crate::querylog::Statement>,
) -> Response<HouseBody> {
    let config = &shared.config;
    let get = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
    let expects_continue = header("Expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue"));

    let mut settings: Vec<(String, String)> = Vec::new();
    let mut query_params = Vec::new();
    for (key, value) in &params {
        if let Some(name) = key.strip_prefix("param_") {
            query_params.push((name.to_string(), value.clone()));
        } else if !NON_SETTINGS.contains(&key.as_str()) {
            settings.push((key.clone(), value.clone()));
        }
    }
    let setting = |name: &str| {
        settings
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let mut meta = Meta {
        query_id,
        format: get("default_format").unwrap_or("TabSeparated").to_string(),
        timezone: setting("session_timezone").unwrap_or_else(|| "UTC".to_string()),
        // HE1 R14.14: `compress=1` is ClickHouse's own framing, which wins over
        // an HTTP coding.
        encoding: if get("compress") == Some("1") {
            Some(compress::Encoding::ClickHouse)
        } else {
            (get("enable_http_compression") == Some("1"))
                .then(|| compress::accepted(header("Accept-Encoding")))
                .flatten()
        },
        keep_alive: config.keep_alive,
        version: config.version.clone(),
        send_progress: get("send_progress_in_http_headers") == Some("1"),
        affinity: None,
        worker: None,
        snapshot: None,
        audit: None,
        held: Held::default(),
        observed: Some(Arc::clone(&observed)),
    };
    // Every statement's audit record (HS1 Task 20): the claimed user and
    // principal now, the signed-in identity once known.
    if let Some(sink) = &config.audit {
        let audit = Audit::new(sink, &meta.query_id);
        let pairs: Vec<(String, String)> = parts
            .headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_string(), v.to_string()))
            })
            .collect();
        audit.update(|event| {
            event.user = auth::credentials(&pairs, &params)
                .map(|c| c.user)
                .unwrap_or_default();
            event.principal = pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(crate::identity::PRINCIPAL_HEADER))
                .map(|(_, v)| v.clone());
        });
        meta.audit = Some(Arc::new(audit));
    }
    // Everything before a worker: who, what, and how.
    let known = shared.pool.known_settings();
    let prepared = prepare(
        config,
        &parts.headers,
        &params,
        &mut body,
        parts.method == Method::POST,
        &known,
        shared.lake.is_some(),
        meta.audit.as_deref(),
        &observed,
    )
    .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            // Do not read the body of a request still waiting for `100 Continue`:
            // the client then never sends it (review M4). Otherwise drain a little,
            // so the answer is read before the connection closes.
            let close = expects_continue || !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    if let Some(format) = &prepared.format {
        meta.format = format.clone();
    }
    if let Some(ForcedFormat(format)) = parts.extensions.get::<ForcedFormat>() {
        meta.format = format.clone();
    }
    let Prepared {
        user,
        sql,
        classified,
        readonly,
        mut input,
        mut decoder,
        first_data,
        ..
    } = prepared;
    let namespace = user.namespace.to_string();

    // `KILL QUERY` (HS1 Task 21): the front's process list, before any queue.
    if let Classified::Known {
        stmt: Stmt::KillQuery(kill),
        ..
    } = &classified
    {
        return kill_query(shared, &meta, &user, kill).await;
    }

    // The host functions answer the House's values, and what runs is what the
    // worker explains ([`gate::statement_text`]).
    let sql = gate::statement_text(&sql, &user.user);
    if let Some(spec) = input.as_mut() {
        spec.insert = deny::rewrite_host_functions(&spec.insert, &gate::host_values(&user.user));
    }

    // The session, for this statement only: 372, 373, 202 or 36 when it cannot be.
    let mut session = match SessionParams::from_params(&params) {
        Ok(Some(session_params)) => {
            match shared
                .sessions
                .checkout(&user.user, user.namespace, &session_params)
            {
                Ok(guard) => Some(guard),
                Err(err) => {
                    let close = !body.drain().await;
                    return meta.error(&err, &Progress::default(), close);
                }
            }
        }
        Ok(None) => None,
        Err(err) => {
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    meta.affinity = session.as_ref().map(SessionGuard::affinity_key);

    // The process list and admission (§49 §12, HS1 Task 21), for everything but
    // what the front answers from the session alone.
    let front_only = matches!(
        &classified,
        Classified::Known {
            stmt: Stmt::Set(_) | Stmt::Use(_),
            ..
        }
    );
    if !front_only {
        match shared.processes.register(&meta.query_id, &user, &sql) {
            Ok(guard) => meta.held.process = Some(guard),
            Err(err) => {
                let close = !body.drain().await;
                return meta.error(&err, &Progress::default(), close);
            }
        }
        let mut all: Vec<(String, String)> = session
            .as_ref()
            .map(|guard| guard.state().settings.pairs().to_vec())
            .unwrap_or_default();
        all.extend(settings.iter().cloned());
        let memory = crate::limits::statement_memory(&all, &config.session_limits);
        let access = match &classified {
            Classified::Known { stmt, .. } => crate::quota::Access::of(stmt),
            Classified::Unparsed { .. } => crate::quota::Access::Read,
        };
        match shared.admission.admit(&namespace, memory, access).await {
            Ok(ticket) => meta.held.ticket = Some(ticket),
            Err(err) => {
                let close = !body.drain().await;
                return meta.error(&err, &Progress::default(), close);
            }
        }
    }

    // The catalog's statements (HS1 Task 10): DDL and `USE` are answered here;
    // `DESCRIBE`, `SHOW CREATE` and `EXISTS` of a catalog table become a read of
    // their rows; everything else goes on.
    let current_database = get("database")
        .map(str::to_string)
        .or_else(|| session.as_ref().map(|g| g.state().database.clone()))
        .unwrap_or_else(|| "default".to_string());
    observed.update(|e| e.database = current_database.clone());
    // `SHOW TABLES`, `SHOW DATABASES`, `SHOW PROCESSLIST`: ClickHouse's own
    // `SELECT` over the House's `system` tables (HS1 Task 11).
    let shown = match &classified {
        Classified::Known { stmt, format } => gate::show_answer(
            shared.lake.as_deref(),
            user.namespace,
            &current_database,
            stmt,
        )
        .await
        .map(|query| query.map(|q| (q, format.clone()))),
        Classified::Unparsed { .. } => Ok(None),
    };
    let (classified, sql) = match shown {
        Ok(Some((query, format))) => (
            Classified::Known {
                stmt: Stmt::Query {
                    text: query.clone(),
                },
                format,
            },
            query,
        ),
        Ok(None) => (classified, sql),
        Err(err) => {
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    let (classified, sql) = if let Classified::Known { stmt, format } = &classified {
        let answered = match &shared.lake {
            Some(lake) if get("database").is_some() => lake
                .require_database(user.namespace, &current_database)
                .await
                .map(|()| Some(lake)),
            lake => Ok(lake.as_ref()),
        };
        let outcome = match answered {
            Ok(lake) => {
                gate::catalog_answer(
                    lake.map(Arc::as_ref),
                    &shared.pool,
                    &user,
                    &current_database,
                    stmt,
                    &sql,
                    session.as_ref(),
                    &meta.query_id,
                )
                .await
            }
            Err(err) => Err(err),
        };
        match outcome {
            Ok(gate::CatalogOutcome::Done) => {
                drop(session);
                return empty_ok(&meta);
            }
            Ok(gate::CatalogOutcome::Rows(query)) => (
                Classified::Known {
                    stmt: Stmt::Query {
                        text: query.clone(),
                    },
                    format: format.clone(),
                },
                query,
            ),
            Ok(gate::CatalogOutcome::NotOurs) => (classified, sql),
            Ok(
                outcome @ (gate::CatalogOutcome::Insert(_)
                | gate::CatalogOutcome::CreateAs(_)
                | gate::CatalogOutcome::Optimize(_)
                | gate::CatalogOutcome::Mutate(_)),
            ) => {
                let mut all: Vec<(String, String)> = session
                    .as_ref()
                    .map(|guard| guard.state().settings.pairs().to_vec())
                    .unwrap_or_default();
                all.extend(settings.iter().cloned());
                let job = match outcome {
                    gate::CatalogOutcome::Insert(target) => LakeJob::Insert(target),
                    gate::CatalogOutcome::CreateAs(plan) => LakeJob::CreateAs(plan),
                    gate::CatalogOutcome::Optimize(plan) => LakeJob::Optimize(plan),
                    gate::CatalogOutcome::Mutate(plan) => LakeJob::Mutate(plan),
                    _ => unreachable!("matched above"),
                };
                let write = LakeWrite {
                    job,
                    database: current_database,
                    params: query_params,
                    settings: all,
                };
                // The front carries a lake write to its commit (HS1 Task 12).
                if let Some(process) = &meta.held.process {
                    process.uncancellable();
                }
                let response = lake_insert(
                    shared,
                    &mut meta,
                    &user,
                    write,
                    &mut body,
                    &mut decoder,
                    first_data,
                )
                .await;
                drop(session);
                return response;
            }
            Err(err) => {
                let close = !body.drain().await;
                return meta.error(&err, &Progress::default(), close);
            }
        }
    } else {
        (classified, sql)
    };

    // What the front answers itself, and what runs. Every statement chDB gets is
    // gated by ClickHouse's class below (`expect`, and sqlparser's message for a
    // form it could not parse).
    let (expect, unparsed, pins) = match classified {
        Classified::Known { stmt, .. } => {
            match gate::front_answer(&stmt, session.as_ref(), &config.session_limits, &known) {
                Some(Ok(())) => {
                    drop(session);
                    return empty_ok(&meta);
                }
                Some(Err(err)) => {
                    let close = !body.drain().await;
                    return meta.error(&err, &Progress::default(), close);
                }
                None => (
                    // What reaches here runs in chDB: a read or an owned write.
                    stmt.expect().unwrap_or(Expect::Read),
                    None,
                    matches!(stmt, Stmt::CreateTemporaryTable { .. }),
                ),
            }
        }
        Classified::Unparsed { message, .. } => (Expect::Read, Some(message), false),
    };

    let buffer_size = get("buffer_size")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(config.default_buffer_size)
        .clamp(*BUFFER_SIZE_RANGE.start(), *BUFFER_SIZE_RANGE.end());
    let wait_end = get("wait_end_of_query") == Some("1");

    // An INSERT takes a worker only once its data has started (review I1).
    if input.is_some() && first_data.is_empty() {
        match body.next().await {
            Ok(Some(piece)) => decoder.push(&piece),
            Ok(None) => decoder.end(),
            Err(err) => return meta.error(&err, &Progress::default(), true),
        }
    }

    // The worker: the session's pinned one, a newly pinned one for its first
    // temporary table, or any (HS1 Task 4).
    let pool = shared.pool.clone();
    let acquired = gate::acquire_for(&pool, &namespace, session.as_mut(), pins).await;
    let mut lease = match acquired {
        Ok(lease) => lease,
        Err(err) => {
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    meta.worker = Some(lease.worker_id().to_string());

    // ClickHouse's class gates every statement, on exactly the text chDB will
    // parse (an `INSERT` whose data streams: its head and format, as chDB joins
    // them), and decides what sqlparser could not parse (FL2 Ruling 6; fix round
    // 1, C1, I1, I2).
    let parsed_text = match &input {
        Some(spec) => format!("{} FORMAT {}", spec.insert, spec.format),
        None => sql.clone(),
    };
    let decided = match lease.analyze(&parsed_text, &query_params).await {
        // HE1 R14.5: a call of the namespace's own (parameterised) view is
        // allowed, as its name would be.
        Ok(analysis) => match gate::view_calls(
            shared.lake.as_deref(),
            user.namespace,
            &current_database,
            &analysis,
        )
        .await
        {
            Ok(views) => gate::admit_views(
                &analysis,
                &parsed_text,
                expect,
                readonly,
                unparsed.as_deref(),
                &views,
            )
            .map(|()| analysis),
            Err(err) => Err(err),
        },
        Err(err) => Err(deny::as_setting_refusal(err)),
    };
    let analysis = match decided {
        Ok(analysis) => analysis,
        Err(err) => {
            pool.release(lease, Outcome::Completed);
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };

    // Settings: the session's, then the request's (which win); Loams's own stay here.
    let mut settings = Vec::new();
    let mut worker_session = None;
    if let Some(guard) = &session {
        let state = guard.state();
        settings.extend(state.settings.for_engine());
        if state.pinned.is_some() {
            worker_session = Some(state.worker_ref(guard.closing()));
        }
    }
    settings.extend(
        params
            .iter()
            .filter(|(k, _)| !k.starts_with("param_") && !NON_SETTINGS.contains(&k.as_str()))
            .filter(|(k, _)| !crate::settings::is_front_only(k))
            .cloned(),
    );
    // What it reads (HS1 Task 11): its lake tables pinned and viewed, its
    // `system` tables the House's. Loams's own settings choose the snapshot.
    let streams_input = input.is_some();
    let (views, sql) = if input.is_some() {
        (Vec::new(), String::new())
    } else {
        let mut all: Vec<(String, String)> = session
            .as_ref()
            .map(|guard| guard.state().settings.pairs().to_vec())
            .unwrap_or_default();
        all.extend(
            params
                .iter()
                .filter(|(k, _)| !k.starts_with("param_") && !NON_SETTINGS.contains(&k.as_str()))
                .cloned(),
        );
        match gate::read_plan(
            shared.lake.as_deref(),
            user.namespace,
            &current_database,
            &user,
            &mut lease,
            &analysis,
            &sql,
            &all,
            None,
            Some(crate::read::Front {
                processes: &shared.processes,
                query_log: shared.observer.log.as_deref(),
            }),
        )
        .await
        {
            Ok(plan) => (plan.views, plan.sql),
            Err(err) => {
                pool.release(lease, Outcome::Completed);
                let close = !body.drain().await;
                return meta.error(&err, &Progress::default(), close);
            }
        }
    };
    let execute = Execute {
        query_id: meta.query_id.clone(),
        session: worker_session,
        settings,
        views,
        sql,
        format: meta.format.clone(),
        params: query_params,
        limits: Limits::default(),
        input,
        // One document per response, whatever the format and the blocks (R8.12);
        // a format that needs the whole result is held to the result cap.
        output: Output::Document {
            max_buffered_bytes: config.wait_end_of_query_max_bytes,
        },
        // An `INSERT` whose data streams reads no table (its target is a
        // temporary table or chDB refuses it), and its worker makes no view
        // databases for it: the connection's database stays.
        database: (!streams_input).then_some(current_database),
    };
    let has_input = execute.input.is_some();
    if let Err(err) = lease.start(execute).await {
        pool.release(lease, Outcome::Completed);
        let close = !body.drain().await;
        return meta.error(&err, &Progress::default(), close);
    }
    observed.started_on_worker();
    // `KILL QUERY` reaches it from now (one that came first kills it now).
    if let Some(process) = &meta.held.process {
        process.started(lease.kill_handle());
    }
    if has_input
        && let Err(err) = stream_input(&mut lease, &mut body, &mut decoder, first_data, None).await
    {
        // The worker is mid-`INSERT`: ending its input would commit a body that did
        // not arrive whole (review I5), so it is killed instead.
        pool.kill(lease, ExitReason::Cancel);
        return meta.error(&err, &Progress::default(), true);
    }

    respond(shared, meta, lease, wait_end, buffer_size, session).await
}

/// Everything a statement needs before a worker is asked.
struct Prepared {
    user: UserMap,
    /// The statement's text minus a trailing `FORMAT`: what chDB runs, unchanged.
    sql: String,
    classified: Classified,
    readonly: bool,
    format: Option<String>,
    input: Option<loams_house_ipc::InputSpec>,
    decoder: Decoder,
    first_data: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
async fn prepare(
    config: &HouseConfig,
    headers: &http::HeaderMap,
    params: &[(String, String)],
    body: &mut RequestBody,
    post: bool,
    known: &std::collections::HashSet<String>,
    has_lake: bool,
    audit: Option<&Audit>,
    observed: &crate::querylog::Statement,
) -> Result<Prepared, HouseError> {
    let get = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let header_pairs: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_string(), v.to_string()))
        })
        .collect();
    let user = auth::identify(config, &header_pairs, params)?;
    observed.identify(&user);

    // Without a catalog only `default` exists; with one, `run` asks it (HS1 Task
    // 10).
    if !has_lake && let Some(database) = get("database").filter(|d| *d != "default") {
        return Err(HouseError::from(ChError::unknown_database(format!(
            "Database {database} does not exist"
        ))));
    }
    // The request's settings: 115 unknown, 164 disallowed or over a cap (Task 4).
    for (name, value) in params
        .iter()
        .filter(|(k, _)| !k.starts_with("param_") && !NON_SETTINGS.contains(&k.as_str()))
    {
        crate::settings::check(name, value, &config.session_limits, known)?;
    }
    let encoding = compress::content_encoding(
        headers
            .get("Content-Encoding")
            .and_then(|v| v.to_str().ok()),
    )?;
    // HE1 R14.14: `decompress=1`, a body in ClickHouse's own framing.
    let encoding = match (get("decompress") == Some("1"), encoding) {
        (false, encoding) => encoding,
        (true, None) => Some(compress::Encoding::ClickHouse),
        (true, Some(http)) => {
            return Err(HouseError::from(ChError::not_implemented(format!(
                "decompress=1 with Content-Encoding: {} is not supported: send one of them",
                http.as_str()
            ))));
        }
    };
    let mut decoder = Decoder::new(encoding, config.body_limits)
        .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?;

    let readonly = !post || user.readonly;
    let plan = match get("query") {
        Some(query) => {
            let plan = request::plan(query.as_bytes(), true);
            if plan.input.is_none() && !body.done {
                // `?query=` and a body that is not `INSERT` data: the body continues
                // the statement, as in ClickHouse.
                let mut text = query.as_bytes().to_vec();
                text.push(b'\n');
                let text = read_statement(body, &mut decoder, text, config.max_query_size).await?;
                request::plan(&text, false)
            } else {
                plan
            }
        }
        None => {
            let text =
                read_statement(body, &mut decoder, Vec::new(), config.max_query_size).await?;
            request::plan(&text, false)
        }
    };
    observed.update(|e| {
        e.query = match &plan.input {
            Some(spec) => format!("{} FORMAT {}", spec.insert, spec.format),
            None => plan.sql.clone(),
        };
    });
    let classified = match &plan.input {
        Some(spec) => {
            // The head the body streams into gets the checks every statement gets.
            check_text(&spec.insert)?;
            Classified::Known {
                stmt: Stmt::Insert(crate::classify::InsertStmt {
                    text: spec.insert.clone(),
                    format: Some(spec.format.clone()),
                    table: None,
                }),
                format: None,
            }
        }
        None => classify(&plan.sql)?,
    };
    // Audited with what it is before GET's and a read-only user's rule
    // (`164`, as ClickHouse) and the identity's scopes (`497`, HS1 Task 20)
    // decide.
    if let Some(audit) = audit {
        audit.update(|event| {
            event.user = user.user.clone();
            event.principal = user.principal.clone();
            event.namespace = Some(user.namespace);
            event.statement = Some(match &classified {
                Classified::Known { stmt, .. } => stmt.kind().to_string(),
                Classified::Unparsed { .. } => "Unparsed".to_string(),
            });
        });
    }
    observed.update(|e| {
        e.kind = match &classified {
            Classified::Known { stmt, .. } => stmt.kind().to_string(),
            Classified::Unparsed { .. } => "Unparsed".to_string(),
        };
    });
    if readonly && matches!(&classified, Classified::Known { stmt, .. } if !stmt.is_read()) {
        return Err(readonly_error());
    }
    gate::authorize(&user, &classified)?;
    let format = match &classified {
        Classified::Known { format, .. } | Classified::Unparsed { format, .. } => {
            format.clone().or(plan.format)
        }
    };
    Ok(Prepared {
        user,
        sql: plan.sql.clone(),
        classified,
        readonly,
        format: if plan.input.is_some() { None } else { format },
        input: plan.input,
        decoder,
        first_data: plan.first_data,
    })
}

fn undecodable(err: io::Error) -> HouseError {
    HouseError::from(ChError::bad_arguments(format!(
        "Cannot decompress the request body: {err}"
    )))
}

/// Reads statement text from the body after `text`: all of it, unless it is an
/// `INSERT … FORMAT` whose data follows (then up to and a little past the data's
/// start; the rest streams). Every buffered byte counts against `max` until that
/// line is found (review I3).
async fn read_statement(
    body: &mut RequestBody,
    decoder: &mut Decoder,
    mut text: Vec<u8>,
    max: usize,
) -> Result<Vec<u8>, HouseError> {
    let too_long = || {
        HouseError::from(ChError::syntax_error(format!(
            "Max query size exceeded: the statement is longer than {max} bytes"
        )))
    };
    let mut scanner = Scanner::new();
    let mut ended = false;
    loop {
        let piece = match decoder.next_piece().map_err(undecodable)? {
            Some(piece) => piece,
            None if ended => return Ok(text),
            None => {
                match body.next().await? {
                    Some(piece) => decoder.push(&piece),
                    None => {
                        decoder.end();
                        ended = true;
                    }
                }
                continue;
            }
        };
        text.extend_from_slice(&piece);
        // Only the first `max` bytes are ever scanned (N2): a statement head must
        // fit in them, and the data after an `INSERT … FORMAT` line is not text.
        let scanned = text.len().min(max);
        scanner.advance(&text[..scanned]);
        if let InsertHead::Insert { .. } = scanner.insert_head(&text[..scanned], false) {
            return Ok(text);
        }
        if text.len() > max {
            return Err(too_long());
        }
    }
}

/// Streams the `INSERT` body into the worker as `Input` frames. A truncated or
/// corrupt compressed body fails here, before `InputEnd` (review I5).
async fn stream_input(
    lease: &mut WorkerLease,
    body: &mut RequestBody,
    decoder: &mut Decoder,
    first: Vec<u8>,
    mut hash: Option<&mut Sha256>,
) -> Result<(), HouseError> {
    let mut send = async |lease: &mut WorkerLease, piece: Vec<u8>| {
        if let Some(hash) = hash.as_deref_mut() {
            hash.update(&piece);
        }
        lease.send_input(Bytes::from(piece)).await
    };
    if !first.is_empty() {
        send(lease, first).await?;
    }
    loop {
        while let Some(piece) = decoder.next_piece().map_err(undecodable)? {
            send(lease, piece).await?;
        }
        match body.next().await? {
            Some(piece) => decoder.push(&piece),
            None => break,
        }
    }
    decoder.end();
    while let Some(piece) = decoder.next_piece().map_err(undecodable)? {
        send(lease, piece).await?;
    }
    lease.end_input().await
}

/// A write to a lake table (HS1 Task 12).
enum LakeJob {
    Insert(Box<crate::insert::Target>),
    CreateAs(Box<crate::insert::CreateAs>),
    Optimize(Box<crate::compact::OptimizePlan>),
    Mutate(Box<crate::mutate::MutationPlan>),
}

/// A write to a lake table and what it runs with (HS1 Task 12).
struct LakeWrite {
    job: LakeJob,
    database: String,
    params: Vec<(String, String)>,
    settings: Vec<(String, String)>,
}

/// The request body as an `INSERT`'s data.
struct BodyFeed<'a> {
    body: &'a mut RequestBody,
    decoder: &'a mut Decoder,
    first: Vec<u8>,
}

#[async_trait::async_trait]
impl crate::insert::Feed for BodyFeed<'_> {
    async fn feed(&mut self, lease: &mut WorkerLease, hash: &mut Sha256) -> Result<(), HouseError> {
        let first = std::mem::take(&mut self.first);
        stream_input(lease, self.body, self.decoder, first, Some(hash)).await
    }
}

/// Runs an `INSERT` into a lake table (HS1 Task 12) and answers it: an empty
/// `200` after the commit, `X-ClickHouse-Summary` with what was written and
/// `X-Loams-Snapshot-Id`.
async fn lake_insert(
    shared: &Arc<Shared>,
    meta: &mut Meta,
    user: &UserMap,
    write: LakeWrite,
    body: &mut RequestBody,
    decoder: &mut Decoder,
    first_data: Vec<u8>,
) -> Response<HouseBody> {
    let Some(lake) = shared.lake.as_deref() else {
        let close = !body.drain().await;
        let err = HouseError::from(ChError::not_implemented("this House has no catalog"));
        return meta.error(&err, &Progress::default(), close);
    };
    let streams = matches!(
        &write.job,
        LakeJob::Insert(target) if matches!(target.source, crate::insert::Source::Stream { .. })
    );
    // As for any INSERT: a worker only once its data has started (review I1).
    if streams && first_data.is_empty() {
        match body.next().await {
            Ok(Some(piece)) => decoder.push(&piece),
            Ok(None) => decoder.end(),
            Err(err) => return meta.error(&err, &Progress::default(), true),
        }
    }
    let lease = match shared.pool.acquire(&user.namespace.to_string()).await {
        Ok(lease) => lease,
        Err(err) => {
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    meta.worker = Some(lease.worker_id().to_string());
    if let Some(observed) = &meta.observed {
        observed.started_on_worker();
    }
    let known = shared.pool.known_settings();
    let ctx = crate::insert::InsertContext {
        lake,
        pool: &shared.pool,
        namespace: user.namespace,
        database: &write.database,
        user,
        query_id: &meta.query_id,
        params: &write.params,
        settings: &write.settings,
        limits: &shared.config.session_limits,
        known: &known,
        max_insert_bytes: shared.config.lake_insert_max_bytes,
        pipe_batch: None,
        admitted: meta.held.ticket.as_ref(),
    };
    let mut feed = BodyFeed {
        body,
        decoder,
        first: first_data,
    };
    let feed: Option<&mut dyn crate::insert::Feed> = if streams { Some(&mut feed) } else { None };
    let inserted_into = match &write.job {
        LakeJob::Insert(target) => Some((target.database.clone(), target.name.clone())),
        _ => None,
    };
    let inserted = match write.job {
        LakeJob::Insert(target) => crate::insert::run(&ctx, lease, *target, feed).await,
        LakeJob::CreateAs(plan) => crate::insert::create_as(&ctx, lease, *plan).await,
        LakeJob::Optimize(plan) => {
            // Compaction takes its own workers.
            shared
                .pool
                .release(lease, crate::admission::Outcome::Completed);
            crate::compact::optimize(&ctx, &plan).await
        }
        LakeJob::Mutate(plan) => {
            // A mutation takes its own workers.
            shared
                .pool
                .release(lease, crate::admission::Outcome::Completed);
            crate::mutate::run(&ctx, &plan).await
        }
    };
    // The source's materialized views run before the answer (HS1 Task 17).
    let inserted = match (inserted, inserted_into, shared.lake.as_ref()) {
        (Ok(inserted), Some((database, name)), Some(lake))
            if inserted.snapshot_id.is_some() && inserted.written_rows > 0 =>
        {
            crate::mv::after_insert(
                lake,
                &shared.pool,
                shared.config.session_limits,
                shared.config.lake_insert_max_bytes,
                user.namespace,
                &database,
                &name,
                &write.settings,
            )
            .await
            .map(|()| inserted)
        }
        (other, _, _) => other,
    };
    match inserted {
        Ok(inserted) => {
            meta.snapshot = inserted.snapshot_header();
            meta.head(
                StatusCode::OK,
                content_type(&meta.format),
                None,
                &inserted.progress(),
                false,
            )
            .body(HouseBody::full(Vec::new()))
            .unwrap_or_else(|_| fallback())
        }
        Err(err) => {
            // A body not read to its end leaves the connection out of step.
            let close = streams;
            meta.error(&err, &Progress::default(), close)
        }
    }
}

/// Runs the statement's events into a response: held until `buffer_size` or the
/// end, then streamed.
async fn respond(
    shared: &Arc<Shared>,
    meta: Meta,
    mut lease: WorkerLease,
    wait_end: bool,
    buffer_size: usize,
    mut session: Option<SessionGuard>,
) -> Response<HouseBody> {
    let pool = shared.pool.clone();
    let mut encoder = meta.encoding.and_then(|e| Encoder::new(e).ok());
    let mut pending: Vec<u8> = Vec::new();
    let mut spool = wait_end.then(|| {
        Spool::new(
            shared.config.tmp_dir.clone(),
            shared.config.wait_end_of_query_max_bytes,
            Arc::clone(&shared.spool),
        )
    });
    let mut last = Progress::default();
    loop {
        match lease.next_event().await {
            Ok(Event::Chunk(chunk)) => {
                let encoded = match encode(&mut encoder, &chunk.bytes) {
                    Ok(encoded) => encoded,
                    Err(err) => {
                        pool.kill(lease, ExitReason::Cancel);
                        return meta.error(&err, &last, true);
                    }
                };
                if let Some(spool) = &mut spool {
                    if let Err(err) = spool.write(&encoded) {
                        // Over the spool's cap: refused, not truncated (review M13).
                        pool.kill(lease, ExitReason::Cancel);
                        return meta.error(&err, &last, false);
                    }
                    continue;
                }
                pending.extend_from_slice(&encoded);
                if pending.len() > buffer_size {
                    return stream_rest(pool, meta, lease, encoder, pending, last, session);
                }
            }
            Ok(Event::Progress(progress)) => last = progress,
            Ok(Event::Done(stats)) => {
                last = stats;
                if let Some(observed) = &meta.observed {
                    observed.cpu(lease.measured().cpu);
                }
                pool.release(lease, Outcome::Completed);
                if let Some(guard) = session.as_mut() {
                    guard.confirm_pin();
                }
                let tail = encoder.take().map(Encoder::finish).transpose();
                let tail = match tail {
                    Ok(tail) => tail.unwrap_or_default(),
                    Err(err) => {
                        let err = HouseError::from(ChError::network_error(format!(
                            "encoding the response: {err}"
                        )));
                        return meta.error(&err, &last, true);
                    }
                };
                let head = meta.head(
                    StatusCode::OK,
                    content_type(&meta.format),
                    None,
                    &last,
                    false,
                );
                return match spool {
                    Some(mut spool) => {
                        if let Err(err) = spool.write(&tail) {
                            return meta.error(&err, &last, false);
                        }
                        spool.into_response(head)
                    }
                    None => {
                        pending.extend_from_slice(&tail);
                        head.body(HouseBody::full(pending))
                            .unwrap_or_else(|_| fallback())
                    }
                };
            }
            Err(err) => {
                pool.release(lease, Outcome::Completed);
                return meta.error(&err, &last, false);
            }
        }
    }
}

fn encode(encoder: &mut Option<Encoder>, bytes: &[u8]) -> Result<Vec<u8>, HouseError> {
    match encoder {
        Some(encoder) => encoder.feed(bytes).map_err(|err| {
            HouseError::from(ChError::network_error(format!(
                "encoding the response: {err}"
            )))
        }),
        None => Ok(bytes.to_vec()),
    }
}

/// The head goes out now; the rest of the statement streams from a task.
fn stream_rest(
    pool: WorkerPool,
    meta: Meta,
    mut lease: WorkerLease,
    mut encoder: Option<Encoder>,
    pending: Vec<u8>,
    last: Progress,
    session: Option<SessionGuard>,
) -> Response<HouseBody> {
    let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
    let head = meta.head(
        StatusCode::OK,
        content_type(&meta.format),
        None,
        &last,
        false,
    );
    tokio::spawn(async move {
        // The session stays checked out until the statement ends (373 meanwhile).
        let mut session = session;
        let abandoned = |meta: &Meta| {
            if let Some(observed) = &meta.observed {
                observed.fail(
                    crate::querylog::CANCELLED,
                    "the client went away before the result ended",
                );
            }
        };
        if tx.send(Piece::Data(Bytes::from(pending))).await.is_err() {
            abandoned(&meta);
            pool.kill(lease, ExitReason::Cancel);
            return;
        }
        loop {
            match lease.next_event().await {
                Ok(Event::Chunk(chunk)) => {
                    let Ok(encoded) = encode(&mut encoder, &chunk.bytes) else {
                        if let Some(observed) = &meta.observed {
                            observed.fail(1001, "encoding the response failed");
                        }
                        pool.kill(lease, ExitReason::Cancel);
                        let _ = tx.send(Piece::Abort).await;
                        return;
                    };
                    if !encoded.is_empty()
                        && tx.send(Piece::Data(Bytes::from(encoded))).await.is_err()
                    {
                        // The client went away: the statement is abandoned.
                        abandoned(&meta);
                        pool.kill(lease, ExitReason::Cancel);
                        return;
                    }
                }
                Ok(Event::Progress(_)) => {}
                Ok(Event::Done(stats)) => {
                    if let Some(observed) = &meta.observed {
                        observed.progress(&stats);
                        observed.cpu(lease.measured().cpu);
                    }
                    pool.release(lease, Outcome::Completed);
                    if let Some(guard) = session.as_mut() {
                        guard.confirm_pin();
                    }
                    if let Some(encoder) = encoder.take()
                        && let Ok(tail) = encoder.finish()
                        && !tail.is_empty()
                    {
                        let _ = tx.send(Piece::Data(Bytes::from(tail))).await;
                    }
                    return;
                }
                Err(err) => {
                    pool.release(lease, Outcome::Completed);
                    if let Some(observed) = &meta.observed {
                        observed.fail(err.code(), err.message());
                    }
                    // Ruling 9: the rows sent stay, the exception text follows, and
                    // the connection closes without the terminating chunk.
                    let text = err.render(&meta.version);
                    let mut tail = encode(&mut encoder, text.as_bytes())
                        .unwrap_or_else(|_| text.clone().into_bytes());
                    if let Some(encoder) = encoder.take() {
                        tail.extend(encoder.finish().unwrap_or_default());
                    }
                    let _ = tx
                        .send(Piece::Failed {
                            text: Bytes::from(tail),
                            error: err,
                        })
                        .await;
                    return;
                }
            }
        }
    });
    head.body(HouseBody::stream(rx, None))
        .unwrap_or_else(|_| fallback())
}

/// The front's total spool budget (review I2).
#[derive(Debug)]
struct SpoolBudget {
    used: AtomicU64,
    total: u64,
}

impl SpoolBudget {
    fn new(total: u64) -> Self {
        Self {
            used: AtomicU64::new(0),
            total,
        }
    }

    fn reserve(&self, bytes: u64) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.total)
            })
            .is_ok()
    }

    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

/// A `wait_end_of_query` result: memory first, then an unlinked temporary file.
#[derive(Debug)]
struct Spool {
    dir: std::path::PathBuf,
    memory: Vec<u8>,
    file: Option<std::fs::File>,
    max: u64,
    len: u64,
    budget: Arc<SpoolBudget>,
}

impl Spool {
    fn new(dir: std::path::PathBuf, max: u64, budget: Arc<SpoolBudget>) -> Self {
        Self {
            dir,
            memory: Vec::new(),
            file: None,
            max,
            len: 0,
            budget,
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), HouseError> {
        let n = bytes.len() as u64;
        if self.len + n > self.max {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "The result is larger than the {} bytes wait_end_of_query=1 buffers; run the \
                 query without wait_end_of_query",
                self.max
            ))));
        }
        if !self.budget.reserve(n) {
            return Err(HouseError::from(ChError::too_many_simultaneous_queries(
                "The House's wait_end_of_query spool is full; retry, or run the query without \
                 wait_end_of_query",
            )));
        }
        self.len += n;
        if self.file.is_none() && self.memory.len() + bytes.len() <= SPOOL_IN_MEMORY {
            self.memory.extend_from_slice(bytes);
            return Ok(());
        }
        let spool_error = |err: io::Error| {
            HouseError::from(ChError::network_error(format!(
                "wait_end_of_query could not spool the result: {err}"
            )))
        };
        if self.file.is_none() {
            let mut file = tempfile::tempfile_in(&self.dir).map_err(spool_error)?;
            file.write_all(&self.memory).map_err(spool_error)?;
            self.memory = Vec::new();
            self.file = Some(file);
        }
        if let Some(file) = &mut self.file {
            file.write_all(bytes).map_err(spool_error)?;
        }
        Ok(())
    }

    /// The response: the spool as its body, with its length.
    fn into_response(mut self, head: http::response::Builder) -> Response<HouseBody> {
        let Some(mut file) = self.file.take() else {
            let memory = std::mem::take(&mut self.memory);
            return head
                .body(HouseBody::full(memory))
                .unwrap_or_else(|_| fallback());
        };
        let len = self.len;
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        tokio::task::spawn_blocking(move || {
            // The spool (and its share of the budget) lives until it is sent.
            let _spool = self;
            if file.seek(SeekFrom::Start(0)).is_err() {
                let _ = tx.blocking_send(Piece::Abort);
                return;
            }
            let mut piece = vec![0u8; 256 * 1024];
            loop {
                match file.read(&mut piece) {
                    Ok(0) => return,
                    Ok(n) => {
                        if tx
                            .blocking_send(Piece::Data(Bytes::copy_from_slice(&piece[..n])))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx.blocking_send(Piece::Abort);
                        return;
                    }
                }
            }
        });
        head.body(HouseBody::stream(rx, Some(len)))
            .unwrap_or_else(|_| fallback())
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        self.budget.release(self.len);
    }
}

/// The `Content-Type` of an output format (review M7: text unless the format is
/// binary). ClickHouse's own table is per format (`IOutputFormat::getContentType`);
/// HS1 Task 29 re-checks these against the reference server.
pub fn content_type(format: &str) -> &'static str {
    let lower = format.to_ascii_lowercase();
    if lower.starts_with("tabseparated") || lower.starts_with("tsv") {
        "text/tab-separated-values; charset=UTF-8"
    } else if lower == "csvwithnames" || lower == "csvwithnamesandtypes" {
        "text/csv; charset=UTF-8; header=present"
    } else if lower.starts_with("csv") {
        "text/csv; charset=UTF-8; header=absent"
    } else if lower.starts_with("json") {
        "application/json; charset=UTF-8"
    } else if lower == "xml" {
        "application/xml; charset=UTF-8"
    } else if [
        "native",
        "rowbinary",
        "parquet",
        "arrow",
        "arrowstream",
        "orc",
        "avro",
        "protobuf",
        "msgpack",
        "capnproto",
        "bson",
    ]
    .iter()
    .any(|f| {
        lower == *f
            || (lower.starts_with("rowbinary") && *f == "rowbinary")
            || lower.starts_with("protobuf")
    }) {
        "application/octet-stream"
    } else {
        "text/plain; charset=UTF-8"
    }
}

/// `X-ClickHouse-Summary` and `X-ClickHouse-Progress`: ClickHouse writes every
/// number as a string.
pub fn progress_json(p: &Progress) -> String {
    format!(
        "{{\"read_rows\":\"{}\",\"read_bytes\":\"{}\",\"written_rows\":\"{}\",\"written_bytes\":\"{}\",\"total_rows_to_read\":\"0\",\"result_rows\":\"{}\",\"result_bytes\":\"{}\",\"elapsed_ns\":\"{}\"}}",
        p.rows_read,
        p.bytes_read,
        p.written_rows,
        p.written_bytes,
        p.result_rows,
        p.result_bytes,
        p.elapsed_ns
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_numbers_are_strings() {
        let json = progress_json(&Progress {
            rows_read: 7,
            ..Progress::default()
        });
        assert!(json.starts_with("{\"read_rows\":\"7\""), "{json}");
        assert!(json.contains("\"total_rows_to_read\":\"0\""));
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type("TabSeparated"),
            "text/tab-separated-values; charset=UTF-8"
        );
        assert_eq!(content_type("Parquet"), "application/octet-stream");
        assert_eq!(
            content_type("RowBinaryWithNames"),
            "application/octet-stream"
        );
        assert_eq!(content_type("PrettyCompact"), "text/plain; charset=UTF-8");
        assert_eq!(
            content_type("SomethingNew"),
            "text/plain; charset=UTF-8",
            "review M7"
        );
    }

    #[test]
    fn kill_rows_render_like_clickhouse() {
        let row = crate::watchdog::Killed {
            status: "waiting",
            query_id: "q1".to_string(),
            user: "alice".to_string(),
            query: "SELECT 'a\tb'".to_string(),
        };
        let tsv = KillLayout::of("TSVWithNames")
            .expect("tsv")
            .render(std::slice::from_ref(&row));
        assert_eq!(
            String::from_utf8(tsv).expect("utf-8"),
            "kill_status\tquery_id\tuser\tquery\nwaiting\tq1\talice\tSELECT \\'a\\tb\\'\n"
        );
        let csv = KillLayout::of("CSV")
            .expect("csv")
            .render(std::slice::from_ref(&row));
        assert_eq!(
            String::from_utf8(csv).expect("utf-8"),
            "\"waiting\",\"q1\",\"alice\",\"SELECT 'a\tb'\"\n"
        );
        let json = KillLayout::of("JSONEachRow").expect("json").render(&[row]);
        let value: serde_json::Value = serde_json::from_slice(&json).expect("json");
        assert_eq!(value["kill_status"], "waiting");
        assert_eq!(value["query"], "SELECT 'a\tb'");
        assert!(KillLayout::of("Pretty").is_none());
        assert!(KillLayout::of("TSV").expect("tsv").render(&[]).is_empty());
    }

    #[test]
    fn spool_budget_is_shared_and_released() {
        let budget = Arc::new(SpoolBudget::new(10));
        let dir = std::env::temp_dir();
        let mut a = Spool::new(dir.clone(), 100, Arc::clone(&budget));
        a.write(b"123456").expect("fits");
        let mut b = Spool::new(dir, 100, Arc::clone(&budget));
        assert_eq!(b.write(b"12345").expect_err("over the budget").code(), 202);
        drop(a);
        b.write(b"12345").expect("released");
    }
}
