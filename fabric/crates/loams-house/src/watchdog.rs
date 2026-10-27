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
//!   messages;
//! * it seals itself before libchdb boots (L3, HS1 Task 6): `--sandbox netns` by
//!   default on Linux, with its forwarder's socket on **fd 4** when the launcher
//!   has an upstream, and `--cgroup` naming the child the front made for it when
//!   the launcher owns a delegated cgroup ([`crate::sandbox`]).
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
use crate::sandbox::{
    self, FORWARDER_PORT, FORWARDER_SOCKET_FD, Forward, ForwardTarget, SandboxMode, WorkerCgroups,
};

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
    /// Whether the kernel's OOM killer ended it in its cgroup (`oom_kill` in the
    /// cgroup's `memory.events`, HS1 Task 21). `false` without a cgroup.
    fn oom_killed(&self) -> bool {
        false
    }
    /// The CPU time the worker has used so far, read from outside it (HS1
    /// Task 22): its cgroup's `cpu.stat`, else `/proc/<pid>/stat`. `None` when
    /// it cannot be measured (an in-process worker, or one already reaped).
    fn cpu_time(&self) -> Option<Duration> {
        None
    }
}

/// Starts workers. One per pool.
pub trait Launcher: Send + Sync + fmt::Debug {
    /// Starts worker `id` and returns the front's end of its socket. The worker is
    /// not ready until it sends `Ready`; the pool waits for that.
    fn launch(&self, id: &str) -> Result<Launched, HouseError>;

    /// Worker `id` was bound to `namespace`. A launcher whose workers read the
    /// bucket through `house-cache` passes it on, so the worker's reads are
    /// scoped to that namespace (HS1 Task 9).
    fn bound(&self, _id: &str, _namespace: &str) {}
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

    /// The CPU time it has used so far ([`WorkerControl::cpu_time`]).
    pub fn cpu_time(&self) -> Option<Duration> {
        self.control.cpu_time()
    }

    /// The reason recorded for its death, if anyone recorded one.
    pub fn reason(&self) -> Option<ExitReason> {
        self.state().reason
    }

    /// Records `reason` unless one is already recorded, and returns the one that
    /// stands. The first reason wins: a cancel that raced a timeout stays a cancel.
    /// A death nobody asked for is `oom` when the cgroup's OOM killer did it.
    pub fn record(&self, reason: ExitReason) -> ExitReason {
        let reason = self.unasked(reason);
        *self.state().reason.get_or_insert(reason)
    }

    /// `crash` is `oom` when the worker's cgroup says its OOM killer ended it
    /// (HS1 Task 21; R2.8, R6.5).
    fn unasked(&self, reason: ExitReason) -> ExitReason {
        if reason == ExitReason::Crash && self.control.oom_killed() {
            ExitReason::Oom
        } else {
            reason
        }
    }

    /// Records `reason` and ends the worker, whatever the epoch: the pool's own
    /// retirement.
    pub fn kill(&self, reason: ExitReason) -> ExitReason {
        let reason = self.unasked(reason);
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

/// The front's running statements, by query id, on both surfaces (ClickHouse
/// HTTP and `loams.house.v1`): what `KILL QUERY` and `CancelQuery` find (§49
/// §12, HS1 Task 21; R8.8).
#[derive(Debug, Default)]
pub struct Processes {
    queries: Mutex<std::collections::HashMap<String, Process>>,
}

#[derive(Debug)]
struct Process {
    user: String,
    namespace: u64,
    query: String,
    /// The current statement's handle, once it has started.
    kill: Option<KillHandle>,
    /// Asked to stop: killed when it starts, if it had not yet.
    cancelled: bool,
    /// A write the front carries to its commit itself: not killable.
    cancellable: bool,
    /// When it was registered (`system.processes.elapsed`, HS1 Task 23).
    since: std::time::Instant,
}

/// A running statement as `system.processes` shows it (HS1 Task 23).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningQuery {
    pub query_id: String,
    pub user: String,
    pub query: String,
    pub elapsed: Duration,
    /// Asked to stop.
    pub cancelled: bool,
}

/// Why a kill found nothing it may kill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillRefusal {
    /// No running statement has the id in the caller's namespace (another
    /// namespace's statements are not even seen).
    NotFound,
    /// It is another user's, and the caller lacks `house:admin`.
    Denied,
}

