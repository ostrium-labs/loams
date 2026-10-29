//! The worker's side of `hsw1`: boot, `Ready`, one `Bind`, then `Execute`s.
//!
//! # Connections (HS1 R1.9)
//!
//! The worker holds a **control connection** for the front's DDL (the views of
//! §49 §7, run before each statement that names them) and opens a **user
//! connection** with `--readonly=2` for every statement, or one per House session
//! when the statement names one. `readonly = 2` is sticky: the user cannot `SET` it
//! away, and it refuses DDL, while `SET` of other settings, temporary tables and
//! `INSERT` into them still work. A fresh user connection costs about a millisecond
//! and guarantees that nothing a statement `SET` reaches the next one.
//!
//! At boot the control connection replaces chDB's `Overlay`/`Filesystem`
//! `default` database with a `Memory` one (R1.9: there, `FROM 'x.csv'` reads host
//! files and `CREATE VIEW` needs `FILE`). The drop and the create run back to back
//! before anything else connects: a `default` dropped and not recreated breaks
//! every later `chdb_connect`.
//!
//! # Threads
//!
//! A reader thread decodes frames off the socket and hands them over a channel;
//! the main thread runs statements and writes. So an end of the socket is seen even
//! while a statement holds the main thread, and the test-only `Abort` acts at once.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use loams_chdb::{ChdbError, Engine, EngineConfig, Session, SessionId, Settings};
use loams_house_ipc::{
    Analysis, Analyze, Bind, CHUNK_BYTES, Chunk, Classification, CodecError, EngineError, Execute,
    Frame, FrameCodec, PROTOCOL_VERSION, Progress, QueryClass, Ready, SandboxProbe,
};

use crate::config::{self, WorkerArgs};
use crate::document::{Plan, Shaper};

/// How often a running statement reports `Progress`, at most.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// How often the serve loop sweeps idle sessions when no frame comes.
pub const SESSION_SWEEP: Duration = Duration::from_secs(1);

/// The largest analysis (both explains) a worker sends: well inside a frame
/// ([`loams_house_ipc::MAX_FRAME_BYTES`]). A statement whose tree is larger is
/// refused (`36`), never run unchecked.
pub const MAX_ANALYSIS_BYTES: usize = 8 * 1024 * 1024;

/// What the worker ends a statement's text with before explaining it, so that
/// the explains are TSV whatever the statement says (HS1 Task 5 fix round 1). On
/// a line of its own: a trailing `--` comment cannot swallow it.
pub const PINNED_FORMAT: &str = "\nFORMAT TabSeparated";

/// The most House sessions one worker keeps; past it the least recently used is
/// dropped (Task 3 review, decision 4).
pub const MAX_SESSIONS: usize = 64;

/// A session's connection and when it was last used.
#[derive(Debug)]
struct SessionSlot {
    session: Session,
    last_used: Instant,
    timeout: Duration,
}

/// The query-level arguments of every user connection: L2 (§49 §13.2), in
/// [`crate::settings`].
pub use crate::settings::USER_CONNECTION_ARGS;

/// Where the serve loop runs, which decides what the end of the socket does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hosting {
    /// In its own process (`loams-house-worker`): when the socket ends, the reader
    /// thread `_exit`s the process at once, even while a statement holds the main
    /// thread (HS1 Task 2 review I3). The label names the worker in the log line.
    Process(String),
    /// On a thread of the front (`InprocWorker`): the end is handed back to the
    /// serve loop, which returns it; the front's process must never exit here.
    InProcess,
}

/// Why [`Worker::serve`] returned.
#[derive(Debug)]
pub enum End {
    /// The front closed the socket between frames.
    Closed,
    /// A frame the worker cannot understand: exit with
    /// [`loams_house_ipc::EXIT_PROTOCOL`].
    Protocol(String),
    /// The socket failed.
    Io(String),
}

impl End {
    /// The process exit code for this end: 0 for a closed socket, 70 for a
    /// protocol error ([`loams_house_ipc::EXIT_PROTOCOL`]), 1 for a socket error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Closed => 0,
            Self::Protocol(_) => loams_house_ipc::EXIT_PROTOCOL,
            Self::Io(_) => 1,
        }
    }

    /// Logs the end (a closed socket is the normal end and is not logged) and
    /// `_exit`s with [`End::exit_code`].
    pub fn exit_process(&self, label: &str) -> ! {
        match self {
            Self::Closed => {}
            Self::Protocol(why) => eprintln!("loams-house-worker {label}: hsw1: {why}"),
            Self::Io(why) => eprintln!("loams-house-worker {label}: socket: {why}"),
        }
        loams_chdb_sys::process::exit_now(self.exit_code())
    }
}

/// A booted worker.
#[derive(Debug)]
pub struct Worker {
    engine: &'static Engine,
    control: Session,
    bound: Option<Bind>,
    sessions: HashMap<String, SessionSlot>,
    ready: Ready,
    /// Whether this worker is its engine's only user (its own process), so it
    /// may drop every view database between statements (HS1 Task 11). In-process
    /// workers share one engine and never do.
    owns_engine: bool,
    /// Whether the last statement left read views behind.
    views_live: bool,
}

/// The engine configuration a worker with these arguments runs.
pub fn engine_config(args: &WorkerArgs) -> EngineConfig {
    EngineConfig {
        tmp_dir: args.data_dir(),
        cache_dir: args.tmp_dir.join("no-cache"),
        cache_bytes: 0,
        max_server_memory: boot_memory_limit(args.memory_limit, available_memory()),
        // HS1 R1.4: no `filesystem_caches_path`, so nothing is cached on disk.
        filesystem_cache: false,
        server_args: vec![format!("--config-file={}", args.config_file().display())],
        // §49 §4.1: the worker is chDB's process, so chDB keeps its own handlers.
        install_signal_handlers: true,
    }
}

/// The smallest ceiling a worker boots with when memory is short.
pub const MIN_BOOT_MEMORY: u64 = 256 * 1024 * 1024;

