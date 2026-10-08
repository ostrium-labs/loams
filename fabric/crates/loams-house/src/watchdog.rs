//! The supervisor's process half (HS1 Task 2, design §49 §4, §10.1, §12): starting
//! a worker, killing it, knowing why it died, and killing it at a deadline.
//!
//! # Starting a worker
//!
//! [`ProcessLauncher`] starts `loams-house-worker` with `posix_spawn`:
//!
//! * one end of a Unix socket pair is `dup2`'d onto **fd 3**, and every other
//!   descriptor the front holds is close-on-exec, so the worker inherits its own
//!   socket and nothing else;
//! * the environment is **empty**, so no bucket key or token the front holds can
//!   reach it (`worker_env_has_no_secret`), and the arguments are
//!   `loams_house_worker::WorkerArgs`, which carry none either;
//! * stdin and stdout are `/dev/null`; stderr is the front's, for chDB's own
//!   messages.
//!
//! # Killing a worker
//!
//! Every exit the front causes is a `SIGKILL` (§49 §10.1): libchdb's shutdown path
//! is slow and has hung, so there is none. A pid is only safe to signal while it
//! has not been reaped, so the waiter thread first waits **without reaping**
//! (`waitid(…, WEXITED | WNOWAIT)`), marks the worker exited under the same lock
//! [`KillHandle::kill`] takes, and only then reaps. A kill therefore either reaches
//! the worker (alive, or a zombie that ignores it) or sees "exited" and does
//! nothing; it never reaches a recycled pid.
//!
//! # Why it died
//!
//! The reason is recorded before the signal is sent, so the lease that sees its
//! socket end can tell a cancellation (`394`) from a timeout (`159`), a memory
//! kill (`241`) and a crash nobody asked for (`210`). See [`ExitReason`].

use std::fmt;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal, kill};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::Pid;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::errors::{ChError, HouseError};

/// Why a worker stopped: the `reason` label of
/// `loams_house_worker_kills_total{reason}` (HS1 Shared contracts).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExitReason {
    /// It ran `max_queries_per_worker` statements.
    Budget,
    /// It sat bound and idle past `idle_unbind_after`, or its slot was needed for
    /// another namespace.
    Idle,
    /// Its statement was cancelled (`KILL QUERY`, a client that went away).
    Cancel,
    /// Its statement ran past `max_execution_time` plus the watchdog's grace.
    Timeout,
    /// Its resident set passed `worker_rss_ceiling`.
    Rss,
    /// The kernel's OOM killer, or a cgroup limit, ended it.
    Oom,
    /// It reported an FFI error class that poisons a process (FL2 Ruling 11), or
    /// broke the protocol.
    Poisoned,
    /// It died and nobody asked it to.
    Crash,
    /// The pool shut down.
    Drain,
}

impl ExitReason {
    /// Every reason, in label order.
    pub const ALL: [ExitReason; 9] = [
        Self::Budget,
        Self::Idle,
        Self::Cancel,
        Self::Timeout,
        Self::Rss,
        Self::Oom,
        Self::Poisoned,
        Self::Crash,
        Self::Drain,
    ];

    /// The metric label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Idle => "idle",
            Self::Cancel => "cancel",
            Self::Timeout => "timeout",
            Self::Rss => "rss",
            Self::Oom => "oom",
            Self::Poisoned => "poisoned",
            Self::Crash => "crash",
            Self::Drain => "drain",
        }
    }

    /// What the client of a statement that was running when the worker died is
    /// told (§49 §11, §12): `394` for a cancel, `159` for a timeout, `241` for
    /// memory, and `210 NETWORK_ERROR` for anything else, which a client retries.
    pub fn client_error(self, query_id: &str, exit: Option<&str>) -> HouseError {
        let how = exit.map(|e| format!(" ({e})")).unwrap_or_default();
        HouseError::from(match self {
            Self::Cancel => ChError::query_was_cancelled(format!("Query {query_id} was cancelled")),
            Self::Timeout => ChError::timeout_exceeded(format!(
                "Query {query_id} exceeded its execution time and its worker was stopped"
            )),
            Self::Rss | Self::Oom => ChError::memory_limit_exceeded(format!(
                "Query {query_id} exceeded the worker's memory limit and its worker was stopped"
            )),
            Self::Crash => ChError::network_error(format!(
                "The worker running query {query_id} exited unexpectedly{how}; the query can \
                 be retried"
            )),
            Self::Budget | Self::Idle | Self::Poisoned | Self::Drain => {
                ChError::network_error(format!(
                    "The worker running query {query_id} was stopped ({}); the query can be \
                     retried",
                    self.as_str()
                ))
            }
        })
    }
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a launcher's worker is signalled and observed. The process launcher kills
/// with `SIGKILL`; the in-process one closes the socket.
pub trait WorkerControl: Send + Sync + fmt::Debug {
    /// The worker's pid as the front sees it.
    fn pid(&self) -> u32;
    /// Ends the worker now. Idempotent; a no-op once it has exited.
    fn terminate(&self);
    /// The exit, once it has happened: `exited with 0`, `killed by SIGKILL`, …
    fn exit(&self) -> watch::Receiver<Option<String>>;
}

