//! The pipeline: drives a `SplitEnumerator` and its `Source`s into a
//! `FabricWriter`, and a `FabricReader` into a `Sink` (CN2 Task 4).
//!
//! **Source side.** The coordinator task runs the enumerator (at start, on a
//! timer, and on [`PipelineHandle::rediscover`]) and keeps one task per split,
//! adding and removing split tasks without a restart. A split task waits for a
//! [`BudgetLease`] from the instance's [`BudgetPool`], asks the rate limiter,
//! calls `Source::next` with the lease's budget, and hands the batch to its
//! appender task; the appender calls `FabricWriter::append` and reports the
//! ack. On the ack the split task saves the checkpoint (fenced by the lease
//! epoch) and then calls `Source::committed`. Leases are held until the ack,
//! so a slow Fabric stops reads instead of buffering them.
//!
//! **Sink side.** One reader task polls the `FabricReader` under the same
//! budget rules and hands batches to the sink task; the sink task writes and
//! flushes (or writes, prepares and commits a transaction) and reports the
//! batch done; the reader task then commits the Fabric offset.
//!
//! Every operation that fails is handled by its [`ErrorClass`]: retried with
//! backoff, sent to the DLQ, paused, or the instance stops.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Notify, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;

use crate::budget::{BudgetLease, BudgetPool, BudgetStats};
use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::delivery::{DeadLetter, DeadLetterQueue};
use crate::envelope::CloudEvent;
use crate::error::{CheckpointError, ConnectorError, ErrorClass};
use crate::fabric::{FabricReader, FabricWriter};
use crate::observe::{ConnectorObserver, NoopObserver};
use crate::position::{InstanceKey, Position, SplitId};
use crate::ratelimit::{RateLimit, RateLimiter};
use crate::retry::{RetryDecision, RetryPolicy};
use crate::secrets::Scrubber;
use crate::sink::{Sink, SinkHandle};
use crate::source::{EventBatch, Source, SourceBatch};
use crate::split::{SinkFactory, SourceFactory, Split, SplitEnumerator, SplitState};

/// The split id under which a sink's Fabric position is checkpointed in a
/// transactional sink's [`Checkpoint`].
pub const FABRIC_SPLIT: &str = "fabric";

/// The hint a `PositionLost` stop carries.
const RESNAPSHOT_HINT: &str = "the upstream position no longer exists (purged, trimmed or dropped): \
     re-snapshot the instance (reset its checkpoint and start from a fresh snapshot)";

// ------------------------------------------------------------------ config --

/// Pipeline defaults; per-instance overrides in config `pipeline.*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineConfig {
    /// 65 536 (D356).
    pub max_batch_rows: usize,
    /// 8 MiB.
    pub max_batch_bytes: usize,
    /// 200 ms: how long an idle reader waits before asking again.
    pub linger: Duration,
    /// 4.
    pub max_in_flight_batches: usize,
    /// 64 MiB per instance.
    pub memory_budget_bytes: usize,
    pub retry: RetryPolicy,
    /// 5 s, or every acknowledged batch for transactional sinks.
    pub checkpoint_interval: Duration,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_batch_rows: 65_536,
            max_batch_bytes: 8 << 20,
            linger: Duration::from_millis(200),
            max_in_flight_batches: 4,
            memory_budget_bytes: 64 << 20,
            retry: RetryPolicy::default(),
            checkpoint_interval: Duration::from_secs(5),
        }
    }
}

impl PipelineConfig {
    /// Apply an instance config's `pipeline` object: `max_batch_rows`,
    /// `max_batch_bytes`, `linger_ms`, `max_in_flight_batches`,
    /// `memory_budget_bytes`, `checkpoint_interval_ms`, and `retry` with
    /// `initial_ms`, `max_ms`, `max_attempts` and `honour_retry_after`.
    pub fn with_overrides(mut self, config: &serde_json::Value) -> Result<Self, ConnectorError> {
        let Some(p) = config.get("pipeline") else {
            return Ok(self);
        };
        let p = p
            .as_object()
            .ok_or_else(|| ConnectorError::fatal("config.pipeline must be an object"))?;
        let num = |v: &serde_json::Value, k: &str| {
            v.as_u64()
                .filter(|n| *n > 0 || k.ends_with("_ms"))
                .ok_or_else(|| {
                    ConnectorError::fatal(format!("config.pipeline.{k} must be a positive integer"))
                })
        };
        for (k, v) in p {
            match k.as_str() {
                "max_batch_rows" => self.max_batch_rows = num(v, k)? as usize,
                "max_batch_bytes" => self.max_batch_bytes = num(v, k)? as usize,
                "linger_ms" => self.linger = Duration::from_millis(num(v, k)?),
                "max_in_flight_batches" => self.max_in_flight_batches = num(v, k)? as usize,
                "memory_budget_bytes" => self.memory_budget_bytes = num(v, k)? as usize,
                "checkpoint_interval_ms" => {
                    self.checkpoint_interval = Duration::from_millis(num(v, k)?)
                }
                "retry" => {
                    let r = v.as_object().ok_or_else(|| {
                        ConnectorError::fatal("config.pipeline.retry must be an object")
                    })?;
                    for (rk, rv) in r {
                        match rk.as_str() {
                            "initial_ms" => self.retry.initial = Duration::from_millis(num(rv, rk)?),
                            "max_ms" => self.retry.max = Duration::from_millis(num(rv, rk)?),
                            "max_attempts" => self.retry.max_attempts = num(rv, rk)? as u32,
                            "honour_retry_after" => {
                                self.retry.honour_retry_after = rv.as_bool().ok_or_else(|| {
                                    ConnectorError::fatal(
                                        "config.pipeline.retry.honour_retry_after must be a boolean",
                                    )
                                })?
                            }
                            other => {
                                return Err(ConnectorError::fatal(format!(
                                    "unknown setting config.pipeline.retry.{other}"
                                )));
                            }
                        }
                    }
                }
                other => {
                    return Err(ConnectorError::fatal(format!(
                        "unknown setting config.pipeline.{other}"
                    )));
                }
            }
        }
        Ok(self)
    }

    fn pool(&self) -> Arc<BudgetPool> {
        BudgetPool::new(
            self.max_batch_rows,
            self.max_batch_bytes,
            self.max_in_flight_batches,
            self.memory_budget_bytes,
        )
    }
}

// ------------------------------------------------------------------ status --