/// `MemAvailable` from `/proc/meminfo`, in bytes; `None` where there is none.
pub fn available_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// The `--max_server_memory_usage` a worker boots with: its limit, or 90% of
/// the memory available now when that is less (never under
/// [`MIN_BOOT_MEMORY`]).
///
/// BQ1-house (2026-10-28), measured with chDB 26.9: `chdb_connect` returns
/// null, with no error text, when `max_server_memory_usage` is above the
/// machine's available memory at boot (9 GiB held by another process, 1.6 GiB
/// available: 4.5 GiB and 2 GB failed, 1 GB booted). Under memory pressure
/// every worker the pool started failed this way, and statements failed with
/// `210`. A worker booted with the smaller ceiling answers what fits and
/// `241` past it, as it would with that much memory.
pub fn boot_memory_limit(limit: u64, available: Option<u64>) -> u64 {
    match available {
        Some(available) if available < limit => {
            let clamped = (available / 10 * 9).max(MIN_BOOT_MEMORY).min(limit);
            eprintln!(
                "loams-house-worker: {} MiB available, below the worker's {} MiB memory \
                 limit; booting with {} MiB",
                available >> 20,
                limit >> 20,
                clamped >> 20
            );
            clamped
        }
        _ => limit,
    }
}

impl Worker {
    /// Writes the config, moves into the private directory, starts the engine and
    /// prepares the control connection. `started` is when the process started, for
    /// `Ready::boot_ms`.
    pub fn boot(args: &WorkerArgs, started: Instant) -> Result<Self, EngineError> {
        config::write_files(args).map_err(|err| loams_error("CANNOT_WRITE_CONFIG", err))?;
        // HS1 R1.10: `INTO OUTFILE` and `file()` resolve relative paths against
        // the working directory, so it is the worker's own.
        std::env::set_current_dir(args.files_dir())
            .map_err(|err| loams_error("CANNOT_CHDIR", err))?;
        let mut worker = Self::boot_on(engine_config(args), started)?;
        worker.ready.s3_endpoint = args.s3_endpoint.clone();
        Ok(worker)
    }

