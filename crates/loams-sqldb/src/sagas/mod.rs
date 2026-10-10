//! The lifecycle host: suspend and resume sagas, and wake on connect
//! (design §47 §14, D733; plan SQ1 Task 5).
//!
//! [`Lifecycles`] runs one actor per branch around the sans-I/O
//! `loams_sqlrouter::machines::lifecycle::Lifecycle` machine (trace-checked
//! against `spec/tla/router/Lifecycle.tla`). The actor:
//! - stores the machine's record ([`LifecycleStore`]) before it executes
//!   anything else the same input asked for;
//! - answers [`Lifecycles::ensure_running`] (the gate's `EnsureRunning`):
//!   at once while the branch runs, otherwise after a resume, or with
//!   [`EnsureError::Resuming`] (1040 `database is resuming, retry`) after
//!   30 s;
//! - hands every admitted session a [`SessionLease`], through which it asks
//!   the session to close ([`Close`]) and learns when it ended;
//! - runs the saga steps' effects ([`suspend`], [`resume`]) as tasks and
//!   feeds their results back;
//! - sends `Idle` when the [`IdleDetector`] says the branch was idle for
//!   its `suspend_after`.
//!
//! **Crashes.** If storing a record fails (or a test's [`FaultPoint`] hook
//! says so) the actor restarts its saga: it reloads the record, rebuilds
//! the machine with the connections the gate still holds and its open
//! sessions, and repeats the step in progress; results of the old run are
//! ignored. Every step is idempotent. Waiting connections and sessions are
//! the gate's and survive (the gate and this host share a process in SQ1;
//! a gate pool behind `loams.internal.v1` keeps them on its side).
//!
//! The sagas' steps carry deterministic keys (`Record::step_key`), the
//! durable-execution step ids when the host runs on Resonate (Task 12).

pub mod resume;
pub mod suspend;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use loams_sqlrouter::machine::{Ctx, Machine, Millis};
use loams_sqlrouter::machines::lifecycle::{
    ConnId, Input, Lifecycle, LifecycleConfig, Output, Record, State,
};
use loams_sqlrouter::trace::{SpecEvent, VecSink};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::idle::IdleDetector;
use crate::model::BranchId;
use crate::runtime::{Member, SqlRuntime};

/// A lifecycle record could not be stored or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("lifecycle store: {0}")]
pub struct StoreError(pub String);

/// Where branches' lifecycle records live (Task 8 stores them with the
/// branch records in the metastore).
#[async_trait]
pub trait LifecycleStore: Send + Sync + fmt::Debug {
    /// The stored record, if any.
    async fn load(&self, branch: &BranchId) -> Result<Option<Record>, StoreError>;
    /// Stores `record`, replacing the last one.
    async fn save(&self, branch: &BranchId, record: &Record) -> Result<(), StoreError>;
}

/// An in-memory [`LifecycleStore`] (tests, and `loams dev` until Task 8).
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<BranchId, Vec<u8>>>);

impl MemoryStore {
    fn lock(&self) -> MutexGuard<'_, HashMap<BranchId, Vec<u8>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The stored record of `branch`.
    pub fn get(&self, branch: &BranchId) -> Option<Record> {
        self.lock().get(branch).and_then(|b| Record::decode(b).ok())
    }
}

#[async_trait]
impl LifecycleStore for MemoryStore {
    async fn load(&self, branch: &BranchId) -> Result<Option<Record>, StoreError> {
        self.lock()
            .get(branch)
            .map(|b| Record::decode(b).map_err(|e| StoreError(e.to_string())))
            .transpose()
    }

    async fn save(&self, branch: &BranchId, record: &Record) -> Result<(), StoreError> {
        self.lock().insert(branch.clone(), record.encode());
        Ok(())
    }
}

/// The resume's health check: a login through the gate's connector (TLS,
/// PROXY v2) and `SELECT 1` on one member.
#[async_trait]
pub trait Prober: Send + Sync + fmt::Debug {
    /// `Ok` when `member` answered `SELECT 1`.
    async fn probe(&self, branch: &BranchId, member: &Member) -> Result<(), String>;
}

/// What the host asks of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// Keep it open (also: an earlier request was cancelled).
    Open,
    /// Close it with 1053 as soon as no command is in flight.
    WhenIdle,
    /// Close it now (1053 where the protocol allows).
    Now,
}

/// One admitted session's hold on its branch. Dropping it tells the host
/// the session ended.
pub struct SessionLease {
    conn: ConnId,
    close: watch::Receiver<Close>,
    tx: mpsc::UnboundedSender<Msg>,
}

