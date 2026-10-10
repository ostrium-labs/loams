//! Isolated workers, the host's side (LV1 plan Task 5; design §45 §3.1,
//! D681; rows T3-10, T5-*).
//!
//! A [`WorkerHandle`] serves one (node, app, deployment): it runs the
//! deployment's calls in up to [`JsConfig::contexts`] sandboxed worker
//! processes (`loams live-worker`, `child.rs`), one call per process at a
//! time, each with a runtime of [`JsConfig::memory_limit`]. Calls speak the
//! worker protocol (`ipc.rs`); the host runs each `ctx.db` operation on the
//! call's transaction with a [`HostDriver`], exactly as for an in-process
//! slot.
//!
//! **The wall-clock kill (row T3-10).** The host kills a worker whose call
//! has run JavaScript for longer than `cpu_limit` plus [`KILL_GRACE`] (time
//! spent in host calls excluded), from outside the process, and answers the
//! call `FUNCTION_TIMEOUT`. The interrupt handler inside the worker stops
//! ordinary code at `cpu_limit`; the kill bounds the C built-ins that never
//! poll it.
//!
//! **Crashes.** A worker that dies, or breaks the protocol, fails its call
//! with `FUNCTION_ERROR`, reason `live_worker_crashed`
//! ([`LiveError::WorkerCrashed`]); a mutation is not retried, since its
//! transaction never committed. The next call starts a fresh worker, after
//! a backoff (10 ms doubling to 5 s while crashes repeat). Five crashes
//! within a minute mark the handle [degraded](WorkerHandle::degraded) and
//! log an error; calls are still served.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use loams_live::{FnKind, Function, LiveError, LiveTxn, LiveValue, Visibility};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Semaphore;

use crate::host::HostOp;
use crate::ipc::{
    self, CallContext, DoneOutcome, FrameError, HostError, HostReply, Invoke, Load, ProbeFrame,
    ProbeKind, ReplyAnswer, ToHost, ToWorker, WorkerMessage,
};
use crate::runtime::{
    BUNDLE, Failure, FunctionMeta, HostAnswer, HostDriver, JsConfig, SLOT_HEALTHY_AFTER,
    add_output, budgets, check_exports, check_source, restart_delay,
};
use crate::validators;

/// How long past `cpu_limit` a call may run JavaScript before the host
/// kills its worker (row T3-10).
pub const KILL_GRACE: Duration = Duration::from_millis(500);

/// What starting a worker may take beyond evaluating the bundle: the exec,
/// the dynamic loader, the runtime and the sandbox.
const START_GRACE: Duration = Duration::from_secs(10);

/// Crashes within this window that mark a handle degraded.
const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// How many crashes within [`CRASH_WINDOW`] mark a handle degraded.
const DEGRADED_AFTER: usize = 5;

/// How long a worker that stopped talking may take to exit before it is
/// killed, so its own exit status is the crash's description.
const EXIT_WAIT: Duration = Duration::from_millis(200);

/// The `stderr` chunks of one worker that reach the host's log.
const STDERR_CHUNKS: usize = 64;

/// The program that runs a worker: by default this binary as
/// `<binary> live-worker`, the `loams` binary's hidden subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommand {
    program: PathBuf,
    args: Vec<OsString>,
}

impl WorkerCommand {
    /// `program`, with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        WorkerCommand {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Adds an argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// This binary as `<binary> live-worker`.
    pub fn current_exe() -> Result<Self, LiveError> {
        let exe = std::env::current_exe()
            .map_err(|e| LiveError::Internal(format!("finding this binary for a worker: {e}")))?;
        Ok(WorkerCommand::new(exe).arg("live-worker"))
    }
}

/// Starts the isolated workers of a node's deployments.
#[derive(Debug, Clone)]
pub struct WorkerPool {
    command: Arc<WorkerCommand>,
}

impl WorkerPool {
    /// Workers run `command`.
    pub fn new(command: WorkerCommand) -> Self {
        WorkerPool {
            command: Arc::new(command),
        }
    }