    /// Starts (or joins) the engine `config` describes and prepares a worker on it.
    pub fn boot_on(config: EngineConfig, started: Instant) -> Result<Self, EngineError> {
        let engine = Engine::start(config).map_err(|err| engine_error(&err))?;
        let control = engine
            .session(SessionId::new("control"), &Settings::new())
            .map_err(|err| engine_error(&err))?;
        replace_default_database(&control).map_err(|err| engine_error(&err))?;
        let settings = control
            .query("SELECT name FROM system.settings", "TSVRaw", &[])
            .map(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .lines()
                    .map(str::to_string)
                    .collect()
            })
            .map_err(|err| engine_error(&err))?;
        let ready = Ready {
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            chdb_version: engine.chdb_version().to_string(),
            clickhouse_version: engine.version().to_string(),
            boot_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
            settings,
            s3_endpoint: None,
        };
        Ok(Self {
            engine,
            control,
            bound: None,
            sessions: HashMap::new(),
            ready,
            owns_engine: false,
            views_live: false,
        })
    }

    /// The `Ready` frame this worker sends.
    pub fn ready(&self) -> &Ready {
        &self.ready
    }

    /// Sends `Ready`, then serves frames until the socket ends.
    pub fn serve<R, W>(mut self, reader: R, mut writer: W, hosting: Hosting) -> End
    where
        R: Read + Send + 'static,
        W: Write,
    {
        self.owns_engine = matches!(hosting, Hosting::Process(_));
        let frames = spawn_reader(reader, hosting);
        if let Err(err) = send(&mut writer, &Frame::Ready(self.ready.clone())) {
            return err;
        }
        loop {
            // Idle sessions expire on a timer too, not only when a statement comes
            // (fix round 2, N9).
            let frame = match frames.recv_timeout(SESSION_SWEEP) {
                Ok(Ok(frame)) => frame,
                Ok(Err(end)) => return end,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.expire_sessions();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return End::Closed,
            };
            let answer = match frame {
                Frame::Bind(bind) => self.bind(bind),
                Frame::Classify(sql) => self.classify(&sql),
                Frame::Analyze(analyze) => self.analyze(&analyze),
                Frame::Execute(execute) => match self.execute(execute, &frames, &mut writer) {
                    Ok(answer) => answer,
                    Err(end) => return end,
                },
                // Test-only (`Frame::Probe` exists only with `test-hooks`).
                other if other.probe().is_some() => other.probe().map(probe).unwrap_or_default(),
                other => {
                    return End::Protocol(format!(
                        "a worker does not expect {} here",
                        other.kind()
                    ));
                }
            };
            for frame in answer {
                if let Err(err) = send(&mut writer, &frame) {
                    return err;
                }
            }
        }
    }

    /// `Bind`: once per worker, before any `Execute`.
    fn bind(&mut self, bind: Bind) -> Vec<Frame> {
        if let Some(bound) = &self.bound {
            // A worker serves one namespace for its whole life (§49 §10.1). A
            // second `Bind` is a front bug, and the worker refuses to be reused.
            return vec![error_frame(
                EngineError {
                    code: 0,
                    name: "LOAMS_ALREADY_BOUND".to_string(),
                    message: format!(
                        "this worker is bound to namespace {:?} and cannot serve {:?}",
                        bound.namespace, bind.namespace
                    ),
                },
                true,
            )];
        }
        if let Some(bad) = bind
            .settings
            .iter()
            .find(|(name, _)| !is_setting_name(name))
        {
            return vec![error_frame(bad_setting(&bad.0), true)];
        }
        self.bound = Some(bind);
        vec![Frame::Done]
    }

    /// `Execute`, including its `Input` frames. `Err` ends the serve loop.
    fn execute<W: Write>(
        &mut self,
        execute: Execute,
        frames: &mpsc::Receiver<Result<Frame, End>>,
        writer: &mut W,
    ) -> Result<Vec<Frame>, End> {
        let mut input_failure = None;
        let result = self.run(&execute, frames, writer, &mut input_failure);
        // An `INSERT` body that failed part-way still has to be read to its
        // `InputEnd`, or the next frames would be taken for a new request.
        if let Some(true) = input_failure {
            drain_input(frames)?;
        }
        Ok(match result {
            Ok(mut stats) => {
                stats.sessions = u32::try_from(self.sessions.len()).unwrap_or(u32::MAX);
                vec![Frame::Stats(stats), Frame::Done]
            }
            Err(Failure::Engine(err)) => {
                // Only chDB's fatal-signal path leaves no worker to trust. Code 0
                // is a Loams-side refusal — SQL holding a NUL byte, an unknown
                // setting name — which a user can cause at will, so it must not
                // retire the worker (HS1 Task 2 review M2).
                let poisoned = err.is_fatal();
                vec![error_frame(err, poisoned)]
            }
            Err(Failure::Wire(end)) => return Err(end),
        })
    }

    /// Runs one statement, writing its `Chunk` and `Progress` frames; the answer
    /// is the final counters.
    fn run<W: Write>(
        &mut self,
        execute: &Execute,
        frames: &mpsc::Receiver<Result<Frame, End>>,
        writer: &mut W,
        input_failure: &mut Option<bool>,
    ) -> Result<Progress, Failure> {
        let Some(bind) = self.bound.clone() else {
            if execute.input.is_some() {
                *input_failure = Some(true);
            }
            return Err(Failure::Engine(EngineError {
                code: 0,
                name: "LOAMS_NOT_BOUND".to_string(),
                message: "a worker runs nothing before it is bound".to_string(),
            }));
        };
        if execute.input.is_some() {
            *input_failure = Some(true);
        }

        // Every entry is checked before any runs (HS1 R11.11): a batch with one
        // refused entry makes nothing, which matters most to in-process workers,
        // which never drop views between statements.
        for view in &execute.views {
            self.check_control_ddl(view).map_err(Failure::Engine)?;
        }

        // The read views of §49 §7 are the statement's own (HS1 Task 11): what an
        // earlier statement made is gone before this one's are made, so a name it
        // does not name, or a pin it did not take, is never read.
        if self.owns_engine && self.views_live {
            drop_views(&self.control).map_err(|err| Failure::Engine(engine_error(&err)))?;
            self.views_live = false;
        }
        if !execute.views.is_empty() {
            self.views_live = true;
        }
        for view in &execute.views {
            self.control
                .execute_simple(view)
                .map_err(|err| Failure::Engine(engine_error(&err)))?;
        }

        let now = Instant::now();
        let Some(spec) = &execute.session else {
            let fresh = self.user_connection(&execute.query_id)?;
            use_database(&fresh, execute)?;
            apply_settings(&fresh, &bind, execute)?;
            return statement(&fresh, execute, frames, writer, input_failure);
        };

        // Sessions on this worker (Task 3 review, decision 4): idle ones expire,
        // the least recently used goes past `MAX_SESSIONS`, and `close` drops one
        // after its statement.
        self.expire_sessions();
        if !self.sessions.contains_key(&spec.key) {
            if self.sessions.len() >= MAX_SESSIONS
                && let Some(oldest) = self
                    .sessions
                    .iter()
                    .min_by_key(|(_, slot)| slot.last_used)
                    .map(|(key, _)| key.clone())
            {
                self.sessions.remove(&oldest);
            }
            let session = self.user_connection(&spec.key)?;
            self.sessions.insert(
                spec.key.clone(),
                SessionSlot {
                    session,
                    last_used: now,
                    timeout: Duration::from_millis(spec.timeout_ms),
                },
            );
        }
        let slot = self
            .sessions
            .get_mut(&spec.key)
            .ok_or_else(|| Failure::Engine(loams_error("SESSION_LOST", &spec.key)))?;
        slot.timeout = Duration::from_millis(spec.timeout_ms);

        // URL settings and limits are for this statement only: what they replace is
        // read first and put back after, unless the statement set it itself.
        let names: Vec<String> = execute
            .settings
            .iter()
            .cloned()
            .chain(execute.limits.as_settings())
            .map(|(name, _)| name)
            .filter(|name| is_setting_name(name))
            .collect();
        let before = snapshot(&slot.session, &names);
        let applied_ok = use_database(&slot.session, execute)
            .and_then(|()| apply_settings(&slot.session, &bind, execute));
        let applied = snapshot(&slot.session, &names);
        let result = match applied_ok {
            Ok(()) => statement(&slot.session, execute, frames, writer, input_failure),
            Err(failure) => Err(failure),
        };
        let after = snapshot(&slot.session, &names);
        let restored = match (&before, &applied, &after) {
            (Ok(before), Ok(applied), Ok(after)) => {
                restore(&slot.session, before, applied, after) && !spec.fails_restore_for_test()
            }
            _ => false,
        };
        slot.last_used = Instant::now();
        // A session whose settings could not be read or put back is retired rather
        // than left with the request's settings in it (N9).
        if spec.close || !restored {
            self.sessions.remove(&spec.key);
        }
        result
    }

    /// Whether the control connection may run `sql` (HS1 R11.11). It is not
    /// read-only, and chDB has one user (`--user=` is ignored, `CREATE USER`
    /// and `EXECUTE AS` are refused, R1.8), so no grant can confine it to view
    /// DDL; the worker does: one statement, ClickHouse's own parser says so, of
    /// one of the shapes [`control_ddl_shape`] takes.
    fn check_control_ddl(&self, sql: &str) -> Result<(), EngineError> {
        let one = matches!(
            self.control.classify(sql),
            Ok(analysis) if analysis.statements == 1
        );
        if one && control_ddl_shape(sql) {
            return Ok(());
        }
        Err(EngineError {
            code: 497,
            name: "ACCESS_DENIED".to_string(),
            message: "the worker's control connection runs only the House's view DDL \
                      (one CREATE DATABASE … ENGINE = Memory, CREATE OR REPLACE VIEW, \
                      DROP TABLE IF EXISTS, CREATE OR REPLACE FUNCTION or CREATE \
                      DICTIONARY over a view)"
                .to_string(),
        })
    }

    /// `Classify`: ClickHouse's parser on the control connection, nothing run
    /// (HS1 Task 4, FL2 Ruling 6).
    fn classify(&self, sql: &str) -> Vec<Frame> {
        match self.control.classify(sql) {
            Ok(analysis) => vec![Frame::Classified(Classification {
                class: match analysis.class {
                    loams_chdb::QueryClass::ReadOnly => QueryClass::ReadOnly,
                    loams_chdb::QueryClass::Mutating => QueryClass::Mutating,
                    loams_chdb::QueryClass::MutatingGlobal => QueryClass::MutatingGlobal,
                    loams_chdb::QueryClass::Control => QueryClass::Control,
                    loams_chdb::QueryClass::Unknown => QueryClass::Unknown,
                },
                statements: analysis.statements,
            })],
            Err(err) => vec![error_frame(engine_error(&err), false)],
        }
    }

    /// `Analyze`: ClickHouse's class and, for one statement it can parse, its
    /// syntax trees for the front's deny list (HS1 Task 5). Both explains are
    /// syntax only — `EXPLAIN AST`, and `EXPLAIN QUERY TREE run_passes = 0`, which
    /// builds the tree without resolving it — because a resolved tree opens what
    /// it names: `EXPLAIN QUERY TREE SELECT * FROM url('http://…')` connects to
    /// infer the schema (measured, Task 5). They run on the control connection
    /// with the statement's parameters; nothing is executed.
    fn analyze(&self, analyze: &Analyze) -> Vec<Frame> {
        let classification = match self.classify(&analyze.sql).pop() {
            Some(Frame::Classified(classification)) => classification,
            Some(other) => return vec![other],
            None => return Vec::new(),
        };
        let mut analysis = Analysis {
            classification,
            ast: String::new(),
            query_tree: None,
        };
        if classification.statements != 1 || classification.class == QueryClass::Unknown {
            // The front refuses these on the class alone (`62`).
            return vec![Frame::Analyzed(analysis)];
        }
        // The explains are TSV, always (fix round 1): a statement's own
        // top-level `FORMAT` (one the front did not strip, as in `… FORMAT JSON
        // SETTINGS …`) would be the explain's output format, and the deny list
        // would read JSON. The worker ends the text with `FORMAT TabSeparated`;
        // a statement that already has one no longer parses, and is refused.
        // An `INSERT`'s `FORMAT` names its data, never the explain's (measured):
        // it is explained as it came.
        let pinned = format!("{}{PINNED_FORMAT}", analyze.sql);
        let parses = matches!(
            self.classify(&pinned).pop(),
            Some(Frame::Classified(c)) if c.statements == 1 && c.class != QueryClass::Unknown
        );
        let text = if parses { &pinned } else { &analyze.sql };
        let explain = |kind: &str| {
            self.control
                .query(&format!("EXPLAIN {kind} {text}"), "TSV", &analyze.params)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        analysis.ast = match explain("AST") {
            Ok(ast) => ast,
            Err(err) => return vec![error_frame(engine_error(&err), false)],
        };
        if !parses && !analysis.ast.starts_with("InsertQuery ") {
            return vec![error_frame(
                EngineError {
                    code: 62,
                    name: "SYNTAX_ERROR".to_string(),
                    message: "FORMAT is read only as a statement's last clause on the House \
                              (put SETTINGS before FORMAT)"
                        .to_string(),
                },
                false,
            )];
        }
        if classification.class == QueryClass::ReadOnly {
            // Only a query has a query tree; `SHOW`, `DESCRIBE` and `EXPLAIN`
            // fail to parse here and keep their AST alone.
            analysis.query_tree = explain("QUERY TREE run_passes = 0").ok();
        }
        let size = analysis.ast.len() + analysis.query_tree.as_ref().map_or(0, String::len);
        if size > MAX_ANALYSIS_BYTES {
            return vec![error_frame(
                EngineError {
                    code: 36,
                    name: "BAD_ARGUMENTS".to_string(),
                    message: format!(
                        "the statement's syntax tree is {size} bytes, more than the \
                         {MAX_ANALYSIS_BYTES} the House analyses"
                    ),
                },
                false,
            )];
        }
        vec![Frame::Analyzed(analysis)]
    }

    /// Drops sessions idle past their timeout.
    fn expire_sessions(&mut self) {
        let now = Instant::now();
        self.sessions
            .retain(|_, slot| now.duration_since(slot.last_used) <= slot.timeout);
    }

    /// A user connection: the engine's arguments plus `--readonly=2`.
    fn user_connection(&self, id: &str) -> Result<Session, Failure> {
        let args: Vec<String> = USER_CONNECTION_ARGS.iter().map(|a| a.to_string()).collect();
        self.engine
            .session_with_args(SessionId::new(id), &Settings::new(), &args)
            .map_err(|err| Failure::Engine(engine_error(&err)))
    }
}

