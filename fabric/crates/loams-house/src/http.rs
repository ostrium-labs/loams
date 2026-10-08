//! The ClickHouse HTTP interface (FL2 Task 2's contract, HS1 Task 3), served on the
//! worker pool.
//!
//! # Why a server of its own
//!
//! ClickHouse writes `X-ClickHouse-Progress` header lines **while the query runs**,
//! keeping the header block open until the first byte of the body: that is what
//! `send_progress_in_http_headers = 1` means on the wire, and what clients use to
//! keep a long query's connection alive. hyper (and so axum) writes a response's
//! headers in one piece, so it cannot do that. This module is a small HTTP/1.1
//! server over tokio instead — `httparse` for request heads, `Content-Length` and
//! chunked request bodies, keep-alive, `Expect: 100-continue` — which also gives the
//! exact header order and the deliberate protocol break of Ruling 9 (plan R3.1).
//!
//! # A request
//!
//! `GET /` and `GET /ping` answer `Ok.\n`. Otherwise the statement is `?query=`, or
//! the POST body; with both, the parameter is the statement and the body its data
//! (an `INSERT … FORMAT <f>`), or the rest of the statement. Credentials, settings
//! and the response headers are FL2 Task 2's; see [`crate::auth`] and
//! [`crate::compress`]. GET, and a read-only user, may only read: anything else is
//! `164 READONLY` before a worker is asked (HS1 Task 4's classifier replaces the
//! keyword check here).
//!
//! # The response
//!
//! Output is held until `buffer_size` (1 MiB) bytes have accumulated, so a statement
//! that fails early still gets its own status and `X-ClickHouse-Exception-Code`, and
//! a small result's `X-ClickHouse-Summary` is final. Past it, the headers go out
//! with the summary so far and the body streams. A failure after that is Ruling 9's:
//! the exception text is appended to the body and the connection is closed without
//! the terminating chunk ([`crate::errors::MidStreamBody`]'s rule).
//! `wait_end_of_query = 1` instead spools the whole result (in memory up to
//! `buffer_size`, then an unlinked temporary file, at most 1 GiB) and answers only
//! when the statement is over.

use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use loams_house_ipc::{Execute, InputSpec, Limits, Progress};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::admission::{Event, Outcome, WorkerLease, WorkerPool};
use crate::auth;
use crate::compress::{self, Decoder, Encoder, Encoding};
use crate::config::{self, DISPLAY_NAME, HouseConfig, UserMap};
use crate::errors::{ChError, HouseError, status_for};
use crate::watchdog::ExitReason;

/// The largest request head: 64 KiB.
const MAX_HEAD: usize = 64 * 1024;
/// The most a rejected request's unread body is drained to keep the connection.
const MAX_DRAIN: u64 = 1024 * 1024;

/// The parameters that are not settings (FL2 Task 2). `param_<name>` are query
/// parameters; every other parameter is a setting.
pub const NON_SETTINGS: &[&str] = &[
    "query",
    "database",
    "default_format",
    "query_id",
    "session_id",
    "session_timeout",
    "session_check",
    "user",
    "password",
    "compress",
    "decompress",
    "enable_http_compression",
    "wait_end_of_query",
    "buffer_size",
    "send_progress_in_http_headers",
];

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
}

/// Serves the ClickHouse HTTP interface on `config.listen`, running statements on
/// `pool`. A non-loopback address is refused (D111).
pub async fn serve(config: HouseConfig, pool: WorkerPool) -> Result<HouseHandle, HouseError> {
    config::check_listen(config.listen)?;
    let listen = config.listen;
    let listener = TcpListener::bind(listen).await.map_err(|err| {
        HouseError::from(ChError::network_error(format!(
            "house listen on {listen}: {err}"
        )))
    })?;
    let addr = listener.local_addr().map_err(|err| {
        HouseError::from(ChError::network_error(format!(
            "house listen on {listen}: {err}"
        )))
    })?;
    let shared = Arc::new(Shared { config, pool });
    let accept = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move {
                        let _ = stream.set_nodelay(true);
                        Conn::new(stream, shared).run().await;
                    });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    });
    Ok(HouseHandle { addr, accept })
}

/// A request head.
#[derive(Debug)]
struct Head {
    method: String,
    target: String,
    minor: u8,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// HTTP/1.1 keeps the connection unless asked not to; 1.0 only when asked.
    fn wants_keep_alive(&self) -> bool {
        let connection = self.header("Connection").unwrap_or("").to_ascii_lowercase();
        if self.minor == 0 {
            connection.contains("keep-alive")
        } else {
            !connection.contains("close")
        }
    }
}

/// How the request body is framed, and how much of it is left.
#[derive(Debug)]
enum Body {
    Empty,
    Length(u64),
    Chunked(ChunkState),
    Done,
}

#[derive(Debug, Clone, Copy)]
enum ChunkState {
    Size,
    Data(u64),
    DataEnd,
    Trailers,
}

impl Body {
    fn is_empty_or_done(&self) -> bool {
        matches!(self, Self::Empty | Self::Done | Self::Length(0))
    }
}

/// One client connection.
struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    shared: Arc<Shared>,
}

/// Whether the connection may serve another request.
type KeepAlive = bool;

impl Conn {
    fn new(stream: TcpStream, shared: Arc<Shared>) -> Self {
        Self {
            stream,
            buf: Vec::new(),
            shared,
        }
    }

    async fn run(mut self) {
        loop {
            let idle = self.shared.config.keep_alive;
            let head = match tokio::time::timeout(idle, self.read_head()).await {
                Ok(Ok(Some(head))) => head,
                Ok(Err(HeadError::TooLarge)) => {
                    let _ = self
                        .plain(431, "Request Header Fields Too Large", "", false)
                        .await;
                    return;
                }
                Ok(Err(HeadError::Bad)) => {
                    let _ = self.plain(400, "Bad Request", "", false).await;
                    return;
                }
                _ => return,
            };
            if !self.request(head).await {
                let _ = self.stream.shutdown().await;
                return;
            }
        }
    }