/// What `KILL QUERY` did to one statement: the row ClickHouse answers.
#[derive(Clone, Debug)]
pub struct Killed {
    /// `kill_status`: `waiting` (asked; it ends at once or when it starts),
    /// `finished` (`SYNC`: it is gone) or `cant_cancel`.
    pub status: &'static str,
    /// The statement's id.
    pub query_id: String,
    /// Its user.
    pub user: String,
    /// Its text.
    pub query: String,
}

impl Processes {
    fn lock(&self) -> MutexGuard<'_, std::collections::HashMap<String, Process>> {
        self.queries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records a statement as running until the guard drops. An id already
    /// running is refused (ClickHouse's `216`, which is not in the House's
    /// codes: `36`, as R3.4 does for the House's own refusals).
    pub fn register(
        self: &Arc<Self>,
        query_id: &str,
        user: &crate::config::UserMap,
        query: &str,
    ) -> Result<ProcessGuard, HouseError> {
        let mut queries = self.lock();
        if queries.contains_key(query_id) {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "Query with id = {query_id} is already running"
            ))));
        }
        queries.insert(
            query_id.to_string(),
            Process {
                user: user.user.clone(),
                namespace: user.namespace,
                query: query.to_string(),
                kill: None,
                cancelled: false,
                cancellable: true,
                since: std::time::Instant::now(),
            },
        );
        Ok(ProcessGuard {
            processes: Arc::clone(self),
            query_id: query_id.to_string(),
        })
    }

    /// Whether statement `query_id` is still running.
    pub fn is_running(&self, query_id: &str) -> bool {
        self.lock().contains_key(query_id)
    }

    /// The statements running now in `namespace`, by id.
    pub fn list(&self, namespace: u64) -> Vec<RunningQuery> {
        let mut out: Vec<RunningQuery> = self
            .lock()
            .iter()
            .filter(|(_, p)| p.namespace == namespace)
            .map(|(id, p)| RunningQuery {
                query_id: id.clone(),
                user: p.user.clone(),
                query: p.query.clone(),
                elapsed: p.since.elapsed(),
                cancelled: p.cancelled,
            })
            .collect();
        out.sort_by(|a, b| a.query_id.cmp(&b.query_id));
        out
    }

    /// Statements running now.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether none is running.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Kills statement `query_id` for `by` (§49 §12): its own namespace only,
    /// and another user's only with `house:admin`. A statement that has not
    /// started is killed when it starts. With `test`, nothing is killed
    /// (`KILL QUERY … TEST`): the row says what would be.
    pub fn kill(
        &self,
        query_id: &str,
        by: &crate::config::UserMap,
        test: bool,
    ) -> Result<Killed, KillRefusal> {
        let mut queries = self.lock();
        let Some(query) = queries
            .get_mut(query_id)
            .filter(|q| q.namespace == by.namespace)
        else {
            return Err(KillRefusal::NotFound);
        };
        if query.user != by.user && !by.scopes.admin {
            return Err(KillRefusal::Denied);
        }
        let mut killed = Killed {
            status: "waiting",
            query_id: query_id.to_string(),
            user: query.user.clone(),
            query: query.query.clone(),
        };
        if !query.cancellable {
            killed.status = "cant_cancel";
            return Ok(killed);
        }
        if !test {
            query.cancelled = true;
            if let Some(kill) = &query.kill {
                let _ = kill.kill(ExitReason::Cancel);
            }
        }
        Ok(killed)
    }
}

/// A running statement's entry in [`Processes`]; dropping it removes the entry.
#[derive(Debug)]
pub struct ProcessGuard {
    processes: Arc<Processes>,
    query_id: String,
}

impl ProcessGuard {
    /// Whether the statement was killed before it got this far.
    pub fn cancelled(&self) -> bool {
        self.processes
            .lock()
            .get(&self.query_id)
            .is_some_and(|q| q.cancelled)
    }

    /// The statement started on a worker: later kills reach it, and an earlier
    /// one does now.
    pub fn started(&self, kill: Option<KillHandle>) {
        let mut queries = self.processes.lock();
        if let Some(query) = queries.get_mut(&self.query_id) {
            if query.cancelled
                && let Some(kill) = &kill
            {
                let _ = kill.kill(ExitReason::Cancel);
            }
            query.kill = kill;
        }
    }