/// Why a statement stopped.
enum Failure {
    /// The engine (or the worker) refused it: an `Error` frame.
    Engine(EngineError),
    /// The socket is gone or out of step: the serve loop ends.
    Wire(End),
}

/// One statement on `session`: the settings, the `INSERT` body if any, the query.
fn statement<W: Write>(
    session: &Session,
    execute: &Execute,
    frames: &mpsc::Receiver<Result<Frame, End>>,
    writer: &mut W,
    input_failure: &mut Option<bool>,
) -> Result<Progress, Failure> {
    let mut stats = Progress::default();
    if let Some(spec) = &execute.input {
        let mut insert = session
            .insert(&spec.insert, &spec.format)
            .map_err(|err| Failure::Engine(engine_error(&err)))?;
        loop {
            match frames.recv() {
                Ok(Ok(Frame::Input(bytes))) => insert
                    .append(&bytes)
                    .map_err(|err| Failure::Engine(engine_error(&err)))?,
                Ok(Ok(Frame::InputEnd)) => break,
                Ok(Ok(other)) => {
                    return Err(Failure::Wire(End::Protocol(format!(
                        "{} inside an INSERT body",
                        other.kind()
                    ))));
                }
                Ok(Err(end)) => return Err(Failure::Wire(end)),
                Err(_) => return Err(Failure::Wire(End::Closed)),
            }
        }
        // The body is read to its end, whatever happens next.
        *input_failure = Some(false);
        let summary = insert
            .finish()
            .map_err(|err| Failure::Engine(engine_error(&err)))?;
        stats.written_rows = summary.rows;
        stats.written_bytes = summary.bytes;
        stats.elapsed_ns = (summary.elapsed * 1e9) as u64;
    }

    if execute.sql.trim().is_empty() {
        fill_memory(&mut stats);
        return Ok(stats);
    }

    let mut shaper = match Shaper::plan(&execute.output, &execute.format) {
        Plan::Stream(shaper) => shaper,
        Plan::Whole { max_bytes } => {
            // One output format for the whole result (R8.12): chDB formats it in
            // one piece, held to the House's result cap.
            let (bytes, counters) = session
                .query_with_stats(&execute.sql, &execute.format, &execute.params)
                .map_err(|err| Failure::Engine(engine_error(&err)))?;
            if bytes.len() as u64 > max_bytes {
                return Err(Failure::Engine(over_the_cap(
                    &execute.format,
                    bytes.len(),
                    max_bytes,
                )));
            }
            send_bytes(writer, Bytes::from(bytes))?;
            absorb(&mut stats, &counters);
            fill_memory(&mut stats);
            return Ok(stats);
        }
    };
    let started = session.execute_with_id(
        &execute.query_id,
        &execute.sql,
        &execute.format,
        &execute.params,
    );
    let mut stream = match started {
        Ok(stream) => stream,
        Err(err) if is_not_streamable(&err) => {
            // DDL, `SET`-like and `INSERT … VALUES` statements cannot stream;
            // they run buffered and answer in one chunk.
            let bytes = session
                .query(&execute.sql, &execute.format, &execute.params)
                .map_err(|err| Failure::Engine(engine_error(&err)))?;
            send_bytes(writer, Bytes::from(bytes))?;
            fill_memory(&mut stats);
            return Ok(stats);
        }
        Err(err) => return Err(Failure::Engine(engine_error(&err))),
    };
    let mut last_progress = Instant::now();
    loop {
        match stream.next_chunk() {
            Ok(Some(block)) => {
                let shaped = shaper.push(block).map_err(Failure::Engine)?;
                send_bytes(writer, shaped)?;
                if last_progress.elapsed() >= PROGRESS_EVERY {
                    last_progress = Instant::now();
                    let mut progress = stats;
                    absorb(&mut progress, &stream.stats());
                    fill_memory(&mut progress);
                    send(writer, &Frame::Progress(progress)).map_err(Failure::Wire)?;
                }
            }
            Ok(None) => break,
            Err(err) => return Err(Failure::Engine(engine_error(&err))),
        }
    }
    let tail = shaper.finish().map_err(Failure::Engine)?;
    send_bytes(writer, tail)?;
    absorb(&mut stats, &stream.stats());
    fill_memory(&mut stats);
    Ok(stats)
}

