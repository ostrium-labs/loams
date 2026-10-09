//! The supervisor's pool half (HS1 Task 2, design §49 §10.1): warm workers, one
//! namespace per worker, and the rules that retire them.
//!
//! # The rules
//!
//! * **Warm pool.** At least [`PoolConfig::min_idle_workers`] booted, unbound
//!   workers wait for a namespace. [`WorkerPool::start`] boots the first one
//!   before it returns, which also pulls the 554 MB library into the page cache
//!   (HS1 R1.12: 1 s cold, 140 ms warm).
//! * **Binding.** [`WorkerPool::acquire`] gives a namespace one of its own idle
//!   workers first, then binds an unbound one with `Bind`, then starts a new one.
//!   A bound worker never serves another namespace: the pool only ever hands it to
//!   its own, and the worker refuses a second `Bind` (`binding_is_exclusive`).
//! * **Recycling.** A worker is killed after `max_queries_per_worker` statements
//!   (`budget`), when it sits bound and idle past `idle_unbind_after` (`idle`),
//!   after any cancellation or timeout, when its resident set passes
//!   `worker_rss_ceiling` (`rss`), and when it reports a poisoning error
//!   (`poisoned`). Every exit is a `SIGKILL` ([`crate::watchdog`]).
//! * **Limits.** At most `max_workers` per node and `max_workers_per_namespace`
//!   per namespace. A namespace at its limit waits; a node at its limit retires
//!   another namespace's idle worker, or waits. Waiting past `acquire_timeout`
//!   answers `202 TOO_MANY_SIMULTANEOUS_QUERIES` (§49 §12's queue is Task 21's).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use bytes::Bytes;
use loams_house_ipc::{
    Bind, CHUNK_BYTES, Chunk, Classification, CodecError, Execute, Frame, FrameCodec,
    PROTOCOL_VERSION, Progress, Ready,
};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::errors::{ChError, HouseError};
use crate::watchdog::{ExitReason, KillHandle, Launcher, WorkerShared};

/// The most bytes [`WorkerLease::run`] collects (R2.11, M6).
pub const RUN_MAX_BYTES: usize = 64 * 1024 * 1024;

/// The pool's knobs, with §49 §10.1's defaults.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Booted, unbound workers kept waiting (default 2).
    pub min_idle_workers: usize,
    /// Workers per node (default the node's cores ÷ 2).
    pub max_workers: usize,
    /// Workers per namespace (default 8).
    pub max_workers_per_namespace: usize,
    /// Workers a namespace's sessions may pin for their temporary tables (§49
    /// §10.3, default 2); past it, `CREATE TEMPORARY TABLE` answers `202`.
    pub max_pinned_workers_per_namespace: usize,
    /// Statements before a worker is retired (default 500).
    pub max_queries_per_worker: u32,
    /// How long a bound worker may sit idle (default 60 s).
    pub idle_unbind_after: Duration,
    /// The resident set past which a worker is retired after its statement.
    pub worker_rss_ceiling: u64,
    /// How long `acquire` waits for a worker before answering `202`.
    pub acquire_timeout: Duration,
    /// How long a new worker has to send `Ready`.
    pub boot_timeout: Duration,
    /// How often the reaper looks for idle and dead workers.
    pub reap_every: Duration,
    /// The isolation class `Bind` names.
    pub isolation_class: String,
    /// The temporary-directory quota `Bind` names.
    pub temp_dir_quota_bytes: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Self {
            min_idle_workers: 2,
            max_workers: (cores / 2).max(1),
            max_workers_per_namespace: 8,
            max_pinned_workers_per_namespace: 2,
            max_queries_per_worker: 500,
            idle_unbind_after: Duration::from_secs(60),
            worker_rss_ceiling: 6 * 1024 * 1024 * 1024,
            acquire_timeout: Duration::from_secs(30),
            boot_timeout: Duration::from_secs(30),
            reap_every: Duration::from_secs(1),
            isolation_class: "shared".to_string(),
            temp_dir_quota_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// How the caller thinks a lease went. The pool also looks at the lease itself:
/// a worker still running a statement, dead, or poisoned is retired whatever this
/// says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The statements ended normally (with results or with engine errors).
    Completed,
    /// The caller gave up on the worker; it is retired as `poisoned`.
    Poisoned,
}

/// A snapshot of the pool, for tests and metrics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Booted workers bound to no namespace.
    pub idle_unbound: usize,
    /// Idle workers bound to a namespace.
    pub idle_bound: usize,
    /// Workers lent out.
    pub leased: usize,
    /// Workers starting, or being bound for an `acquire`.
    pub booting: usize,
    /// Workers started since the pool began.
    pub spawned_total: u64,
    /// Leases handed out since the pool began.
    pub acquired_total: u64,
    /// Workers pinned to sessions.
    pub pinned: usize,
    /// Workers retired, by reason (`loams_house_worker_kills_total{reason}`).
    pub kills: BTreeMap<ExitReason, u64>,
    /// Workers (idle or lent) per bound namespace.
    pub bound: BTreeMap<String, usize>,
}