    /// Starts the workers of one deployment of `app`: checks the bundle,
    /// evaluates it in a first sandboxed worker (which then serves calls)
    /// and returns the handle its calls go through. `app` labels the
    /// workers' log lines. The bundle's errors are those of
    /// [`Bundle::load`](crate::Bundle::load).
    pub async fn spawn(
        &self,
        app: &str,
        bundle: &[u8],
        config: JsConfig,
    ) -> Result<WorkerHandle, LiveError> {
        if !cfg!(target_os = "linux") {
            return Err(LiveError::IsolationUnavailable);
        }
        let source = std::str::from_utf8(bundle)
            .map_err(|_| LiveError::InvalidArgument("the bundle is not UTF-8".into()))?;
        check_source(source, &config)?;
        let shared = Arc::new(Shared {
            command: self.command.clone(),
            app: app.to_string(),
            bundle: Arc::from(bundle),
            slots: Semaphore::new(config.contexts),
            config,
            idle: Mutex::new(Vec::new()),
            pids: Arc::new(Mutex::new(BTreeSet::new())),
            health: Mutex::new(Health::default()),
        });
        let (process, metas) = shared.start().await?;
        check_exports(metas.len())?;
        lock(&shared.idle).push(process);
        Ok(WorkerHandle {
            shared,
            metas: metas.into(),
        })
    }
}

/// The isolated workers of one deployment. Cheap to clone; the workers
/// stop when the handle, its clones and every function taken from it are
/// dropped.
#[derive(Clone)]
pub struct WorkerHandle {
    shared: Arc<Shared>,
    metas: Arc<[FunctionMeta]>,
}

impl std::fmt::Debug for WorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle")
            .field("app", &self.shared.app)
            .field("functions", &self.metas.len())
            .field("config", &self.shared.config)
            .finish_non_exhaustive()
    }
}

/// A system call the test-only probe asks a worker to make
/// ([`WorkerHandle::probe`]).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Nothing: the worker answers.
    Ping,
    /// `open("/etc/passwd")`.
    OpenFile,
    /// A TCP socket.
    OpenSocket,
    /// Starting `/bin/true`.
    Exec,
}

impl WorkerHandle {
    /// The deployment's functions, in export order.
    pub fn functions(&self) -> Vec<FunctionMeta> {
        self.metas.to_vec()
    }

    /// The function `path` (`module:export`), or `None`.
    pub fn function(&self, path: &str) -> Option<Arc<dyn Function>> {
        let meta = self.metas.iter().find(|m| m.path == path)?;
        Some(Arc::new(WorkerFunction {
            meta: meta.clone(),
            shared: self.shared.clone(),
        }))
    }

    /// Whether five workers crashed within a minute (design §45 §3.1): the
    /// deployment is degraded. It stays set; calls are still served.
    pub fn degraded(&self) -> bool {
        lock(&self.shared.health).degraded
    }

    /// How many workers have crashed (died, or broken the protocol).
    pub fn crashes(&self) -> u64 {
        lock(&self.shared.health).total
    }

    /// The process ids of the running workers (for tests).
    #[doc(hidden)]
    pub fn pids(&self) -> Vec<u32> {
        lock(&self.shared.pids).iter().copied().collect()
    }

    /// Test-only: asks a worker to make the system call `probe` names,
    /// after its sandbox is applied. `Ok` with the call's own error text
    /// (empty if it succeeded) when the worker survives; a worker the
    /// sandbox kills fails like a crashed call.
    #[doc(hidden)]
    pub async fn probe(&self, probe: Probe) -> Result<String, LiveError> {
        self.shared.probe(probe).await
    }

    pub(crate) fn metas(&self) -> Arc<[FunctionMeta]> {
        self.metas.clone()
    }

    pub(crate) fn config(&self) -> &JsConfig {
        &self.shared.config
    }
}

/// A function of an isolated deployment.
struct WorkerFunction {
    meta: FunctionMeta,
    shared: Arc<Shared>,
}

impl Function for WorkerFunction {
    fn name(&self) -> &str {
        &self.meta.path
    }

    fn kind(&self) -> FnKind {
        self.meta.kind
    }

    fn visibility(&self) -> Visibility {
        self.meta.visibility
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            // As in process: checked on the host before any worker runs.
            if let Some(validator) = &self.meta.args {
                validators::check_args(txn, &self.meta.path, validator, &args).await?;
            }
            self.shared.call(txn, &self.meta.path, args).await
        })
    }
}

/// Crash bookkeeping.
#[derive(Debug, Default)]
struct Health {
    /// Crashes within the last [`CRASH_WINDOW`].
    recent: VecDeque<Instant>,
    total: u64,
    /// Crashes in a row, for the backoff; reset after a quiet minute.
    in_a_row: u32,
    last: Option<Instant>,
    degraded: bool,
}

/// What a deployment's workers share.
struct Shared {
    command: Arc<WorkerCommand>,
    app: String,
    bundle: Arc<[u8]>,
    config: JsConfig,
    /// One permit per worker process that may exist.
    slots: Semaphore,
    idle: Mutex<Vec<Process>>,
    pids: Arc<Mutex<BTreeSet<u32>>>,
    health: Mutex<Health>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One worker process. Dropping it kills the process.
struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    pid: u32,
    pids: Arc<Mutex<BTreeSet<u32>>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        // `kill_on_drop` kills it; tokio reaps it.
        lock(&self.pids).remove(&self.pid);
    }
}