/// The lifecycle state of a running pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Running,
    /// `SchemaIncompatible`, or an operator's [`PipelineHandle::pause`]:
    /// reading stopped until [`PipelineHandle::resume`].
    Paused,
    /// Stopped on request.
    Stopped,
    /// Stopped by an `Auth`, `Fatal` or `PositionLost` error, or by retries
    /// running out; see `last_error`.
    Failed,
    /// A newer lease holder saved a checkpoint: this pipeline is stale and
    /// stopped without committing upstream again.
    Fenced,
}

/// The error an instance stopped or paused with, scrubbed of secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastError {
    pub class: ErrorClass,
    pub message: String,
    /// What an operator should do (`PositionLost`: re-snapshot).
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineStatus {
    pub state: PipelineState,
    pub last_error: Option<LastError>,
    /// The splits currently running (a source pipeline).
    pub splits: Vec<SplitId>,
}

// ------------------------------------------------------------------ shared --

/// What to do after an error.
enum Action {
    Retry(Duration),
    DeadLetter,
    Paused,
    Stop,
}

/// The outcome of an operation run under the retry rules.
enum Step<T> {
    Done(T),
    /// The error was `Poison`: the caller dead-letters what it holds.
    Poison(ConnectorError),
    /// The instance is stopping (on request, or because of this error).
    Halt,
}

/// Run `$op` (an expression producing a future of `Result<T,
/// ConnectorError>`, re-evaluated per attempt) under `$shared`'s retry rules.
macro_rules! retrying {
    ($shared:expr, $op:expr) => {{
        let shared: &Shared = &$shared;
        let mut failures = 0u32;
        loop {
            match $op.await {
                Ok(v) => break Step::Done(v),
                Err(e) => {
                    failures += 1;
                    match shared.decide(&e, failures) {
                        Action::Retry(d) => {
                            if !shared.sleep(d).await {
                                break Step::Halt;
                            }
                        }
                        Action::DeadLetter => break Step::Poison(e),
                        Action::Paused => {
                            if !shared.wait_resumed().await {
                                break Step::Halt;
                            }
                            failures = 0;
                        }
                        Action::Stop => break Step::Halt,
                    }
                }
            }
        }
    }};
}

struct Shared {
    instance: InstanceKey,
    config: PipelineConfig,
    epoch: u64,
    pool: Arc<BudgetPool>,
    limiter: RateLimiter,
    observer: Arc<dyn ConnectorObserver>,
    scrubber: Arc<Scrubber>,
    dlq: Option<Arc<dyn DeadLetterQueue>>,
    status: watch::Sender<PipelineStatus>,
    stop: watch::Sender<bool>,
    resume: Notify,
    tasks: Mutex<Vec<AbortHandle>>,
}

impl Shared {
    fn new(c: Common) -> Arc<Self> {
        Arc::new(Self {
            pool: c.config.pool(),
            instance: c.instance,
            epoch: c.epoch,
            limiter: RateLimiter::new(c.rate_limit),
            observer: c.observer.unwrap_or_else(|| Arc::new(NoopObserver)),
            scrubber: c.scrubber.unwrap_or_default(),
            dlq: c.dlq,
            config: c.config,
            status: watch::Sender::new(PipelineStatus {
                state: if c.start_paused {
                    PipelineState::Paused
                } else {
                    PipelineState::Running
                },
                last_error: None,
                splits: Vec::new(),
            }),
            stop: watch::Sender::new(false),
            resume: Notify::new(),
            tasks: Mutex::new(Vec::new()),
        })
    }