/// The namespace's settings, the request's (URL) settings and the limits, one
/// `SET` each, in that order.
fn apply_settings(session: &Session, bind: &Bind, execute: &Execute) -> Result<(), Failure> {
    let settings = bind
        .settings
        .iter()
        .chain(execute.settings.iter())
        .cloned()
        .chain(execute.limits.as_settings());
    for (name, value) in settings {
        if !is_setting_name(&name) {
            return Err(Failure::Engine(bad_setting(&name)));
        }
        let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
        session
            .execute_simple(&format!("SET {name} = '{escaped}'"))
            .map_err(|err| Failure::Engine(engine_error(&err)))?;
    }
    Ok(())
}

/// The current values of `names` on a session's connection. A name the engine
/// does not know is simply absent: its `SET` fails the statement with `115`.
fn snapshot(session: &Session, names: &[String]) -> Result<Vec<(String, String)>, ChdbError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let list = names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT name, value FROM system.settings WHERE name IN ({list})");
    let bytes = session.query(&sql, "TSVRaw", &[])?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect())
}

/// Puts back what the request's settings replaced — but only where the value is
/// still the one the request set: a `SET` the statement made itself stays, as a
/// ClickHouse session keeps it (fix round 2, N9). `false` if a `SET` failed.
fn restore(
    session: &Session,
    before: &[(String, String)],
    applied: &[(String, String)],
    after: &[(String, String)],
) -> bool {
    let value = |list: &[(String, String)], name: &str| {
        list.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    for (name, old) in before {
        if value(after, name) != value(applied, name) {
            continue;
        }
        let escaped = old.replace('\\', "\\\\").replace('\'', "\\'");
        if session
            .execute_simple(&format!("SET {name} = '{escaped}'"))
            .is_err()
        {
            return false;
        }
    }
    true
}

/// The databases a worker never drops: the engine's own and `default`, which
/// it empties instead (a dropped `default` breaks later connects, R1.9).
const KEPT_DATABASES: &[&str] = &[
    "system",
    "information_schema",
    "INFORMATION_SCHEMA",
    "default",
    "_temporary_and_external_tables",
];

/// A name in backquotes.
fn quoted_name(name: &str) -> String {
    format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
}

/// A name as the front prints it: a plain word, or back-quoted with `\\` and
/// `` \` `` escaped. Returns the text after it.
fn after_name(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    match bytes.first()? {
        b'`' => {
            let mut at = 1;
            while at < bytes.len() {
                match bytes[at] {
                    b'\\' => at += 2,
                    b'`' => return Some(&text[at + 1..]),
                    _ => at += 1,
                }
            }
            None
        }
        first if first.is_ascii_alphabetic() || *first == b'_' => {
            let end = bytes
                .iter()
                .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
                .unwrap_or(bytes.len());
            Some(&text[end..])
        }
        _ => None,
    }
}

/// `<name> <type>, …)`, a read view's column list after its `(` (HS1 R-ACL.1),
/// returning the text after the `)`. Each type is held to the form
/// `loams_house::views` prints: names, numbers, `( ) , = - + .`, spaces and
/// string literals with no quote, backslash or line break in them; never `--`.
fn after_columns(text: &str) -> Option<&str> {
    let mut rest = text;
    loop {
        rest = after_name(rest)?.strip_prefix(' ')?;
        rest = after_type(rest)?;
        if let Some(next) = rest.strip_prefix(", ") {
            rest = next;
        } else {
            return rest.strip_prefix(')');
        }
    }
}

/// One column type, returning the text from the `,` or `)` that ends it. At the
/// top level only a name and its parenthesised arguments; inside them also
/// spaces, `, = - + .` and string literals.
fn after_type(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_literal = false;
    let mut previous = ' ';
    for (at, ch) in text.char_indices() {
        if in_literal {
            match ch {
                '\'' => in_literal = false,
                '\\' | '\n' | '\r' => return None,
                _ => {}
            }
            continue;
        }
        match ch {
            '\'' if depth > 0 => in_literal = true,
            '(' => depth += 1,
            ')' | ',' if depth == 0 => return (at > 0).then(|| &text[at..]),
            ')' => depth -= 1,
            '-' if previous == '-' => return None,
            // At the top a type is one name, or a name and its arguments: a
            // space there would start something else (`DEFAULT …`).
            c if c.is_ascii_alphanumeric() || c == '_' => {}
            c if depth > 0 && " ,=-+.".contains(c) => {}
            _ => return None,
        }
        previous = ch;
    }
    None
}

/// `<name>.<name>`, returning the text after it.
fn after_qualified(text: &str) -> Option<&str> {
    after_name(after_name(text)?.strip_prefix('.')?)
}

/// The shapes of the front's view DDL (`loams_house::views`, `system`): `CREATE
/// DATABASE IF NOT EXISTS d ENGINE = Memory`, `CREATE OR REPLACE VIEW d.v AS\n…`,
/// a read view of the bucket `CREATE OR REPLACE VIEW d.v (<name> <type>, …) SQL
/// SECURITY NONE AS\n…` (HS1 R-ACL.1), `DROP TABLE IF EXISTS d.t`, and a SQL
/// function `CREATE OR REPLACE FUNCTION f AS …` (HE1 R14.11), exactly.
pub fn control_ddl_shape(sql: &str) -> bool {
    if let Some(rest) = sql.strip_prefix("CREATE DATABASE IF NOT EXISTS ") {
        return after_name(rest) == Some(" ENGINE = Memory");
    }
    if let Some(rest) = sql.strip_prefix("CREATE OR REPLACE VIEW ") {
        let Some(after) = after_qualified(rest) else {
            return false;
        };
        if after.starts_with(" AS\n") {
            return true;
        }
        return after
            .strip_prefix(" (")
            .and_then(after_columns)
            .is_some_and(|body| body.starts_with(" SQL SECURITY NONE AS\n"));
    }
    if let Some(rest) = sql.strip_prefix("DROP TABLE IF EXISTS ") {
        return after_qualified(rest) == Some("");
    }
    // HE1 R14.12: a dictionary over one of the statement's read views.
    if let Some(rest) = sql.strip_prefix("CREATE DICTIONARY ") {
        return dictionary_shape(rest);
    }
    // HE1 R14.11: one of the namespace's SQL functions, whose lambda the
    // front held to the deny list.
    if let Some(rest) = sql.strip_prefix("CREATE OR REPLACE FUNCTION ") {
        return after_name(rest)
            .and_then(|after| after.strip_prefix(" AS "))
            .is_some_and(|lambda| !lambda.trim().is_empty());
    }
    false
}

/// After `CREATE DICTIONARY `: `d.n (<name> <type>, …) PRIMARY KEY
/// <name>[, <name>…] SOURCE(CLICKHOUSE(DB '<name>' TABLE '<name>'))
/// LAYOUT(DIRECT())` or `LAYOUT(COMPLEX_KEY_DIRECT())` (HE1 R14.12): the
/// source is the local server (no `HOST`), read by names alone.
fn dictionary_shape(text: &str) -> bool {
    let Some(rest) = after_qualified(text).and_then(|r| r.strip_prefix(" (")) else {
        return false;
    };
    let mut rest = rest;
    loop {
        let Some(after) = after_name(rest)
            .and_then(|r| r.strip_prefix(' '))
            .and_then(after_type)
        else {
            return false;
        };
        rest = after;
        if let Some(next) = rest.strip_prefix(", ") {
            rest = next;
        } else if let Some(next) = rest.strip_prefix(") PRIMARY KEY ") {
            rest = next;
            break;
        } else {
            return false;
        }
    }
    loop {
        let Some(after) = after_name(rest) else {
            return false;
        };
        rest = after;
        match rest.strip_prefix(", ") {
            Some(next) => rest = next,
            None => break,
        }
    }
    let Some(rest) = rest.strip_prefix(" SOURCE(CLICKHOUSE(DB ") else {
        return false;
    };
    let plain_literal = |text: &str| -> Option<usize> {
        let inner = text.strip_prefix('\'')?;
        let end = inner.find('\'')?;
        let name = &inner[..end];
        (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then_some(end + 2)
    };
    let Some(db) = plain_literal(rest) else {
        return false;
    };
    let Some(rest) = rest[db..].strip_prefix(" TABLE ") else {
        return false;
    };
    let Some(table) = plain_literal(rest) else {
        return false;
    };
    matches!(
        &rest[table..],
        ")) LAYOUT(DIRECT())" | ")) LAYOUT(COMPLEX_KEY_DIRECT())"
    )
}

/// Drops every read view an earlier statement made (HS1 Task 11): the views in
/// `default` and every other `Memory` database the front created. Session
/// temporary tables live elsewhere and stay.
pub fn drop_views(control: &Session) -> Result<(), ChdbError> {
    let names = |sql: &str| -> Result<Vec<String>, ChdbError> {
        let bytes = control.query(sql, "TSVRaw", &[])?;
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    };
    let kept = KEPT_DATABASES
        .iter()
        .map(|d| format!("'{d}'"))
        .collect::<Vec<_>>()
        .join(", ");
    // HE1 R14.12: dictionaries first, since a view they read cannot be
    // dropped under them (`630`, measured).
    for row in names("SELECT database, name FROM system.dictionaries")? {
        let Some((database, name)) = row.split_once('\t') else {
            continue;
        };
        control.execute_simple(&format!(
            "DROP DICTIONARY IF EXISTS {}.{}",
            quoted_name(database),
            quoted_name(name)
        ))?;
    }
    for database in names(&format!(
        "SELECT name FROM system.databases WHERE engine = 'Memory' AND name NOT IN ({kept})"
    ))? {
        control.execute_simple(&format!(
            "DROP DATABASE IF EXISTS {}",
            quoted_name(&database)
        ))?;
    }
    for table in
        names("SELECT name FROM system.tables WHERE database = 'default' AND NOT is_temporary")?
    {
        control.execute_simple(&format!(
            "DROP TABLE IF EXISTS default.{}",
            quoted_name(&table)
        ))?;
    }
    // HE1 R14.11: the SQL functions an earlier statement's plan made.
    for function in names("SELECT name FROM system.functions WHERE origin = 'SQLUserDefined'")? {
        control.execute_simple(&format!(
            "DROP FUNCTION IF EXISTS {}",
            quoted_name(&function)
        ))?;
    }
    Ok(())
}

/// `USE` the statement's database on its user connection (HS1 Task 11).
fn use_database(session: &Session, execute: &Execute) -> Result<(), Failure> {
    let Some(database) = &execute.database else {
        return Ok(());
    };
    session
        .execute_simple(&format!("USE {}", quoted_name(database)))
        .map_err(|err| Failure::Engine(engine_error(&err)))
}

/// `DROP DATABASE default; CREATE DATABASE default ENGINE = Memory`, once.
///
/// Skipped when `default` is already a `Memory` database, which is what a second
/// worker on the same engine finds (`InprocWorker`), so it never drops another
/// worker's views.
pub fn replace_default_database(control: &Session) -> Result<(), ChdbError> {
    let mut stream = control.execute(
        "SELECT engine FROM system.databases WHERE name = 'default'",
        "TSV",
        &[],
    )?;
    let mut engine = Vec::new();
    while let Some(chunk) = stream.next_chunk()? {
        engine.extend_from_slice(&chunk);
    }
    drop(stream);
    if String::from_utf8_lossy(&engine).trim() == "Memory" {
        return Ok(());
    }
    control.execute_simple("DROP DATABASE IF EXISTS default")?;
    control.execute_simple("CREATE DATABASE default ENGINE = Memory")
}

/// Tries one thing natively, outside chDB, for the sandbox tests (HS1 Task 6):
/// `Done` if it worked, `Error` with the OS's words if it did not.
fn probe(probe: &SandboxProbe) -> Vec<Frame> {
    let outcome: std::io::Result<()> = match probe {
        SandboxProbe::Exec(program) => std::process::Command::new(program)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(drop),
        SandboxProbe::Read(path) => std::fs::read(path).map(drop),
        SandboxProbe::Write(path) => std::fs::write(path, b"x"),
        SandboxProbe::Connect(addr) => addr
            .parse::<std::net::SocketAddr>()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
            .and_then(|addr| {
                std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map(drop)
            }),
        SandboxProbe::UnixConnect(path) => std::os::unix::net::UnixStream::connect(path).map(drop),
        SandboxProbe::Signal(pid) => i32::try_from(*pid)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
            .and_then(|pid| {
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
                    .map_err(std::io::Error::from)
            }),
        SandboxProbe::Allocate(bytes) => usize::try_from(*bytes)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
            .map(|bytes| {
                // Every page written, so it is resident and charged to the cgroup.
                let held = vec![1u8; bytes];
                std::hint::black_box(&held);
            }),
    };
    match outcome {
        Ok(()) => vec![Frame::Done],
        Err(err) => vec![error_frame(loams_error("PROBE_REFUSED", err), false)],
    }
}

/// Starts the thread that reads frames into a channel.
fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    hosting: Hosting,
) -> mpsc::Receiver<Result<Frame, End>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hsw1-reader".to_string())
        .spawn(move || {
            loop {
                let next = match FrameCodec::read(&mut reader) {
                    Ok(Some(frame)) if frame.is_abort() => {
                        // Test-only (`Frame::Abort` exists only with `test-hooks`):
                        // a crash the front did not cause by `SIGKILL`.
                        std::process::abort();
                    }
                    Ok(Some(frame)) => Ok(frame),
                    Ok(None) => Err(End::Closed),
                    Err(CodecError::Io(err)) => Err(End::Io(err.to_string())),
                    Err(err) => Err(End::Protocol(err.to_string())),
                };
                if let (Err(end), Hosting::Process(label)) = (&next, &hosting) {
                    // The front is gone or out of step: nothing this process is
                    // doing is wanted any more, and the main thread may be inside
                    // a statement that runs for hours.
                    end.exit_process(label);
                }
                let last = next.is_err();
                if tx.send(next).is_err() || last {
                    return;
                }
            }
        });
    if let Err(err) = spawned {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err(End::Io(format!(
            "the reader thread did not start: {err}"
        ))));
        return rx;
    }
    rx
}

