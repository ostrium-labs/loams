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
    Bind, CHUNK_BYTES, Chunk, CodecError, EngineError, Execute, Frame, FrameCodec,
    PROTOCOL_VERSION, Progress, Ready,
};

use crate::config::{self, WorkerArgs};

/// How often a running statement reports `Progress`, at most.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// The query-level arguments of every user connection (HS1 R1.9).
pub const USER_CONNECTION_ARGS: &[&str] = &["--readonly=2"];

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
    sessions: HashMap<String, Session>,
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
        let ready = Ready {
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            chdb_version: engine.chdb_version().to_string(),
            clickhouse_version: engine.version().to_string(),
            boot_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
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
            let frame = match frames.recv() {
                Ok(Ok(frame)) => frame,
                Ok(Err(end)) => return end,
                Err(_) => return End::Closed,
            };
            let answer = match frame {
                Frame::Bind(bind) => self.bind(bind),
                Frame::Execute(execute) => match self.execute(execute, &frames, &mut writer) {
                    Ok(answer) => answer,
                    Err(end) => return end,
                },
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
            Ok(stats) => vec![Frame::Stats(stats), Frame::Done],
            Err(Failure::Engine(err)) => {
                // Code 0 is a failure with no ClickHouse code (the FFI itself), and
                // 236 is chDB's fatal-signal path: neither leaves a worker to trust.
                let poisoned = err.code == 0 || err.code == 236;
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

        let fresh;
        let session = match &execute.session {
            Some(id) => {
                if !self.sessions.contains_key(id) {
                    let session = self.user_connection(id)?;
                    self.sessions.insert(id.clone(), session);
                }
                self.sessions
                    .get(id)
                    .ok_or_else(|| Failure::Engine(loams_error("SESSION_LOST", id)))?
            }
            None => {
                fresh = self.user_connection(&execute.query_id)?;
                &fresh
            }
        };

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