impl Process {
    async fn send(&mut self, message: ToWorker) -> std::io::Result<()> {
        ipc::write_frame_async(&mut self.stdin, &ipc::to_worker(message)).await
    }

    async fn recv(&mut self) -> Result<ToHost, FrameError> {
        let frame: WorkerMessage = ipc::read_frame_async(&mut self.stdout).await?;
        frame
            .message
            .ok_or_else(|| FrameError::Broken("sent an empty frame".into()))
    }
}

/// Why a worker is being given up.
enum Lost {
    /// It died or broke the protocol: a crash.
    Crashed(String),
    /// It ran past the call's time: killed, not a crash.
    TimedOut,
}

impl Shared {
    /// Starts a worker process and loads the bundle into it.
    async fn start(&self) -> Result<(Process, Vec<FunctionMeta>), LiveError> {
        let mut command = Command::new(&self.command.program);
        command
            .args(&self.command.args)
            // Nothing of the host's environment (credentials, paths) reaches
            // the sandbox. TZ in POSIX form names no file to read.
            .env_clear()
            .env("TZ", "UTC0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            LiveError::Internal(format!(
                "starting a Live worker ({}): {e}",
                self.command.program.display()
            ))
        })?;
        let (Some(stdin), Some(stdout), Some(stderr), Some(pid)) = (
            child.stdin.take(),
            child.stdout.take(),
            child.stderr.take(),
            child.id(),
        ) else {
            return Err(LiveError::Internal(
                "a Live worker started without its pipes".into(),
            ));
        };
        tokio::spawn(forward_stderr(self.app.clone(), pid, stderr));
        lock(&self.pids).insert(pid);
        let mut process = Process {
            child,
            stdin,
            stdout,
            pid,
            pids: self.pids.clone(),
        };
        let load = Load {
            bundle: self.bundle.to_vec(),
            memory_limit: self.config.memory_limit as u64,
            cpu_limit_ns: u64::try_from(self.config.cpu_limit.as_nanos()).unwrap_or(u64::MAX),
            console_lines: u32::try_from(self.config.console_lines).unwrap_or(u32::MAX),
            console_line_bytes: u32::try_from(self.config.console_line_bytes).unwrap_or(u32::MAX),
            ..Default::default()
        };
        if let Err(e) = process.send(ToWorker::Load(Box::new(load))).await {
            let why = format!("did not take the bundle: {e}");
            return Err(self.give_up(process, Lost::Crashed(why), BUNDLE).await);
        }
        let deadline = self.config.cpu_limit + KILL_GRACE + START_GRACE;
        let loaded = match tokio::time::timeout(deadline, process.recv()).await {
            Err(_) => return Err(self.give_up(process, Lost::TimedOut, BUNDLE).await),
            Ok(Err(e)) => {
                return Err(self
                    .give_up(process, Lost::Crashed(e.to_string()), BUNDLE)
                    .await);
            }
            Ok(Ok(ToHost::Loaded(loaded))) => *loaded,
            Ok(Ok(_)) => {
                let why = "answered Load out of turn".to_string();
                return Err(self.give_up(process, Lost::Crashed(why), BUNDLE).await);
            }
        };
        if let Some(failure) = loaded.error.into_option() {
            // The bundle failed to load; the worker exits on its own.
            return Err(
                match ipc::failure_from_wire(
                    failure,
                    self.config.cpu_limit,
                    self.config.memory_limit,
                ) {
                    Ok(Failure::Live(e)) => e,
                    Ok(Failure::Host(_)) | Err(_) => {
                        let why = "reported a load failure it cannot have".to_string();
                        self.give_up(process, Lost::Crashed(why), BUNDLE).await
                    }
                },
            );
        }
        let mut metas = Vec::with_capacity(loaded.functions.len());
        for meta in loaded.functions {
            match ipc::meta_from_wire(meta) {
                Ok(m) => metas.push(m),
                Err(why) => return Err(self.give_up(process, Lost::Crashed(why), BUNDLE).await),
            }
        }
        Ok((process, metas))
    }

    /// An idle worker, or a fresh one (after the crash backoff), with the
    /// permit that lets it run.
    async fn checkout(&self) -> Result<(Process, tokio::sync::SemaphorePermit<'_>), LiveError> {
        let permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| LiveError::Internal("the Live workers have stopped".into()))?;
        if let Some(process) = lock(&self.idle).pop() {
            return Ok((process, permit));
        }
        if let Some(wait) = self.backoff() {
            tokio::time::sleep(wait).await;
        }
        let (process, _) = self.start().await?;
        Ok((process, permit))
    }

    /// What is left of the backoff after the last crash, if crashes are
    /// repeating.
    fn backoff(&self) -> Option<Duration> {
        let mut health = lock(&self.health);
        let last = health.last?;
        let since = last.elapsed();
        if since > SLOT_HEALTHY_AFTER {
            health.in_a_row = 0;
            return None;
        }
        restart_delay(health.in_a_row).checked_sub(since)
    }

    /// Runs one call in a worker.
    async fn call(
        &self,
        txn: &mut LiveTxn<'_>,
        path: &str,
        args: LiveValue,
    ) -> Result<LiveValue, LiveError> {
        let (mut process, _permit) = self.checkout().await?;
        let (result_bytes, host_bytes) = budgets(txn);
        let invoke = Invoke {
            path: path.to_string(),
            args: buffa::MessageField::some(args.to_proto()),
            ctx: buffa::MessageField::some(CallContext {
                start_ts: txn.start_ts().0,
                request_id: txn.ctx().request_id.clone(),
                result_bytes: result_bytes as u64,
                host_bytes: host_bytes as u64,
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Err(e) = process.send(ToWorker::Invoke(Box::new(invoke))).await {
            let why = format!("did not take the call: {e}");
            return Err(self.give_up(process, Lost::Crashed(why), path).await);
        }
        let mut driver = HostDriver::default();
        // JavaScript time left before the kill; host calls do not count.
        let mut left = self.config.cpu_limit + KILL_GRACE;
        loop {
            let started = Instant::now();
            let message = match tokio::time::timeout(left, process.recv()).await {
                Err(_) => return Err(self.give_up(process, Lost::TimedOut, path).await),
                Ok(Err(e)) => {
                    return Err(self
                        .give_up(process, Lost::Crashed(e.to_string()), path)
                        .await);
                }
                Ok(Ok(m)) => m,
            };
            left = left.saturating_sub(started.elapsed());
            match message {
                ToHost::HostCall(c) => {
                    let request = HostOp::parse(&c.op).zip(
                        c.args
                            .into_option()
                            .and_then(|a| LiveValue::from_proto(a).ok()),
                    );
                    let Some((op, args)) = request else {
                        let why = "asked for a host operation that does not exist".to_string();
                        return Err(self.give_up(process, Lost::Crashed(why), path).await);
                    };
                    let answer = match driver.answer(txn, op, args).await {
                        HostAnswer::Ok(v) => ReplyAnswer::Ok(Box::new(v.to_proto())),
                        HostAnswer::Error { message, index } => {
                            ReplyAnswer::Error(Box::new(HostError {
                                message,
                                index: u32::try_from(index).unwrap_or(u32::MAX),
                                ..Default::default()
                            }))
                        }
                        HostAnswer::Abort => ReplyAnswer::Abort(true),
                    };
                    let reply = HostReply {
                        id: c.id,
                        answer: Some(answer),
                        ..Default::default()
                    };
                    if let Err(e) = process.send(ToWorker::HostReply(Box::new(reply))).await {
                        let why = format!("did not take a host reply: {e}");
                        return Err(self.give_up(process, Lost::Crashed(why), path).await);
                    }
                }
                ToHost::Done(done) => {
                    let done = *done;
                    let outcome = match done.outcome {
                        Some(DoneOutcome::Result(v)) => LiveValue::from_proto(*v)
                            .map(Ok)
                            .map_err(|e| format!("returned a bad value: {e}")),
                        Some(DoneOutcome::Error(f)) => ipc::failure_from_wire(
                            *f,
                            self.config.cpu_limit,
                            self.config.memory_limit,
                        )
                        .map(Err),
                        None => Err("finished a call with no outcome".to_string()),
                    };
                    let result = match outcome {
                        Ok(result) => result,
                        Err(why) => {
                            return Err(self.give_up(process, Lost::Crashed(why), path).await);
                        }
                    };
                    add_output(
                        txn,
                        ipc::logs_from_wire(
                            done.logs,
                            done.dropped,
                            self.config.console_lines,
                            self.config.console_line_bytes,
                        ),
                    );
                    // A retiring worker (it ran out of memory) exits on its
                    // own; dropping it reaps it.
                    if !done.retire {
                        lock(&self.idle).push(process);
                    }
                    return driver.finish(result);
                }
                _ => {
                    let why = "sent a frame out of turn".to_string();
                    return Err(self.give_up(process, Lost::Crashed(why), path).await);
                }
            }
        }
    }

    /// Test-only: one probe ([`WorkerHandle::probe`]).
    async fn probe(&self, probe: Probe) -> Result<String, LiveError> {
        let (mut process, _permit) = self.checkout().await?;
        let kind = match probe {
            Probe::Ping => ProbeKind::PROBE_KIND_PING,
            Probe::OpenFile => ProbeKind::PROBE_KIND_OPEN_FILE,
            Probe::OpenSocket => ProbeKind::PROBE_KIND_OPEN_SOCKET,
            Probe::Exec => ProbeKind::PROBE_KIND_EXEC,
        };
        let message = ToWorker::Probe(Box::new(ProbeFrame {
            kind: kind.into(),
            ..Default::default()
        }));
        if let Err(e) = process.send(message).await {
            let why = format!("did not take the probe: {e}");
            return Err(self.give_up(process, Lost::Crashed(why), "<probe>").await);
        }
        match tokio::time::timeout(START_GRACE, process.recv()).await {
            Err(_) => Err(self.give_up(process, Lost::TimedOut, "<probe>").await),
            Ok(Err(e)) => Err(self
                .give_up(process, Lost::Crashed(e.to_string()), "<probe>")
                .await),
            Ok(Ok(ToHost::ProbeDone(done))) => {
                lock(&self.idle).push(process);
                Ok(done.error)
            }
            Ok(Ok(_)) => {
                let why = "answered a probe out of turn".to_string();
                Err(self.give_up(process, Lost::Crashed(why), "<probe>").await)
            }
        }
    }

    /// Ends a worker that crashed or ran out of time, and the error its
    /// call (of `function`) gets.
    async fn give_up(&self, mut process: Process, lost: Lost, function: &str) -> LiveError {
        match lost {
            Lost::TimedOut => {
                let _ = process.child.start_kill();
                let _ = process.child.wait().await;
                tracing::warn!(
                    app = %self.app,
                    function,
                    limit = ?self.config.cpu_limit,
                    "killed a Loams Live worker whose call ran past its CPU limit"
                );
                LiveError::FunctionTimeout {
                    function: function.to_string(),
                    limit: self.config.cpu_limit,
                }
            }
            Lost::Crashed(why) => {
                // Let a dying worker finish dying, so its own exit status
                // describes the crash; one that keeps running is killed.
                let status = match tokio::time::timeout(EXIT_WAIT, process.child.wait()).await {
                    Ok(status) => status,
                    Err(_) => {
                        let _ = process.child.start_kill();
                        process.child.wait().await
                    }
                };
                let exit =
                    status.map_or_else(|e| format!("could not be waited for: {e}"), describe_exit);
                let message = format!("the isolated worker running {function} {why}; it {exit}");
                self.crashed(&message);
                LiveError::WorkerCrashed(message)
            }
        }
    }

    /// Records a crash; the fifth within a minute marks the handle
    /// degraded.
    fn crashed(&self, message: &str) {
        let mut health = lock(&self.health);
        let now = Instant::now();
        health.recent.push_back(now);
        while health
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
        {
            health.recent.pop_front();
        }
        health.total = health.total.saturating_add(1);
        health.in_a_row = health.in_a_row.saturating_add(1);
        health.last = Some(now);
        tracing::warn!(app = %self.app, crashes = health.total, "{message}");
        if !health.degraded && health.recent.len() >= DEGRADED_AFTER {
            health.degraded = true;
            tracing::error!(
                app = %self.app,
                crashes = health.recent.len(),
                "Loams Live workers crashed five times within a minute; the deployment is degraded"
            );
        }
    }
}

/// How a worker process ended, for messages: its exit code, or the signal
/// that killed it, by name.
fn describe_exit(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("was killed by signal {signal} ({})", signal_name(signal));
        }
    }
    match status.code() {
        Some(code) => format!("exited with status {code}"),
        None => "ended".to_string(),
    }
}

/// The name of a signal that ends a worker.
#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    match signal {
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        24 => "SIGXCPU",
        31 => "SIGSYS",
        _ => "another signal",
    }
}

/// Forwards a worker's own log output (its `stderr`) to the host's log, a
/// bounded number of chunks of at most 1 KiB, so a worker cannot flood it.
async fn forward_stderr(app: String, pid: u32, mut stderr: tokio::process::ChildStderr) {
    let mut buf = [0u8; 1024];
    let mut chunks = 0usize;
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        if chunks < STDERR_CHUNKS {
            let text = String::from_utf8_lossy(&buf[..n]);
            tracing::warn!(app = %app, pid, "Loams Live worker: {}", text.trim_end());
        }
        chunks = chunks.saturating_add(1);
    }
}
