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
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body::{Body as HttpBody, Frame as BodyFrame, SizeHint};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use loams_house_ipc::{Execute, Limits, Progress};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Sleep;

use crate::admission::{Event, Outcome, WorkerLease, WorkerPool};
use crate::auth;
use crate::classify::{Classified, Stmt, check_text, classify, decide, readonly_error};
use crate::compress::{self, Decoder, Encoder, Encoding};
use crate::config::{self, BUFFER_SIZE_RANGE, DISPLAY_NAME, HouseConfig, SPOOL_IN_MEMORY, UserMap};
use crate::errors::{ChError, HouseError};
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
    accept: JoinHandle<()>,
}

impl HouseHandle {
    /// The address it listens on (the port chosen, for `:0`).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting connections.
    pub fn shutdown(self) {
        self.accept.abort();
    }
}

impl Drop for HouseHandle {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

#[derive(Debug)]
struct Shared {
    config: HouseConfig,
    pool: WorkerPool,
    spool: Arc<SpoolBudget>,
    sessions: Arc<SessionTable>,
}

/// Serves the ClickHouse HTTP interface on `config.listen`, running statements on
/// `pool`. A non-loopback address is refused (D111).
pub async fn serve(config: HouseConfig, pool: WorkerPool) -> Result<HouseHandle, HouseError> {
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
    let permits = Arc::new(Semaphore::new(config.max_connections.max(1)));
    let unpin_pool = pool.clone();
    let sessions = SessionTable::new(
        config.max_live_sessions,
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
    let shared = Arc::new(Shared {
        spool: Arc::new(SpoolBudget::new(config.spool_budget_bytes)),
        config,
        pool,
        sessions,
    });
    let accept = tokio::spawn(async move {
        loop {
            let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                return;
            };
            match listener.accept().await {
                Ok((stream, _)) => {
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
    Ok(HouseHandle { addr, accept })
}

async fn connection(stream: TcpStream, shared: Arc<Shared>) {
    let _ = stream.set_nodelay(true);
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
struct IdleIo {
    stream: TcpStream,
    write_idle: Duration,
    write_deadline: Option<Pin<Box<Sleep>>>,
}

impl IdleIo {
    fn new(stream: TcpStream, write_idle: Duration) -> Self {
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

impl AsyncRead for IdleIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for IdleIo {
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
    },
}

/// A piece of a streamed body.
#[derive(Debug)]
enum Piece {
    Data(Bytes),
    /// The statement failed after the head went out: end the body with an error,
    /// so hyper closes the connection without the terminating chunk (Ruling 9).
    Abort,
}

impl HouseBody {
    fn full(bytes: impl Into<Bytes>) -> Self {
        Self {
            kind: BodyKind::Full(Some(bytes.into())),
        }
    }

    fn stream(rx: mpsc::Receiver<Piece>, len: Option<u64>) -> Self {
        Self {
            kind: BodyKind::Stream {
                rx,
                len,
                abort: None,
            },
        }
    }
}

impl HttpBody for HouseBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame<Bytes>, io::Error>>> {
        match &mut self.get_mut().kind {
            BodyKind::Full(bytes) => Poll::Ready(bytes.take().map(|b| Ok(BodyFrame::data(b)))),
            BodyKind::Stream { rx, abort, .. } => {
                if let Some(sleep) = abort {
                    ready!(sleep.as_mut().poll(cx));
                    return Poll::Ready(Some(Err(io::Error::other(
                        "the statement failed after the response started (Ruling 9)",
                    ))));
                }
                match ready!(rx.poll_recv(cx)) {
                    Some(Piece::Data(bytes)) => Poll::Ready(Some(Ok(BodyFrame::data(bytes)))),
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
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.kind, BodyKind::Full(None))
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            BodyKind::Full(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            BodyKind::Full(None) => SizeHint::with_exact(0),
            BodyKind::Stream { len: Some(len), .. } => SizeHint::with_exact(*len),
            BodyKind::Stream { .. } => SizeHint::default(),
        }
    }
}

/// The request body, read a data frame at a time.
struct RequestBody {
    body: Incoming,
    done: bool,
    idle: Duration,
}

impl RequestBody {
    fn new(body: Incoming, idle: Duration) -> Self {
        let done = body.is_end_stream();
        Self { body, done, idle }
    }

    /// The next data, or `None` at the end. A read that fails (broken framing) or a
    /// client silent for `receive_timeout` between frames is `210`.
    async fn next(&mut self) -> Result<Option<Bytes>, HouseError> {
        while !self.done {
            let Ok(frame) = tokio::time::timeout(self.idle, self.body.frame()).await else {
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
#[derive(Debug, Clone)]
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
        let mut builder = Response::builder()
            .status(status)
            .header("Content-Type", content_type)
            .header("X-ClickHouse-Server-Display-Name", DISPLAY_NAME);
        if let Some(encoding) = self.encoding {
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
        match self.encoding.and_then(|e| Encoder::new(e).ok()) {
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
        let body = self.encode_whole(error.render(&self.version).as_bytes());
        let status =
            StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        self.head(
            status,
            "text/plain; charset=UTF-8",
            Some(error.code()),
            last,
            close,
        )
        .body(HouseBody::full(body))
        .unwrap_or_else(|_| fallback())
    }
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
        "/" if reading || method == Method::POST => run(&shared, request, params).await,
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

/// A statement.
async fn run(
    shared: &Arc<Shared>,
    request: Request<Incoming>,
    params: Vec<(String, String)>,
) -> Response<HouseBody> {
    let config = &shared.config;
    let get = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let (parts, body) = request.into_parts();
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
        query_id: get("query_id")
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        format: get("default_format").unwrap_or("TabSeparated").to_string(),
        timezone: setting("session_timezone").unwrap_or_else(|| "UTC".to_string()),
        encoding: (get("enable_http_compression") == Some("1"))
            .then(|| compress::accepted(header("Accept-Encoding")))
            .flatten(),
        keep_alive: config.keep_alive,
        version: config.version.clone(),
        send_progress: get("send_progress_in_http_headers") == Some("1"),
        affinity: None,
        worker: None,
    };
    let mut body = RequestBody::new(body, config.receive_timeout);

    // Everything before a worker: who, what, and how.
    let known = shared.pool.known_settings();
    let prepared = prepare(
        config,
        &parts.headers,
        &params,
        &mut body,
        parts.method == Method::POST,
        &known,
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
    let Prepared {
        user,
        sql,
        classified,
        readonly,
        input,
        mut decoder,
        first_data,
        ..
    } = prepared;
    let namespace = user.namespace.to_string();

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

    // What the front answers itself, and what runs. Every statement chDB gets as
    // text that is not one of the owned writes is gated by ClickHouse's class
    // below (`gate`: sqlparser's message for a form it could not parse).
    let (gate, pins) = match classified {
        Classified::Known { stmt, .. } => {
            match front_answer(&stmt, session.as_ref(), &config.session_limits, &known) {
                Some(Ok(())) => {
                    drop(session);
                    return meta
                        .head(
                            StatusCode::OK,
                            content_type(&meta.format),
                            None,
                            &Progress::default(),
                            false,
                        )
                        .body(HouseBody::full(Vec::new()))
                        .unwrap_or_else(|_| fallback());
                }
                Some(Err(err)) => {
                    let close = !body.drain().await;
                    return meta.error(&err, &Progress::default(), close);
                }
                None => (
                    stmt.is_read().then_some(None),
                    matches!(stmt, Stmt::CreateTemporaryTable { .. }),
                ),
            }
        }
        Classified::Unparsed { message, .. } => (Some(Some(message)), false),
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
    let acquired = acquire_for(&pool, &namespace, session.as_mut(), pins).await;
    let mut lease = match acquired {
        Ok(lease) => lease,
        Err(err) => {
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    };
    meta.worker = Some(lease.worker_id().to_string());

    // ClickHouse's class gates every read and decides what sqlparser could not
    // parse (FL2 Ruling 6; fix round 1, C1), on exactly the text that runs.
    if let Some(unparsed) = gate {
        let decided = match lease.classify(&sql).await {
            Ok(classification) => decide(classification, readonly, unparsed.as_deref()),
            Err(err) => Err(err),
        };
        if let Err(err) = decided {
            pool.release(lease, Outcome::Completed);
            let close = !body.drain().await;
            return meta.error(&err, &Progress::default(), close);
        }
    }

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
            .filter(|(k, _)| !crate::settings::is_loams(k))
            .cloned(),
    );
    let query_params = params
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("param_")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();
    let execute = Execute {
        query_id: meta.query_id.clone(),
        session: worker_session,
        settings,
        views: Vec::new(),
        sql: if input.is_some() { String::new() } else { sql },
        format: meta.format.clone(),
        params: query_params,
        limits: Limits::default(),
        input,
    };
    let has_input = execute.input.is_some();
    if let Err(err) = lease.start(execute).await {
        pool.release(lease, Outcome::Completed);
        let close = !body.drain().await;
        return meta.error(&err, &Progress::default(), close);
    }
    if has_input
        && let Err(err) = stream_input(&mut lease, &mut body, &mut decoder, first_data).await
    {
        // The worker is mid-`INSERT`: ending its input would commit a body that did
        // not arrive whole (review I5), so it is killed instead.
        pool.kill(lease, ExitReason::Cancel);
        return meta.error(&err, &Progress::default(), true);
    }

    respond(shared, meta, lease, wait_end, buffer_size, session).await
}

/// The task that will serve an owned statement this front does not yet (`48`).
fn later(stmt: &Stmt) -> Option<String> {
    let (what, task) = match stmt {
        Stmt::CreateDatabase { .. } => ("CREATE DATABASE", "HS1 Task 10"),
        Stmt::CreateTable { .. } => ("CREATE TABLE (a lake table)", "HS1 Task 10"),
        Stmt::CreateTableAs { .. } => ("CREATE TABLE … AS SELECT", "HS1 Task 12"),
        Stmt::CreatePipe { .. } => (
            "A pipe (LoamsStream, IggyTopic, S3Queue)",
            "HS1 Tasks 16 and 18",
        ),
        Stmt::CreateMaterializedView { .. } => ("CREATE MATERIALIZED VIEW", "HS1 Task 17"),
        Stmt::Drop(drop) if !drop.temporary => ("DROP (a lake object)", "HS1 Task 10"),
        Stmt::Truncate { .. } => ("TRUNCATE", "HS1 Task 12"),
        Stmt::AlterAddColumns { .. } => ("ALTER TABLE … ADD COLUMN", "HS1 Task 10"),
        Stmt::Rename { .. } => ("RENAME TABLE", "HS1 Task 10"),
        Stmt::Undrop { .. } => ("UNDROP TABLE", "HS1 Task 15"),
        Stmt::Optimize { .. } => ("OPTIMIZE TABLE", "HS1 Task 13"),
        Stmt::KillQuery(_) => ("KILL QUERY", "HS1 Task 21"),
        Stmt::Unsupported { kind } => {
            return Some(format!("{kind} is not on the House's ClickHouse surface"));
        }
        _ => return None,
    };
    Some(format!(
        "{what} is served from {task}; this House does not serve it yet"
    ))
}

/// What the front answers itself: `Some(Ok)` for an empty success (`SET`, `USE`),
/// `Some(Err)` for an error, `None` for a statement a worker runs.
fn front_answer(
    stmt: &Stmt,
    session: Option<&SessionGuard>,
    limits: &crate::settings::SessionLimits,
    known: &std::collections::HashSet<String>,
) -> Option<Result<(), HouseError>> {
    if let Some(message) = later(stmt) {
        return Some(Err(HouseError::from(ChError::not_implemented(message))));
    }
    match stmt {
        Stmt::Set(pairs) => Some((|| {
            for (name, value) in pairs {
                crate::settings::check(name, value, limits, known)?;
            }
            // Without a session a SET has nothing to keep it, as in ClickHouse.
            if let Some(guard) = session {
                let mut state = guard.state();
                for (name, value) in pairs {
                    state.settings.apply(name, value, limits, known)?;
                }
            }
            Ok(())
        })()),
        Stmt::Use(database) => Some(if database != "default" {
            Err(HouseError::from(ChError::unknown_database(format!(
                "Database {database} does not exist"
            ))))
        } else {
            if let Some(guard) = session {
                guard.state().database = database.clone();
            }
            Ok(())
        }),
        _ => None,
    }
}

/// The worker for a statement: the session's pinned worker (a lost pin falls back
/// to any worker: its temporary tables are gone), a newly pinned one for the
/// session's first temporary table, or any worker of the namespace.
async fn acquire_for(
    pool: &WorkerPool,
    namespace: &str,
    session: Option<&mut SessionGuard>,
    pins: bool,
) -> Result<WorkerLease, HouseError> {
    let Some(guard) = session else {
        return pool.acquire(namespace).await;
    };
    let pinned = guard.state().pinned.clone();
    if let Some(worker) = pinned {
        if let Some(lease) = pool.acquire_pinned(namespace, &worker).await? {
            return Ok(lease);
        }
        guard.state().pinned = None;
    }
    if pins {
        let lease = pool.acquire_and_pin(namespace).await?;
        guard.state().pinned = Some(lease.worker_id().to_string());
        return Ok(lease);
    }
    pool.acquire(namespace).await
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

async fn prepare(
    config: &HouseConfig,
    headers: &http::HeaderMap,
    params: &[(String, String)],
    body: &mut RequestBody,
    post: bool,
    known: &std::collections::HashSet<String>,
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
    let credentials = auth::credentials(&header_pairs, params)?;
    let user = auth::authenticate(&config.users, &credentials)?.clone();
    for name in ["compress", "decompress"] {
        if get(name) == Some("1") {
            return Err(HouseError::from(ChError::not_implemented(format!(
                "{name}=1 (ClickHouse's own compressed framing) is not supported yet; use \
                 enable_http_compression=1 or Content-Encoding"
            ))));
        }
    }
    if let Some(database) = get("database").filter(|d| *d != "default") {
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
    if readonly && matches!(&classified, Classified::Known { stmt, .. } if !stmt.is_read()) {
        return Err(readonly_error());
    }
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
) -> Result<(), HouseError> {
    if !first.is_empty() {
        lease.send_input(Bytes::from(first)).await?;
    }
    loop {
        while let Some(piece) = decoder.next_piece().map_err(undecodable)? {
            lease.send_input(Bytes::from(piece)).await?;
        }
        match body.next().await? {
            Some(piece) => decoder.push(&piece),
            None => break,
        }
    }
    decoder.end();
    while let Some(piece) = decoder.next_piece().map_err(undecodable)? {
        lease.send_input(Bytes::from(piece)).await?;
    }
    lease.end_input().await
}

/// Runs the statement's events into a response: held until `buffer_size` or the
/// end, then streamed.
async fn respond(
    shared: &Arc<Shared>,
    meta: Meta,
    mut lease: WorkerLease,
    wait_end: bool,
    buffer_size: usize,
    session: Option<SessionGuard>,
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
                pool.release(lease, Outcome::Completed);
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
        let _session = session;
        if tx.send(Piece::Data(Bytes::from(pending))).await.is_err() {
            pool.kill(lease, ExitReason::Cancel);
            return;
        }
        loop {
            match lease.next_event().await {
                Ok(Event::Chunk(chunk)) => {
                    let Ok(encoded) = encode(&mut encoder, &chunk.bytes) else {
                        pool.kill(lease, ExitReason::Cancel);
                        let _ = tx.send(Piece::Abort).await;
                        return;
                    };
                    if !encoded.is_empty()
                        && tx.send(Piece::Data(Bytes::from(encoded))).await.is_err()
                    {
                        // The client went away: the statement is abandoned.
                        pool.kill(lease, ExitReason::Cancel);
                        return;
                    }
                }
                Ok(Event::Progress(_)) => {}
                Ok(Event::Done(_)) => {
                    pool.release(lease, Outcome::Completed);
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
                    // Ruling 9: the rows sent stay, the exception text follows, and
                    // the connection closes without the terminating chunk.
                    let text = err.render(&meta.version);
                    let mut tail = encode(&mut encoder, text.as_bytes())
                        .unwrap_or_else(|_| text.clone().into_bytes());
                    if let Some(encoder) = encoder.take() {
                        tail.extend(encoder.finish().unwrap_or_default());
                    }
                    let _ = tx.send(Piece::Data(Bytes::from(tail))).await;
                    let _ = tx.send(Piece::Abort).await;
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
