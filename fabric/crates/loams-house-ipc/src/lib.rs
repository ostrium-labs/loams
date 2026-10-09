//! `hsw1`: the frames between the Loams House front and its chDB workers
//! (design §49 §4.2, HS1 Task 2).
//!
//! The front (`loams-fabric house`, which links no libchdb) and the worker
//! (`loams-house-worker`, the only binary that does) talk over one Unix-domain
//! socket per worker. The worker inherits its end as fd 3. This crate is the whole
//! contract between them, and it carries no libchdb, so the front can depend on it.
//!
//! # The wire
//!
//! ```text
//! u32 length (big-endian, counts what follows) | u8 version | postcard(Frame)
//! ```
//!
//! A frame longer than [`MAX_FRAME_BYTES`] is refused before anything is
//! allocated for it, so a compromised worker cannot make the front reserve 4 GiB
//! with four bytes. A version byte other than [`PROTOCOL_VERSION`] is
//! [`CodecError::UnknownVersion`], and a worker that reads one exits with
//! [`EXIT_PROTOCOL`] (70, `EX_SOFTWARE`).
//!
//! # The conversation
//!
//! ```text
//! worker                                   front
//!   boots libchdb, connects
//!   Ready ------------------------------->
//!         <------------------------------- Bind          (once, before any Execute)
//!   Done | Error ------------------------>
//!         <------------------------------- Execute
//!         <------------------------------- Input* InputEnd   (only if Execute.input)
//!   (Chunk | Progress)* then
//!   Stats Done | Error ------------------>
//!         <------------------------------- Execute …
//! ```
//!
//! Every request (`Bind`, `Execute`) ends in exactly one terminal frame: `Done`
//! (success) or `Error` (failure). A successful `Execute` sends `Stats` just before
//! its `Done`. There is **no `Cancel` frame**: cancelling is `SIGKILL` of the
//! worker, and the front answers the client itself (§49 §4.2, §12).
//!
//! Postcard encodes an enum by variant index, so a front and a worker must be built
//! from the same [`Frame`]; the version byte is what changes when they cannot be.

use std::io::{self, Read, Write};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The protocol's name, for logs and the `Ready` check.
pub const PROTOCOL_NAME: &str = "hsw1";

/// The version byte every frame carries: `hsw1` is version 1.
pub const PROTOCOL_VERSION: u8 = 1;

/// The largest frame body, version byte included: 16 MiB. Bigger engine blocks
/// are split into [`Chunk`]s of at most [`CHUNK_BYTES`].
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// The most output bytes one [`Chunk`] carries: 4 MiB, well inside
/// [`MAX_FRAME_BYTES`] whatever else the frame holds.
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// The exit code of a worker that reads a frame it cannot understand: a version it
/// does not speak, or a body that does not decode. `EX_SOFTWARE` from `sysexits.h`.
pub const EXIT_PROTOCOL: i32 = 70;

/// The fd a worker finds its socket on.
pub const WORKER_SOCKET_FD: i32 = 3;

/// One `hsw1` frame.
///
/// The variant order is the wire encoding: append, never reorder.
// `Execute` is the large variant (it carries the statement and the session). A
// frame is built, encoded and dropped one at a time, never stored in bulk, so the
// size does not matter, and boxing it would only add an allocation per statement.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// worker → front, once: the engine is booted and the worker is unbound.
    Ready(Ready),
    /// front → worker, once: the namespace this worker serves from now on.
    Bind(Bind),
    /// front → worker: run one statement.
    Execute(Execute),
    /// front → worker: a piece of an `INSERT` body, in [`InputSpec::format`].
    Input(Bytes),
    /// front → worker: the `INSERT` body is complete.
    InputEnd,
    /// worker → front: output bytes in the statement's format.
    Chunk(Chunk),
    /// worker → front: counters so far, at most every 100 ms.
    Progress(Progress),
    /// worker → front: the final counters, just before `Done`.
    Stats(Progress),
    /// worker → front: the request failed. Terminal.
    Error {
        /// ClickHouse's code, name and message.
        error: EngineError,
        /// Whether the worker considers itself unfit for another statement (an FFI
        /// error class FL2 Ruling 11 lists as poisoning). The front recycles it.
        poisoned: bool,
    },
    /// worker → front: the request succeeded. Terminal.
    Done,
    /// front → worker: classify a statement with ClickHouse's parser, without
    /// running it (HS1 Task 4, FL2 Ruling 6). Answered by `Classified` or `Error`.
    Classify(String),
    /// worker → front: what `Classify` found. Terminal.
    Classified(Classification),
    /// front → worker: classify a statement and, when it is one statement
    /// ClickHouse can parse, explain its syntax tree (`EXPLAIN AST`, and `EXPLAIN
    /// QUERY TREE run_passes = 0` for a query) for the deny list (HS1 Task 5).
    /// Nothing runs and nothing is resolved, so no table function is opened.
    /// Answered by `Analyzed` or `Error`.
    Analyze(Analyze),
    /// worker → front: what `Analyze` found. Terminal.
    Analyzed(Analysis),
    /// front → worker, tests only: `abort()` now. What
    /// `crash_does_not_reach_the_front` uses to crash a worker on demand. Last, so
    /// a worker built without it fails to decode it and exits, which is also a crash.
    #[cfg(feature = "test-hooks")]
    Abort,
}