impl PoolStats {
    /// Retired workers with `reason`.
    pub fn kills_for(&self, reason: ExitReason) -> u64 {
        self.kills.get(&reason).copied().unwrap_or(0)
    }
}

/// One worker the pool holds.
#[derive(Debug)]
struct Worker {
    shared: Arc<WorkerShared>,
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    ready: Ready,
    namespace: Option<String>,
    queries: u32,
    last_rss: u64,
    idle_since: Instant,
    /// A request was sent and its terminal frame not yet read.
    in_flight: bool,
    /// The worker said it is unfit for more work.
    poisoned: bool,
    /// The socket ended.
    dead: bool,
}

#[derive(Debug, Default)]
struct State {
    idle: Vec<Worker>,
    /// Lent workers' namespaces, by worker id.
    leased: BTreeMap<String, String>,
    /// Workers an `acquire` holds outside `idle` and `leased`: starting or being
    /// bound. Counted against the node through a [`Reservation`], so a dropped
    /// `acquire` future gives the slot back (review I2).
    in_hand: usize,
    /// Workers starting to refill the warm pool.
    warming: usize,
    spawned_total: u64,
    acquired_total: u64,
    /// Workers pinned to a session, by worker id, with their namespace.
    pinned: BTreeMap<String, String>,
    kills: BTreeMap<ExitReason, u64>,
    closed: bool,
}

impl State {
    fn total(&self) -> usize {
        self.idle.len() + self.leased.len() + self.in_hand + self.warming
    }

    fn for_namespace(&self, ns: &str) -> usize {
        self.idle
            .iter()
            .filter(|w| w.namespace.as_deref() == Some(ns))
            .count()
            + self.leased.values().filter(|n| n.as_str() == ns).count()
    }

    fn unbound_idle(&self) -> usize {
        self.idle.iter().filter(|w| w.namespace.is_none()).count()
    }
}

#[derive(Debug)]
struct PoolInner {
    config: PoolConfig,
    launcher: Arc<dyn Launcher>,
    runtime: Handle,
    state: Mutex<State>,
    changed: Notify,
    next_id: AtomicU64,
    /// Every setting name the engine knows, from the first worker's `Ready`.
    known_settings: std::sync::OnceLock<Arc<std::collections::HashSet<String>>>,
}

/// The pool of worker processes. Cheap to clone.
#[derive(Clone, Debug)]
pub struct WorkerPool {
    inner: Arc<PoolInner>,
}

/// A worker lent to one caller, bound to one namespace. Return it with
/// [`WorkerPool::release`] or [`WorkerPool::kill`]; dropping it kills the worker
/// as a cancellation, because a worker abandoned mid-statement cannot be reused.
#[derive(Debug)]
pub struct WorkerLease {
    pool: Weak<PoolInner>,
    worker: Option<Worker>,
    namespace: String,
    query_id: String,
}

/// What a running statement produces, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Output bytes.
    Chunk(Chunk),
    /// Counters so far.
    Progress(Progress),
    /// The statement finished; the final counters.
    Done(Progress),
}

/// A whole statement's output, from [`WorkerLease::run`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// Every chunk, concatenated.
    pub bytes: Vec<u8>,
    /// How many `Chunk` frames carried them.
    pub chunks: usize,
    /// The final counters.
    pub stats: Progress,
}

