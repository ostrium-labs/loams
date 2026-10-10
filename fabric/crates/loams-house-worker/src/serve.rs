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
}

/// The engine configuration a worker with these arguments runs.
pub fn engine_config(args: &WorkerArgs) -> EngineConfig {
    EngineConfig {
        tmp_dir: args.data_dir(),
        cache_dir: args.tmp_dir.join("no-cache"),
        cache_bytes: 0,
        max_server_memory: args.memory_limit,
        // HS1 R1.4: no `filesystem_caches_path`, so nothing is cached on disk.
        filesystem_cache: false,
        server_args: vec![format!("--config-file={}", args.config_file().display())],
        // §49 §4.1: the worker is chDB's process, so chDB keeps its own handlers.
        install_signal_handlers: true,
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
        Self::boot_on(engine_config(args), started)
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
        };
        Ok(Self {
            engine,
            control,
            bound: None,
            sessions: HashMap::new(),
            ready,
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

        for view in &execute.views {
            self.control
                .execute_simple(view)
                .map_err(|err| Failure::Engine(engine_error(&err)))?;
        }

        let now = Instant::now();
        let Some(spec) = &execute.session else {
            let fresh = self.user_connection(&execute.query_id)?;
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
        let applied_ok = apply_settings(&slot.session, &bind, execute);
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
            for chunk in Chunk::split(Bytes::from(bytes), CHUNK_BYTES) {
                send(writer, &Frame::Chunk(chunk)).map_err(Failure::Wire)?;
            }
            fill_memory(&mut stats);
            return Ok(stats);
        }
        Err(err) => return Err(Failure::Engine(engine_error(&err))),
    };
    let mut last_progress = Instant::now();
    loop {
        match stream.next_chunk() {
            Ok(Some(block)) => {
                for chunk in Chunk::split(block, CHUNK_BYTES) {
                    send(writer, &Frame::Chunk(chunk)).map_err(Failure::Wire)?;
                }
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
    use super::is_setting_name;

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
