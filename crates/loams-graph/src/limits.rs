//! Limits v1 (GR1 Task 6; design §48 §13.1, §13.2; ruling R0.8).
//!
//! * [`StatementLimits`]: what one statement may cost — its deadline, the rows and bytes of a
//!   unary answer, its size, parameters and batch length, the property size the engine accepts,
//!   its variable-length patterns and operator chains, and how many of a graph's statements may
//!   run on past their deadline. [`StatementLimits::DEFAULT`] and [`StatementLimits::MAXIMUM`]
//!   are §48 §13.1's columns; a graph's own `GraphLimits` and a request's fields choose within
//!   them.
//! * [`StatementPool`]: the threads statements run on. Each has the parser's big stack
//!   ([`crate::classify::PARSE_STACK_BYTES`]), so a statement no longer starts a thread of its
//!   own (Task 3 re-review 2c), and the pool takes disk opens too (Task 4 review M8).
//! * [`NamespaceSlots`]: at most so many statements of one namespace at once (§48 §13.2's
//!   "concurrent statements", 64), answered `RESOURCE_EXHAUSTED`/`quota_exceeded`.
//! * [`Detached`]: statements that ran past their deadline. Grafeo cannot stop a running
//!   statement (R0.8), so at the deadline the RPC answers and the statement runs on, counted, on
//!   its pool thread and holding its slots; a graph with `max_detached` of them refuses more.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::catalog::GraphLimits;
use crate::engine::GraphError;

/// What one statement may cost (§48 §13.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatementLimits {
    /// The statement's deadline, from when the RPC arrives. At it the RPC answers
    /// `graph_statement_timeout` and the statement is detached (R0.8 (a)).
    pub timeout: Duration,
    /// Rows in a unary answer; more are cut and `truncated` is set.
    pub max_rows: u32,
    /// Encoded bytes of a unary answer's rows; past it `graph_result_too_large`.
    pub max_result_bytes: u64,
    /// Bytes of statement text (`INVALID_ARGUMENT` past it).
    pub max_statement_bytes: usize,
    /// Parameters of one statement (`INVALID_ARGUMENT` past it).
    pub max_parameters: usize,
    /// Statements of one batch (`INVALID_ARGUMENT` past it).
    pub max_batch_statements: usize,
    /// The largest property value the engine stores (Grafeo's `max_property_size`).
    pub max_property_bytes: usize,
    /// The longest variable-length pattern (R0.8 (b)).
    pub max_path_hops: u32,
    /// The most operator-chain links in a statement (Task 3 re-review 2c): what the parser's
    /// stack can take.
    pub max_chain_tokens: usize,
    /// How many of one graph's statements may run on past their deadline before the graph
    /// refuses new ones (R0.8 (a)).
    pub max_detached: usize,
}

impl StatementLimits {
    /// §48 §13.1's defaults.
    pub const DEFAULT: Self = Self {
        timeout: Duration::from_secs(30),
        max_rows: 10_000,
        max_result_bytes: 16 << 20,
        max_statement_bytes: 1 << 20,
        max_parameters: 1_000,
        max_batch_statements: 1_000,
        max_property_bytes: 1 << 20,
        max_path_hops: crate::classify::MAX_PATH_HOPS,
        max_chain_tokens: crate::classify::MAX_CHAIN_TOKENS,
        max_detached: 2,
    };

    /// §48 §13.1's maxima: no configuration, graph or request goes past them.
    pub const MAXIMUM: Self = Self {
        timeout: Duration::from_secs(300),
        max_rows: 100_000,
        max_result_bytes: 64 << 20,
        max_statement_bytes: 1 << 20,
        max_parameters: 10_000,
        max_batch_statements: 10_000,
        max_property_bytes: 16 << 20,
        max_path_hops: crate::classify::MAX_PATH_HOPS,
        max_chain_tokens: crate::classify::MAX_CHAIN_TOKENS,
        max_detached: 64,
    };

