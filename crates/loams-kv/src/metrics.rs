//! Operation metrics and the observer hook (design §77 §6, D1021; KV1 plan
//! KV0 Task 7).
//!
//! Every [`Store::run`](crate::Store::run), and so every one-shot
//! operation, is counted in `loams_kv_ops_total{backend,op,outcome}` and
//! timed in `loams_kv_op_seconds{backend,op}`. The labels name the backend
//! (`embedded` or `tikv`), the run's `op` (a `&'static str` from the code,
//! so the label set is bounded) and the [`Outcome`]: **never a keyspace,
//! namespace, tenant or root** (D1021). Per-tenant numbers leave the
//! process only through a [`KvObserver`], which sees the keyspace and has
//! no wire format; the default observes nothing.
//!
//! The workspace has no metrics facade, so the registry is this module's
//! own: atomic-free counters under one mutex per family, rendered in the
//! Prometheus text format by [`OpMetrics::render`] for whatever endpoint
//! serves it (Ruling T7-1).

use std::collections::BTreeMap;
use std::fmt::{self, Debug, Write as _};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crate::{Backend, TxnError};

/// How a run ended: the label `outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    Ok,
    Conflict,
    NotApplied,
    Undetermined,
    AlreadyExists,
    Fatal,
    Deadline,
}

impl Outcome {
    /// Every outcome.
    pub const ALL: [Outcome; 7] = [
        Outcome::Ok,
        Outcome::Conflict,
        Outcome::NotApplied,
        Outcome::Undetermined,
        Outcome::AlreadyExists,
        Outcome::Fatal,
        Outcome::Deadline,
    ];

    /// The outcome of a run's result.
    pub fn of<T>(result: &Result<T, TxnError>) -> Self {
        match result {
            Ok(_) => Outcome::Ok,
            Err(TxnError::Conflict) => Outcome::Conflict,
            Err(TxnError::NotApplied(_)) => Outcome::NotApplied,
            Err(TxnError::Undetermined { .. }) => Outcome::Undetermined,
            Err(TxnError::AlreadyExists(_)) => Outcome::AlreadyExists,
            Err(TxnError::Fatal(_)) => Outcome::Fatal,
            Err(TxnError::Deadline) => Outcome::Deadline,
        }
    }

    /// The label value.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Conflict => "conflict",
            Outcome::NotApplied => "not_applied",
            Outcome::Undetermined => "undetermined",
            Outcome::AlreadyExists => "already_exists",
            Outcome::Fatal => "fatal",
            Outcome::Deadline => "deadline",
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Sees every run of a store, with its keyspace: the only way per-tenant
/// numbers leave the store (D1021). Called after the run, on the caller's
/// task; keep it cheap.
pub trait KvObserver: Send + Sync + Debug {
    /// A run of `op` on `keyspace` ended with `outcome` after `elapsed`.
    fn on_op(&self, _keyspace: &str, _op: &'static str, _outcome: Outcome, _elapsed: Duration) {}
}

/// Observes nothing (the default).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopObserver;

impl KvObserver for NoopObserver {}

/// The upper bounds of `loams_kv_op_seconds`' buckets, in seconds.
pub const BUCKETS: [f64; 14] = [
    0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Debug, Clone, Default)]
struct Histogram {
    /// Per bucket (not cumulative); the last slot is `+Inf`.
    counts: [u64; BUCKETS.len() + 1],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, seconds: f64) {
        let slot = BUCKETS
            .iter()
            .position(|&le| seconds <= le)
            .unwrap_or(BUCKETS.len());
        self.counts[slot] += 1;
        self.sum += seconds;
        self.count += 1;
    }
}

fn backend_label(backend: Backend) -> &'static str {
    match backend {
        Backend::Embedded => "embedded",
        Backend::Tikv => "tikv",
    }
}

/// The process's operation metrics ([`op_metrics`]).
#[derive(Debug, Default)]
pub struct OpMetrics {
    ops: Mutex<BTreeMap<(&'static str, &'static str, Outcome), u64>>,
    seconds: Mutex<BTreeMap<(&'static str, &'static str), Histogram>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Escapes a label value for the text format.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

impl OpMetrics {
    /// Counts and times one run.
    pub fn record(&self, backend: Backend, op: &'static str, outcome: Outcome, elapsed: Duration) {
        let backend = backend_label(backend);
        *lock(&self.ops).entry((backend, op, outcome)).or_default() += 1;
        lock(&self.seconds)
            .entry((backend, op))
            .or_default()
            .observe(elapsed.as_secs_f64());
    }