    /// Spawn a task the handle can abort all at once.
    fn spawn<F>(&self, fut: F) -> JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let h = tokio::spawn(fut);
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.retain(|t| !t.is_finished());
        tasks.push(h.abort_handle());
        h
    }

    fn abort_all(&self) {
        for t in self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
        {
            t.abort();
        }
    }

    fn stopping(&self) -> bool {
        *self.stop.borrow()
    }

    async fn stopped(&self) {
        let mut rx = self.stop.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    fn request_stop(&self) {
        self.stop.send_replace(true);
        self.resume.notify_waiters();
    }

    /// Sleep `d`; `false` if the instance began stopping meanwhile.
    async fn sleep(&self, d: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(d) => !self.stopping(),
            _ = self.stopped() => false,
        }
    }

    fn state(&self) -> PipelineState {
        self.status.borrow().state
    }

    fn fenced(&self) -> bool {
        self.state() == PipelineState::Fenced
    }

    fn halted(&self) -> bool {
        matches!(self.state(), PipelineState::Failed | PipelineState::Fenced)
    }

    fn scrub(&self, s: &str) -> String {
        self.scrubber.scrub(s)
    }

    fn last_error(&self, class: ErrorClass, message: &str, hint: Option<&str>) -> LastError {
        let message = self.scrub(message);
        self.observer.error(&self.instance, &class, &message);
        LastError {
            class,
            message,
            hint: hint.map(str::to_owned),
        }
    }

    /// Stop the instance with `last_error` (Failed), unless it is already
    /// failed or fenced.
    fn fail(&self, class: ErrorClass, message: &str, hint: Option<&str>) {
        let le = self.last_error(class, message, hint);
        self.status.send_modify(|s| {
            if !matches!(s.state, PipelineState::Failed | PipelineState::Fenced) {
                s.state = PipelineState::Failed;
                s.last_error = Some(le);
            }
        });
        self.request_stop();
    }

    fn fence(&self, e: &CheckpointError) {
        let le = self.last_error(ErrorClass::Fatal, &e.to_string(), None);
        self.status.send_modify(|s| {
            s.state = PipelineState::Fenced;
            s.last_error = Some(le);
        });
        self.request_stop();
    }

    fn pause(&self, e: &ConnectorError) {
        let le = self.last_error(e.class.clone(), &e.message, None);
        self.status.send_modify(|s| {
            if s.state == PipelineState::Running {
                s.state = PipelineState::Paused;
                s.last_error = Some(le);
            }
        });
    }

    /// An operator's pause: no `next` or `poll` call starts until resumed;
    /// what is in flight finishes, and its acks are checkpointed on resume.
    fn pause_by_request(&self) {
        self.status.send_modify(|s| {
            if s.state == PipelineState::Running {
                s.state = PipelineState::Paused;
            }
        });
    }

    fn resume(&self) {
        self.status.send_modify(|s| {
            if s.state == PipelineState::Paused {
                s.state = PipelineState::Running;
            }
        });
        self.resume.notify_waiters();
    }

    /// Wait while paused; `false` if the instance stops instead.
    async fn wait_resumed(&self) -> bool {
        loop {
            let notified = self.resume.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.stopping() {
                return false;
            }
            if self.state() != PipelineState::Paused {
                return true;
            }
            tokio::select! {
                _ = notified => {}
                _ = self.stopped() => return false,
            }
        }
    }

    /// The error-class rules of CN2 Task 4.
    fn decide(&self, e: &ConnectorError, failures: u32) -> Action {
        match &e.class {
            class @ (ErrorClass::Retryable { .. } | ErrorClass::Throttled { .. }) => {
                match self.config.retry.decide(class, failures) {
                    RetryDecision::RetryAfter(d) => {
                        if let ErrorClass::Throttled { after } = class {
                            self.limiter.throttled(*after);
                        }
                        self.observer.retry(&self.instance, class, failures, d);
                        Action::Retry(d)
                    }
                    RetryDecision::GiveUp => {
                        self.fail(
                            class.clone(),
                            &format!("gave up after {failures} attempts: {}", e.message),
                            None,
                        );
                        Action::Stop
                    }
                }
            }
            ErrorClass::Poison => {
                if self.dlq.is_some() {
                    Action::DeadLetter
                } else {
                    self.fail(
                        ErrorClass::Poison,
                        &e.message,
                        Some("the instance has no dead-letter topic"),
                    );
                    Action::Stop
                }
            }
            ErrorClass::SchemaIncompatible => {
                self.pause(e);
                Action::Paused
            }
            ErrorClass::PositionLost => {
                self.fail(ErrorClass::PositionLost, &e.message, Some(RESNAPSHOT_HINT));
                Action::Stop
            }
            class @ (ErrorClass::Auth | ErrorClass::Fatal) => {
                self.fail(class.clone(), &e.message, None);
                Action::Stop
            }
        }
    }

    /// Send one record to the DLQ; `false` if that failed for good.
    async fn dead_letter(
        &self,
        split: Option<SplitId>,
        event: Option<CloudEvent>,
        e: &ConnectorError,
    ) -> bool {
        let Some(dlq) = self.dlq.clone() else {
            return false;
        };
        let dead = DeadLetter {
            instance: self.instance.clone(),
            split,
            event,
            class: e.class.clone(),
            error: self.scrub(&e.message),
        };
        match retrying!(self, async {
            dlq.send(dead.clone()).await.map_err(ConnectorError::from)
        }) {
            Step::Done(()) => {
                self.observer.dead_letter(&self.instance);
                true
            }
            Step::Poison(e) => {
                self.fail(
                    ErrorClass::Fatal,
                    &format!("the DLQ refused a record: {}", e.message),
                    None,
                );
                false
            }
            Step::Halt => false,
        }
    }

    /// Mark a stopped pipeline's final state.
    fn finish(&self) {
        self.status.send_modify(|s| {
            if matches!(s.state, PipelineState::Running | PipelineState::Paused) {
                s.state = PipelineState::Stopped;
            }
            s.splits.clear();
        });
    }
}

/// A task handle that aborts the task when dropped (a dropped pipeline
/// behaves as a crash).
struct AbortOnDrop(Option<JoinHandle<()>>);

impl AbortOnDrop {
    async fn join(mut self) {
        if let Some(h) = self.0.take() {
            let _ = h.await;
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(h) = &self.0 {
            h.abort();
        }
    }
}

// ---------------------------------------------------------------- builders --

/// The entry point: `Pipeline::source(..)` and `Pipeline::sink(..)` build the
/// two kinds of pipeline.
#[derive(Debug, Clone, Copy)]
pub struct Pipeline;

impl Pipeline {
    pub fn source(instance: InstanceKey, config: PipelineConfig) -> SourcePipeline {
        SourcePipeline {
            common: Common::new(instance, config),
            enumerator: None,
            factory: None,
            writer: None,
            checkpoints: None,
            discover_every: Duration::from_secs(30),
        }
    }

    pub fn sink(instance: InstanceKey, config: PipelineConfig) -> SinkPipeline {
        SinkPipeline {
            common: Common::new(instance, config),
            reader: None,
            factory: None,
        }
    }
}

struct Common {
    instance: InstanceKey,
    config: PipelineConfig,
    epoch: u64,
    observer: Option<Arc<dyn ConnectorObserver>>,
    scrubber: Option<Arc<Scrubber>>,
    dlq: Option<Arc<dyn DeadLetterQueue>>,
    rate_limit: RateLimit,
    start_paused: bool,
}

impl Common {
    fn new(instance: InstanceKey, config: PipelineConfig) -> Self {
        Self {
            instance,
            config,
            epoch: 0,
            observer: None,
            scrubber: None,
            dlq: None,
            rate_limit: RateLimit::default(),
            start_paused: false,
        }
    }
}

macro_rules! common_builders {
    () => {
        /// The lease epoch this pipeline's checkpoints are written under.
        pub fn epoch(mut self, epoch: u64) -> Self {
            self.common.epoch = epoch;
            self
        }

        pub fn observer(mut self, o: Arc<dyn ConnectorObserver>) -> Self {
            self.common.observer = Some(o);
            self
        }

        /// The scrubber the instance's `ConnectorContext` records resolved
        /// secrets in.
        pub fn scrubber(mut self, s: Arc<Scrubber>) -> Self {
            self.common.scrubber = Some(s);
            self
        }

        /// The instance's DLQ topic. Without one, a `Poison` error stops the
        /// instance.
        pub fn dead_letters(mut self, d: Arc<dyn DeadLetterQueue>) -> Self {
            self.common.dlq = Some(d);
            self
        }

        pub fn rate_limit(mut self, r: RateLimit) -> Self {
            self.common.rate_limit = r;
            self
        }

        /// Start paused (an instance an operator paused before a restart):
        /// nothing is read until [`PipelineHandle::resume`]. A transactional
        /// sink still recovers first.
        pub fn start_paused(mut self, paused: bool) -> Self {
            self.common.start_paused = paused;
            self
        }
    };
}

/// A source pipeline under construction.
pub struct SourcePipeline {
    common: Common,
    enumerator: Option<Box<dyn SplitEnumerator>>,
    factory: Option<Arc<dyn SourceFactory>>,
    writer: Option<Arc<dyn FabricWriter>>,
    checkpoints: Option<Arc<dyn CheckpointStore>>,
    discover_every: Duration,
}

impl std::fmt::Debug for SourcePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourcePipeline")
            .field("instance", &self.common.instance)
            .finish_non_exhaustive()
    }
}