    /// Reads more bytes into the buffer; 0 is the end of the stream.
    async fn fill(&mut self) -> std::io::Result<usize> {
        let mut chunk = [0u8; 16 * 1024];
        let n = self.stream.read(&mut chunk).await?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    async fn read_head(&mut self) -> Result<Option<Head>, HeadError> {
        loop {
            let mut slots = [httparse::EMPTY_HEADER; 128];
            let mut request = httparse::Request::new(&mut slots);
            match request.parse(&self.buf) {
                Ok(httparse::Status::Complete(len)) => {
                    let head = Head {
                        method: request.method.unwrap_or("").to_string(),
                        target: request.path.unwrap_or("/").to_string(),
                        minor: request.version.unwrap_or(1),
                        headers: request
                            .headers
                            .iter()
                            .map(|h| {
                                (
                                    h.name.to_string(),
                                    String::from_utf8_lossy(h.value).into_owned(),
                                )
                            })
                            .collect(),
                    };
                    self.buf.drain(..len);
                    return Ok(Some(head));
                }
                Ok(httparse::Status::Partial) => {}
                Err(_) => return Err(HeadError::Bad),
            }
            if self.buf.len() > MAX_HEAD {
                return Err(HeadError::TooLarge);
            }
            match self.fill().await {
                Ok(0) if self.buf.is_empty() => return Ok(None),
                Ok(0) | Err(_) => return Err(HeadError::Bad),
                Ok(_) => {}
            }
        }
    }

    /// The next piece of the request body, or `None` at its end.
    async fn body_chunk(&mut self, body: &mut Body) -> std::io::Result<Option<Vec<u8>>> {
        let eof = || std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the body ended early");
        loop {
            match body {
                Body::Empty | Body::Done | Body::Length(0) => {
                    *body = Body::Done;
                    return Ok(None);
                }
                Body::Length(left) => {
                    if self.buf.is_empty() && self.fill().await? == 0 {
                        return Err(eof());
                    }
                    let take = (*left).min(self.buf.len() as u64) as usize;
                    *left -= take as u64;
                    return Ok(Some(self.buf.drain(..take).collect()));
                }
                Body::Chunked(state) => match *state {
                    ChunkState::Size => {
                        let Some(end) = find(&self.buf, b"\r\n") else {
                            if self.fill().await? == 0 {
                                return Err(eof());
                            }
                            continue;
                        };
                        let line = String::from_utf8_lossy(&self.buf[..end]).into_owned();
                        self.buf.drain(..end + 2);
                        let size =
                            u64::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
                                .map_err(|_| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "a bad chunk size",
                                    )
                                })?;
                        *state = if size == 0 {
                            ChunkState::Trailers
                        } else {
                            ChunkState::Data(size)
                        };
                    }
                    ChunkState::Data(left) => {
                        if self.buf.is_empty() && self.fill().await? == 0 {
                            return Err(eof());
                        }
                        let take = left.min(self.buf.len() as u64) as usize;
                        *state = if left == take as u64 {
                            ChunkState::DataEnd
                        } else {
                            ChunkState::Data(left - take as u64)
                        };
                        return Ok(Some(self.buf.drain(..take).collect()));
                    }
                    ChunkState::DataEnd => {
                        while self.buf.len() < 2 {
                            if self.fill().await? == 0 {
                                return Err(eof());
                            }
                        }
                        self.buf.drain(..2);
                        *state = ChunkState::Size;
                    }
                    ChunkState::Trailers => {
                        let Some(end) = find(&self.buf, b"\r\n") else {
                            if self.fill().await? == 0 {
                                return Err(eof());
                            }
                            continue;
                        };
                        self.buf.drain(..end + 2);
                        if end == 0 {
                            *body = Body::Done;
                            return Ok(None);
                        }
                    }
                },
            }
        }
    }

    /// Reads and discards what is left of a body, up to [`MAX_DRAIN`]; whether the
    /// connection is still usable.
    async fn drain(&mut self, body: &mut Body) -> KeepAlive {
        let mut drained = 0u64;
        loop {
            match self.body_chunk(body).await {
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

    async fn plain(
        &mut self,
        status: u16,
        reason: &str,
        body: &str,
        keep_alive: bool,
    ) -> std::io::Result<()> {
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nDate: {}\r\nConnection: {}\r\nContent-Type: text/plain; charset=UTF-8\r\nX-ClickHouse-Server-Display-Name: {DISPLAY_NAME}\r\nContent-Length: {}\r\n{}\r\n{body}",
            http_date(SystemTime::now()),
            if keep_alive { "Keep-Alive" } else { "Close" },
            body.len(),
            if keep_alive {
                "Keep-Alive: timeout=10\r\n"
            } else {
                ""
            },
        );
        self.stream.write_all(head.as_bytes()).await
    }

    /// One request. Returns whether the connection may serve another.
    async fn request(&mut self, head: Head) -> KeepAlive {
        let mut body = match framing(&head) {
            Ok(body) => body,
            Err(()) => {
                let _ = self.plain(400, "Bad Request", "", false).await;
                return false;
            }
        };
        if !body.is_empty_or_done()
            && head
                .header("Expect")
                .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
            && self
                .stream
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .is_err()
        {
            return false;
        }
        let keep_alive = head.wants_keep_alive();
        let (path, query) = head
            .target
            .split_once('?')
            .unwrap_or((head.target.as_str(), ""));
        let path = path.to_string();
        let params = parse_query(query);
        let has_query = params.iter().any(|(k, _)| k == "query");

        match (head.method.as_str(), path.as_str()) {
            ("GET" | "HEAD", "/ping") | ("GET" | "HEAD", "/") if !has_query => {
                let keep = keep_alive && self.drain(&mut body).await;
                let ok = self.plain(200, "OK", "Ok.\n", keep).await.is_ok();
                ok && keep
            }
            ("GET" | "POST", "/") => {
                let keep = self.query(&head, params, &mut body, keep_alive).await;
                keep && body.is_empty_or_done() || (keep && self.drain(&mut body).await)
            }
            (_, "/" | "/ping") => {
                let keep = keep_alive && self.drain(&mut body).await;
                let ok = self
                    .plain(405, "Method Not Allowed", "Use GET or POST.\n", keep)
                    .await
                    .is_ok();
                ok && keep
            }
            _ => {
                let keep = keep_alive && self.drain(&mut body).await;
                let text = format!(
                    "There is no handle {path}\n\nUse / or /ping for health checks.\n\
                     Send queries with POST or GET /?query=...\n"
                );
                let ok = self.plain(404, "Not Found", &text, keep).await.is_ok();
                ok && keep
            }
        }
    }

    /// A statement. Returns whether the connection may serve another request.
    async fn query(
        &mut self,
        head: &Head,
        params: Vec<(String, String)>,
        body: &mut Body,
        keep_alive: bool,
    ) -> KeepAlive {
        let config = self.shared.config.clone();
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let query_id = get("query_id")
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let encoding = (get("enable_http_compression") == Some("1"))
            .then(|| compress::accepted(head.header("Accept-Encoding")))
            .flatten();
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
        let mut reply = Reply {
            query_id: query_id.clone(),
            format: get("default_format").unwrap_or("TabSeparated").to_string(),
            timezone: setting("session_timezone").unwrap_or_else(|| "UTC".to_string()),
            keep_alive,
            encoding,
            version: config.version.clone(),
            state: HeadState::NotSent,
            encoder: None,
            pending: Vec::new(),
            spool: None,
            last: Progress::default(),
        };

        let prepared = match self.prepare(head, &params, body, &config).await {
            Ok(prepared) => prepared,
            Err(failure) => {
                let keep = keep_alive && self.drain(body).await;
                reply.keep_alive = keep;
                return reply.fail(&mut self.stream, failure).await && keep;
            }
        };
        let Prepared {
            user,
            sql,
            format,
            input,
            mut decoder,
            first_data,
        } = prepared;
        if let Some(format) = format {
            reply.format = format;
        }

        let buffer_size = get("buffer_size")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(config.default_buffer_size);
        let wait_end = get("wait_end_of_query") == Some("1");
        let send_progress = get("send_progress_in_http_headers") == Some("1");
        let progress_every = setting("http_headers_progress_interval_ms")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(config.progress_interval);
        if let Some(encoding) = reply.encoding {
            match Encoder::new(encoding) {
                Ok(encoder) => reply.encoder = Some(encoder),
                Err(_) => reply.encoding = None,
            }
        }
        if wait_end {
            reply.spool = Some(Spool::new(
                config.tmp_dir.clone(),
                buffer_size,
                config.wait_end_of_query_max_bytes,
            ));
        }

        let execute = Execute {
            query_id: query_id.clone(),
            session: get("session_id").map(str::to_string),
            settings,
            views: Vec::new(),
            sql,
            format: reply.format.clone(),
            params: query_params,
            limits: Limits::default(),
            input,
        };
        let has_input = execute.input.is_some();

        let pool = self.shared.pool.clone();
        let mut lease = match pool.acquire(&user.namespace.to_string()).await {
            Ok(lease) => lease,
            Err(err) => {
                let keep = keep_alive && self.drain(body).await;
                reply.keep_alive = keep;
                return reply.fail(&mut self.stream, err).await && keep;
            }
        };
        if let Err(err) = lease.start(execute).await {
            pool.release(lease, Outcome::Completed);
            let keep = keep_alive && self.drain(body).await;
            reply.keep_alive = keep;
            return reply.fail(&mut self.stream, err).await && keep;
        }

        if has_input
            && let Err(failure) = self
                .stream_input(&mut lease, body, &mut decoder, first_data)
                .await
        {
            // The worker is mid-`INSERT`: ending its input would commit a body
            // that did not arrive whole, so it is killed instead.
            pool.kill(lease, ExitReason::Cancel);
            reply.keep_alive = false;
            let _ = reply.fail(&mut self.stream, failure).await;
            return false;
        }

        let keep = self
            .respond(
                &mut reply,
                &mut lease,
                wait_end,
                send_progress,
                progress_every,
                buffer_size,
            )
            .await;
        pool.release(lease, Outcome::Completed);
        keep
    }

    /// Everything a statement needs before a worker is asked: who, what, and how.
    async fn prepare(
        &mut self,
        head: &Head,
        params: &[(String, String)],
        body: &mut Body,
        config: &HouseConfig,
    ) -> Result<Prepared, HouseError> {
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let credentials = auth::credentials(&head.headers, params)?;
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
        let mut decoder =
            Decoder::new(compress::content_encoding(head.header("Content-Encoding"))?)
                .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?;

        // The statement: the parameter, else the body's head.
        let (text, rest_known) = match get("query") {
            Some(query) => (query.as_bytes().to_vec(), true),
            None => (
                self.read_statement(body, &mut decoder, config.max_query_size)
                    .await?,
                false,
            ),
        };
        let readonly = head.method == "GET" || user.readonly;

        let mut plan = plan(&text, rest_known)?;
        if rest_known && plan.input.is_none() && !body.is_empty_or_done() {
            // `?query=` and a body that is not `INSERT` data: the body continues the
            // statement, as in ClickHouse.
            let more = self
                .read_statement(body, &mut decoder, config.max_query_size)
                .await?;
            if !more.is_empty() {
                let mut joined = text.clone();
                joined.push(b'\n');
                joined.extend_from_slice(&more);
                plan = self::plan(&joined, false)?;
            }
        }
        if readonly && (plan.input.is_some() || !is_read(&plan.sql)) {
            return Err(HouseError::from(ChError::readonly(
                "Cannot execute query in readonly mode. For queries over HTTP, method GET \
                 implies readonly. You should use method POST for modifying queries",
            )));
        }
        Ok(Prepared {
            user,
            sql: plan.sql,
            format: plan.format,
            input: plan.input,
            decoder,
            first_data: plan.first_data,
        })
    }

    /// Reads the body as statement text: all of it, unless it is an `INSERT … FORMAT`
    /// whose data follows, in which case only up to the data (the rest streams).
    async fn read_statement(
        &mut self,
        body: &mut Body,
        decoder: &mut Decoder,
        max: usize,
    ) -> Result<Vec<u8>, HouseError> {
        let mut text = Vec::new();
        loop {
            if let InsertHead::Insert { .. } = insert_head(&text, false) {
                return Ok(text);
            }
            match self.body_chunk(body).await {
                Ok(Some(piece)) => {
                    let decoded = decoder.feed(&piece).map_err(|err| {
                        HouseError::from(ChError::bad_arguments(format!(
                            "Cannot decompress the request body: {err}"
                        )))
                    })?;
                    text.extend_from_slice(&decoded);
                }
                Ok(None) => {
                    let rest = std::mem::replace(decoder, Decoder::Identity)
                        .finish()
                        .map_err(|err| {
                            HouseError::from(ChError::bad_arguments(format!(
                                "Cannot decompress the request body: {err}"
                            )))
                        })?;
                    text.extend_from_slice(&rest);
                    return Ok(text);
                }
                Err(err) => {
                    return Err(HouseError::from(ChError::network_error(format!(
                        "reading the request body: {err}"
                    ))));
                }
            }
            if text.len() > max && !matches!(insert_head(&text, false), InsertHead::Incomplete) {
                return Err(HouseError::from(ChError::syntax_error(format!(
                    "Max query size exceeded: the statement is longer than {max} bytes"
                ))));
            }
        }
    }

    /// Streams the `INSERT` body into the worker as `Input` frames.
    async fn stream_input(
        &mut self,
        lease: &mut WorkerLease,
        body: &mut Body,
        decoder: &mut Decoder,
        first: Vec<u8>,
    ) -> Result<(), HouseError> {
        let decode_error = |err: std::io::Error| {
            HouseError::from(ChError::bad_arguments(format!(
                "Cannot decompress the request body: {err}"
            )))
        };
        if !first.is_empty() {
            lease.send_input(Bytes::from(first)).await?;
        }
        loop {
            match self.body_chunk(body).await {
                Ok(Some(piece)) => {
                    let decoded = decoder.feed(&piece).map_err(decode_error)?;
                    if !decoded.is_empty() {
                        lease.send_input(Bytes::from(decoded)).await?;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    return Err(HouseError::from(ChError::network_error(format!(
                        "reading the request body: {err}"
                    ))));
                }
            }
        }
        let rest = std::mem::replace(decoder, Decoder::Identity)
            .finish()
            .map_err(decode_error)?;
        if !rest.is_empty() {
            lease.send_input(Bytes::from(rest)).await?;
        }
        lease.end_input().await
    }

    /// Streams the statement's events into the response.
    async fn respond(
        &mut self,
        reply: &mut Reply,
        lease: &mut WorkerLease,
        wait_end: bool,
        send_progress: bool,
        progress_every: Duration,
        buffer_size: usize,
    ) -> KeepAlive {
        let mut last_progress_header: Option<Instant> = None;
        loop {
            let event = lease.next_event().await;
            let outcome = match event {
                Ok(Event::Chunk(chunk)) => {
                    reply
                        .output(&mut self.stream, &chunk.bytes, wait_end, buffer_size)
                        .await
                }
                Ok(Event::Progress(progress)) => {
                    reply.last = progress;
                    let due = last_progress_header.is_none_or(|at| at.elapsed() >= progress_every);
                    if send_progress && due && reply.can_add_headers() {
                        last_progress_header = Some(Instant::now());
                        reply.progress_header(&mut self.stream).await
                    } else {
                        Ok(())
                    }
                }
                Ok(Event::Done(stats)) => {
                    reply.last = stats;
                    return reply.finish(&mut self.stream).await;
                }
                Err(err) => return reply.fail(&mut self.stream, err).await,
            };
            match outcome {
                Ok(()) => {}
                Err(Stop::Client) => {
                    // The client went away: the lease's drop kills the worker.
                    return false;
                }
                Err(Stop::Fail(err)) => {
                    // The spool is full: the statement is abandoned (its worker
                    // killed when the lease is dropped below) and refused.
                    if let Some(handle) = lease.kill_handle() {
                        let _ = handle.kill(ExitReason::Cancel);
                    }
                    reply.keep_alive = false;
                    let _ = reply.fail(&mut self.stream, err).await;
                    return false;
                }
            }
        }
    }
}

#[derive(Debug)]
enum HeadError {
    TooLarge,
    Bad,
}

/// Why streaming stopped early.
#[derive(Debug)]
enum Stop {
    /// The socket failed.
    Client,
    /// A limit of the House's: answered as an error.
    Fail(HouseError),
}

impl From<std::io::Error> for Stop {
    fn from(_: std::io::Error) -> Self {
        Self::Client
    }
}

/// What [`Conn::prepare`] works out.
struct Prepared {
    user: UserMap,
    sql: String,
    format: Option<String>,
    input: Option<InputSpec>,
    decoder: Decoder,
    first_data: Vec<u8>,
}

/// How far the response has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadState {
    /// Nothing written: the status can still be anything.
    NotSent,
    /// The status line and the fixed headers are written, and header lines can still
    /// be added (`X-ClickHouse-Progress`).
    Open,
    /// The headers are ended and the chunked body has started.
    Body,
}

/// The response being written.
struct Reply {
    query_id: String,
    format: String,
    timezone: String,
    keep_alive: bool,
    encoding: Option<Encoding>,
    version: String,
    state: HeadState,
    encoder: Option<Encoder>,
    /// Encoded output not yet written.
    pending: Vec<u8>,
    spool: Option<Spool>,
    last: Progress,
}

impl Reply {
    fn can_add_headers(&self) -> bool {
        self.state != HeadState::Body
    }

    /// The status line and fixed headers, in ClickHouse's order, up to and including
    /// `Keep-Alive`. `framing` is `Transfer-Encoding: chunked` or a
    /// `Content-Length`.
    fn fixed_headers(
        &self,
        status: u16,
        content_type: &str,
        framing: &str,
        code: Option<i32>,
    ) -> String {
        let mut head = format!(
            "HTTP/1.1 {status} {}\r\nDate: {}\r\nConnection: {}\r\nContent-Type: {content_type}\r\nX-ClickHouse-Server-Display-Name: {DISPLAY_NAME}\r\n",
            reason(status),
            http_date(SystemTime::now()),
            if self.keep_alive {
                "Keep-Alive"
            } else {
                "Close"
            },
        );
        if let Some(encoding) = self.encoding {
            head.push_str(&format!("Content-Encoding: {}\r\n", encoding.as_str()));
        }
        head.push_str(framing);
        head.push_str(&format!(
            "X-ClickHouse-Query-Id: {}\r\nX-ClickHouse-Format: {}\r\nX-ClickHouse-Timezone: {}\r\n",
            header_safe(&self.query_id),
            header_safe(&self.format),
            header_safe(&self.timezone),
        ));
        if let Some(code) = code {
            head.push_str(&format!("X-ClickHouse-Exception-Code: {code}\r\n"));
        }
        if self.keep_alive {
            head.push_str("Keep-Alive: timeout=10\r\n");
        }
        head
    }

    fn summary_line(&self) -> String {
        format!("X-ClickHouse-Summary: {}\r\n", progress_json(&self.last))
    }

    /// Opens the head for progress lines (status 200, chunked: the length is not
    /// known yet).
    async fn open(&mut self, stream: &mut TcpStream) -> std::io::Result<()> {
        if self.state == HeadState::NotSent {
            let head = self.fixed_headers(
                200,
                content_type(&self.format),
                "Transfer-Encoding: chunked\r\n",
                None,
            );
            stream.write_all(head.as_bytes()).await?;
            self.state = HeadState::Open;
        }
        Ok(())
    }

    async fn progress_header(&mut self, stream: &mut TcpStream) -> Result<(), Stop> {
        self.open(stream).await?;
        let line = format!("X-ClickHouse-Progress: {}\r\n", progress_json(&self.last));
        stream.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Ends the head (summary so far) and starts the chunked body.
    async fn start_body(&mut self, stream: &mut TcpStream) -> std::io::Result<()> {
        self.open(stream).await?;
        if self.state == HeadState::Open {
            let mut tail = self.summary_line();
            tail.push_str("\r\n");
            stream.write_all(tail.as_bytes()).await?;
            self.state = HeadState::Body;
        }
        Ok(())
    }

    fn encode(&mut self, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        match &mut self.encoder {
            Some(encoder) => encoder.feed(bytes),
            None => Ok(bytes.to_vec()),
        }
    }

    fn finish_encoder(&mut self) -> std::io::Result<Vec<u8>> {
        match self.encoder.take() {
            Some(encoder) => encoder.finish(),
            None => Ok(Vec::new()),
        }
    }

    /// Output bytes from the worker.
    async fn output(
        &mut self,
        stream: &mut TcpStream,
        bytes: &[u8],
        wait_end: bool,
        buffer_size: usize,
    ) -> Result<(), Stop> {
        let encoded = self.encode(bytes)?;
        if wait_end {
            if let Some(spool) = &mut self.spool {
                spool.write(&encoded).map_err(Stop::Fail)?;
            }
            return Ok(());
        }
        self.pending.extend_from_slice(&encoded);
        if self.state == HeadState::Body || self.pending.len() > buffer_size {
            self.start_body(stream).await?;
            let pending = std::mem::take(&mut self.pending);
            write_chunk(stream, &pending).await?;
        }
        Ok(())
    }

    /// The statement finished. Returns whether the connection may continue.
    async fn finish(&mut self, stream: &mut TcpStream) -> KeepAlive {
        let result = self.finish_inner(stream).await;
        result.is_ok() && self.keep_alive
    }

    async fn finish_inner(&mut self, stream: &mut TcpStream) -> std::io::Result<()> {
        let tail = self.finish_encoder()?;
        if let Some(mut spool) = self.spool.take() {
            spool
                .write(&tail)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            if self.state == HeadState::NotSent {
                let framing = format!("Content-Length: {}\r\n", spool.len());
                let mut head = self.fixed_headers(200, content_type(&self.format), &framing, None);
                head.push_str(&self.summary_line());
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).await?;
                spool.copy_to(stream, false).await?;
            } else {
                self.start_body(stream).await?;
                spool.copy_to(stream, true).await?;
                stream.write_all(b"0\r\n\r\n").await?;
            }
            return stream.flush().await;
        }
        self.pending.extend_from_slice(&tail);
        let pending = std::mem::take(&mut self.pending);
        match self.state {
            HeadState::NotSent => {
                let framing = format!("Content-Length: {}\r\n", pending.len());
                let mut head = self.fixed_headers(200, content_type(&self.format), &framing, None);
                head.push_str(&self.summary_line());
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).await?;
                stream.write_all(&pending).await?;
            }
            HeadState::Open | HeadState::Body => {
                self.start_body(stream).await?;
                write_chunk(stream, &pending).await?;
                stream.write_all(b"0\r\n\r\n").await?;
            }
        }
        stream.flush().await
    }

    /// The statement failed. Returns whether the connection may continue.
    async fn fail(&mut self, stream: &mut TcpStream, error: HouseError) -> KeepAlive {
        let text = error.render(&self.version);
        match self.state {
            HeadState::NotSent => {
                // Nothing went out: a proper error response, whatever was buffered
                // or spooled discarded.
                self.pending.clear();
                self.spool = None;
                self.encoder = self.encoding.and_then(|e| Encoder::new(e).ok());
                let body = self
                    .encode(text.as_bytes())
                    .and_then(|mut b| {
                        b.extend(self.finish_encoder()?);
                        Ok(b)
                    })
                    .unwrap_or_else(|_| text.clone().into_bytes());
                let framing = format!("Content-Length: {}\r\n", body.len());
                let status = status_for(error.code());
                let mut head = self.fixed_headers(
                    status,
                    "text/plain; charset=UTF-8",
                    &framing,
                    Some(error.code()),
                );
                head.push_str(&self.summary_line());
                head.push_str("\r\n");
                let sent = async {
                    stream.write_all(head.as_bytes()).await?;
                    stream.write_all(&body).await?;
                    stream.flush().await
                }
                .await;
                sent.is_ok() && self.keep_alive
            }
            HeadState::Open => {
                // Progress lines went out with a 200: the code goes in a header, the
                // text in the body, and the response still ends properly.
                self.pending.clear();
                self.spool = None;
                self.encoder = self.encoding.and_then(|e| Encoder::new(e).ok());
                let sent = async {
                    let line = format!("X-ClickHouse-Exception-Code: {}\r\n", error.code());
                    stream.write_all(line.as_bytes()).await?;
                    self.start_body(stream).await?;
                    let mut body = self.encode(text.as_bytes())?;
                    body.extend(self.finish_encoder()?);
                    write_chunk(stream, &body).await?;
                    stream.write_all(b"0\r\n\r\n").await?;
                    stream.flush().await
                }
                .await;
                sent.is_ok() && self.keep_alive
            }
            HeadState::Body => {
                // Ruling 9: the rows already sent stay, the exception text follows
                // them, and the connection closes without the terminating chunk so
                // the client cannot read a partial result as a whole one.
                let sent = async {
                    let mut body = std::mem::take(&mut self.pending);
                    body.extend(self.encode(text.as_bytes())?);
                    body.extend(self.finish_encoder()?);
                    write_chunk(stream, &body).await?;
                    stream.flush().await
                }
                .await;
                let _ = sent;
                false
            }
        }
    }
}