impl PoolInner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_id(&self) -> String {
        format!(
            "w{}-{}",
            std::process::id(),
            self.next_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Kills a worker and counts it. The reason recorded first wins, so a worker
    /// cancelled through its [`KillHandle`] counts as `cancel`, not as whatever the
    /// caller retires it with afterwards.
    fn retire(&self, worker: Worker, reason: ExitReason) {
        let reason = worker.shared.kill(reason);
        self.state().pinned.remove(worker.shared.id());
        *self.state().kills.entry(reason).or_insert(0) += 1;
        drop(worker);
        self.changed.notify_waiters();
    }

    /// Starts a worker and waits for its `Ready`.
    async fn boot(&self) -> Result<Worker, HouseError> {
        let id = self.next_id();
        let launched = self.launcher.launch(&id)?;
        let shared = WorkerShared::new(id, launched.control);
        // Until `Ready` makes it a `Worker` (whose drop kills it), a boot that is
        // abandoned — a cancelled `acquire`, a timeout — kills it here (review I2).
        let abandoned = KillOnDrop(Some(Arc::clone(&shared)));
        launched
            .socket
            .set_nonblocking(true)
            .map_err(|err| network(format!("worker socket: {err}")))?;
        let socket = UnixStream::from_std(launched.socket)
            .map_err(|err| network(format!("worker socket: {err}")))?;
        let (mut reader, writer) = socket.into_split();
        self.state().spawned_total += 1;

        let first = tokio::time::timeout(
            self.config.boot_timeout,
            FrameCodec::read_async(&mut reader),
        )
        .await;
        let failure = match first {
            Ok(Ok(Some(Frame::Ready(mut ready)))) if ready.protocol == PROTOCOL_VERSION => {
                abandoned.disarm();
                let names = std::mem::take(&mut ready.settings);
                self.known_settings
                    .get_or_init(|| Arc::new(names.into_iter().collect()));
                return Ok(Worker {
                    shared,
                    reader,
                    writer,
                    ready,
                    namespace: None,
                    queries: 0,
                    last_rss: 0,
                    idle_since: Instant::now(),
                    in_flight: false,
                    poisoned: false,
                    dead: false,
                });
            }
            Ok(Ok(Some(Frame::Ready(ready)))) => {
                format!("it speaks hsw version {}", ready.protocol)
            }
            Ok(Ok(Some(Frame::Error { error, .. }))) => format!("it failed to boot: {error}"),
            Ok(Ok(Some(other))) => format!("it sent {} before Ready", other.kind()),
            Ok(Ok(None)) => "it exited before Ready".to_string(),
            Ok(Err(err)) => format!("its socket failed: {err}"),
            Err(_) => format!("it sent no Ready in {:?}", self.config.boot_timeout),
        };
        let reason = shared.kill(ExitReason::Crash);
        *self.state().kills.entry(reason).or_insert(0) += 1;
        let exit = shared.wait_exit(Duration::from_millis(500)).await;
        Err(network(format!(
            "a House worker did not start: {failure}{}",
            exit.map(|e| format!(" ({e})")).unwrap_or_default()
        )))
    }

    /// Starts warm workers until `min_idle_workers` are idle or starting.
    fn top_up(self: &Arc<Self>) {
        let mut state = self.state();
        if state.closed {
            return;
        }
        while state.unbound_idle() + state.warming < self.config.min_idle_workers
            && state.total() < self.config.max_workers
        {
            state.warming += 1;
            let pool = Arc::clone(self);
            self.runtime.spawn(async move {
                let booted = pool.boot().await;
                let mut state = pool.state();
                state.warming -= 1;
                match booted {
                    Ok(worker) if !state.closed => state.idle.push(worker),
                    Ok(worker) => {
                        drop(state);
                        pool.retire(worker, ExitReason::Drain);
                        return;
                    }
                    Err(_) => {}
                }
                drop(state);
                pool.changed.notify_waiters();
            });
        }
    }

    /// Retires bound workers idle too long and idle workers that died.
    fn reap(self: &Arc<Self>) {
        let now = Instant::now();
        let mut retired = Vec::new();
        {
            let mut state = self.state();
            let mut keep = Vec::with_capacity(state.idle.len());
            let idle = std::mem::take(&mut state.idle);
            for worker in idle {
                if worker.shared.has_exited() {
                    retired.push((worker, ExitReason::Crash));
                } else if worker.namespace.is_some()
                    && !state.pinned.contains_key(worker.shared.id())
                    && now.duration_since(worker.idle_since) >= self.config.idle_unbind_after
                {
                    retired.push((worker, ExitReason::Idle));
                } else {
                    keep.push(worker);
                }
            }
            state.idle = keep;
        }
        let any = !retired.is_empty();
        for (worker, reason) in retired {
            self.retire(worker, reason);
        }
        if any {
            self.top_up();
        }
    }
}

/// Kills a booting worker unless disarmed.
struct KillOnDrop(Option<Arc<WorkerShared>>);

impl KillOnDrop {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take() {
            shared.kill(ExitReason::Cancel);
        }
    }
}

/// One `in_hand` slot. Dropping it gives the slot back and wakes waiters, which is
/// what makes [`WorkerPool::acquire`] cancel-safe (review I2): a caller that gives
/// up while a worker is being started or bound no longer shrinks the pool.
struct Reservation {
    inner: Arc<PoolInner>,
    held: bool,
}

impl Reservation {
    /// Takes a slot the caller already counted under the state lock.
    fn counted(inner: &Arc<PoolInner>) -> Self {
        Self {
            inner: Arc::clone(inner),
            held: true,
        }
    }

    /// Turns the slot into a lease, under one lock, so the node's total never
    /// dips between the two.
    fn into_lease(mut self, id: String, namespace: &str) {
        let mut state = self.inner.state();
        state.in_hand -= 1;
        state.leased.insert(id, namespace.to_string());
        self.held = false;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            self.inner.state().in_hand -= 1;
            self.inner.changed.notify_waiters();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // A worker the pool lets go of is never left running: a cancelled
        // `acquire` dropping it mid-`Bind`, a shutdown, or a retirement (which has
        // already killed it; a second `SIGKILL` is a no-op).
        self.shared.kill(ExitReason::Cancel);
    }
}