impl SourcePipeline {
    common_builders!();

    pub fn enumerator(mut self, e: Box<dyn SplitEnumerator>) -> Self {
        self.enumerator = Some(e);
        self
    }

    pub fn sources(mut self, f: Arc<dyn SourceFactory>) -> Self {
        self.factory = Some(f);
        self
    }

    pub fn writer(mut self, w: Arc<dyn FabricWriter>) -> Self {
        self.writer = Some(w);
        self
    }

    pub fn checkpoints(mut self, c: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoints = Some(c);
        self
    }

    /// How often the enumerator re-runs (default 30 s); it also runs on
    /// [`PipelineHandle::rediscover`].
    pub fn discover_every(mut self, d: Duration) -> Self {
        self.discover_every = d;
        self
    }

    /// Load the checkpoint (refusing a stale epoch), then discover the splits
    /// and start reading.
    pub async fn start(self) -> Result<PipelineHandle, ConnectorError> {
        let enumerator = self
            .enumerator
            .ok_or_else(|| ConnectorError::fatal("source pipeline without a split enumerator"))?;
        let factory = self
            .factory
            .ok_or_else(|| ConnectorError::fatal("source pipeline without a source factory"))?;
        let writer = self
            .writer
            .ok_or_else(|| ConnectorError::fatal("source pipeline without a Fabric writer"))?;
        let store = self
            .checkpoints
            .ok_or_else(|| ConnectorError::fatal("source pipeline without a checkpoint store"))?;
        let shared = Shared::new(self.common);
        let loaded = store.load(&shared.instance).await?;
        if let Some(cp) = &loaded
            && cp.epoch > shared.epoch
        {
            return Err(ConnectorError::fatal(
                CheckpointError::Fenced {
                    stored: cp.epoch,
                    attempted: shared.epoch,
                }
                .to_string(),
            ));
        }
        let checkpointer = Arc::new(Checkpointer {
            store,
            instance: shared.instance.clone(),
            epoch: shared.epoch,
            interval: shared.config.checkpoint_interval,
            state: tokio::sync::Mutex::new(CpState {
                splits: loaded.map(|cp| cp.splits).unwrap_or_default(),
                dirty: false,
                last_save: None,
            }),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = SourceCtx {
            shared: shared.clone(),
            factory,
            writer,
            checkpointer,
        };
        let task = shared.spawn(coordinator(ctx, enumerator, rx, self.discover_every));
        Ok(PipelineHandle::new(shared, task, Some(tx)))
    }
}

/// A sink pipeline under construction.
pub struct SinkPipeline {
    common: Common,
    reader: Option<Box<dyn FabricReader>>,
    factory: Option<Arc<dyn SinkFactory>>,
}

impl std::fmt::Debug for SinkPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkPipeline")
            .field("instance", &self.common.instance)
            .finish_non_exhaustive()
    }
}

impl SinkPipeline {
    common_builders!();

    pub fn reader(mut self, r: Box<dyn FabricReader>) -> Self {
        self.reader = Some(r);
        self
    }

    pub fn sink(mut self, f: Arc<dyn SinkFactory>) -> Self {
        self.factory = Some(f);
        self
    }

    /// Create the sink, recover (exactly-once) and start writing.
    pub async fn start(self) -> Result<PipelineHandle, ConnectorError> {
        let reader = self
            .reader
            .ok_or_else(|| ConnectorError::fatal("sink pipeline without a Fabric reader"))?;
        let factory = self
            .factory
            .ok_or_else(|| ConnectorError::fatal("sink pipeline without a sink factory"))?;
        let sink = factory.create()?;
        let shared = Shared::new(self.common);
        let task = shared.spawn(run_sink(shared.clone(), reader, sink));
        Ok(PipelineHandle::new(shared, task, None))
    }
}

// ------------------------------------------------------------------ handle --

/// A running pipeline. Dropping it aborts the pipeline's tasks, as a crash
/// would; [`PipelineHandle::stop`] stops it gracefully.
pub struct PipelineHandle {
    shared: Arc<Shared>,
    status: watch::Receiver<PipelineStatus>,
    task: Option<AbortOnDrop>,
    rediscover: Option<mpsc::UnboundedSender<()>>,
}

impl std::fmt::Debug for PipelineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineHandle")
            .field("instance", &self.shared.instance)
            .field("status", &*self.status.borrow())
            .finish()
    }
}

impl PipelineHandle {
    fn new(
        shared: Arc<Shared>,
        task: JoinHandle<()>,
        rediscover: Option<mpsc::UnboundedSender<()>>,
    ) -> Self {
        Self {
            status: shared.status.subscribe(),
            shared,
            task: Some(AbortOnDrop(Some(task))),
            rediscover,
        }
    }

    pub fn status(&self) -> PipelineStatus {
        self.status.borrow().clone()
    }

    /// Wait until the status satisfies `pred`, and return it.
    pub async fn wait_for(&self, pred: impl FnMut(&PipelineStatus) -> bool) -> PipelineStatus {
        let mut rx = self.status.clone();
        let r = rx.wait_for(pred).await.map(|s| s.clone());
        r.unwrap_or_else(|_| self.status())
    }

    /// Run the split enumerator now (a source pipeline; no-op for a sink).
    pub fn rediscover(&self) {
        if let Some(tx) = &self.rediscover {
            let _ = tx.send(());
        }
    }

    /// Resume a pipeline paused by `SchemaIncompatible` or by
    /// [`PipelineHandle::pause`].
    pub fn resume(&self) {
        self.shared.resume();
    }

    /// Pause reading (CC1 Task 1, the connect role's admin API): a source
    /// starts no `next`, a sink no Fabric `poll`, until [`Self::resume`].
    /// Calls in flight finish. A failed or stopped pipeline is unchanged.
    pub fn pause(&self) {
        self.shared.pause_by_request();
    }

    /// Change the instance's rate limits while it runs (CC1 Task 1).
    pub fn set_rate_limit(&self, limit: RateLimit) {
        self.shared.limiter.set_limit(limit);
    }

    /// The instance's current rate limits.
    pub fn rate_limit(&self) -> RateLimit {
        self.shared.limiter.limit()
    }

    /// The budget pool's use so far.
    pub fn stats(&self) -> BudgetStats {
        self.shared.pool.stats()
    }