/// A `wait_end_of_query` result: memory first, then an unlinked temporary file.
struct Spool {
    dir: std::path::PathBuf,
    memory: Vec<u8>,
    file: Option<std::fs::File>,
    in_memory_limit: usize,
    max: u64,
    len: u64,
}

impl Spool {
    fn new(dir: std::path::PathBuf, in_memory_limit: usize, max: u64) -> Self {
        Self {
            dir,
            memory: Vec::new(),
            file: None,
            in_memory_limit,
            max,
            len: 0,
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), HouseError> {
        if self.len + bytes.len() as u64 > self.max {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "The result is larger than the {} bytes wait_end_of_query=1 buffers; run the \
                 query without wait_end_of_query",
                self.max
            ))));
        }
        self.len += bytes.len() as u64;
        if self.file.is_none() && self.memory.len() + bytes.len() <= self.in_memory_limit {
            self.memory.extend_from_slice(bytes);
            return Ok(());
        }
        let spool_error = |err: std::io::Error| {
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

    /// Writes the spooled bytes out, as one chunk per piece when `chunked`.
    async fn copy_to(&mut self, stream: &mut TcpStream, chunked: bool) -> std::io::Result<()> {
        match &mut self.file {
            None => {
                let memory = std::mem::take(&mut self.memory);
                if chunked {
                    write_chunk(stream, &memory).await
                } else {
                    stream.write_all(&memory).await
                }
            }
            Some(file) => {
                file.seek(SeekFrom::Start(0))?;
                let mut piece = vec![0u8; 256 * 1024];
                loop {
                    let n = file.read(&mut piece)?;
                    if n == 0 {
                        return Ok(());
                    }
                    if chunked {
                        write_chunk(stream, &piece[..n]).await?;
                    } else {
                        stream.write_all(&piece[..n]).await?;
                    }
                }
            }
        }
    }
}

async fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(bytes.len() + 16);
    out.extend_from_slice(format!("{:x}\r\n", bytes.len()).as_bytes());
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\r\n");
    stream.write_all(&out).await
}

/// The request body's framing.
fn framing(head: &Head) -> Result<Body, ()> {
    if head
        .header("Transfer-Encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return Ok(Body::Chunked(ChunkState::Size));
    }
    match head.header("Content-Length") {
        Some(length) => length
            .trim()
            .parse::<u64>()
            .map(Body::Length)
            .map_err(|_| ()),
        None => Ok(Body::Empty),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `a=b&c=d`, percent-decoded, `+` as a space.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 3 <= bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A header value without line breaks.
fn header_safe(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Request Entity Too Large",
        415 => "Unsupported Media Type",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

/// The `Content-Type` of an output format. ClickHouse's own table is per format
/// (`IOutputFormat::getContentType`); these are its values for the declared
/// formats, to be re-checked against the reference server in HS1 Task 29.
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
    } else if lower.starts_with("pretty")
        || lower == "vertical"
        || lower == "values"
        || lower == "markdown"
        || lower == "lineasstring"
        || lower.starts_with("raw")
    {
        "text/plain; charset=UTF-8"
    } else {
        "application/octet-stream"
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

/// An RFC 7231 date: `Thu, 08 Oct 2026 22:55:30 GMT`.
pub fn http_date(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {d:02} {} {year} {h:02}:{m:02}:{s:02} GMT",
        WEEKDAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize],
    )
}

// ---------------------------------------------------------------------------
// The statement's shape. HS1 Task 4's classifier replaces these with sqlparser;
// until then they know just enough: the first keyword, a trailing `FORMAT`, and
// where an `INSERT`'s data starts.
// ---------------------------------------------------------------------------

/// A word or a symbol of the statement, outside strings and comments.
#[derive(Debug, Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
    word: bool,
    depth: i32,
}