    /// The statement is a lake write the front carries to its commit: `KILL
    /// QUERY` answers `cant_cancel` (its worker stops at the statement's
    /// deadline like any other).
    pub fn uncancellable(&self) {
        if let Some(query) = self.processes.lock().get_mut(&self.query_id) {
            query.cancellable = false;
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.processes.lock().remove(&self.query_id);
    }
}

/// Starts `loams-house-worker` processes.
#[derive(Debug, Clone)]
pub struct ProcessLauncher {
    binary: PathBuf,
    tmp_root: PathBuf,
    memory_limit: Option<u64>,
    max_execution_time_s: Option<u64>,
    s3_endpoint: Option<String>,
    sandbox: SandboxMode,
    forwarder: Option<Forward>,
    cgroups: Option<WorkerCgroups>,
    open_acl: bool,
}

impl ProcessLauncher {
    /// Workers run `binary` and get a private directory under `tmp_root` each.
    /// They are sealed by [`sandbox::default_mode`]: `netns` on Linux; elsewhere
    /// they run unsealed, which is logged here.
    pub fn new(binary: impl Into<PathBuf>, tmp_root: impl Into<PathBuf>) -> Self {
        let sandbox = sandbox::default_mode();
        if sandbox == SandboxMode::None {
            eprintln!("{}", sandbox::unsandboxed_warning("no L3 on this system"));
        }
        Self {
            binary: binary.into(),
            tmp_root: tmp_root.into(),
            memory_limit: None,
            max_execution_time_s: None,
            s3_endpoint: None,
            sandbox,
            forwarder: None,
            cgroups: None,
            open_acl: false,
        }
    }

    /// Seals workers with `mode` (`--sandbox=netns|pods`). `none` is refused
    /// here: it is [`ProcessLauncher::unsandboxed`], which must be asked for by
    /// name (HS1 R1.10).
    pub fn with_sandbox(mut self, mode: SandboxMode) -> Result<Self, HouseError> {
        if mode == SandboxMode::None && cfg!(target_os = "linux") {
            return Err(HouseError::from(ChError::bad_arguments(
                "--sandbox=none is development only and must be forced; \
                 House workers run sealed (netns or pods) on Linux"
                    .to_string(),
            )));
        }
        self.sandbox = mode;
        Ok(self)
    }

    /// Workers run **without L3** (`--sandbox=none`, forced): development only,
    /// logged at once. The deny list (L1) and chDB's own controls (L2) still
    /// apply; nothing stops what gets past them.
    pub fn unsandboxed(mut self) -> Self {
        eprintln!("{}", sandbox::unsandboxed_warning("forced"));
        self.sandbox = SandboxMode::None;
        self
    }

    /// The sandbox workers get.
    pub fn sandbox(&self) -> SandboxMode {
        self.sandbox
    }

    /// Where a worker's bucket reads go. In `netns`, through its forwarder (fd 4)
    /// to `upstream`, so the worker's endpoint is its own loopback
    /// (`http://127.0.0.1:FORWARDER_PORT/`); in `pods` and `none`, to `upstream`
    /// directly. Replaces any [`ProcessLauncher::with_s3_endpoint`].
    pub fn with_forwarder(mut self, upstream: std::net::SocketAddr) -> Self {
        self.forwarder = Some(Forward::Relay(upstream));
        self
    }

    /// Where a worker's bucket reads go: `target`, a server in the front's own
    /// process (`house-cache`, HS1 Task 9) that knows each connection's worker.
    /// In `netns`, through the worker's forwarder channel; otherwise through a
    /// loopback endpoint `target` opens for that worker. The pool tells it which
    /// namespace each worker is bound to ([`Launcher::bound`]). Replaces any
    /// [`ProcessLauncher::with_s3_endpoint`].
    pub fn with_forward_target(mut self, target: Arc<dyn ForwardTarget>) -> Self {
        self.forwarder = Some(Forward::Target(target));
        self
    }

    /// Each worker gets a child of `cgroups`, with its limits, and joins it
    /// before it boots.
    pub fn with_cgroups(mut self, cgroups: WorkerCgroups) -> Self {
        self.cgroups = Some(cgroups);
        self
    }