impl Frame {
    /// Whether this is the test-only `Abort`. Present in every build (always false
    /// without `test-hooks`), so the worker can test for it without a feature of
    /// its own.
    pub fn is_abort(&self) -> bool {
        #[cfg(feature = "test-hooks")]
        if matches!(self, Self::Abort) {
            return true;
        }
        false
    }

    /// The variant's name, for logs and protocol errors. Never the contents: an
    /// `Execute` carries user SQL and an `Input` user data.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ready(_) => "Ready",
            Self::Bind(_) => "Bind",
            Self::Execute(_) => "Execute",
            Self::Input(_) => "Input",
            Self::InputEnd => "InputEnd",
            Self::Chunk(_) => "Chunk",
            Self::Progress(_) => "Progress",
            Self::Stats(_) => "Stats",
            Self::Error { .. } => "Error",
            Self::Done => "Done",
            Self::Classify(_) => "Classify",
            Self::Classified(_) => "Classified",
            Self::Analyze(_) => "Analyze",
            Self::Analyzed(_) => "Analyzed",
            #[cfg(feature = "test-hooks")]
            Self::Abort => "Abort",
        }
    }
}

/// What a worker says when it is ready to be bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// The worker's [`PROTOCOL_VERSION`], repeated in the body so a front can log a
    /// mismatch it did not cause.
    pub protocol: u8,
    /// The worker's pid, as the worker sees it (inside a pid namespace this differs
    /// from the front's view; the front kills by its own).
    pub pid: u32,
    /// `chdb_version()`: `26.9.0`.
    pub chdb_version: String,
    /// `SELECT version()`: `26.9.2.1`, what `versions.rs` pins.
    pub clickhouse_version: String,
    /// Process start to `Ready`, in milliseconds.
    pub boot_ms: u32,
    /// Every setting name the engine knows (`system.settings`), so the front can
    /// tell an unknown setting (`115`) from a known, disallowed one (`164`)
    /// without libchdb (HS1 Task 4).
    pub settings: Vec<String>,
}

/// ClickHouse's class of a statement (`chdb_query_class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryClass {
    /// `SELECT`, `SHOW`, `DESCRIBE`, `EXPLAIN`, `EXISTS`, `CHECK`.
    ReadOnly,
    /// `INSERT`, `CREATE`, `ALTER`, `DROP`, …
    Mutating,
    /// Functions, access management, `system` writes.
    MutatingGlobal,
    /// `USE`, `SET`, `SYSTEM`, `KILL`, `INTO OUTFILE`, …
    Control,
    /// Did not parse.
    Unknown,
}

/// What `Classify` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    /// The class.
    pub class: QueryClass,
    /// Executable statements (0 when it does not parse).
    pub statements: u32,
}

/// A statement to analyse: its text exactly as chDB will parse it, and the
/// query parameters it will run with (`{name:Type}` substitution happens while
/// parsing, so an explain without them fails).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analyze {
    /// The statement, as chDB will run it.
    pub sql: String,
    /// `param_<name>` values.
    pub params: Vec<(String, String)>,
}

/// What `Analyze` found (HS1 Task 5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    /// ClickHouse's class, as `Classify` reports it.
    pub classification: Classification,
    /// `EXPLAIN AST` of the statement, one TSV-escaped row per line. Empty when
    /// the classification already refuses the statement (more than one, or
    /// `Unknown`), so nothing was explained.
    pub ast: String,
    /// `EXPLAIN QUERY TREE run_passes = 0` of the statement, the same way, when it
    /// is a query (`EXPLAIN QUERY TREE` takes nothing else).
    pub query_tree: Option<String>,
}