fn too_many(namespace: &str, waited: Duration) -> HouseError {
    HouseError::from(ChError::too_many_simultaneous_queries(format!(
        "No House worker for namespace {namespace} became free in {waited:?}"
    )))
}

/// A frame the front could not encode: the request's fault (too large), never the
/// worker's (review I4).
fn unsendable(query_id: &str, err: &CodecError) -> HouseError {
    HouseError::from(ChError::bad_arguments(format!(
        "query {query_id}: the statement cannot be sent to a worker: {err}"
    )))
}

fn network(message: String) -> HouseError {
    HouseError::from(ChError::network_error(message))
}

/// What `acquire` decided under the lock.
enum Step {
    Ready(Worker),
    Bind(Worker),
    Spawn,
    ReplaceThenSpawn(Worker),
    Wait,
}

impl WorkerPool {
    /// Starts a pool: boots one worker and waits for it (warming the page cache),
    /// then fills the warm pool and starts the reaper in the background.
    pub async fn start(
        config: PoolConfig,
        launcher: Arc<dyn Launcher>,
    ) -> Result<Self, HouseError> {
        let inner = Arc::new(PoolInner {
            config,
            launcher,
            runtime: Handle::current(),
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            next_id: AtomicU64::new(1),
            known_settings: std::sync::OnceLock::new(),
        });
        let first = inner.boot().await?;
        inner.state().idle.push(first);
        inner.top_up();

        let weak = Arc::downgrade(&inner);
        let every = inner.config.reap_every;
        inner.runtime.spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                let Some(pool) = weak.upgrade() else { return };
                if pool.state().closed {
                    return;
                }
                pool.reap();
            }
        });
        Ok(Self { inner })
    }

    /// The pool's configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.inner.config
    }

    /// A worker bound to `namespace`, ready for a statement.
    pub async fn acquire(&self, namespace: &str) -> Result<WorkerLease, HouseError> {
        let inner = &self.inner;
        let deadline = Instant::now() + inner.config.acquire_timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(too_many(namespace, inner.config.acquire_timeout));
            }
            let notified = inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let step = {
                let mut state = inner.state();
                if state.closed {
                    return Err(network(
                        "the House worker pool is shutting down".to_string(),
                    ));
                }
                if let Some(at) = state.idle.iter().position(|w| {
                    w.namespace.as_deref() == Some(namespace)
                        && !w.shared.has_exited()
                        && !state.pinned.contains_key(w.shared.id())
                }) {
                    Step::Ready(state.idle.swap_remove(at))
                } else if state.for_namespace(namespace) >= inner.config.max_workers_per_namespace {
                    Step::Wait
                } else if let Some(at) = state
                    .idle
                    .iter()
                    .position(|w| w.namespace.is_none() && !w.shared.has_exited())
                {
                    state.in_hand += 1;
                    Step::Bind(state.idle.swap_remove(at))
                } else if state.total() < inner.config.max_workers {
                    state.in_hand += 1;
                    Step::Spawn
                } else if let Some(at) = state.idle.iter().position(|w| {
                    w.namespace.as_deref() != Some(namespace)
                        && !state.pinned.contains_key(w.shared.id())
                }) {
                    // The node is full and another namespace has an idle worker:
                    // it gives up its slot.
                    state.in_hand += 1;
                    Step::ReplaceThenSpawn(state.idle.swap_remove(at))
                } else {
                    Step::Wait
                }
            };

            // Every step but `Ready` and `Wait` counted a slot under the lock; the
            // guard owns it from here, across every `.await` below.
            let reservation = match &step {
                Step::Bind(_) | Step::Spawn | Step::ReplaceThenSpawn(_) => {
                    Some(Reservation::counted(inner))
                }
                Step::Ready(_) | Step::Wait => None,
            };
            let worker = match step {
                Step::Ready(worker) => worker,
                Step::Bind(worker) => {
                    match tokio::time::timeout_at(deadline, self.bind(worker, namespace)).await {
                        Ok(Ok(worker)) => worker,
                        // The worker refused and was retired: try again.
                        Ok(Err(_)) => continue,
                        Err(_) => return Err(too_many(namespace, inner.config.acquire_timeout)),
                    }
                }
                Step::Spawn | Step::ReplaceThenSpawn(_) => {
                    if let Step::ReplaceThenSpawn(old) = step {
                        inner.retire(old, ExitReason::Idle);
                    }
                    let worker = match tokio::time::timeout_at(deadline, inner.boot()).await {
                        Ok(Ok(worker)) => worker,
                        Ok(Err(err)) => return Err(err),
                        Err(_) => return Err(too_many(namespace, inner.config.acquire_timeout)),
                    };
                    match tokio::time::timeout_at(deadline, self.bind(worker, namespace)).await {
                        Ok(Ok(worker)) => worker,
                        Ok(Err(err)) => return Err(err),
                        Err(_) => return Err(too_many(namespace, inner.config.acquire_timeout)),
                    }
                }
                Step::Wait => {
                    if tokio::time::timeout_at(deadline, notified).await.is_err() {
                        return Err(too_many(namespace, inner.config.acquire_timeout));
                    }
                    continue;
                }
            };

            let id = worker.shared.id().to_string();
            // A new lease: handles from earlier leases of this worker go stale.
            worker.shared.bump_epoch();
            match reservation {
                Some(reservation) => reservation.into_lease(id, namespace),
                None => {
                    inner.state().leased.insert(id, namespace.to_string());
                }
            }
            inner.state().acquired_total += 1;
            inner.top_up();
            return Ok(WorkerLease {
                pool: Arc::downgrade(inner),
                worker: Some(worker),
                namespace: namespace.to_string(),
                query_id: String::new(),
            });
        }
    }

    /// Sends `Bind` and waits for its answer. A worker that refuses is retired.
    async fn bind(&self, mut worker: Worker, namespace: &str) -> Result<Worker, HouseError> {
        let bind = Frame::Bind(Bind {
            namespace: namespace.to_string(),
            isolation_class: self.inner.config.isolation_class.clone(),
            settings: Vec::new(),
            proxy_endpoint: None,
            temp_dir_quota_bytes: self.inner.config.temp_dir_quota_bytes,
        });
        let answer = match FrameCodec::write_async(&mut worker.writer, &bind).await {
            Ok(()) => FrameCodec::read_async(&mut worker.reader).await,
            Err(err) => Err(err),
        };
        match answer {
            Ok(Some(Frame::Done)) => {
                worker.namespace = Some(namespace.to_string());
                Ok(worker)
            }
            other => {
                let why = match other {
                    Ok(Some(Frame::Error { error, .. })) => HouseError::from(error),
                    Ok(Some(frame)) => {
                        network(format!("a worker answered Bind with {}", frame.kind()))
                    }
                    Ok(None) => network("a worker exited while being bound".to_string()),
                    Err(err) => network(format!("a worker's socket failed during Bind: {err}")),
                };
                self.inner.retire(worker, ExitReason::Poisoned);
                self.inner.top_up();
                Err(why)
            }
        }
    }

    /// A worker for `namespace`, pinned to the caller's session (its temporary
    /// tables live on it): only [`WorkerPool::acquire_pinned`] lends it again, and
    /// neither the query budget nor the idle reaper retires it, until
    /// [`WorkerPool::unpin`]. At most `max_pinned_workers_per_namespace` per
    /// namespace; past it, `202` (§49 §10.3, Q690).
    pub async fn acquire_and_pin(&self, namespace: &str) -> Result<WorkerLease, HouseError> {
        let full = |inner: &PoolInner| {
            inner
                .state()
                .pinned
                .values()
                .filter(|ns| ns.as_str() == namespace)
                .count()
                >= inner.config.max_pinned_workers_per_namespace
        };
        let too_many = || {
            HouseError::from(ChError::too_many_simultaneous_queries(format!(
                "Too many sessions of namespace {namespace} hold temporary tables (at most {}); \
                 end one, or run the query without a temporary table",
                self.inner.config.max_pinned_workers_per_namespace
            )))
        };
        if full(&self.inner) {
            return Err(too_many());
        }
        let lease = self.acquire(namespace).await?;
        let id = lease.worker_id().to_string();
        let mut state = self.inner.state();
        if state
            .pinned
            .values()
            .filter(|ns| ns.as_str() == namespace)
            .count()
            >= self.inner.config.max_pinned_workers_per_namespace
        {
            drop(state);
            self.release(lease, Outcome::Completed);
            return Err(too_many());
        }
        state.pinned.insert(id, namespace.to_string());
        Ok(lease)
    }

    /// The worker pinned as `worker_id`, for its session; `Ok(None)` when it is no
    /// longer there (it died or was retired: its temporary tables are gone).
    pub async fn acquire_pinned(
        &self,
        namespace: &str,
        worker_id: &str,
    ) -> Result<Option<WorkerLease>, HouseError> {
        let inner = &self.inner;
        let deadline = Instant::now() + inner.config.acquire_timeout;
        loop {
            let notified = inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let taken = {
                let mut state = inner.state();
                if !state.pinned.contains_key(worker_id) {
                    return Ok(None);
                }
                state
                    .idle
                    .iter()
                    .position(|w| w.shared.id() == worker_id)
                    .map(|at| state.idle.swap_remove(at))
            };
            if let Some(worker) = taken {
                if worker.shared.has_exited() {
                    inner.retire(worker, ExitReason::Crash);
                    inner.top_up();
                    return Ok(None);
                }
                worker.shared.bump_epoch();
                {
                    let mut state = inner.state();
                    state
                        .leased
                        .insert(worker_id.to_string(), namespace.to_string());
                    state.acquired_total += 1;
                }
                return Ok(Some(WorkerLease {
                    pool: Arc::downgrade(inner),
                    worker: Some(worker),
                    namespace: namespace.to_string(),
                    query_id: String::new(),
                }));
            }
            // Lent out (a statement of the same session still finishing): wait.
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(too_many(namespace, inner.config.acquire_timeout));
            }
        }
    }

    /// Every setting name the engine knows (the first worker's `Ready`), so the
    /// front tells `115` from `164` (HS1 Task 4).
    pub fn known_settings(&self) -> Arc<std::collections::HashSet<String>> {
        self.inner.known_settings.get().cloned().unwrap_or_default()
    }

    /// Releases a pin: the worker is an ordinary worker of its namespace again.
    pub fn unpin(&self, worker_id: &str) {
        self.inner.state().pinned.remove(worker_id);
        self.inner.changed.notify_waiters();
    }

    /// Returns a lease. The worker goes back to its namespace's idle list unless a
    /// recycle rule retires it.
    pub fn release(&self, mut lease: WorkerLease, outcome: Outcome) {
        let Some(mut worker) = lease.worker.take() else {
            return;
        };
        // The lease is over: its handles go stale before the worker can idle or be
        // lent again (review I1).
        worker.shared.bump_epoch();
        let inner = &self.inner;
        inner.state().leased.remove(worker.shared.id());
        let retire = if worker.dead || worker.shared.has_exited() {
            Some(ExitReason::Crash)
        } else if worker.in_flight {
            Some(ExitReason::Cancel)
        } else if worker.poisoned || outcome == Outcome::Poisoned {
            Some(ExitReason::Poisoned)
        } else if worker.queries >= inner.config.max_queries_per_worker
            && !inner.state().pinned.contains_key(worker.shared.id())
        {
            Some(ExitReason::Budget)
        } else if worker.last_rss > inner.config.worker_rss_ceiling {
            Some(ExitReason::Rss)
        } else {
            None
        };
        match retire {
            Some(reason) => inner.retire(worker, reason),
            None => {
                worker.idle_since = Instant::now();
                let mut state = inner.state();
                if state.closed {
                    drop(state);
                    inner.retire(worker, ExitReason::Drain);
                } else {
                    state.idle.push(worker);
                    drop(state);
                    inner.changed.notify_waiters();
                }
            }
        }
        inner.top_up();
    }

    /// Retires a lease's worker with `reason`.
    pub fn kill(&self, mut lease: WorkerLease, reason: ExitReason) {
        let Some(worker) = lease.worker.take() else {
            return;
        };
        self.inner.state().leased.remove(worker.shared.id());
        self.inner.retire(worker, reason);
        self.inner.top_up();
    }

    /// A snapshot of the pool.
    pub fn stats(&self) -> PoolStats {
        let state = self.inner.state();
        let mut bound: BTreeMap<String, usize> = BTreeMap::new();
        for ns in state
            .idle
            .iter()
            .filter_map(|w| w.namespace.clone())
            .chain(state.leased.values().cloned())
        {
            *bound.entry(ns).or_insert(0) += 1;
        }
        PoolStats {
            idle_unbound: state.unbound_idle(),
            idle_bound: state.idle.len() - state.unbound_idle(),
            leased: state.leased.len(),
            booting: state.in_hand + state.warming,
            spawned_total: state.spawned_total,
            acquired_total: state.acquired_total,
            pinned: state.pinned.len(),
            kills: state.kills.clone(),
            bound,
        }
    }

    /// The pids of the idle workers, for tests and debugging.
    pub fn idle_pids(&self) -> Vec<u32> {
        self.inner
            .state()
            .idle
            .iter()
            .map(|w| w.shared.pid())
            .collect()
    }

    /// Stops lending workers and kills the idle ones (`drain`). Lent workers are
    /// killed when they come back.
    pub fn shutdown(&self) {
        let idle = {
            let mut state = self.inner.state();
            state.closed = true;
            std::mem::take(&mut state.idle)
        };
        for worker in idle {
            self.inner.retire(worker, ExitReason::Drain);
        }
    }
}