/// Reads and discards frames up to the next `InputEnd`.
fn drain_input(frames: &mpsc::Receiver<Result<Frame, End>>) -> Result<(), End> {
    loop {
        match frames.recv() {
            Ok(Ok(Frame::Input(_))) => {}
            Ok(Ok(Frame::InputEnd)) => return Ok(()),
            Ok(Ok(other)) => {
                return Err(End::Protocol(format!(
                    "{} inside an INSERT body",
                    other.kind()
                )));
            }
            Ok(Err(end)) => return Err(end),
            Err(_) => return Err(End::Closed),
        }
    }
}

fn send<W: Write>(writer: &mut W, frame: &Frame) -> Result<(), End> {
    FrameCodec::write(writer, frame).map_err(|err| End::Io(err.to_string()))?;
    writer.flush().map_err(|err| End::Io(err.to_string()))
}

/// One piece of output as `Chunk` frames (none when it is empty).
fn send_bytes<W: Write>(writer: &mut W, bytes: Bytes) -> Result<(), Failure> {
    for chunk in Chunk::split(bytes, CHUNK_BYTES) {
        send(writer, &Frame::Chunk(chunk)).map_err(Failure::Wire)?;
    }
    Ok(())
}

/// A result formatted whole that is over the House's result cap: `36`, naming the
/// cap, as the front's own size caps answer (R3.4).
fn over_the_cap(format: &str, len: usize, max_bytes: u64) -> EngineError {
    EngineError {
        code: 36,
        name: "BAD_ARGUMENTS".to_string(),
        message: format!(
            "the result in FORMAT {format}, which the House formats whole, is {len} bytes, \
             over the {max_bytes} bytes of wait_end_of_query_max_bytes; use a row format \
             (TSV, JSONEachRow, Parquet, ArrowStream) for larger results"
        ),
    }
}