/// Binds a worker to one namespace for the rest of its life (§49 §10.1).
///
/// Carries no credential of any kind: the worker reaches the bucket only through
/// `house-cache`'s forwarder, which signs in the front (§49 §13.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bind {
    /// The namespace id.
    pub namespace: String,
    /// The namespace's isolation class (`shared`, `dedicated`, …).
    pub isolation_class: String,
    /// Engine settings for this namespace, applied to every statement before the
    /// session's own.
    pub settings: Vec<(String, String)>,
    /// The loopback endpoint the worker reads the bucket through
    /// (`http://127.0.0.1:<p>/`), once `house-cache` exists (HS1 Task 9).
    pub proxy_endpoint: Option<String>,
    /// The most bytes the worker's private temporary directory may hold.
    pub temp_dir_quota_bytes: u64,
}

/// One statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execute {
    /// The query id the client sees in `X-ClickHouse-Query-Id`.
    pub query_id: String,
    /// The House session this statement belongs to, when it needs one connection
    /// across statements (temporary tables). `None` runs on a fresh connection that
    /// is closed afterwards, so nothing leaks between statements.
    pub session: Option<SessionRef>,
    /// `SET`s for this statement, in order: the session's settings.
    pub settings: Vec<(String, String)>,
    /// DDL the front generated (the views of §49 §7), run on the worker's control
    /// connection before the statement.
    pub views: Vec<String>,
    /// The statement text, without a `FORMAT` clause.
    pub sql: String,
    /// The output format: `TSV`, `Native`, `Parquet`, …
    pub format: String,
    /// Query parameters (`{name:Type}`), bound by the engine.
    pub params: Vec<(String, String)>,
    /// The front's caps, applied after `settings` so they win.
    pub limits: Limits,
    /// When present, `Input` frames follow this one and are streamed into this
    /// `INSERT` before `sql` runs (HS1 R1.7).
    pub input: Option<InputSpec>,
}

/// A House session on a worker (Task 3 review, decision 4; Task 4 adds pinning).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    /// Opaque to the worker. The front makes it from the user **and** the
    /// `session_id`, so two users' sessions of the same id never meet.
    pub key: String,
    /// How long the session may sit idle on the worker before it is dropped.
    pub timeout_ms: u64,
    /// Drop the session once this statement is over (`close_session=1`).
    pub close: bool,
}

/// The key prefix that makes a worker treat a session's settings restore as failed
/// (test-only, `test-hooks`), so the retire-on-failure path can be exercised.
pub const TEST_FAIL_RESTORE: &str = "\u{0}loams-test-fail-restore/";

impl SessionRef {
    /// Whether this session asks the worker to fail its restore (always false
    /// without `test-hooks`).
    pub fn fails_restore_for_test(&self) -> bool {
        cfg!(feature = "test-hooks") && self.key.starts_with(TEST_FAIL_RESTORE)
    }
}

/// Where an `INSERT` body goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSpec {
    /// The `INSERT` statement without `FORMAT` or data: `INSERT INTO tmp`.
    pub insert: String,
    /// The body's format.
    pub format: String,
}

/// The per-statement caps of §49 §12, each applied as the chDB setting of the same
/// name when present.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// `max_execution_time`, in milliseconds (the setting takes seconds; the worker
    /// converts).
    pub max_execution_time_ms: Option<u64>,
    /// `max_memory_usage`.
    pub max_memory_usage: Option<u64>,
    /// `max_threads`.
    pub max_threads: Option<u32>,
    /// `max_bytes_to_read`.
    pub max_bytes_to_read: Option<u64>,
    /// `max_result_bytes`.
    pub max_result_bytes: Option<u64>,
    /// `max_result_rows`.
    pub max_result_rows: Option<u64>,
}

impl Limits {
    /// The limits as `(setting, value)` pairs, in a fixed order.
    pub fn as_settings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(ms) = self.max_execution_time_ms {
            // ClickHouse takes seconds, and accepts a fraction.
            out.push((
                "max_execution_time".to_string(),
                format!("{}.{:03}", ms / 1000, ms % 1000),
            ));
        }
        let mut push = |name: &str, value: Option<u64>| {
            if let Some(value) = value {
                out.push((name.to_string(), value.to_string()));
            }
        };
        push("max_memory_usage", self.max_memory_usage);
        push("max_threads", self.max_threads.map(u64::from));
        push("max_bytes_to_read", self.max_bytes_to_read);
        push("max_result_bytes", self.max_result_bytes);
        push("max_result_rows", self.max_result_rows);
        out
    }
}