    /// Stop gracefully: stop reading, finish what is in flight, checkpoint,
    /// close; returns the final status.
    pub async fn stop(mut self) -> PipelineStatus {
        self.shared.request_stop();
        if let Some(t) = self.task.take() {
            t.join().await;
        }
        self.shared.finish();
        self.status()
    }

    /// Abort every task at once, as a crash would: no drain, no final
    /// checkpoint, no close.
    pub fn abort(self) {
        self.shared.abort_all();
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        self.shared.abort_all();
    }
}

// ------------------------------------------------------------- checkpoints --

struct CpState {
    splits: BTreeMap<SplitId, Position>,
    dirty: bool,
    last_save: Option<Instant>,
}

/// The instance's split positions, saved (fenced) every `interval` or on
/// every ack when `interval` is zero.
struct Checkpointer {
    store: Arc<dyn CheckpointStore>,
    instance: InstanceKey,
    epoch: u64,
    interval: Duration,
    state: tokio::sync::Mutex<CpState>,
}

impl Checkpointer {
    async fn position(&self, split: &SplitId) -> Option<Position> {
        self.state.lock().await.splits.get(split).cloned()
    }

    async fn advance(
        &self,
        shared: &Shared,
        split: &SplitId,
        pos: &Position,
    ) -> Result<(), CheckpointError> {
        let mut s = self.state.lock().await;
        s.splits.insert(
            split.clone(),
            Position {
                opaque: pos.opaque.clone(),
                display: shared.scrub(&pos.display),
            },
        );
        s.dirty = true;
        let due =
            self.interval.is_zero() || s.last_save.is_none_or(|t| t.elapsed() >= self.interval);
        if due {
            self.save(shared, &mut s).await?;
        }
        Ok(())
    }

    async fn remove(&self, shared: &Shared, split: &SplitId) -> Result<(), CheckpointError> {
        let mut s = self.state.lock().await;
        if s.splits.remove(split).is_some() {
            s.dirty = true;
            self.save(shared, &mut s).await?;
        }
        Ok(())
    }

    async fn flush(&self, shared: &Shared) -> Result<(), CheckpointError> {
        let mut s = self.state.lock().await;
        if s.dirty {
            self.save(shared, &mut s).await?;
        }
        Ok(())
    }

    async fn save(&self, shared: &Shared, s: &mut CpState) -> Result<(), CheckpointError> {
        let cp = Checkpoint {
            instance: self.instance.clone(),
            epoch: self.epoch,
            splits: s.splits.clone(),
        };
        self.store.save(&cp).await?;
        s.dirty = false;
        s.last_save = Some(Instant::now());
        shared.observer.checkpoint(&self.instance, self.epoch);
        Ok(())
    }

    /// Save or flush, handling the result: `false` if the pipeline is fenced.
    fn handle(shared: &Shared, r: Result<(), CheckpointError>) -> bool {
        match r {
            Ok(()) => true,
            Err(e @ CheckpointError::Fenced { .. }) => {
                shared.fence(&e);
                false
            }
            Err(e) => {
                // Unavailable: the positions stay dirty and the next save
                // retries; upstream commits continue (a crash meanwhile
                // re-reads more, which `ingest` dedups).
                let msg = shared.scrub(&e.to_string());
                shared.observer.error(
                    &shared.instance,
                    &ErrorClass::Retryable { after: None },
                    &msg,
                );
                tracing::warn!(instance = %shared.instance, "checkpoint save failed: {msg}");
                true
            }
        }
    }
}

// ------------------------------------------------------------------ source --

#[derive(Clone)]
struct SourceCtx {
    shared: Arc<Shared>,
    factory: Arc<dyn SourceFactory>,
    writer: Arc<dyn FabricWriter>,
    checkpointer: Arc<Checkpointer>,
}

struct SplitTask {
    split: Split,
    stop: watch::Sender<bool>,
    join: AbortOnDrop,
}

async fn coordinator(
    ctx: SourceCtx,
    mut enumerator: Box<dyn SplitEnumerator>,
    mut rediscover: mpsc::UnboundedReceiver<()>,
    discover_every: Duration,
) {
    let shared = ctx.shared.clone();
    let mut splits: BTreeMap<SplitId, SplitTask> = BTreeMap::new();
    let mut discover = tokio::time::interval(discover_every.max(Duration::from_millis(1)));
    discover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let flush_every = shared
        .config
        .checkpoint_interval
        .max(Duration::from_millis(10));
    let mut flush = tokio::time::interval(flush_every);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut discover_failures = 0u32;
    loop {
        tokio::select! {
            _ = shared.stopped() => break,
            _ = discover.tick() => {
                reconcile(&ctx, enumerator.as_mut(), &mut splits, &mut discover, &mut discover_failures).await;
            }
            Some(()) = rediscover.recv() => {
                reconcile(&ctx, enumerator.as_mut(), &mut splits, &mut discover, &mut discover_failures).await;
            }
            _ = flush.tick() => {
                let r = ctx.checkpointer.flush(&shared).await;
                Checkpointer::handle(&shared, r);
            }
        }
    }
    // Graceful: every split drains its in-flight batches and closes.
    for task in splits.values() {
        task.stop.send_replace(true);
    }
    for (_, task) in std::mem::take(&mut splits) {
        task.join.join().await;
    }
    if !shared.halted() {
        let r = ctx.checkpointer.flush(&shared).await;
        Checkpointer::handle(&shared, r);
    }
    shared.finish();
}