    /// Each limit capped at [`StatementLimits::MAXIMUM`], and at least 1.
    #[must_use]
    pub fn capped(self) -> Self {
        let max = Self::MAXIMUM;
        Self {
            timeout: self.timeout.clamp(Duration::from_millis(1), max.timeout),
            max_rows: self.max_rows.clamp(1, max.max_rows),
            max_result_bytes: self.max_result_bytes.clamp(1, max.max_result_bytes),
            max_statement_bytes: self.max_statement_bytes.clamp(1, max.max_statement_bytes),
            max_parameters: self.max_parameters.min(max.max_parameters),
            max_batch_statements: self.max_batch_statements.clamp(1, max.max_batch_statements),
            max_property_bytes: self.max_property_bytes.clamp(1, max.max_property_bytes),
            max_path_hops: self.max_path_hops.clamp(1, max.max_path_hops),
            max_chain_tokens: self.max_chain_tokens.clamp(1, max.max_chain_tokens),
            max_detached: self.max_detached.min(max.max_detached),
        }
    }

    /// These (the server's) limits with a graph's own on top: each nonzero field of `graph`
    /// replaces the server's, within [`StatementLimits::MAXIMUM`].
    #[must_use]
    pub fn for_graph(&self, graph: &GraphLimits) -> Self {
        let mut out = *self;
        if graph.timeout_ms > 0 {
            out.timeout = Duration::from_millis(u64::from(graph.timeout_ms));
        }
        if graph.max_rows > 0 {
            out.max_rows = graph.max_rows;
        }
        if graph.max_result_bytes > 0 {
            out.max_result_bytes = graph.max_result_bytes;
        }
        if graph.max_path_hops > 0 {
            out.max_path_hops = graph.max_path_hops;
        }
        out.capped()
    }

    /// A request's deadline: `requested_ms`, or these limits' timeout when it is 0, never past
    /// `ceiling` (the server's largest: [`StatementLimits::MAXIMUM`], or less when the engine's
    /// own `query_timeout` is lower, R0.8 (a)).
    #[must_use]
    pub fn timeout_for(&self, requested_ms: u32, ceiling: Duration) -> Duration {
        let ceiling = ceiling.min(Self::MAXIMUM.timeout);
        if requested_ms == 0 {
            self.timeout.min(ceiling)
        } else {
            Duration::from_millis(u64::from(requested_ms)).min(ceiling)
        }
    }

    /// A unary request's row cap: `requested`, or these limits' when it is 0, never past
    /// [`StatementLimits::MAXIMUM`] (the proto: "capped by the server's maximum").
    #[must_use]
    pub fn rows_for(&self, requested: u32) -> u32 {
        if requested == 0 {
            self.max_rows
        } else {
            requested.min(Self::MAXIMUM.max_rows)
        }
    }

    /// Refuses statement text over `max_statement_bytes`.
    ///
    /// # Errors
    ///
    /// [`GraphError::OverLimit`].
    pub fn check_statement(&self, statement: &str) -> Result<(), GraphError> {
        if statement.len() > self.max_statement_bytes {
            return Err(GraphError::OverLimit(format!(
                "the statement is {} bytes; at most {} are accepted",
                statement.len(),
                self.max_statement_bytes
            )));
        }
        Ok(())
    }

    /// Refuses more than `max_parameters` parameters.
    ///
    /// # Errors
    ///
    /// [`GraphError::OverLimit`].
    pub fn check_parameters(&self, count: usize) -> Result<(), GraphError> {
        if count > self.max_parameters {
            return Err(GraphError::OverLimit(format!(
                "the statement binds {count} parameters; at most {} are accepted",
                self.max_parameters
            )));
        }
        Ok(())
    }