/// Output bytes. A block bigger than [`CHUNK_BYTES`] arrives as several chunks, all
/// but the last with `continued` set, so a reader that needs whole engine blocks
/// (`Native` per HS1 R1.7, `ArrowStream` messages per Task 8) can reassemble them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// The bytes.
    pub bytes: Bytes,
    /// Whether the next chunk continues the same engine block.
    pub continued: bool,
}

impl Chunk {
    /// Splits one engine block into chunks of at most `limit` bytes. An empty
    /// block is no chunks.
    pub fn split(block: Bytes, limit: usize) -> Vec<Chunk> {
        let limit = limit.max(1);
        let mut out = Vec::with_capacity(block.len().div_ceil(limit));
        let mut rest = block;
        while !rest.is_empty() {
            let piece = rest.split_to(limit.min(rest.len()));
            out.push(Chunk {
                bytes: piece,
                continued: !rest.is_empty(),
            });
        }
        out
    }
}

/// The counters `X-ClickHouse-Summary` and `system.query_log` want, plus the
/// worker's memory, which the front's recycle rule reads (§49 §10.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Rows read from storage.
    pub rows_read: u64,
    /// Bytes read from storage.
    pub bytes_read: u64,
    /// Rows in the result so far.
    pub result_rows: u64,
    /// Bytes in the result so far.
    pub result_bytes: u64,
    /// Rows an `INSERT` body wrote.
    pub written_rows: u64,
    /// Bytes an `INSERT` body wrote.
    pub written_bytes: u64,
    /// Time since the statement started, in nanoseconds.
    pub elapsed_ns: u64,
    /// The worker's resident set now (`VmRSS`).
    pub rss_bytes: u64,
    /// The worker's peak resident set (`VmHWM`).
    pub peak_rss_bytes: u64,
    /// House sessions the worker holds now (HS1 Task 4).
    pub sessions: u32,
}

/// A ClickHouse error as the engine raised it: the parts `loams-house` renders.
///
/// This is `loams_chdb::ChdbError`'s shape without the dependency on libchdb, so the
/// front can carry and render it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    /// The ClickHouse code, or 0 for an engine failure that carried none.
    pub code: i32,
    /// The ClickHouse name: `UNKNOWN_TABLE`.
    pub name: String,
    /// The message without the trailing `. (NAME)`.
    pub message: String,
}

impl EngineError {
    /// ClickHouse's `ABORTED`.
    pub const ABORTED: i32 = 236;

    /// The text chDB's fatal-signal handler fails a running statement with,
    /// measured at 26.9.2.1: `The server is shutting down due to a fatal error`.
    pub const FATAL_TEXT: &'static str = "shutting down due to a fatal error";

    /// Whether this is chDB's fatal-signal path — code 236 **and** its text — so
    /// the process is going down (HS1 R2.4). Code alone is not enough: `ABORTED`
    /// has other uses, and a user must not be able to fake a crash.
    pub fn is_fatal(&self) -> bool {
        self.code == Self::ABORTED && self.message.contains(Self::FATAL_TEXT)
    }

    /// ClickHouse's own shape, `Code: 60. DB::Exception: … (UNKNOWN_TABLE)`.
    pub fn to_clickhouse_text(&self) -> String {
        format!(
            "Code: {}. DB::Exception: {}. ({})",
            self.code, self.message, self.name
        )
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_clickhouse_text())
    }
}

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The stream ended inside a frame.
    #[error("the stream ended inside an hsw1 frame")]
    Truncated,
    /// A length prefix of zero: no version byte.
    #[error("an hsw1 frame with an empty body")]
    Empty,
    /// A length prefix over [`MAX_FRAME_BYTES`].
    #[error("an hsw1 frame of {0} bytes is over the {MAX_FRAME_BYTES}-byte limit")]
    TooLarge(u32),
    /// A version byte this build does not speak.
    #[error("hsw1 version {0} is not {PROTOCOL_VERSION}")]
    UnknownVersion(u8),
    /// A body that does not decode as a [`Frame`], or has bytes left over.
    #[error("a malformed hsw1 frame: {0}")]
    Malformed(String),
    /// The socket failed.
    #[error("hsw1 i/o: {0}")]
    Io(#[from] io::Error),
}