async fn reconcile(
    ctx: &SourceCtx,
    enumerator: &mut dyn SplitEnumerator,
    splits: &mut BTreeMap<SplitId, SplitTask>,
    discover: &mut tokio::time::Interval,
    failures: &mut u32,
) {
    let shared = &ctx.shared;
    let mut known = Vec::with_capacity(splits.len());
    for t in splits.values() {
        known.push(SplitState {
            split: t.split.clone(),
            position: ctx.checkpointer.position(&t.split.id).await,
        });
    }
    let found = match enumerator.discover(&known).await {
        Ok(found) => {
            *failures = 0;
            found
        }
        Err(e) => {
            *failures += 1;
            match shared.decide(&e, *failures) {
                Action::Retry(d) => discover.reset_after(d),
                Action::DeadLetter => {
                    shared.fail(
                        ErrorClass::Fatal,
                        &format!("split discovery: {}", e.message),
                        None,
                    );
                }
                Action::Paused | Action::Stop => {}
            }
            return;
        }
    };
    let found_ids: BTreeSet<SplitId> = found.iter().map(|s| s.id.clone()).collect();
    // Removed splits drain, close, and lose their position.
    let removed: Vec<SplitId> = splits
        .keys()
        .filter(|id| !found_ids.contains(*id))
        .cloned()
        .collect();
    for id in removed {
        if let Some(task) = splits.remove(&id) {
            task.stop.send_replace(true);
            task.join.join().await;
        }
        let r = ctx.checkpointer.remove(shared, &id).await;
        if !Checkpointer::handle(shared, r) {
            return;
        }
    }
    // New splits start from their checkpointed position.
    for split in found {
        if splits.contains_key(&split.id) || shared.stopping() {
            continue;
        }
        let pos = ctx.checkpointer.position(&split.id).await;
        let (stop_tx, stop_rx) = watch::channel(false);
        let join = shared.spawn(run_split(ctx.clone(), split.clone(), pos, stop_rx));
        splits.insert(
            split.id.clone(),
            SplitTask {
                split,
                stop: stop_tx,
                join: AbortOnDrop(Some(join)),
            },
        );
    }
    let ids: Vec<SplitId> = splits.keys().cloned().collect();
    shared.status.send_modify(|s| s.splits = ids);
}

/// A batch the Fabric acknowledged.
struct Acked {
    position: Position,
    rows: u64,
    bytes: u64,
}

/// Resolves when the split must stop: its own stop or the instance's.
async fn split_halted(shared: &Shared, stop: &mut watch::Receiver<bool>) {
    tokio::select! {
        _ = shared.stopped() => {}
        _ = stop.wait_for(|v| *v) => {}
    }
}

async fn run_split(
    ctx: SourceCtx,
    split: Split,
    pos: Option<Position>,
    mut stop: watch::Receiver<bool>,
) {
    let shared = ctx.shared.clone();
    let mut source = match ctx.factory.create(&split) {
        Ok(s) => s,
        Err(e) => {
            shared.decide(&ConnectorError::new(ErrorClass::Fatal, e.message), 1);
            return;
        }
    };
    match retrying!(shared, source.open(&split, pos.clone())) {
        Step::Done(()) => {}
        Step::Poison(e) => {
            shared.fail(
                ErrorClass::Fatal,
                &format!("opening split {}: {}", split.id, e.message),
                None,
            );
            return;
        }
        Step::Halt => return,
    }

    let (batch_tx, batch_rx) =
        mpsc::channel::<(SourceBatch, BudgetLease)>(shared.config.max_in_flight_batches.max(1));
    let (ack_tx, mut ack_rx) = mpsc::unbounded_channel::<Acked>();
    let appender = AbortOnDrop(Some(shared.spawn(append_loop(
        shared.clone(),
        ctx.writer.clone(),
        split.id.clone(),
        batch_rx,
        ack_tx,
    ))));

    let mut failures = 0u32;
    'read: loop {
        while let Ok(ack) = ack_rx.try_recv() {
            if !on_ack(&ctx, &split.id, source.as_mut(), ack).await {
                break 'read;
            }
        }
        if shared.stopping() || *stop.borrow() {
            break;
        }
        if shared.state() == PipelineState::Paused && !shared.wait_resumed().await {
            break;
        }
        let mut lease = tokio::select! {
            biased;
            _ = split_halted(&shared, &mut stop) => break,
            Some(ack) = ack_rx.recv() => {
                if !on_ack(&ctx, &split.id, source.as_mut(), ack).await {
                    break;
                }
                continue;
            }
            lease = acquire_reporting_wait(&shared) => lease,
        };
        tokio::select! {
            _ = split_halted(&shared, &mut stop) => break,
            _ = shared.limiter.acquire(0) => {}
        }
        shared.observer.api_call(&shared.instance);
        let budget = lease.budget();
        let result = tokio::select! {
            _ = split_halted(&shared, &mut stop) => break,
            r = source.next(budget) => r,
        };
        match result {
            Ok(Some(batch)) => {
                failures = 0;
                shared.limiter.succeeded();
                let rows = batch.events.rows();
                let bytes = batch.events.size_bytes();
                if !budget.admits(rows, bytes) {
                    shared.fail(
                        ErrorClass::Fatal,
                        &format!(
                            "split {}: the source returned {rows} rows and {bytes} bytes, over its budget of {} rows and {} bytes",
                            split.id, budget.max_rows, budget.max_bytes
                        ),
                        None,
                    );
                    break;
                }
                lease.shrink_to(bytes);
                // The batch's bytes against the instance's byte rate: read
                // before they could be counted, so charged now, and the next
                // read waits out any debt (CC1 Task 1).
                tokio::select! {
                    _ = split_halted(&shared, &mut stop) => {
                        // Stopping: the batch is not handed on; the source
                        // re-reads it after a restart.
                        break;
                    }
                    _ = shared.limiter.acquire_bytes(bytes as u64) => {}
                }
                if batch_tx.send((batch, lease)).await.is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(lease);
                tokio::select! {
                    _ = split_halted(&shared, &mut stop) => break,
                    _ = tokio::time::sleep(shared.config.linger) => {}
                }
            }
            Err(e) => {
                drop(lease);
                failures += 1;
                match shared.decide(&e, failures) {
                    Action::Retry(d) => {
                        tokio::select! {
                            _ = split_halted(&shared, &mut stop) => break,
                            _ = tokio::time::sleep(d) => {}
                        }
                    }
                    Action::DeadLetter => {
                        failures = 0;
                        if !shared.dead_letter(Some(split.id.clone()), None, &e).await {
                            break;
                        }
                    }
                    Action::Paused => {
                        failures = 0;
                        if !shared.wait_resumed().await {
                            break;
                        }
                    }
                    Action::Stop => break,
                }
            }
        }
    }

    // Drain: the appender finishes what is in flight, and each ack is
    // checkpointed and committed upstream (unless the instance failed).
    drop(batch_tx);
    while let Some(ack) = ack_rx.recv().await {
        if shared.fenced() {
            continue;
        }
        on_ack(&ctx, &split.id, source.as_mut(), ack).await;
    }
    appender.join().await;
    if let Err(e) = source.close().await {
        let msg = shared.scrub(&e.message);
        tracing::warn!(instance = %shared.instance, split = %split.id, "closing the split: {msg}");
    }
}