impl SessionLease {
    /// The connection's id.
    pub fn conn(&self) -> ConnId {
        self.conn
    }

    /// The next change of what the host asks of the session. Never
    /// returns once the host is gone.
    pub async fn closing(&mut self) -> Close {
        if self.close.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        *self.close.borrow_and_update()
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Closed(self.conn));
    }
}

impl fmt::Debug for SessionLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLease")
            .field("conn", &self.conn)
            .finish_non_exhaustive()
    }
}

/// `EnsureRunning`'s answer: the ready members and the session's lease.
#[derive(Debug)]
pub struct Admission {
    /// Members that passed the probe.
    pub members: Vec<Member>,
    /// Held for the session's lifetime.
    pub lease: SessionLease,
}

/// Why `EnsureRunning` admitted nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnsureError {
    /// Not running within 30 s: 1040 `database is resuming, retry`.
    #[error("database is resuming, retry")]
    Resuming,
    /// In a state the lifecycle does not serve (creating, failed, …).
    #[error("database is unavailable")]
    Unavailable,
    /// Not registered with the host.
    #[error("unknown branch")]
    UnknownBranch,
}

/// The host's settings.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The machine's timings and resume size.
    pub lifecycle: LifecycleConfig,
    /// The default `suspend_after` (5 min); zero never suspends.
    pub suspend_after: Duration,
    /// How often the idle detector is asked.
    pub idle_check: Duration,
    /// How long one probe waits for a member's port before it fails (and
    /// is retried).
    pub probe_wait: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleConfig::new(1),
            suspend_after: Duration::from_secs(300),
            idle_check: Duration::from_secs(1),
            probe_wait: Duration::from_secs(5),
        }
    }
}

/// A place a test may crash the saga (see [`Lifecycles::set_fault`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    /// Before this record is stored: the input that led to it is lost.
    Persist(Record),
    /// Before a scale or probe runs.
    Before(Output),
    /// After its effect, before its result is delivered.
    After(Output),
}

/// Returns `true` to crash at the point.
pub type FaultHook = Arc<dyn Fn(&FaultPoint) -> bool + Send + Sync>;

/// What the actors share.
struct Deps {
    runtime: Arc<dyn SqlRuntime>,
    store: Arc<dyn LifecycleStore>,
    prober: Arc<dyn Prober>,
    config: HostConfig,
    idle: IdleDetector,
    fault: Mutex<Option<FaultHook>>,
    base: Instant,
}

impl Deps {
    fn now(&self) -> Millis {
        Millis(u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    fn instant(&self, m: Millis) -> Instant {
        self.base + Duration::from_millis(m.0)
    }

    fn fault(&self, point: &FaultPoint) -> bool {
        let hook = self
            .fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        hook.is_some_and(|h| h(point))
    }
}

enum Msg {
    Connect {
        conn: ConnId,
        arrived: Millis,
        reply: oneshot::Sender<Result<Admission, EnsureError>>,
    },
    Closed(ConnId),
    Idle,
    Done {
        generation: u64,
        input: Input,
        members: Option<Vec<Member>>,
    },
    Crash {
        generation: u64,
    },
}

struct Handle {
    tx: mpsc::UnboundedSender<Msg>,
    record: watch::Receiver<Record>,
    restarts: Arc<AtomicU32>,
    task: JoinHandle<()>,
}

struct Inner {
    deps: Arc<Deps>,
    branches: Mutex<HashMap<BranchId, Handle>>,
    next_conn: AtomicU64,
    idle_task: Mutex<Option<JoinHandle<()>>>,
}

impl Inner {
    fn branches(&self) -> MutexGuard<'_, HashMap<BranchId, Handle>> {
        self.branches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        for h in self.branches().values() {
            h.task.abort();
        }
        if let Some(t) = self
            .idle_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            t.abort();
        }
    }
}

/// The lifecycle host; see the module docs. Cloning shares it.
#[derive(Clone)]
pub struct Lifecycles {
    inner: Arc<Inner>,
}

impl fmt::Debug for Lifecycles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycles").finish_non_exhaustive()
    }
}

/// Sends `Closed` for a waiting connection whose caller gave up.
struct WaitGuard {
    conn: ConnId,
    tx: mpsc::UnboundedSender<Msg>,
    armed: bool,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.tx.send(Msg::Closed(self.conn));
        }
    }
}