/// Encodes and decodes [`Frame`]s.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameCodec;

impl FrameCodec {
    /// The frame's wire bytes: length, version, body.
    pub fn encode(frame: &Frame) -> Result<Vec<u8>, CodecError> {
        let payload =
            postcard::to_stdvec(frame).map_err(|err| CodecError::Malformed(err.to_string()))?;
        let body = u32::try_from(payload.len() + 1)
            .ok()
            .filter(|len| *len <= MAX_FRAME_BYTES)
            .ok_or(CodecError::TooLarge(
                u32::try_from(payload.len() + 1).unwrap_or(u32::MAX),
            ))?;
        let mut wire = Vec::with_capacity(payload.len() + 5);
        wire.extend_from_slice(&body.to_be_bytes());
        wire.push(PROTOCOL_VERSION);
        wire.extend_from_slice(&payload);
        Ok(wire)
    }

    /// Decodes one frame body (version byte first), as read after its length.
    pub fn decode_body(body: &[u8]) -> Result<Frame, CodecError> {
        let (&version, payload) = body.split_first().ok_or(CodecError::Empty)?;
        if version != PROTOCOL_VERSION {
            return Err(CodecError::UnknownVersion(version));
        }
        let (frame, rest) = postcard::take_from_bytes::<Frame>(payload)
            .map_err(|err| CodecError::Malformed(err.to_string()))?;
        if !rest.is_empty() {
            return Err(CodecError::Malformed(format!(
                "{} bytes after the {} frame",
                rest.len(),
                frame.kind()
            )));
        }
        Ok(frame)
    }

    /// Checks a length prefix.
    fn body_len(prefix: [u8; 4]) -> Result<usize, CodecError> {
        let len = u32::from_be_bytes(prefix);
        if len == 0 {
            return Err(CodecError::Empty);
        }
        if len > MAX_FRAME_BYTES {
            return Err(CodecError::TooLarge(len));
        }
        Ok(len as usize)
    }

    /// Writes one frame (blocking).
    pub fn write<W: Write>(writer: &mut W, frame: &Frame) -> Result<(), CodecError> {
        writer.write_all(&Self::encode(frame)?)?;
        Ok(())
    }

    /// Reads one frame (blocking). `Ok(None)` is a clean end: the stream closed
    /// between frames.
    pub fn read<R: Read>(reader: &mut R) -> Result<Option<Frame>, CodecError> {
        let mut prefix = [0u8; 4];
        match read_full(reader, &mut prefix)? {
            0 => return Ok(None),
            4 => {}
            _ => return Err(CodecError::Truncated),
        }
        let len = Self::body_len(prefix)?;
        let mut body = vec![0u8; len];
        if read_full(reader, &mut body)? != len {
            return Err(CodecError::Truncated);
        }
        Self::decode_body(&body).map(Some)
    }

    /// Writes one frame.
    pub async fn write_async<W: AsyncWrite + Unpin>(
        writer: &mut W,
        frame: &Frame,
    ) -> Result<(), CodecError> {
        writer.write_all(&Self::encode(frame)?).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Reads one frame. `Ok(None)` is a clean end, as for [`FrameCodec::read`].
    pub async fn read_async<R: AsyncRead + Unpin>(
        reader: &mut R,
    ) -> Result<Option<Frame>, CodecError> {
        let mut prefix = [0u8; 4];
        match read_full_async(reader, &mut prefix).await? {
            0 => return Ok(None),
            4 => {}
            _ => return Err(CodecError::Truncated),
        }
        let len = Self::body_len(prefix)?;
        let mut body = vec![0u8; len];
        if read_full_async(reader, &mut body).await? != len {
            return Err(CodecError::Truncated);
        }
        Self::decode_body(&body).map(Some)
    }
}

/// Reads until `buf` is full or the stream ends; returns how much was read.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

async fn read_full_async<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]).await {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn limits_become_settings_in_seconds() {
        let limits = Limits {
            max_execution_time_ms: Some(2_500),
            max_threads: Some(8),
            ..Limits::default()
        };
        assert_eq!(
            limits.as_settings(),
            vec![
                ("max_execution_time".to_string(), "2.500".to_string()),
                ("max_threads".to_string(), "8".to_string()),
            ]
        );
    }
}