/// Starts workers. One per pool.
pub trait Launcher: Send + Sync + fmt::Debug {
    /// Starts worker `id` and returns the front's end of its socket. The worker is
    /// not ready until it sends `Ready`; the pool waits for that.
    fn launch(&self, id: &str) -> Result<Launched, HouseError>;
}

/// A started worker.
#[derive(Debug)]
pub struct Launched {
    /// The front's end of the `hsw1` socket.
    pub socket: StdUnixStream,
    /// How to end it.
    pub control: Arc<dyn WorkerControl>,
}

/// What the pool and every lease share about one worker: its control, and the
/// reason it was killed, written before the signal.
#[derive(Debug)]
pub struct WorkerShared {
    id: String,
    control: Arc<dyn WorkerControl>,
    state: Mutex<KillState>,
}

/// The lease/statement epoch and the recorded reason, behind one lock, so a
/// [`KillHandle`]'s epoch check and its signal cannot straddle a new lease or
/// statement (HS1 Task 2 review I1).
#[derive(Debug, Default)]
struct KillState {
    epoch: u64,
    reason: Option<ExitReason>,
}

impl WorkerShared {
    /// Wraps a launched worker.
    pub fn new(id: String, control: Arc<dyn WorkerControl>) -> Arc<Self> {
        Arc::new(Self {
            id,
            control,
            state: Mutex::new(KillState::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, KillState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Starts a new epoch: a new lease or a new statement. Handles taken before it
    /// no longer reach this worker.
    pub fn bump_epoch(&self) -> u64 {
        let mut state = self.state();
        state.epoch += 1;
        state.epoch
    }

    /// The current epoch.
    pub fn epoch(&self) -> u64 {
        self.state().epoch
    }

    /// The worker's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The worker's pid.
    pub fn pid(&self) -> u32 {
        self.control.pid()
    }

    /// The reason recorded for its death, if anyone recorded one.
    pub fn reason(&self) -> Option<ExitReason> {
        self.state().reason
    }

    /// Records `reason` unless one is already recorded, and returns the one that
    /// stands. The first reason wins: a cancel that raced a timeout stays a cancel.
    pub fn record(&self, reason: ExitReason) -> ExitReason {
        *self.state().reason.get_or_insert(reason)
    }

    /// Records `reason` and ends the worker, whatever the epoch: the pool's own
    /// retirement.
    pub fn kill(&self, reason: ExitReason) -> ExitReason {
        let mut state = self.state();
        let reason = *state.reason.get_or_insert(reason);
        self.control.terminate();
        reason
    }

    /// [`WorkerShared::kill`], but only while the epoch is still `epoch`; the check
    /// and the signal happen under the lock [`WorkerShared::bump_epoch`] takes.
    /// `None` when the handle is stale.
    pub fn kill_in_epoch(&self, epoch: u64, reason: ExitReason) -> Option<ExitReason> {
        let mut state = self.state();
        if state.epoch != epoch {
            return None;
        }
        let reason = *state.reason.get_or_insert(reason);
        self.control.terminate();
        Some(reason)
    }

    /// The exit, if it has happened.
    pub fn exit(&self) -> Option<String> {
        self.control.exit().borrow().clone()
    }

    /// Whether it has exited.
    pub fn has_exited(&self) -> bool {
        self.exit().is_some()
    }

    /// Waits up to `limit` for the exit.
    pub async fn wait_exit(&self, limit: Duration) -> Option<String> {
        let mut exit = self.control.exit();
        let waited = tokio::time::timeout(limit, async {
            loop {
                if let Some(done) = exit.borrow().clone() {
                    return Some(done);
                }
                if exit.changed().await.is_err() {
                    return exit.borrow().clone();
                }
            }
        })
        .await;
        waited.unwrap_or(None)
    }
}

/// Kills one worker from anywhere: a `KILL QUERY`, a client that went away, the
/// watchdog. Cloneable; it does not hold the lease.
#[derive(Clone, Debug)]
pub struct KillHandle {
    shared: Arc<WorkerShared>,
    epoch: u64,
}

impl KillHandle {
    /// A handle for the worker's current epoch.
    pub(crate) fn new(shared: Arc<WorkerShared>) -> Self {
        let epoch = shared.epoch();
        Self { shared, epoch }
    }

    /// Records `reason` and `SIGKILL`s the worker, **if it is still in the lease
    /// and statement this handle was taken for**; the statement it was running
    /// answers the client with [`ExitReason::client_error`]. A handle that outlived
    /// its statement (a late `KILL QUERY`, a deadline that fired after `Done`) is a
    /// no-op and answers `None`, so it can never kill a later query (review I1).
    pub fn kill(&self, reason: ExitReason) -> Option<ExitReason> {
        self.shared.kill_in_epoch(self.epoch, reason)
    }

    /// The worker's pid.
    pub fn pid(&self) -> u32 {
        self.shared.pid()
    }

    /// Waits up to `limit` for the worker to be gone, and says how it ended. For an
    /// `InprocWorker` that is when its thread returns, which is when the statement
    /// it was running finishes: a front must not exit before then, because libchdb's
    /// static destructors racing a running statement crash the process.
    pub async fn wait_exit(&self, limit: Duration) -> Option<String> {
        self.shared.wait_exit(limit).await
    }

    /// Kills the worker with `reason` at `deadline` unless the returned guard is
    /// dropped first (§49 §12: `max_execution_time` plus 2 s).
    pub fn kill_at(&self, deadline: Instant, reason: ExitReason) -> Deadline {
        let handle = self.clone();
        Deadline {
            task: tokio::spawn(async move {
                tokio::time::sleep_until(deadline).await;
                let _ = handle.kill(reason);
            }),
        }
    }
}

/// An armed deadline; dropping it disarms it.
#[derive(Debug)]
pub struct Deadline {
    task: JoinHandle<()>,
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts `loams-house-worker` processes.
#[derive(Debug, Clone)]
pub struct ProcessLauncher {
    binary: PathBuf,
    tmp_root: PathBuf,
    memory_limit: Option<u64>,
    s3_endpoint: Option<String>,
}

impl ProcessLauncher {
    /// Workers run `binary` and get a private directory under `tmp_root` each.
    pub fn new(binary: impl Into<PathBuf>, tmp_root: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            tmp_root: tmp_root.into(),
            memory_limit: None,
            s3_endpoint: None,
        }
    }

    /// The memory each worker sizes its caches and engine for.
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit = Some(bytes);
        self
    }

    /// The loopback endpoint each worker's `READ ON S3` grant names.
    pub fn with_s3_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.s3_endpoint = Some(endpoint.into());
        self
    }

    /// The command line of worker `id`, without the program name. Never a
    /// credential: the grammar is `loams_house_worker::WorkerArgs`.
    pub fn args(&self, id: &str) -> Vec<String> {
        let mut args = vec![
            "--tmp-dir".to_string(),
            self.tmp_root.join(id).display().to_string(),
            "--worker-id".to_string(),
            id.to_string(),
        ];
        if let Some(bytes) = self.memory_limit {
            args.push("--memory-limit".to_string());
            args.push(bytes.to_string());
        }
        if let Some(endpoint) = &self.s3_endpoint {
            args.push("--s3-endpoint".to_string());
            args.push(endpoint.clone());
        }
        args
    }
}

fn spawn_error(what: &str, err: impl fmt::Display) -> HouseError {
    HouseError::from(ChError::network_error(format!(
        "could not start a House worker: {what}: {err}"
    )))
}

impl Launcher for ProcessLauncher {
    fn launch(&self, id: &str) -> Result<Launched, HouseError> {
        let dir = self.tmp_root.join(id);
        std::fs::create_dir_all(&dir).map_err(|err| spawn_error("its directory", err))?;

        let (front, worker) =
            StdUnixStream::pair().map_err(|err| spawn_error("a socket pair", err))?;
        // `dup2(3, 3)` would leave close-on-exec set, so never hand fd 3 to itself.
        let worker = if worker.as_raw_fd() == loams_house_ipc::WORKER_SOCKET_FD {
            worker
                .try_clone()
                .map_err(|err| spawn_error("a socket", err))?
        } else {
            worker
        };
        let null = std::fs::File::open("/dev/null").map_err(|err| spawn_error("/dev/null", err))?;

        let mut actions =
            PosixSpawnFileActions::init().map_err(|err| spawn_error("file actions", err))?;
        actions
            .add_dup2(null.as_raw_fd(), 0)
            .and_then(|()| actions.add_dup2(null.as_raw_fd(), 1))
            .and_then(|()| actions.add_dup2(worker.as_raw_fd(), loams_house_ipc::WORKER_SOCKET_FD))
            .map_err(|err| spawn_error("file actions", err))?;

        // The worker starts with no signal blocked and SIGPIPE back at its default
        // (Rust ignores it in the front, and an ignored disposition survives exec).
        let mut attr = PosixSpawnAttr::init().map_err(|err| spawn_error("attributes", err))?;
        let mut defaults = SigSet::empty();
        defaults.add(Signal::SIGPIPE);
        attr.set_flags(
            PosixSpawnFlags::POSIX_SPAWN_SETSIGMASK | PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF,
        )
        .and_then(|()| attr.set_sigmask(&SigSet::empty()))
        .and_then(|()| attr.set_sigdefault(&defaults))
        .map_err(|err| spawn_error("attributes", err))?;

        let program = cstring(&self.binary.display().to_string())?;
        let mut argv = vec![program];
        for arg in self.args(id) {
            argv.push(cstring(&arg)?);
        }
        // §49 §13.1: workers hold no credentials. Nothing from the front's
        // environment is passed on.
        let envp: Vec<std::ffi::CString> = Vec::new();

        let pid = posix_spawn(self.binary.as_path(), &actions, &attr, &argv, &envp)
            .map_err(|err| spawn_error(&self.binary.display().to_string(), err))?;
        drop(worker);
        drop(null);

        Ok(Launched {
            socket: front,
            control: ProcessControl::start(pid, dir),
        })
    }
}

fn cstring(text: &str) -> Result<std::ffi::CString, HouseError> {
    std::ffi::CString::new(text).map_err(|_| spawn_error("an argument", "it holds a NUL byte"))
}

/// A worker process: its pid, and a waiter thread that reaps it.
#[derive(Debug)]
struct ProcessControl {
    pid: Pid,
    /// True once the waiter has seen the exit (and before it reaps). Kill checks it
    /// under this lock, so it never signals a reaped, possibly recycled, pid.
    exited: Mutex<bool>,
    exit: watch::Sender<Option<String>>,
}

impl ProcessControl {
    fn start(pid: Pid, dir: PathBuf) -> Arc<Self> {
        let (exit, _) = watch::channel(None);
        let control = Arc::new(Self {
            pid,
            exited: Mutex::new(false),
            exit,
        });
        let waiter = Arc::clone(&control);
        let spawned = std::thread::Builder::new()
            .name(format!("house-worker-wait-{pid}"))
            .spawn(move || waiter.wait(&dir));
        if spawned.is_err() {
            // No waiter: kill it rather than leave an unsupervised worker.
            let _ = kill(pid, Signal::SIGKILL);
        }
        control
    }

    fn exited(&self) -> MutexGuard<'_, bool> {
        self.exited
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits for the exit without reaping, records it, then reaps and removes the
    /// worker's private directory.
    fn wait(&self, dir: &Path) {
        let status = loop {
            match waitid(
                Id::Pid(self.pid),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
            ) {
                Ok(status) => break describe(status),
                Err(nix::errno::Errno::EINTR) => continue,
                Err(err) => break format!("could not be waited for: {err}"),
            }
        };
        *self.exited() = true;
        let _ = waitpid(self.pid, None);
        let _ = std::fs::remove_dir_all(dir);
        self.exit.send_replace(Some(status));
    }
}

fn describe(status: WaitStatus) -> String {
    match status {
        WaitStatus::Exited(_, code) => format!("exited with {code}"),
        WaitStatus::Signaled(_, signal, core) => {
            format!(
                "killed by {}{}",
                signal.as_str(),
                if core { ", core dumped" } else { "" }
            )
        }
        other => format!("{other:?}"),
    }
}

impl WorkerControl for ProcessControl {
    fn pid(&self) -> u32 {
        self.pid.as_raw().unsigned_abs()
    }

    fn terminate(&self) {
        let exited = self.exited();
        if !*exited {
            let _ = kill(self.pid, Signal::SIGKILL);
        }
    }

    fn exit(&self) -> watch::Receiver<Option<String>> {
        self.exit.subscribe()
    }
}

/// `InprocWorker`: the worker's serve loop on a thread of the front, behind feature
/// `inproc-worker` (`--workers=inproc`, §49 §4.1, Q686).
///
/// **Not isolated.** chDB runs in the front's process, every "worker" shares one
/// engine, a crash in libchdb takes the front down, and a kill only closes the
/// socket: the statement it was running keeps its thread until it finishes. The
/// client still gets its answer at once. For development and the desktop only.
#[cfg(feature = "inproc-worker")]
#[derive(Debug, Clone)]
pub struct InprocWorker {
    args: loams_house_worker::WorkerArgs,
}

#[cfg(feature = "inproc-worker")]
impl InprocWorker {
    /// In-process workers whose shared engine lives under `tmp_root`.
    pub fn new(tmp_root: impl Into<PathBuf>) -> Self {
        Self {
            args: loams_house_worker::WorkerArgs {
                tmp_dir: tmp_root.into(),
                worker_id: "inproc".to_string(),
                memory_limit: loams_house_worker::config::DEFAULT_MEMORY_LIMIT,
                s3_endpoint: None,
            },
        }
    }
}

#[cfg(feature = "inproc-worker")]
impl InprocWorker {
    /// The engine the in-process workers share: the worker's configuration, but
    /// **without chDB's signal handlers** (HS1 Task 2 review I5). The process is
    /// the front's, and a library must not take its signals (FL2 Ruling 2's
    /// default); only a worker process, which is chDB's own, keeps them (§49 §4.1).
    pub fn engine_config(&self) -> loams_house_worker::EngineConfig {
        let mut config = loams_house_worker::engine_config(&self.args);
        config.install_signal_handlers = false;
        config
    }
}

#[cfg(feature = "inproc-worker")]
impl Launcher for InprocWorker {
    fn launch(&self, id: &str) -> Result<Launched, HouseError> {
        loams_house_worker::config::write_files(&self.args)
            .map_err(|err| spawn_error("the in-process engine's config", err))?;
        let (front, worker) =
            StdUnixStream::pair().map_err(|err| spawn_error("a socket pair", err))?;
        let reader = worker
            .try_clone()
            .map_err(|err| spawn_error("a socket", err))?;
        let shutdown = front
            .try_clone()
            .map_err(|err| spawn_error("a socket", err))?;
        let (exit, _) = watch::channel(None);
        let control = Arc::new(InprocControl {
            socket: shutdown,
            exit,
        });
        let done = Arc::clone(&control);
        let config = self.engine_config();
        std::thread::Builder::new()
            .name(format!("house-inproc-{id}"))
            .spawn(move || {
                let started = std::time::Instant::now();
                let end = match loams_house_worker::Worker::boot_on(config, started) {
                    Ok(served) => format!(
                        "{:?}",
                        served.serve(reader, worker, loams_house_worker::Hosting::InProcess)
                    ),
                    Err(error) => format!("boot failed: {error}"),
                };
                done.exit
                    .send_replace(Some(format!("in-process worker ended: {end}")));
            })
            .map_err(|err| spawn_error("a thread", err))?;
        Ok(Launched {
            socket: front,
            control,
        })
    }
}

#[cfg(feature = "inproc-worker")]
#[derive(Debug)]
struct InprocControl {
    socket: StdUnixStream,
    exit: watch::Sender<Option<String>>,
}

#[cfg(feature = "inproc-worker")]
impl WorkerControl for InprocControl {
    fn pid(&self) -> u32 {
        std::process::id()
    }

    fn terminate(&self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }

    fn exit(&self) -> watch::Receiver<Option<String>> {
        self.exit.subscribe()
    }
}