fn error_frame(error: EngineError, poisoned: bool) -> Frame {
    Frame::Error { error, poisoned }
}

/// Whether chDB refused to *stream* a statement it can run buffered. Measured at
/// 26.9.2.1: `CREATE TEMPORARY TABLE` through `chdb_stream_query` answers `36
/// BAD_ARGUMENTS` "Streaming query is not supported for query: …".
fn is_not_streamable(err: &ChdbError) -> bool {
    err.code == 36 && err.message.starts_with("Streaming query is not supported")
}

/// A [`ChdbError`] as the frame carries it.
pub fn engine_error(err: &ChdbError) -> EngineError {
    EngineError {
        code: err.code,
        name: err.name.clone(),
        message: err.message.clone(),
    }
}

fn loams_error(name: &str, detail: impl std::fmt::Display) -> EngineError {
    EngineError {
        code: 0,
        name: format!("LOAMS_{name}"),
        message: detail.to_string(),
    }
}

/// ClickHouse's `115 UNKNOWN_SETTING` for a name that is not a setting name at
/// all, so it never reaches a `SET` statement.
fn bad_setting(name: &str) -> EngineError {
    EngineError {
        code: 115,
        name: "UNKNOWN_SETTING".to_string(),
        message: format!("{name:?} is not a setting name"),
    }
}