impl Drop for PoolInner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for worker in state.idle.drain(..) {
            worker.shared.kill(ExitReason::Drain);
        }
    }
}

impl WorkerLease {
    fn worker(&mut self) -> Result<&mut Worker, HouseError> {
        self.worker
            .as_mut()
            .ok_or_else(|| network("this lease has no worker".to_string()))
    }

    /// The worker's opaque id (`X-Loams-Worker`).
    pub fn worker_id(&self) -> &str {
        self.worker.as_ref().map_or("", |w| w.shared.id())
    }

    /// The worker's pid.
    pub fn pid(&self) -> u32 {
        self.worker.as_ref().map_or(0, |w| w.shared.pid())
    }

    /// The namespace the worker is bound to.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// What the worker said when it booted.
    pub fn ready(&self) -> Option<&Ready> {
        self.worker.as_ref().map(|w| &w.ready)
    }

    /// A handle that kills this worker from elsewhere (`KILL QUERY`, a client that
    /// went away, a deadline), for the **current statement**: take it after
    /// [`WorkerLease::start`]. It is a no-op once a later statement starts or the
    /// worker is lent again (review I1).
    pub fn kill_handle(&self) -> Option<KillHandle> {
        self.worker
            .as_ref()
            .map(|w| KillHandle::new(Arc::clone(&w.shared)))
    }

