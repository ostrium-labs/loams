//! The outbox relay (§74 §5.5 rule 2): claim undispatched rows, call the
//! port idempotently, mark them dispatched. Delivery is at least once: a
//! crash between the call and the mark repeats an idempotent call.
//!
//! When the server ignores `resonate:delay` ([`DelaySupport::Defer`]: the SQL
//! stores at 31ffb201), a create whose `delay_until_ms` is still ahead stays
//! in the outbox, unmarked, and so does every later row for the same
//! promise; it is sent once its time has come. Each pass claims the batch
//! plus the rows the previous pass deferred, so deferred rows at the head of
//! the outbox never starve the rest.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;

use crate::port::{DelaySupport, Durability, DurabilityError, OutboxRequest};

/// The metric counting rows whose ID exists with a different parameter.
pub const OUTBOX_CONFLICT_METRIC: &str = "loams_temporal_outbox_conflict_total";
/// The metric counting rows dispatched.
pub const OUTBOX_DISPATCHED_METRIC: &str = "loams_temporal_outbox_dispatched_total";
/// The metric counting calls that got no answer (rows left for a retry).
pub const OUTBOX_UNAVAILABLE_METRIC: &str = "loams_temporal_outbox_unavailable_total";
/// The metric counting rows Resonate refused outright (marked, not retried).
pub const OUTBOX_REJECTED_METRIC: &str = "loams_temporal_outbox_rejected_total";

/// Where the relay reads the outbox (the store implements it).
#[async_trait]
pub trait OutboxSource: Send + Sync {
    /// Up to `limit` undispatched rows, oldest first, claimed for this relay.
    async fn claim(&self, limit: usize) -> Result<Vec<(i64, OutboxRequest)>, String>;
    /// Set `dispatched_at` on these rows.
    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), String>;
    /// Return when new rows may be there (`LISTEN`/`NOTIFY`, or a timer).
    async fn wait(&self);
}

/// The relay's counters: plain atomics, no tenant or namespace labels
/// (D1021, D1532). [`RelayMetrics::render`] writes them in the Prometheus
/// text format for whatever serves `/metrics`.
#[derive(Debug, Default)]
pub struct RelayMetrics {
    conflict: AtomicU64,
    dispatched: AtomicU64,
    unavailable: AtomicU64,
    rejected: AtomicU64,
}

impl RelayMetrics {
    /// `loams_temporal_outbox_conflict_total`.
    pub fn conflict_total(&self) -> u64 {
        self.conflict.load(Ordering::Relaxed)
    }

    /// `loams_temporal_outbox_dispatched_total`.
    pub fn dispatched_total(&self) -> u64 {
        self.dispatched.load(Ordering::Relaxed)
    }

    /// `loams_temporal_outbox_unavailable_total`.
    pub fn unavailable_total(&self) -> u64 {
        self.unavailable.load(Ordering::Relaxed)
    }

    /// `loams_temporal_outbox_rejected_total`.
    pub fn rejected_total(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// The counters in the Prometheus text format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (name, help, v) in [
            (
                OUTBOX_CONFLICT_METRIC,
                "Outbox rows whose promise ID exists with a different parameter (a bug).",
                self.conflict_total(),
            ),
            (
                OUTBOX_DISPATCHED_METRIC,
                "Outbox rows dispatched to Resonate.",
                self.dispatched_total(),
            ),
            (
                OUTBOX_UNAVAILABLE_METRIC,
                "Outbox calls that got no answer; the rows are retried.",
                self.unavailable_total(),
            ),
            (
                OUTBOX_REJECTED_METRIC,
                "Outbox rows Resonate refused; marked, not retried.",
                self.rejected_total(),
            ),
        ] {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {v}\n"
            ));
        }
        out
    }
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayPass {
    /// Rows claimed.
    pub claimed: usize,
    /// Rows marked dispatched (successes, conflicts and refusals).
    pub marked: usize,
    /// Whether a call got no answer; the rest of the batch waits for the
    /// next pass, in order.
    pub unavailable: bool,
    /// Rows left for later because their delay has not ended (only when
    /// the server ignores `resonate:delay`).
    pub deferred: usize,
    /// The earliest time a deferred row is due, Unix ms.
    pub next_due_ms: Option<i64>,
}

/// The relay's knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayConfig {
    /// Rows per claim.
    pub batch: usize,
    /// How long `run` waits after a pass that hit an unavailable Resonate.
    pub unavailable_backoff: Duration,
    /// Calls to Resonate in flight at once, each for a different promise
    /// (BQ1-durable: the relay was one call at a time).
    pub concurrency: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            batch: 64,
            unavailable_backoff: Duration::from_millis(500),
            concurrency: 64,
        }
    }
}