    /// `loams_kv_ops_total{backend,op,outcome}`.
    pub fn ops_total(&self, backend: Backend, op: &str, outcome: Outcome) -> u64 {
        lock(&self.ops)
            .iter()
            .find(|((b, o, out), _)| *b == backend_label(backend) && *o == op && *out == outcome)
            .map_or(0, |(_, n)| *n)
    }

    /// `loams_kv_op_seconds_count{backend,op}`.
    pub fn op_seconds_count(&self, backend: Backend, op: &str) -> u64 {
        lock(&self.seconds)
            .iter()
            .find(|((b, o), _)| *b == backend_label(backend) && *o == op)
            .map_or(0, |(_, h)| h.count)
    }

    /// Both families in the Prometheus text exposition format (0.0.4).
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# HELP loams_kv_ops_total Store runs by backend, operation and outcome.\n\
             # TYPE loams_kv_ops_total counter\n",
        );
        for ((backend, op, outcome), n) in lock(&self.ops).iter() {
            let _ = writeln!(
                out,
                "loams_kv_ops_total{{backend=\"{backend}\",op=\"{}\",outcome=\"{outcome}\"}} {n}",
                escape(op)
            );
        }
        out.push_str(
            "# HELP loams_kv_op_seconds Store run latency by backend and operation.\n\
             # TYPE loams_kv_op_seconds histogram\n",
        );
        for ((backend, op), h) in lock(&self.seconds).iter() {
            let op = escape(op);
            let mut cumulative = 0;
            for (i, le) in BUCKETS.iter().enumerate() {
                cumulative += h.counts[i];
                let _ = writeln!(
                    out,
                    "loams_kv_op_seconds_bucket{{backend=\"{backend}\",op=\"{op}\",le=\"{le}\"}} {cumulative}"
                );
            }
            let _ = writeln!(
                out,
                "loams_kv_op_seconds_bucket{{backend=\"{backend}\",op=\"{op}\",le=\"+Inf\"}} {}",
                h.count
            );
            let _ = writeln!(
                out,
                "loams_kv_op_seconds_sum{{backend=\"{backend}\",op=\"{op}\"}} {}",
                h.sum
            );
            let _ = writeln!(
                out,
                "loams_kv_op_seconds_count{{backend=\"{backend}\",op=\"{op}\"}} {}",
                h.count
            );
        }
        out
    }
}

static OP_METRICS: LazyLock<OpMetrics> = LazyLock::new(OpMetrics::default);

/// The process's operation metrics, which every store records into.
pub fn op_metrics() -> &'static OpMetrics {
    &OP_METRICS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_histogram_renders_cumulative_buckets() {
        let m = OpMetrics::default();
        for ms in [0, 3, 3, 700, 20_000] {
            m.record(
                Backend::Embedded,
                "t.op",
                Outcome::Ok,
                Duration::from_millis(ms),
            );
        }
        m.record(Backend::Tikv, "t.op", Outcome::Conflict, Duration::ZERO);
        let text = m.render();
        assert!(
            text.contains("loams_kv_ops_total{backend=\"embedded\",op=\"t.op\",outcome=\"ok\"} 5")
        );
        assert!(
            text.contains(
                "loams_kv_ops_total{backend=\"tikv\",op=\"t.op\",outcome=\"conflict\"} 1"
            )
        );
        assert!(text.contains(
            "loams_kv_op_seconds_bucket{backend=\"embedded\",op=\"t.op\",le=\"0.0005\"} 1"
        ));
        assert!(text.contains(
            "loams_kv_op_seconds_bucket{backend=\"embedded\",op=\"t.op\",le=\"0.005\"} 3"
        ));
        assert!(
            text.contains(
                "loams_kv_op_seconds_bucket{backend=\"embedded\",op=\"t.op\",le=\"1\"} 4"
            )
        );
        assert!(
            text.contains(
                "loams_kv_op_seconds_bucket{backend=\"embedded\",op=\"t.op\",le=\"10\"} 4"
            )
        );
        assert!(text.contains(
            "loams_kv_op_seconds_bucket{backend=\"embedded\",op=\"t.op\",le=\"+Inf\"} 5"
        ));
        assert!(text.contains("loams_kv_op_seconds_count{backend=\"embedded\",op=\"t.op\"} 5"));
        assert_eq!(m.ops_total(Backend::Embedded, "t.op", Outcome::Ok), 5);
        assert_eq!(m.op_seconds_count(Backend::Tikv, "t.op"), 1);
    }

    #[test]
    fn outcome_labels_are_distinct() {
        let labels: std::collections::BTreeSet<&str> =
            Outcome::ALL.iter().map(|o| o.as_str()).collect();
        assert_eq!(labels.len(), Outcome::ALL.len());
    }
}