    /// Sends a statement. Its output is read with [`WorkerLease::next_event`].
    pub async fn start(&mut self, execute: Execute) -> Result<(), HouseError> {
        self.query_id = execute.query_id.clone();
        let query_id = self.query_id.clone();
        let worker = self.worker()?;
        if worker.in_flight {
            return Err(network(format!(
                "query {query_id}: the worker is still running the previous statement"
            )));
        }
        worker.in_flight = true;
        worker.queries += 1;
        // A new statement: a handle taken for the previous one goes stale.
        worker.shared.bump_epoch();
        match FrameCodec::write_async(&mut worker.writer, &Frame::Execute(execute)).await {
            Ok(()) => Ok(()),
            Err(CodecError::Io(_)) => Err(self.died()),
            Err(err) => {
                // Nothing was written: the frame could not be encoded (review I4).
                // The worker is untouched and the statement never started.
                worker.in_flight = false;
                worker.queries -= 1;
                worker.shared.bump_epoch();
                Err(unsendable(&query_id, &err))
            }
        }
    }

    /// Sends a piece of the statement's `INSERT` body, split into frames of at most
    /// [`CHUNK_BYTES`] (review I4).
    pub async fn send_input(&mut self, bytes: Bytes) -> Result<(), HouseError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let piece = rest.split_to(CHUNK_BYTES.min(rest.len()));
            self.send_body(Frame::Input(piece)).await?;
        }
        Ok(())
    }

    /// Ends the statement's `INSERT` body.
    pub async fn end_input(&mut self) -> Result<(), HouseError> {
        self.send_body(Frame::InputEnd).await
    }

    async fn send_body(&mut self, frame: Frame) -> Result<(), HouseError> {
        let query_id = self.query_id.clone();
        let worker = self.worker()?;
        match FrameCodec::write_async(&mut worker.writer, &frame).await {
            Ok(()) => Ok(()),
            // Only a socket failure means the worker went away.
            Err(CodecError::Io(_)) => Err(self.died()),
            Err(err) => Err(unsendable(&query_id, &err)),
        }
    }

    /// The next event of the running statement. An engine error is the statement's
    /// answer; a worker that dies answers with its [`ExitReason::client_error`].
    pub async fn next_event(&mut self) -> Result<Event, HouseError> {
        let worker = self.worker()?;
        if !worker.in_flight {
            return Err(network(
                "no statement is running on this worker".to_string(),
            ));
        }
        match FrameCodec::read_async(&mut worker.reader).await {
            Ok(Some(Frame::Chunk(chunk))) => Ok(Event::Chunk(chunk)),
            Ok(Some(Frame::Progress(progress))) => {
                worker.last_rss = progress.rss_bytes;
                Ok(Event::Progress(progress))
            }
            Ok(Some(Frame::Stats(stats))) => {
                worker.last_rss = stats.rss_bytes;
                match FrameCodec::read_async(&mut worker.reader).await {
                    Ok(Some(Frame::Done)) => {
                        worker.in_flight = false;
                        // The statement is over: its handles go stale now, not when
                        // the next one starts (Task 2 re-review N1).
                        worker.shared.bump_epoch();
                        Ok(Event::Done(stats))
                    }
                    Ok(Some(other)) => Err(self.broke(other.kind())),
                    _ => Err(self.died()),
                }
            }
            Ok(Some(Frame::Error { error, poisoned })) => {
                worker.in_flight = false;
                worker.poisoned |= poisoned;
                // As for `Done`: the statement is over (N1). A fatal error kills
                // below through the pool's own, epoch-free path.
                worker.shared.bump_epoch();
                if error.is_fatal() {
                    // chDB's own fatal-signal handler (which the worker keeps, §49
                    // §4.1) fails the running statement with `236 ABORTED` "The
                    // server is shutting down due to a fatal error" and then the
                    // process dies. That is a crash of the worker, and a client is
                    // told so the way §49 §11 says: `210`, retryable.
                    worker.dead = true;
                    let reason = worker.shared.kill(ExitReason::Crash);
                    return Err(reason.client_error(&self.query_id, Some(&error.message)));
                }
                Err(HouseError::from(error))
            }
            Ok(Some(other)) => Err(self.broke(other.kind())),
            Ok(None) | Err(_) => Err(self.died()),
        }
    }

    /// ClickHouse's class of `sql`, from the worker's parser (HS1 Task 4, FL2
    /// Ruling 6). Nothing runs.
    pub async fn classify(&mut self, sql: &str) -> Result<Classification, HouseError> {
        let worker = self.worker()?;
        if worker.in_flight {
            return Err(network(
                "the worker is still running a statement".to_string(),
            ));
        }
        match FrameCodec::write_async(&mut worker.writer, &Frame::Classify(sql.to_string())).await {
            Ok(()) => {}
            Err(CodecError::Io(_)) => return Err(self.died()),
            Err(err) => return Err(unsendable(&self.query_id.clone(), &err)),
        }
        let worker = self.worker()?;
        match FrameCodec::read_async(&mut worker.reader).await {
            Ok(Some(Frame::Classified(classification))) => Ok(classification),
            Ok(Some(Frame::Error { error, .. })) => Err(HouseError::from(error)),
            Ok(Some(other)) => Err(self.broke(other.kind())),
            Ok(None) | Err(_) => Err(self.died()),
        }
    }

    /// Runs a statement to its end and collects its output, at most
    /// [`RUN_MAX_BYTES`]: for tests and small internal statements. Client results
    /// stream through [`WorkerLease::next_event`] instead (R2.11, M6).
    pub async fn run(&mut self, execute: Execute) -> Result<Collected, HouseError> {
        self.run_capped(execute, RUN_MAX_BYTES).await
    }

    /// [`WorkerLease::run`] with its own cap. Past `max_bytes` the statement is
    /// cancelled — the worker is killed, as for any statement abandoned mid-way —
    /// and the answer is `36 BAD_ARGUMENTS` naming the cap.
    pub async fn run_capped(
        &mut self,
        execute: Execute,
        max_bytes: usize,
    ) -> Result<Collected, HouseError> {
        let query_id = execute.query_id.clone();
        self.start(execute).await?;
        let mut out = Collected::default();
        loop {
            match self.next_event().await? {
                Event::Chunk(chunk) => {
                    if out.bytes.len() + chunk.bytes.len() > max_bytes {
                        if let Some(worker) = self.worker.as_mut() {
                            worker.shared.kill(ExitReason::Cancel);
                            worker.dead = true;
                            worker.in_flight = false;
                        }
                        return Err(HouseError::from(ChError::bad_arguments(format!(
                            "query {query_id}: the result is larger than the {max_bytes} \
                             bytes WorkerLease::run collects; stream it with next_event"
                        ))));
                    }
                    out.chunks += 1;
                    out.bytes.extend_from_slice(&chunk.bytes);
                }
                Event::Progress(_) => {}
                Event::Done(stats) => {
                    out.stats = stats;
                    return Ok(out);
                }
            }
        }
    }

    /// Test-only: sends `Frame::Abort`, which makes the worker `abort()`.
    #[cfg(feature = "test-hooks")]
    pub async fn abort_for_test(&mut self) -> Result<(), HouseError> {
        let worker = self.worker()?;
        FrameCodec::write_async(&mut worker.writer, &Frame::Abort)
            .await
            .map_err(|err| network(format!("Abort: {err}")))
    }

    /// The worker's socket ended: the client's answer, by why it died.
    fn died(&mut self) -> HouseError {
        let query_id = self.query_id.clone();
        let Some(worker) = self.worker.as_mut() else {
            return network("this lease has no worker".to_string());
        };
        worker.dead = true;
        worker.in_flight = false;
        let reason = worker.shared.record(ExitReason::Crash);
        reason.client_error(&query_id, worker.shared.exit().as_deref())
    }

    /// The worker broke the protocol: it is poisoned and killed.
    fn broke(&mut self, kind: &str) -> HouseError {
        if let Some(worker) = self.worker.as_mut() {
            worker.shared.kill(ExitReason::Poisoned);
            worker.dead = true;
            worker.in_flight = false;
        }
        network(format!(
            "query {}: the worker sent {kind} out of turn",
            self.query_id
        ))
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        // An abandoned lease may be mid-statement: never reuse it.
        match self.pool.upgrade() {
            Some(pool) => {
                pool.state().leased.remove(worker.shared.id());
                pool.retire(worker, ExitReason::Cancel);
                pool.top_up();
            }
            None => {
                worker.shared.kill(ExitReason::Cancel);
            }
        }
    }
}