    /// Workers whose settings profile caps `max_execution_time` at `seconds`
    /// (`--max-execution-time`, D1571): the front's own cap.
    pub fn with_max_execution_time(mut self, seconds: u64) -> Self {
        self.max_execution_time_s = Some(seconds);
        self
    }

    /// The memory each worker sizes its caches and engine for.
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit = Some(bytes);
        self
    }

    /// The loopback endpoint each worker's read views name (no grant names it:
    /// HS1 R-ACL.1).
    pub fn with_s3_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.s3_endpoint = Some(endpoint.into());
        self
    }

    /// Test builds only: workers start with chDB's own all-powerful `default`
    /// and no settings profile (`--test-open-acl`, HS1 R-ACL.1), so a test can
    /// show that the engine's access control is what refuses a statement, or test
    /// the layers behind it (`house-cache`, the sandbox) on their own.
    #[cfg(feature = "test-hooks")]
    pub fn with_open_acl(mut self) -> Self {
        self.open_acl = true;
        self
    }

    /// The command line of worker `id`, without the program name. Never a
    /// credential: the grammar is `loams_house_worker::WorkerArgs`.
    ///
    /// It has no side effect: an unsealed worker whose reads go to a
    /// [`ForwardTarget`] gets its endpoint only when [`Launcher::launch`] opens
    /// it, so `--s3-endpoint` is absent here.
    pub fn args(&self, id: &str) -> Vec<String> {
        self.args_with(id, self.fixed_endpoint())
    }

    /// The bucket endpoint every worker is given, when it does not depend on
    /// the worker (`--s3-endpoint`).
    fn fixed_endpoint(&self) -> Option<String> {
        match (&self.forwarder, self.sandbox) {
            (Some(_), SandboxMode::Netns) => Some(format!("http://127.0.0.1:{FORWARDER_PORT}/")),
            (Some(Forward::Relay(upstream)), _) => Some(format!("http://{upstream}/")),
            (Some(Forward::Target(_)), _) => None,
            (None, _) => self.s3_endpoint.clone(),
        }
    }

    /// The target an unsealed worker's private endpoint is opened on, if any.
    fn endpoint_target(&self) -> Option<&Arc<dyn ForwardTarget>> {
        match (&self.forwarder, self.sandbox) {
            (Some(Forward::Target(target)), mode) if mode != SandboxMode::Netns => Some(target),
            _ => None,
        }
    }

    fn args_with(&self, id: &str, endpoint: Option<String>) -> Vec<String> {
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
        if let Some(seconds) = self.max_execution_time_s {
            args.push("--max-execution-time".to_string());
            args.push(seconds.to_string());
        }
        if let Some(endpoint) = endpoint {
            args.push("--s3-endpoint".to_string());
            args.push(endpoint);
        }
        args.push("--sandbox".to_string());
        args.push(self.sandbox.as_str().to_string());
        if self.has_forwarder() {
            args.push("--forwarder-port".to_string());
            args.push(FORWARDER_PORT.to_string());
        }
        if let Some(cgroups) = &self.cgroups {
            args.push("--cgroup".to_string());
            args.push(cgroups.dir(id).display().to_string());
        }
        if self.open_acl {
            args.push("--test-open-acl".to_string());
        }
        args
    }

    /// Whether workers get a forwarder socket on fd 4.
    fn has_forwarder(&self) -> bool {
        self.forwarder.is_some() && self.sandbox == SandboxMode::Netns
    }
}

/// `stream` again, on a descriptor above the ones a worker inherits (3 and 4),
/// so the spawn's `dup2`s can neither be no-ops (`dup2(3, 3)` would leave
/// close-on-exec set) nor overwrite each other's source.
fn above_inherited(stream: StdUnixStream) -> Result<StdUnixStream, HouseError> {
    let mut held = Vec::new();
    let mut stream = stream;
    while stream.as_raw_fd() <= FORWARDER_SOCKET_FD {
        let higher = stream
            .try_clone()
            .map_err(|err| spawn_error("a socket", err))?;
        held.push(stream);
        stream = higher;
    }
    Ok(stream)
}