    /// Refuses a batch of more than `max_batch_statements` statements.
    ///
    /// # Errors
    ///
    /// [`GraphError::OverLimit`].
    pub fn check_batch(&self, count: usize) -> Result<(), GraphError> {
        if count > self.max_batch_statements {
            return Err(GraphError::OverLimit(format!(
                "the batch holds {count} statements; at most {} are accepted",
                self.max_batch_statements
            )));
        }
        Ok(())
    }
}

impl Default for StatementLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The default number of one namespace's statements that may run at once (§48 §13.2).
pub const DEFAULT_NAMESPACE_STATEMENTS: usize = 64;

/// A job on the pool.
type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    threads: usize,
    idle: usize,
    closed: bool,
}

struct PoolShared {
    queue: Mutex<Queue>,
    ready: Condvar,
    max_threads: usize,
}

/// How long an idle worker waits for a job before it exits; the pool starts another when work
/// comes back.
const IDLE_EXIT: Duration = Duration::from_secs(60);

/// The threads graph statements and disk opens run on (GR1 Task 6).
///
/// Each worker has [`crate::classify::PARSE_STACK_BYTES`] of stack (virtual memory, touched only
/// as deep as a statement nests) and is marked for [`crate::classify::on_big_stack`], which then
/// runs in place instead of starting a thread per parse and per engine call. Workers start on
/// demand, up to `max_threads`, and leave after a minute idle. A job past that queues: the
/// statement slots in front of the pool keep the queue short, because a statement takes its
/// slot before it submits.
///
/// A job's panic is caught on the worker, which carries on; the job's answer is then `Err`.
/// Dropping the pool lets idle workers exit; a worker running a detached statement exits when it
/// ends.
pub struct StatementPool {
    shared: Arc<PoolShared>,
}

impl std::fmt::Debug for StatementPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatementPool")
            .field("max_threads", &self.shared.max_threads)
            .finish_non_exhaustive()
    }
}

impl StatementPool {
    /// A pool of at most `max_threads` workers (at least one).
    #[must_use]
    pub fn new(max_threads: usize) -> Self {
        Self {
            shared: Arc::new(PoolShared {
                queue: Mutex::new(Queue::default()),
                ready: Condvar::new(),
                max_threads: max_threads.max(1),
            }),
        }
    }

    /// The most workers this pool starts.
    #[must_use]
    pub fn max_threads(&self) -> usize {
        self.shared.max_threads
    }

    /// Runs `f` on a worker. The answer is `Err` when `f` panicked or the worker could not
    /// start.
    pub fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<std::thread::Result<T>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job: Job = Box::new(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(outcome);
        });
        let spawn = {
            let mut queue = lock(&self.shared.queue);
            queue.jobs.push_back(job);
            // More jobs waiting than idle workers to take them (an idle worker that was woken
            // has not popped its job yet, so the queue still counts it).
            let spawn = queue.jobs.len() > queue.idle && queue.threads < self.shared.max_threads;
            if spawn {
                queue.threads += 1;
            }
            spawn
        };
        if spawn {
            let shared = Arc::clone(&self.shared);
            let started = std::thread::Builder::new()
                .name("loams-graph-pool".to_string())
                .stack_size(crate::classify::PARSE_STACK_BYTES)
                .spawn(move || worker(&shared));
            if let Err(err) = started {
                tracing::error!(error = %err, "could not start a graph statement worker");
                let mut queue = lock(&self.shared.queue);
                queue.threads -= 1;
                // With no worker at all the job would wait forever: fail it (its sender drops).
                if queue.threads == 0 {
                    queue.jobs.clear();
                }
            }
        } else {
            self.shared.ready.notify_one();
        }
        rx
    }
}

impl Drop for StatementPool {
    fn drop(&mut self) {
        lock(&self.shared.queue).closed = true;
        self.shared.ready.notify_all();
    }
}