impl Lifecycles {
    /// A host over `runtime`, storing records in `store` and probing
    /// resumed pools with `prober`. Needs a tokio runtime.
    pub fn new(
        runtime: Arc<dyn SqlRuntime>,
        store: Arc<dyn LifecycleStore>,
        prober: Arc<dyn Prober>,
        config: HostConfig,
    ) -> Self {
        let deps = Arc::new(Deps {
            runtime,
            store,
            prober,
            idle: IdleDetector::new(config.suspend_after),
            config,
            fault: Mutex::new(None),
            base: Instant::now(),
        });
        let inner = Arc::new(Inner {
            deps,
            branches: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            idle_task: Mutex::new(None),
        });
        let task = tokio::spawn(idle_loop(Arc::downgrade(&inner)));
        *inner
            .idle_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
        Self { inner }
    }

    /// Starts managing `branch`: its stored record, or a new record in
    /// `state` (Task 9's `CreateDatabase` registers `RUNNING` after the
    /// bootstrap). Registering twice keeps the first.
    pub async fn register(&self, branch: &BranchId, state: State) -> Result<(), StoreError> {
        if self.inner.branches().contains_key(branch) {
            return Ok(());
        }
        let deps = self.inner.deps.clone();
        let (record, recovered) = match deps.store.load(branch).await? {
            Some(r) => (r, true),
            None => {
                let r = Record::new(state);
                deps.store.save(branch, &r).await?;
                (r, false)
            }
        };
        let mut branches = self.inner.branches();
        if branches.contains_key(branch) {
            return Ok(());
        }
        deps.idle.activity(branch, Instant::now());
        let (tx, rx) = mpsc::unbounded_channel();
        let (record_tx, record_rx) = watch::channel(record);
        let restarts = Arc::new(AtomicU32::new(0));
        let config = deps.config.lifecycle.clone();
        let machine = if recovered {
            Lifecycle::recover(config, record, [], [])
        } else {
            Lifecycle::new(config, record)
        };
        let actor = Actor {
            deps,
            branch: branch.clone(),
            tx: tx.clone(),
            machine,
            generation: 0,
            waiters: BTreeMap::new(),
            leases: BTreeMap::new(),
            members: Vec::new(),
            record: record_tx,
            restarts: restarts.clone(),
        };
        let task = tokio::spawn(actor.run(rx));
        branches.insert(
            branch.clone(),
            Handle {
                tx,
                record: record_rx,
                restarts,
                task,
            },
        );
        Ok(())
    }

    /// `EnsureRunning`: admits one session of `branch` once it runs,
    /// waking it if suspended; refused with [`EnsureError::Resuming`] after
    /// the hold timeout (30 s). Dropping the future gives up the wait.
    pub async fn ensure_running(&self, branch: &BranchId) -> Result<Admission, EnsureError> {
        let tx = self
            .inner
            .branches()
            .get(branch)
            .map(|h| h.tx.clone())
            .ok_or(EnsureError::UnknownBranch)?;
        let conn = ConnId(self.inner.next_conn.fetch_add(1, Ordering::Relaxed));
        let (reply, answer) = oneshot::channel();
        tx.send(Msg::Connect {
            conn,
            arrived: self.inner.deps.now(),
            reply,
        })
        .map_err(|_| EnsureError::Unavailable)?;
        let mut guard = WaitGuard {
            conn,
            tx,
            armed: true,
        };
        // The machine refuses at the hold timeout; this is a backstop.
        let backstop = Duration::from_millis(self.inner.deps.config.lifecycle.hold_timeout)
            + Duration::from_secs(5);
        match tokio::time::timeout(backstop, answer).await {
            Ok(Ok(result)) => {
                guard.armed = false;
                result
            }
            Ok(Err(_)) => Err(EnsureError::Unavailable),
            Err(_) => Err(EnsureError::Resuming),
        }
    }

    /// A command (not `COM_PING`) ran on `branch` (`ReportActivity`).
    pub fn activity(&self, branch: &BranchId) {
        self.inner.deps.idle.activity(branch, Instant::now());
    }

    /// `branch`'s `suspend_after`; zero never suspends it.
    pub fn set_suspend_after(&self, branch: &BranchId, after: Duration) {
        self.inner
            .deps
            .idle
            .set_suspend_after(branch, after, Instant::now());
    }

    /// Suspends `branch` now if it runs (`SuspendDatabase`, and tests).
    pub fn suspend_now(&self, branch: &BranchId) {
        if let Some(h) = self.inner.branches().get(branch) {
            let _ = h.tx.send(Msg::Idle);
        }
    }