/// Tokens of `text` (best effort: strings, quoted names and comments are skipped).
fn tokens(text: &[u8]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut depth = 0;
    while i < text.len() {
        let c = text[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'-' && text.get(i + 1) == Some(&b'-') || c == b'#' {
            while i < text.len() && text[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && text.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < text.len() && !(text[i] == b'*' && text[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'\'' || c == b'"' || c == b'`' {
            let start = i;
            i += 1;
            while i < text.len() {
                if text[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if text[i] == c {
                    if text.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            out.push(Token {
                start,
                end: i.min(text.len()),
                word: false,
                depth,
            });
        } else if c.is_ascii_alphanumeric() || c == b'_' {
            let start = i;
            while i < text.len() && (text[i].is_ascii_alphanumeric() || text[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                start,
                end: i,
                word: true,
                depth,
            });
        } else {
            if c == b'(' {
                depth += 1;
            }
            out.push(Token {
                start: i,
                end: i + 1,
                word: false,
                depth,
            });
            if c == b')' {
                depth -= 1;
            }
            i += 1;
        }
    }
    out
}

fn word_is(text: &[u8], t: &Token, word: &str) -> bool {
    t.word && text[t.start..t.end].eq_ignore_ascii_case(word.as_bytes())
}

/// The first keyword, upper-cased, past comments and opening parentheses.
fn first_keyword(sql: &str) -> String {
    let bytes = sql.as_bytes();
    tokens(bytes)
        .into_iter()
        .find(|t| t.word)
        .map(|t| String::from_utf8_lossy(&bytes[t.start..t.end]).to_ascii_uppercase())
        .unwrap_or_default()
}

/// Whether a statement only reads (what GET and read-only users may run).
fn is_read(sql: &str) -> bool {
    matches!(
        first_keyword(sql).as_str(),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXISTS" | "EXPLAIN" | "CHECK"
    )
}

/// Where an `INSERT`'s data starts.
#[derive(Debug, PartialEq, Eq)]
enum InsertHead {
    /// Not an `INSERT` (or not known to be one yet).
    NotInsert,
    /// An `INSERT` whose `FORMAT <f>` line has not been read to its end.
    Incomplete,
    /// An `INSERT … FORMAT <f>`: the statement, the format, and where the data
    /// starts in the text.
    Insert {
        statement: String,
        format: String,
        data_start: usize,
    },
    /// An `INSERT` without `FORMAT` (`VALUES (…)` inline, or `… SELECT`).
    NoFormat,
}

/// Finds an `INSERT`'s `FORMAT <f>` line. `complete` says whether the text can grow:
/// a `FORMAT <f>` at the very end of a complete text is a statement with no data.
fn insert_head(text: &[u8], complete: bool) -> InsertHead {
    let toks = tokens(text);
    let Some(first) = toks.iter().find(|t| t.word) else {
        return if complete {
            InsertHead::NotInsert
        } else {
            InsertHead::Incomplete
        };
    };
    if !word_is(text, first, "INSERT") {
        return InsertHead::NotInsert;
    }
    for (at, t) in toks.iter().enumerate() {
        if t.depth == 0 && word_is(text, t, "FORMAT") {
            let Some(name) = toks.get(at + 1).filter(|n| n.word) else {
                return if complete {
                    InsertHead::NoFormat
                } else {
                    InsertHead::Incomplete
                };
            };
            // The data starts after the format name, its spaces and one newline.
            let mut i = name.end;
            while i < text.len() && (text[i] == b' ' || text[i] == b'\t') {
                i += 1;
            }
            if i < text.len() && text[i] == b'\r' {
                i += 1;
            }
            if i < text.len() && text[i] == b'\n' {
                i += 1;
            } else if i >= text.len() && !complete {
                return InsertHead::Incomplete;
            }
            return InsertHead::Insert {
                statement: String::from_utf8_lossy(&text[..t.start])
                    .trim_end()
                    .to_string(),
                format: String::from_utf8_lossy(&text[name.start..name.end]).into_owned(),
                data_start: i,
            };
        }
    }
    if complete {
        InsertHead::NoFormat
    } else {
        InsertHead::Incomplete
    }
}

/// The statement as the worker gets it.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    sql: String,
    format: Option<String>,
    input: Option<InputSpec>,
    first_data: Vec<u8>,
}

/// What to run for `text`. `body_follows`: the data of an `INSERT … FORMAT` is (also)
/// in the request body.
fn plan(text: &[u8], body_follows: bool) -> Result<Plan, HouseError> {
    match insert_head(text, true) {
        InsertHead::Insert {
            statement,
            format,
            data_start,
        } => {
            let first_data = text[data_start..].to_vec();
            Ok(Plan {
                sql: String::new(),
                format: None,
                input: Some(InputSpec {
                    insert: statement,
                    format,
                }),
                first_data,
            })
        }
        InsertHead::NoFormat if body_follows && ends_with_word(text, "VALUES") => {
            // `INSERT INTO t VALUES` with the tuples in the body.
            let toks = tokens(text);
            let values = toks.last().map_or(text.len(), |t| t.start);
            Ok(Plan {
                sql: String::new(),
                format: None,
                input: Some(InputSpec {
                    insert: String::from_utf8_lossy(&text[..values])
                        .trim_end()
                        .to_string(),
                    format: "Values".to_string(),
                }),
                first_data: Vec::new(),
            })
        }
        _ => {
            let sql = String::from_utf8_lossy(text).into_owned();
            let (sql, format) = split_format(&sql);
            Ok(Plan {
                sql,
                format,
                input: None,
                first_data: Vec::new(),
            })
        }
    }
}

fn ends_with_word(text: &[u8], word: &str) -> bool {
    tokens(text).last().is_some_and(|t| word_is(text, t, word))
}

/// Strips a trailing `FORMAT <f>` (and `;`) from a statement, returning the format.
fn split_format(sql: &str) -> (String, Option<String>) {
    let bytes = sql.as_bytes();
    let toks: Vec<Token> = tokens(bytes)
        .into_iter()
        .filter(|t| !(bytes[t.start] == b';' && t.end == t.start + 1))
        .collect();
    let trimmed = || sql.trim_end().trim_end_matches(';').trim_end().to_string();
    if toks.len() >= 2 {
        let name = toks[toks.len() - 1];
        let keyword = toks[toks.len() - 2];
        if name.word && keyword.depth == 0 && word_is(bytes, &keyword, "FORMAT") {
            return (
                sql[..keyword.start].trim_end().to_string(),
                Some(sql[name.start..name.end].to_string()),
            );
        }
    }
    (trimmed(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_decode() {
        assert_eq!(
            parse_query("query=SELECT%201+%2B%201&x=&y"),
            vec![
                ("query".to_string(), "SELECT 1 + 1".to_string()),
                ("x".to_string(), String::new()),
                ("y".to_string(), String::new()),
            ]
        );
        assert_eq!(
            parse_query("a=%zz%4"),
            vec![("a".to_string(), "%zz%4".to_string())]
        );
    }

    #[test]
    fn read_only_keywords() {
        for sql in [
            "SELECT 1",
            " -- c\n select 1",
            "/* x */ WITH 1 AS a SELECT a",
            "(SELECT 1)",
            "SHOW TABLES",
            "EXPLAIN SELECT 1",
        ] {
            assert!(is_read(sql), "{sql}");
        }
        for sql in [
            "INSERT INTO t VALUES (1)",
            "SET a = 1",
            "CREATE TABLE t (a Int8)",
            "DROP TABLE t",
            "",
            "KILL QUERY WHERE 1",
        ] {
            assert!(!is_read(sql), "{sql}");
        }
    }

    #[test]
    fn insert_heads() {
        let text = b"INSERT INTO FUNCTION null('n UInt64') FORMAT TSV\n1\n2\n";
        match insert_head(text, true) {
            InsertHead::Insert {
                statement,
                format,
                data_start,
            } => {
                assert_eq!(statement, "INSERT INTO FUNCTION null('n UInt64')");
                assert_eq!(format, "TSV");
                assert_eq!(&text[data_start..], b"1\n2\n");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            insert_head(b"INSERT INTO t FORMAT TS", false),
            InsertHead::Incomplete
        );
        assert_eq!(
            insert_head(b"INSERT INTO t FORMAT TSV", false),
            InsertHead::Incomplete
        );
        assert!(matches!(
            insert_head(b"INSERT INTO t FORMAT TSV", true),
            InsertHead::Insert { .. }
        ));
        assert_eq!(
            insert_head(b"INSERT INTO t VALUES (1)", true),
            InsertHead::NoFormat
        );
        assert_eq!(
            insert_head(b"SELECT 1 FORMAT TSV", true),
            InsertHead::NotInsert
        );
        // `FORMAT` inside a string or parentheses is not the clause.
        assert_eq!(
            insert_head(b"INSERT INTO t SELECT 'FORMAT TSV', format('x')", true),
            InsertHead::NoFormat
        );
    }

    #[test]
    fn trailing_format() {
        assert_eq!(
            split_format("SELECT 1 FORMAT JSON"),
            ("SELECT 1".to_string(), Some("JSON".to_string()))
        );
        assert_eq!(
            split_format("SELECT 1 FORMAT JSON;"),
            ("SELECT 1".to_string(), Some("JSON".to_string()))
        );
        assert_eq!(split_format("SELECT 1;"), ("SELECT 1".to_string(), None));
        assert_eq!(
            split_format("SELECT 'FORMAT JSON'"),
            ("SELECT 'FORMAT JSON'".to_string(), None)
        );
        assert_eq!(
            split_format("SELECT format('x', 1)"),
            ("SELECT format('x', 1)".to_string(), None)
        );
    }

    #[test]
    fn plans() {
        let p = plan(b"INSERT INTO t VALUES", true).expect("plan");
        assert_eq!(
            p.input,
            Some(InputSpec {
                insert: "INSERT INTO t".to_string(),
                format: "Values".to_string()
            })
        );
        let p = plan(b"INSERT INTO t VALUES (1)", true).expect("plan");
        assert_eq!(p.input, None);
        assert_eq!(p.sql, "INSERT INTO t VALUES (1)");
    }

    #[test]
    fn dates() {
        assert_eq!(http_date(UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            http_date(UNIX_EPOCH + Duration::from_secs(1791500130)),
            "Thu, 08 Oct 2026 22:55:30 GMT"
        );
    }

    #[test]
    fn summary_numbers_are_strings() {
        let json = progress_json(&Progress {
            rows_read: 7,
            ..Progress::default()
        });
        assert!(json.starts_with("{\"read_rows\":\"7\""), "{json}");
        assert!(json.contains("\"total_rows_to_read\":\"0\""));
    }
}