/// Reserve a source batch's budget. When none is free the source task is
/// paused by backpressure, and the time it waits is reported to the observer
/// (`loams_flow_connector_budget_wait_seconds_total`, CN2 Task 24) — also
/// when the wait is cut short by an ack or a stop.
async fn acquire_reporting_wait(shared: &Shared) -> BudgetLease {
    if let Some(lease) = shared.pool.try_acquire() {
        return lease;
    }
    struct Waiting<'a> {
        shared: &'a Shared,
        since: Instant,
    }
    impl Drop for Waiting<'_> {
        fn drop(&mut self) {
            let waited = self.since.elapsed();
            if !waited.is_zero() {
                self.shared
                    .observer
                    .budget_wait(&self.shared.instance, waited);
            }
        }
    }
    let _waiting = Waiting {
        shared,
        since: Instant::now(),
    };
    shared.pool.acquire().await
}

/// The Fabric acknowledged a batch: checkpoint (fenced), then commit
/// upstream. `false` when the split must stop.
async fn on_ack(ctx: &SourceCtx, split: &SplitId, source: &mut dyn Source, ack: Acked) -> bool {
    let shared = &ctx.shared;
    // A failed instance still checkpoints and commits what the Fabric has
    // acknowledged; a fenced one must not touch either.
    if shared.fenced() {
        return false;
    }
    shared
        .observer
        .source_batch(&shared.instance, split, ack.rows, ack.bytes);
    let r = ctx.checkpointer.advance(shared, split, &ack.position).await;
    if !Checkpointer::handle(shared, r) {
        return false;
    }
    match retrying!(shared, source.committed(&ack.position)) {
        Step::Done(()) => true,
        Step::Poison(e) => {
            shared.fail(
                ErrorClass::Fatal,
                &format!("upstream commit: {}", e.message),
                None,
            );
            false
        }
        Step::Halt => false,
    }
}

async fn append_loop(
    shared: Arc<Shared>,
    writer: Arc<dyn FabricWriter>,
    split: SplitId,
    mut batches: mpsc::Receiver<(SourceBatch, BudgetLease)>,
    acks: mpsc::UnboundedSender<Acked>,
) {
    while let Some((batch, lease)) = batches.recv().await {
        if shared.fenced() {
            return;
        }
        let rows = batch.events.rows() as u64;
        let bytes = batch.events.size_bytes() as u64;
        let mut failures = 0u32;
        loop {
            match writer.append(&batch).await {
                Ok(_) => break,
                Err(fe) => {
                    let e = ConnectorError::from(fe);
                    failures += 1;
                    match shared.decide(&e, failures) {
                        // Not cut short by a graceful stop: in-flight batches
                        // finish (the policy still bounds the attempts).
                        Action::Retry(d) => tokio::time::sleep(d).await,
                        Action::DeadLetter => {
                            let events = match &batch.events {
                                EventBatch::Events(v) => v.clone(),
                                EventBatch::Arrow { envelope, .. } => vec![envelope.clone()],
                            };
                            for ev in events {
                                if !shared.dead_letter(Some(split.clone()), Some(ev), &e).await {
                                    return;
                                }
                            }
                            break;
                        }
                        Action::Paused => {
                            if !shared.wait_resumed().await {
                                return;
                            }
                            failures = 0;
                        }
                        Action::Stop => return,
                    }
                }
            }
        }
        if acks
            .send(Acked {
                position: batch.position,
                rows,
                bytes,
            })
            .is_err()
        {
            return;
        }
        drop(lease);
    }
}

// -------------------------------------------------------------------- sink --

async fn run_sink(shared: Arc<Shared>, mut reader: Box<dyn FabricReader>, mut sink: SinkHandle) {
    // Exactly-once: the target says how far it got; the reader seeks there,
    // so batches the target already committed are skipped.
    if let SinkHandle::Transactional(txn) = &mut sink {
        let recovered = match retrying!(shared, txn.recover()) {
            Step::Done(cp) => cp,
            Step::Poison(e) => {
                shared.fail(ErrorClass::Fatal, &format!("recover: {}", e.message), None);
                None
            }
            Step::Halt => None,
        };
        if shared.stopping() {
            shared.finish();
            return;
        }
        if let Some(cp) = recovered {
            if cp.epoch > shared.epoch {
                shared.fence(&CheckpointError::Fenced {
                    stored: cp.epoch,
                    attempted: shared.epoch,
                });
                return;
            }
            if let Some(pos) = cp.splits.get(&SplitId::new(FABRIC_SPLIT)) {
                match retrying!(shared, async {
                    reader.seek(pos).await.map_err(ConnectorError::from)
                }) {
                    Step::Done(()) => {}
                    Step::Poison(e) => {
                        shared.fail(ErrorClass::Fatal, &format!("seek: {}", e.message), None);
                        return;
                    }
                    Step::Halt => return,
                }
            }
        }
    }

    let (batch_tx, batch_rx) = mpsc::channel::<(EventBatch, Position, BudgetLease)>(
        shared.config.max_in_flight_batches.max(1),
    );
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Position>();
    let writer = AbortOnDrop(Some(shared.spawn(sink_loop(
        shared.clone(),
        sink,
        batch_rx,
        done_tx,
    ))));

    let mut failures = 0u32;
    'read: loop {
        while let Ok(pos) = done_rx.try_recv() {
            if !commit_fabric(&shared, reader.as_mut(), &pos).await {
                break 'read;
            }
        }
        if shared.stopping() {
            break;
        }
        if shared.state() == PipelineState::Paused && !shared.wait_resumed().await {
            break;
        }
        let mut lease = tokio::select! {
            biased;
            _ = shared.stopped() => break,
            Some(pos) = done_rx.recv() => {
                if !commit_fabric(&shared, reader.as_mut(), &pos).await {
                    break;
                }
                continue;
            }
            lease = shared.pool.acquire() => lease,
        };
        let budget = lease.budget();
        let result = tokio::select! {
            _ = shared.stopped() => break,
            r = reader.poll(budget) => r,
        };
        match result {
            Ok(Some((batch, pos))) => {
                failures = 0;
                let rows = batch.rows();
                let bytes = batch.size_bytes();
                if !budget.admits(rows, bytes) {
                    shared.fail(
                        ErrorClass::Fatal,
                        &format!(
                            "the Fabric reader returned {rows} rows and {bytes} bytes, over its budget of {} rows and {} bytes",
                            budget.max_rows, budget.max_bytes
                        ),
                        None,
                    );
                    break;
                }
                lease.shrink_to(bytes);
                if batch_tx.send((batch, pos, lease)).await.is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(lease);
                tokio::select! {
                    _ = shared.stopped() => break,
                    _ = tokio::time::sleep(shared.config.linger) => {}
                    Some(pos) = done_rx.recv() => {
                        if !commit_fabric(&shared, reader.as_mut(), &pos).await {
                            break;
                        }
                    }
                }
            }
            Err(fe) => {
                drop(lease);
                let e = ConnectorError::from(fe);
                failures += 1;
                match shared.decide(&e, failures) {
                    Action::Retry(d) => {
                        if !shared.sleep(d).await {
                            break;
                        }
                    }
                    Action::DeadLetter => {
                        shared.fail(
                            ErrorClass::Fatal,
                            &format!("Fabric read: {}", e.message),
                            None,
                        );
                        break;
                    }
                    Action::Paused => {
                        failures = 0;
                        if !shared.wait_resumed().await {
                            break;
                        }
                    }
                    Action::Stop => break,
                }
            }
        }
    }

    // Drain: the sink finishes what it holds; each finished batch's Fabric
    // offset is committed.
    drop(batch_tx);
    while let Some(pos) = done_rx.recv().await {
        if !shared.halted() {
            commit_fabric(&shared, reader.as_mut(), &pos).await;
        }
    }
    writer.join().await;
    shared.finish();
}