    /// `branch`'s current record.
    pub fn record(&self, branch: &BranchId) -> Option<Record> {
        self.inner
            .branches()
            .get(branch)
            .map(|h| *h.record.borrow())
    }

    /// Waits until `branch`'s record satisfies `pred` (returns at once for
    /// an unknown branch).
    pub async fn wait_for(&self, branch: &BranchId, pred: impl FnMut(&Record) -> bool) {
        let rx = self.inner.branches().get(branch).map(|h| h.record.clone());
        if let Some(mut rx) = rx {
            let _ = rx.wait_for(pred).await;
        }
    }

    /// How often `branch`'s saga restarted after a crash.
    pub fn restarts(&self, branch: &BranchId) -> u32 {
        self.inner
            .branches()
            .get(branch)
            .map_or(0, |h| h.restarts.load(Ordering::SeqCst))
    }

    /// Tests: crash the saga wherever `hook` returns `true`.
    #[doc(hidden)]
    pub fn set_fault(&self, hook: FaultHook) {
        *self
            .inner
            .deps
            .fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }
}

async fn idle_loop(inner: Weak<Inner>) {
    let every = match inner.upgrade() {
        Some(i) => i.deps.config.idle_check,
        None => return,
    };
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(inner) = inner.upgrade() else { return };
        for branch in inner.deps.idle.due(Instant::now()) {
            if let Some(h) = inner.branches().get(&branch) {
                let _ = h.tx.send(Msg::Idle);
            }
        }
    }
}