fn spawn_error(what: &str, err: impl fmt::Display) -> HouseError {
    HouseError::from(ChError::network_error(format!(
        "could not start a House worker: {what}: {err}"
    )))
}

impl Launcher for ProcessLauncher {
    fn bound(&self, id: &str, namespace: &str) {
        if let Some(Forward::Target(target)) = &self.forwarder {
            target.bound(id, namespace);
        }
    }

    fn launch(&self, id: &str) -> Result<Launched, HouseError> {
        let dir = self.tmp_root.join(id);
        std::fs::create_dir_all(&dir).map_err(|err| spawn_error("its directory", err))?;

        let (front, worker) =
            StdUnixStream::pair().map_err(|err| spawn_error("a socket pair", err))?;
        let worker = above_inherited(worker)?;
        let forwarder = if self.has_forwarder() {
            let (front_end, worker_end) =
                StdUnixStream::pair().map_err(|err| spawn_error("a forwarder socket pair", err))?;
            Some((front_end, above_inherited(worker_end)?))
        } else {
            None
        };
        let cgroup = match &self.cgroups {
            Some(cgroups) => Some(cgroups.create(id)?),
            None => None,
        };
        let null = std::fs::File::open("/dev/null").map_err(|err| spawn_error("/dev/null", err))?;

        let mut actions =
            PosixSpawnFileActions::init().map_err(|err| spawn_error("file actions", err))?;
        actions
            .add_dup2(null.as_raw_fd(), 0)
            .and_then(|()| actions.add_dup2(null.as_raw_fd(), 1))
            .and_then(|()| actions.add_dup2(worker.as_raw_fd(), loams_house_ipc::WORKER_SOCKET_FD))
            .map_err(|err| spawn_error("file actions", err))?;
        if let Some((_, worker_end)) = &forwarder {
            actions
                .add_dup2(worker_end.as_raw_fd(), FORWARDER_SOCKET_FD)
                .map_err(|err| spawn_error("file actions", err))?;
        }

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
        // An unsealed worker's private endpoint: released on every path out of
        // here until its exit is watched.
        let mut opened = None;
        let endpoint = match self.endpoint_target() {
            Some(target) => {
                let addr = target
                    .endpoint(id)
                    .map_err(|err| spawn_error("its bucket endpoint", err))?;
                opened = Some(EndpointGuard {
                    target: Arc::clone(target),
                    id: id.to_string(),
                    armed: true,
                });
                Some(format!("http://{addr}/"))
            }
            None => self.fixed_endpoint(),
        };
        for arg in self.args_with(id, endpoint) {
            argv.push(cstring(&arg)?);
        }
        // §49 §13.1: workers hold no credentials. Nothing from the front's
        // environment is passed on.
        let envp: Vec<std::ffi::CString> = Vec::new();

        let pid = match posix_spawn(self.binary.as_path(), &actions, &attr, &argv, &envp) {
            Ok(pid) => pid,
            Err(err) => {
                if let Some(cgroup) = &cgroup {
                    sandbox::remove(cgroup);
                }
                return Err(spawn_error(&self.binary.display().to_string(), err));
            }
        };
        drop(worker);
        drop(null);
        let control = ProcessControl::start(pid, dir, cgroup);
        if let Some((front_end, worker_end)) = forwarder {
            drop(worker_end);
            let forward = self
                .forwarder
                .clone()
                .ok_or_else(|| spawn_error("the forwarder", "no upstream"))?;
            if let Err(err) = sandbox::serve_forwarder(front_end, forward, id) {
                control.terminate();
                return Err(spawn_error("the forwarder thread", err));
            }
        } else if let Some(Forward::Target(target)) = &self.forwarder {
            // No forwarder channel whose end says the worker is gone: its exit does.
            forget_on_exit(Arc::clone(target), id, control.exit());
            if let Some(guard) = opened.take() {
                guard.disarm();
            }
        }

        Ok(Launched {
            socket: front,
            control,
        })
    }
}

/// Releases a worker's private endpoint unless disarmed: a launch that fails
/// after opening it must not leave its listener behind.
struct EndpointGuard {
    target: Arc<dyn ForwardTarget>,
    id: String,
    armed: bool,
}