/// Setting names are identifiers: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_setting_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn absorb(progress: &mut Progress, stats: &loams_chdb::QueryStats) {
    progress.rows_read = stats.rows_read;
    progress.bytes_read = stats.bytes_read;
    progress.result_rows = stats.result_rows;
    progress.result_bytes = stats.result_bytes;
    progress.elapsed_ns = progress
        .elapsed_ns
        .saturating_add((stats.elapsed * 1e9) as u64);
}

/// `VmRSS` and `VmHWM` from `/proc/self/status`, which the front's recycle rule
/// reads (§49 §10.1).
fn fill_memory(progress: &mut Progress) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return;
    };
    for line in status.lines() {
        let kib = |rest: &str| {
            rest.trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                * 1024
        };
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            progress.rss_bytes = kib(rest);
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            progress.peak_rss_bytes = kib(rest);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn boot_memory_follows_what_is_available() {
        use super::{MIN_BOOT_MEMORY, boot_memory_limit};
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(boot_memory_limit(4 * GIB, Some(8 * GIB)), 4 * GIB);
        assert_eq!(boot_memory_limit(4 * GIB, None), 4 * GIB);
        assert_eq!(boot_memory_limit(4 * GIB, Some(2 * GIB)), 2 * GIB / 10 * 9);
        assert_eq!(boot_memory_limit(4 * GIB, Some(1)), MIN_BOOT_MEMORY);
        assert_eq!(
            boot_memory_limit(100, Some(1)),
            100,
            "never above the limit"
        );
    }

    use super::{control_ddl_shape, is_setting_name};

    #[test]
    fn control_ddl_shapes() {
        for ok in [
            "CREATE DATABASE IF NOT EXISTS d ENGINE = Memory",
            "CREATE DATABASE IF NOT EXISTS `my db` ENGINE = Memory",
            "CREATE DATABASE IF NOT EXISTS `a\\`b` ENGINE = Memory",
            "CREATE OR REPLACE VIEW d.v AS\nSELECT 1",
            "CREATE OR REPLACE VIEW `__loams_system`.`tables` AS\nSELECT 1",
            "DROP TABLE IF EXISTS d.`t`",
            "CREATE OR REPLACE VIEW d.v (a UInt8) SQL SECURITY NONE AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.`t` (k UInt64, `s t` Nullable(String), \
             e Enum8('a b' = 1, 'c' = -2), d DateTime64(3, 'UTC')) SQL SECURITY NONE AS\nSELECT 1",
            "CREATE OR REPLACE FUNCTION f AS (a, b) -> a + b",
            "CREATE DICTIONARY d.n (k UInt64, v String) PRIMARY KEY k \
             SOURCE(CLICKHOUSE(DB 'd' TABLE '__loams_dictionary_n')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY `default`.n (project_id UUID, h String, c Array(Tuple(UInt32, String))) \
             PRIMARY KEY project_id, h SOURCE(CLICKHOUSE(DB 'default' TABLE 'v')) \
             LAYOUT(COMPLEX_KEY_DIRECT())",
        ] {
            assert!(control_ddl_shape(ok), "{ok}");
        }
        for bad in [
            "CREATE DATABASE IF NOT EXISTS d ENGINE = Atomic",
            "CREATE DATABASE IF NOT EXISTS d ENGINE = Memory; SELECT 1",
            "CREATE DATABASE IF NOT EXISTS `d ENGINE = Memory",
            "CREATE OR REPLACE VIEW v AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8) AS SELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8) AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8) SQL SECURITY NONE AS SELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8) SQL SECURITY DEFINER AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8 -- x\n) SQL SECURITY NONE AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a Enum8('x\\' = 1)) SQL SECURITY NONE AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8 DEFAULT file('/etc/passwd')) SQL SECURITY NONE \
             AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v () SQL SECURITY NONE AS\nSELECT 1",
            "CREATE OR REPLACE VIEW d.v (a UInt8; DROP DATABASE d) SQL SECURITY NONE AS\nSELECT 1",
            "CREATE TABLE d.t (a UInt8) ENGINE = Memory",
            "DROP TABLE IF EXISTS d.t; DROP DATABASE d",
            "DROP DATABASE d",
            "INSERT INTO d.v VALUES (1)",
            "CREATE FUNCTION f AS (a) -> a",
            "CREATE OR REPLACE FUNCTION d.f AS (a) -> a",
            "CREATE OR REPLACE FUNCTION f AS ",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(CLICKHOUSE(HOST 'x' DB 'd' \
             TABLE 't')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(CLICKHOUSE(DB 'd' TABLE 't' \
             USER 'u')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(CLICKHOUSE(DB 'd' \
             QUERY 'SELECT 1')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(HTTP(url 'http://x' \
             format 'TSV')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(CLICKHOUSE(DB 'd' \
             TABLE 't')) LAYOUT(HASHED())",
            "CREATE DICTIONARY d.n (k UInt64 DEFAULT file('x')) PRIMARY KEY k \
             SOURCE(CLICKHOUSE(DB 'd' TABLE 't')) LAYOUT(DIRECT())",
            "CREATE DICTIONARY d.n (k UInt64) PRIMARY KEY k SOURCE(CLICKHOUSE(DB 'd' \
             TABLE 't')) LAYOUT(DIRECT()) LIFETIME(0)",
            "CREATE DICTIONARY d.n (k UInt64 INJECTIVE) PRIMARY KEY k SOURCE(CLICKHOUSE(DB 'd' \
             TABLE 't')) LAYOUT(DIRECT())",
        ] {
            assert!(!control_ddl_shape(bad), "{bad}");
        }
    }

    #[test]
    fn setting_names_are_identifiers() {
        assert!(is_setting_name("max_threads"));
        assert!(is_setting_name("_x1"));
        assert!(!is_setting_name(""));
        assert!(!is_setting_name("1x"));
        assert!(!is_setting_name("a = 1; DROP"));
        assert!(!is_setting_name("max-threads"));
    }
}