/// One branch's actor.
struct Actor {
    deps: Arc<Deps>,
    branch: BranchId,
    tx: mpsc::UnboundedSender<Msg>,
    machine: Lifecycle,
    /// Bumped by every restart; results of older runs are ignored.
    generation: u64,
    waiters: BTreeMap<ConnId, (Millis, oneshot::Sender<Result<Admission, EnsureError>>)>,
    leases: BTreeMap<ConnId, watch::Sender<Close>>,
    /// The members the last probe passed.
    members: Vec<Member>,
    record: watch::Sender<Record>,
    restarts: Arc<AtomicU32>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        if self.machine.record().state == State::Running {
            self.members = resume::ready_members(&*self.deps.runtime, &self.branch).await;
        }
        self.feed(Input::Start).await;
        let far = Duration::from_secs(86_400 * 365);
        loop {
            let deadline = self
                .machine
                .next_deadline()
                .map_or_else(|| Instant::now() + far, |m| self.deps.instant(m));
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(m) => self.handle(m).await,
                    None => return,
                },
                () = tokio::time::sleep_until(deadline) => self.feed(Input::Tick).await,
            }
        }
    }

    async fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Connect {
                conn,
                arrived,
                reply,
            } => {
                self.waiters.insert(conn, (arrived, reply));
                self.feed(Input::Connect(conn)).await;
            }
            Msg::Closed(conn) => {
                let known =
                    self.leases.remove(&conn).is_some() || self.waiters.remove(&conn).is_some();
                if known {
                    self.feed(Input::Closed(conn)).await;
                }
            }
            Msg::Idle => self.feed(Input::Idle).await,
            Msg::Done {
                generation,
                input,
                members,
            } => {
                if generation == self.generation {
                    if let Some(m) = members {
                        self.members = m;
                    }
                    self.feed(input).await;
                }
            }
            Msg::Crash { generation } => {
                if generation == self.generation {
                    self.restart().await;
                }
            }
        }
    }

    /// Delivers `input`; restarts the saga if its record cannot be stored.
    async fn feed(&mut self, input: Input) {
        if !self.step(input).await {
            self.restart().await;
        }
    }

    /// One input: store the record first, then execute the rest. `false`
    /// when the record was not stored (a crash).
    async fn step(&mut self, input: Input) -> bool {
        let mut sink = VecSink::default();
        let outputs = {
            let now = self.deps.now();
            let mut rng = SplitMix(now.0);
            let mut ctx = Ctx {
                now,
                rng: &mut rng,
                trace: &mut sink,
            };
            self.machine.on(&mut ctx, input)
        };
        let mut outputs = outputs.into_iter().peekable();
        if let Some(&Output::Persist(record)) = outputs.peek() {
            outputs.next();
            if self.deps.fault(&FaultPoint::Persist(record)) {
                return false;
            }
            if let Err(e) = self.deps.store.save(&self.branch, &record).await {
                tracing::warn!(branch = %self.branch, error = %e, "lifecycle record not stored; restarting the saga");
                return false;
            }
            let was = self.record.send_replace(record);
            if record.state == State::Running && was.state != State::Running {
                // A branch that starts running gets a full suspend_after.
                self.deps.idle.activity(&self.branch, Instant::now());
            }
        }
        emit(&self.branch, &sink.0);
        for o in outputs {
            self.execute(o);
        }
        true
    }

    /// Rebuilds the machine from the stored record and the gate's
    /// connections, then repeats the step in progress.
    async fn restart(&mut self) {
        loop {
            self.generation += 1;
            self.restarts.fetch_add(1, Ordering::SeqCst);
            let stored = loop {
                match self.deps.store.load(&self.branch).await {
                    Ok(Some(r)) => break r,
                    // The store lost the record: write back the last one
                    // this actor stored, so the store and the machine agree.
                    Ok(None) => {
                        let last = *self.record.borrow();
                        match self.deps.store.save(&self.branch, &last).await {
                            Ok(()) => break last,
                            Err(e) => {
                                tracing::warn!(branch = %self.branch, error = %e, "lifecycle record missing and not stored; retrying");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(branch = %self.branch, error = %e, "lifecycle record unreadable; retrying");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            };
            self.record.send_replace(stored);
            self.machine = Lifecycle::recover(
                self.deps.config.lifecycle.clone(),
                stored,
                self.waiters.iter().map(|(c, (at, _))| (*c, *at)),
                self.leases.keys().copied(),
            );
            if self.step(Input::Start).await {
                return;
            }
        }
    }

    fn execute(&mut self, o: Output) {
        match o {
            Output::Persist(_) => {}
            Output::Admit(c) => {
                if let Some((_, reply)) = self.waiters.remove(&c) {
                    let (close, rx) = watch::channel(Close::Open);
                    self.leases.insert(c, close);
                    let lease = SessionLease {
                        conn: c,
                        close: rx,
                        tx: self.tx.clone(),
                    };
                    // A caller that gave up drops the admission, and with
                    // it the lease: the session ends at once.
                    let _ = reply.send(Ok(Admission {
                        members: self.members.clone(),
                        lease,
                    }));
                }
            }
            Output::Refuse(c) => {
                if let Some((_, reply)) = self.waiters.remove(&c) {
                    let e = match self.machine.record().state {
                        State::Suspending | State::Suspended | State::Resuming => {
                            EnsureError::Resuming
                        }
                        _ => EnsureError::Unavailable,
                    };
                    let _ = reply.send(Err(e));
                }
            }
            Output::CloseIdle => suspend::close_sessions(&self.leases, Close::WhenIdle),
            Output::CloseAll => suspend::close_sessions(&self.leases, Close::Now),
            Output::CancelClose => suspend::close_sessions(&self.leases, Close::Open),
            Output::Scale(_) | Output::Probe => self.spawn(o),
        }
    }

    /// Runs a scale or probe as a task; its result comes back as `Done`.
    fn spawn(&self, o: Output) {
        let deps = self.deps.clone();
        let tx = self.tx.clone();
        let branch = self.branch.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            if deps.fault(&FaultPoint::Before(o)) {
                let _ = tx.send(Msg::Crash { generation });
                return;
            }
            let (input, members) = match o {
                Output::Scale(0) => (suspend::scale_down(&*deps.runtime, &branch).await, None),
                Output::Scale(n) => (resume::scale_up(&*deps.runtime, &branch, n).await, None),
                _ => {
                    let (input, members) = resume::probe(
                        &*deps.runtime,
                        &*deps.prober,
                        &branch,
                        deps.config.probe_wait,
                    )
                    .await;
                    let ok = input == Input::Probed { ok: true };
                    (input, ok.then_some(members))
                }
            };
            if deps.fault(&FaultPoint::After(o)) {
                let _ = tx.send(Msg::Crash { generation });
                return;
            }
            let _ = tx.send(Msg::Done {
                generation,
                input,
                members,
            });
        });
    }
}

/// Spec events go to `tracing` target `loams::spec` as JSON lines (§31
/// §11.3, D311).
fn emit(branch: &BranchId, events: &[SpecEvent]) {
    for e in events {
        if let Ok(json) = serde_json::to_string(e) {
            tracing::trace!(target: "loams::spec", branch = %branch, event = %json);
        }
    }
}

/// `Ctx::rng` for the machine, which draws no randomness: a tiny
/// deterministic generator (SplitMix64).
struct SplitMix(u64);

impl rand_core::TryRng for SplitMix {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok((self.try_next_u64()? >> 32) as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Ok(z ^ (z >> 31))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in dst.chunks_mut(8) {
            let v = self.try_next_u64()?.to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        Ok(())
    }
}