impl EndpointGuard {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for EndpointGuard {
    fn drop(&mut self) {
        if self.armed {
            self.target.gone(&self.id);
        }
    }
}

/// Tells `target` that worker `id` is gone once its exit is seen.
fn forget_on_exit(
    target: Arc<dyn ForwardTarget>,
    id: &str,
    mut exit: watch::Receiver<Option<String>>,
) {
    let id = id.to_string();
    let wait = async move {
        let _ = exit.wait_for(Option::is_some).await;
        target.gone(&id);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(wait);
        }
        Err(_) => {
            let _ = std::thread::Builder::new()
                .name("house-forget".to_string())
                .spawn(move || {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_time()
                        .build()
                    {
                        runtime.block_on(wait);
                    }
                });
        }
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
    /// The worker's cgroup child, when the front made one.
    cgroup: Option<PathBuf>,
    /// Its `memory.events` counted an OOM kill, read by the waiter before it
    /// removes the child.
    oom: std::sync::atomic::AtomicBool,
}

/// Whether a cgroup's `memory.events` counts an OOM kill (`oom_kill N`, N > 0).
pub fn oom_killed_in(cgroup: &Path) -> bool {
    std::fs::read_to_string(cgroup.join("memory.events")).is_ok_and(|events| {
        events.lines().any(|line| {
            line.strip_prefix("oom_kill ")
                .and_then(|n| n.trim().parse::<u64>().ok())
                .is_some_and(|n| n > 0)
        })
    })
}

/// `usage_usec` of a cgroup's `cpu.stat`: every process in it, every thread.
pub fn cgroup_cpu(cgroup: &Path) -> Option<Duration> {
    let stat = std::fs::read_to_string(cgroup.join("cpu.stat")).ok()?;
    stat.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|n| n.trim().parse::<u64>().ok())
        .map(Duration::from_micros)
}

/// The kernel's `USER_HZ`, the unit of `/proc/<pid>/stat`'s CPU fields: 100 on
/// every Linux ABI the House runs on.
const USER_HZ: u64 = 100;

/// `utime + stime` of `/proc/<pid>/stat` (fields 14 and 15, all threads),
/// from the text after the command name, which may hold spaces and parentheses.
pub fn proc_stat_cpu(stat: &str) -> Option<Duration> {
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (the state).
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(Duration::from_millis(
        utime.saturating_add(stime).saturating_mul(1000 / USER_HZ),
    ))
}

impl ProcessControl {
    fn start(pid: Pid, dir: PathBuf, cgroup: Option<PathBuf>) -> Arc<Self> {
        let (exit, _) = watch::channel(None);
        let control = Arc::new(Self {
            pid,
            exited: Mutex::new(false),
            exit,
            cgroup,
            oom: std::sync::atomic::AtomicBool::new(false),
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
    /// worker's private directory and its cgroup.
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
        if let Some(cgroup) = &self.cgroup {
            // Read before the child goes (HS1 Task 21).
            if oom_killed_in(cgroup) {
                self.oom.store(true, std::sync::atomic::Ordering::Release);
            }
            sandbox::remove(cgroup);
        }
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

    fn oom_killed(&self) -> bool {
        // The waiter's reading, else the child's own file while it is still
        // there (the lease may see the socket end before the waiter runs), else
        // the waiter's reading again, made before it removed the child.
        let seen = || self.oom.load(std::sync::atomic::Ordering::Acquire);
        seen() || self.cgroup.as_deref().is_some_and(oom_killed_in) || seen()
    }

    fn cpu_time(&self) -> Option<Duration> {
        // Under the exit lock: a pid the waiter has seen exit may be reaped and
        // recycled, and its numbers would be another process's.
        let exited = self.exited();
        if *exited {
            return None;
        }
        self.cgroup.as_deref().and_then(cgroup_cpu).or_else(|| {
            std::fs::read_to_string(format!("/proc/{}/stat", self.pid.as_raw()))
                .ok()
                .as_deref()
                .and_then(proc_stat_cpu)
        })
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
                // Not isolated (see above): chDB runs in the front's own process.
                sandbox: SandboxMode::None,
                forwarder_port: None,
                cgroup: None,
                open_acl: false,
                max_execution_time_s: loams_house_worker::settings::MAX_EXECUTION_TIME_S,
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