fn worker(shared: &PoolShared) {
    crate::classify::mark_big_stack();
    let mut queue = lock(&shared.queue);
    loop {
        if let Some(job) = queue.jobs.pop_front() {
            drop(queue);
            job();
            queue = lock(&shared.queue);
            continue;
        }
        if queue.closed {
            break;
        }
        queue.idle += 1;
        let (next, waited) = shared
            .ready
            .wait_timeout(queue, IDLE_EXIT)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue = next;
        queue.idle -= 1;
        if waited.timed_out() && queue.jobs.is_empty() {
            break;
        }
    }
    queue.threads -= 1;
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One semaphore per namespace: at most `per_namespace` of its statements at once (§48 §13.2).
#[derive(Debug)]
pub struct NamespaceSlots {
    per_namespace: usize,
    slots: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
}

impl NamespaceSlots {
    /// At most `per_namespace` (at least 1) statements of each namespace at once.
    #[must_use]
    pub fn new(per_namespace: usize) -> Self {
        Self {
            per_namespace: per_namespace.max(1),
            slots: Mutex::default(),
        }
    }

    /// The cap per namespace.
    #[must_use]
    pub fn per_namespace(&self) -> usize {
        self.per_namespace
    }

    /// A slot of `namespace`, or `None` when all of its slots are taken.
    #[must_use]
    pub fn try_acquire(&self, namespace: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let semaphore = Arc::clone(
            lock(&self.slots)
                .entry(namespace.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(self.per_namespace))),
        );
        semaphore.try_acquire_owned().ok()
    }
}

/// Statements running on past their deadline (R0.8 (a)), by graph.
#[derive(Debug, Default)]
pub struct Detached {
    by_graph: Mutex<HashMap<(String, String), usize>>,
    total: AtomicUsize,
}

impl Detached {
    /// Detached statements of one graph.
    #[must_use]
    pub fn of(&self, namespace: &str, name: &str) -> usize {
        lock(&self.by_graph)
            .get(&(namespace.to_string(), name.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// Detached statements of every graph (`loams_graph_detached_statements`, Task 27).
    #[must_use]
    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }

    fn add(&self, key: &(String, String)) {
        *lock(&self.by_graph).entry(key.clone()).or_default() += 1;
        self.total.fetch_add(1, Ordering::SeqCst);
    }

    fn remove(&self, key: &(String, String)) {
        let mut by_graph = lock(&self.by_graph);
        if let Some(count) = by_graph.get_mut(key) {
            *count -= 1;
            if *count == 0 {
                by_graph.remove(key);
            }
        }
        self.total.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    /// Its caller gave up (deadline, or the client went away); counted in [`Detached`].
    Abandoned,
    Done,
}

/// One statement's life, seen from both ends: the caller that waits for it, and the work that
/// runs it. Whichever comes second settles the [`Detached`] count.
#[derive(Debug, Clone)]
pub struct Watch {
    phase: Arc<Mutex<Phase>>,
    key: Arc<(String, String)>,
    detached: Arc<Detached>,
}

impl Watch {
    /// A running statement of `namespace`/`name`.
    #[must_use]
    pub fn new(detached: Arc<Detached>, namespace: &str, name: &str) -> Self {
        Self {
            phase: Arc::new(Mutex::new(Phase::Running)),
            key: Arc::new((namespace.to_string(), name.to_string())),
            detached,
        }
    }

    /// The caller gave up on a statement that is still running: it is detached and counted.
    pub fn abandon(&self) {
        let mut phase = lock(&self.phase);
        if *phase == Phase::Running {
            *phase = Phase::Abandoned;
            self.detached.add(&self.key);
            tracing::warn!(
                namespace = %self.key.0,
                graph = %self.key.1,
                "a graph statement runs on past its caller (deadline or disconnect); detached"
            );
        }
    }

    /// The statement ended; a detached one is no longer counted.
    pub fn finish(&self) {
        let mut phase = lock(&self.phase);
        if *phase == Phase::Abandoned {
            self.detached.remove(&self.key);
        }
        *phase = Phase::Done;
    }
}