async fn commit_fabric(shared: &Shared, reader: &mut dyn FabricReader, pos: &Position) -> bool {
    match retrying!(shared, async {
        reader.commit(pos).await.map_err(ConnectorError::from)
    }) {
        Step::Done(()) => true,
        Step::Poison(e) => {
            shared.fail(
                ErrorClass::Fatal,
                &format!("Fabric commit: {}", e.message),
                None,
            );
            false
        }
        Step::Halt => false,
    }
}

async fn sink_loop(
    shared: Arc<Shared>,
    mut sink: SinkHandle,
    mut batches: mpsc::Receiver<(EventBatch, Position, BudgetLease)>,
    done: mpsc::UnboundedSender<Position>,
) {
    while let Some((batch, pos, lease)) = batches.recv().await {
        if shared.halted() {
            return;
        }
        let rows = batch.rows() as u64;
        let bytes = batch.size_bytes() as u64;
        shared.limiter.acquire(bytes).await;
        let ok = match &mut sink {
            SinkHandle::Plain(s) => write_plain(&shared, s.as_mut(), batch).await,
            SinkHandle::Transactional(t) => {
                write_transactional(&shared, t.as_mut(), batch, &pos).await
            }
        };
        drop(lease);
        if !ok {
            return;
        }
        shared.limiter.succeeded();
        shared.observer.sink_batch(&shared.instance, rows, bytes);
        if done.send(pos).is_err() {
            return;
        }
    }
}

/// At-least-once: write, then flush. `false` if the batch was not finished.
async fn write_plain(shared: &Shared, sink: &mut dyn Sink, batch: EventBatch) -> bool {
    if !write_batch(shared, sink, batch).await {
        return false;
    }
    match retrying!(shared, sink.flush()) {
        Step::Done(_) => true,
        Step::Poison(e) => {
            shared.fail(ErrorClass::Fatal, &format!("flush: {}", e.message), None);
            false
        }
        Step::Halt => false,
    }
}

/// Exactly-once: write (staged), prepare with the checkpoint, commit.
async fn write_transactional(
    shared: &Shared,
    sink: &mut dyn crate::txn::TransactionalSink,
    batch: EventBatch,
    pos: &Position,
) -> bool {
    if !write_batch(shared, &mut *sink, batch).await {
        return false;
    }
    let mut cp = Checkpoint::new(shared.instance.clone(), shared.epoch);
    cp.splits.insert(SplitId::new(FABRIC_SPLIT), pos.clone());
    let token = match retrying!(shared, sink.prepare(&cp)) {
        Step::Done(t) => t,
        Step::Poison(e) => {
            shared.fail(ErrorClass::Fatal, &format!("prepare: {}", e.message), None);
            return false;
        }
        Step::Halt => return false,
    };
    match retrying!(shared, sink.commit(token.clone())) {
        Step::Done(()) => true,
        other => {
            if let Step::Poison(e) = other {
                shared.fail(ErrorClass::Fatal, &format!("commit: {}", e.message), None);
            }
            // The token's state decides at the next `recover`; abort now if
            // the target still takes calls.
            let _ = sink.abort(token).await;
            false
        }
    }
}

/// Write one batch; on `Poison`, isolate the poison events one by one and
/// send each to the DLQ with its error.
async fn write_batch(shared: &Shared, sink: &mut dyn Sink, batch: EventBatch) -> bool {
    shared.observer.api_call(&shared.instance);
    let err = match retrying!(shared, sink.write(batch.clone())) {
        Step::Done(_) => return true,
        Step::Poison(e) => e,
        Step::Halt => return false,
    };
    match batch {
        EventBatch::Events(events) if events.len() > 1 => {
            for ev in events {
                shared.observer.api_call(&shared.instance);
                match retrying!(shared, sink.write(EventBatch::Events(vec![ev.clone()]))) {
                    Step::Done(_) => {}
                    Step::Poison(e) => {
                        if !shared.dead_letter(None, Some(ev), &e).await {
                            return false;
                        }
                    }
                    Step::Halt => return false,
                }
            }
            true
        }
        EventBatch::Events(events) => {
            for ev in events {
                if !shared.dead_letter(None, Some(ev), &err).await {
                    return false;
                }
            }
            true
        }
        EventBatch::Arrow { envelope, .. } => shared.dead_letter(None, Some(envelope), &err).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_overrides() {
        let cfg = PipelineConfig::default()
            .with_overrides(&serde_json::json!({
                "pipeline": {"max_batch_rows": 1000, "linger_ms": 50, "retry": {"max_attempts": 3}}
            }))
            .unwrap_or_default();
        assert_eq!(cfg.max_batch_rows, 1000);
        assert_eq!(cfg.linger, Duration::from_millis(50));
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.max_batch_bytes, 8 << 20);
        assert!(
            PipelineConfig::default()
                .with_overrides(&serde_json::json!({"pipeline": {"bogus": 1}}))
                .is_err()
        );
        assert!(
            PipelineConfig::default()
                .with_overrides(&serde_json::json!({"pipeline": {"max_batch_rows": 0}}))
                .is_err()
        );
    }
}