/// The outbox relay (module docs).
pub struct Relay {
    source: Arc<dyn OutboxSource>,
    durability: Arc<dyn Durability>,
    metrics: Arc<RelayMetrics>,
    config: RelayConfig,
    deferred_last: AtomicUsize,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("metrics", &self.metrics)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Relay {
    /// A relay from `source` into `durability`.
    pub fn new(source: Arc<dyn OutboxSource>, durability: Arc<dyn Durability>) -> Self {
        Self {
            source,
            durability,
            metrics: Arc::default(),
            deferred_last: AtomicUsize::new(0),
            config: RelayConfig::default(),
        }
    }

    /// With `config`.
    pub fn with_config(mut self, config: RelayConfig) -> Self {
        self.config = config;
        self
    }

    /// Count into `metrics` (shared with whatever serves them).
    pub fn with_metrics(mut self, metrics: Arc<RelayMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// The counters.
    pub fn metrics(&self) -> Arc<RelayMetrics> {
        Arc::clone(&self.metrics)
    }

    /// One pass: claim a batch, call the port for its rows, mark what got a
    /// definite answer.
    ///
    /// Rows of different promises are independent, so they are sent
    /// concurrently (up to [`RelayConfig::concurrency`] calls in flight: the
    /// pipelining that keeps one slow round trip from serialising the whole
    /// outbox). Rows of the same promise keep their order: they go one after
    /// another, and a row whose call got no answer holds back every later row
    /// of its promise for the next pass (so a create and a later settle of the
    /// same promise keep their order). `Err` only when the source fails;
    /// nothing is lost, the rows are claimed again.
    pub async fn run_once(&self) -> Result<RelayPass, String> {
        let extra = self
            .deferred_last
            .load(Ordering::Relaxed)
            .min(self.config.batch.saturating_mul(16));
        let rows = self.source.claim(self.config.batch + extra).await?;
        let mut pass = RelayPass {
            claimed: rows.len(),
            ..RelayPass::default()
        };
        let defer = self.durability.delay_support() == DelaySupport::Defer;
        let now = self.durability.now_ms();
        let mut held: HashSet<&str> = HashSet::new();
        // The rows to send now (indices into `rows`), grouped by promise in
        // claim order. Indices, not references: the stream's closure then takes
        // an owned argument, which keeps the future `Send` for `tokio::spawn`.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_of: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (at, (_, req)) in rows.iter().enumerate() {
            let due = match req {
                OutboxRequest::CreatePromise(c) if defer => c.delay_until_ms.filter(|at| *at > now),
                _ => None,
            };
            if due.is_some() || held.contains(req.promise_id()) {
                held.insert(req.promise_id());
                pass.deferred += 1;
                pass.next_due_ms = match (pass.next_due_ms, due) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                continue;
            }
            let g = *group_of.entry(req.promise_id()).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[g].push(at);
        }
        let outcomes: Vec<(Vec<i64>, bool)> = futures::stream::iter(groups)
            .map(|g| self.send_group(&rows, g))
            .buffer_unordered(self.config.concurrency.max(1))
            .collect()
            .await;
        let mut done = Vec::with_capacity(rows.len());
        for (marked, unavailable) in outcomes {
            done.extend(marked);
            pass.unavailable |= unavailable;
        }
        if !done.is_empty() {
            self.source.mark_dispatched(&done).await?;
        }
        pass.marked = done.len();
        self.deferred_last.store(pass.deferred, Ordering::Relaxed);
        Ok(pass)
    }

    /// Sends one promise's rows in order. Answers the rows to mark and
    /// whether Resonate was unavailable (the rest of the group then waits
    /// for the next pass).
    async fn send_group(
        &self,
        rows: &[(i64, OutboxRequest)],
        group: Vec<usize>,
    ) -> (Vec<i64>, bool) {
        let mut done = Vec::with_capacity(group.len());
        for at in group {
            let (row, req) = (rows[at].0, &rows[at].1);
            let result = match req {
                OutboxRequest::CreatePromise(c) => {
                    self.durability.promise_create(c).await.map(|_| ())
                }
                OutboxRequest::Settle { id, state } => {
                    self.durability.promise_settle(id, *state).await.map(|_| ())
                }
            };
            match result {
                Ok(()) => {
                    self.metrics.dispatched.fetch_add(1, Ordering::Relaxed);
                    done.push(row);
                }
                Err(DurabilityError::IdConflict { id }) => {
                    self.metrics.conflict.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(
                        outbox_id = row,
                        promise_id = %id,
                        "outbox row conflicts with an existing promise (a different parameter for the same ID); marked, not retried"
                    );
                    done.push(row);
                }
                Err(DurabilityError::Unavailable(e)) => {
                    self.metrics.unavailable.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(outbox_id = row, promise_id = %req.promise_id(), error = %e, "Resonate unavailable; outbox row left for retry");
                    return (done, true);
                }
                Err(e) => {
                    // A definite answer (not found, refused, a task conflict):
                    // repeating the call would get it again.
                    self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(outbox_id = row, promise_id = %req.promise_id(), error = %e, "Resonate refused an outbox row; marked, not retried");
                    done.push(row);
                }
            }
        }
        (done, false)
    }

    /// Run passes until `shutdown` completes: straight on while batches come
    /// back full, after `source.wait()` once the outbox is drained, after a
    /// backoff when Resonate was unavailable or the source failed.
    pub async fn run(&self, shutdown: impl Future<Output = ()> + Send) {
        tokio::pin!(shutdown);
        loop {
            if futures::poll!(&mut shutdown).is_ready() {
                return;
            }
            let mut due_in = None;
            let idle = match self.run_once().await {
                Ok(pass) if pass.unavailable => Some(true),
                Ok(pass) if pass.claimed - pass.deferred < self.config.batch => {
                    due_in = pass.next_due_ms.map(|at| {
                        Duration::from_millis(
                            u64::try_from(at - self.durability.now_ms()).unwrap_or(0),
                        )
                    });
                    Some(false)
                }
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "outbox source failed");
                    Some(true)
                }
            };
            match idle {
                None => {}
                Some(true) => tokio::select! {
                    _ = &mut shutdown => return,
                    _ = tokio::time::sleep(self.config.unavailable_backoff) => {}
                },
                Some(false) => tokio::select! {
                    _ = &mut shutdown => return,
                    _ = self.source.wait() => {}
                    _ = tokio::time::sleep(due_in.unwrap_or(Duration::ZERO)), if due_in.is_some() => {}
                },
            }
        }
    }
}
