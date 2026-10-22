//! The front's Prometheus metrics (HS1 Task 23, design §49 §15), on the admin
//! listener (`--admin-listen`, 8125, loopback only): `GET /metrics`.
//!
//! * Statements: `loams_house_queries_total{kind,status}`,
//!   `loams_house_query_seconds` (histogram), `loams_house_read_bytes_total`,
//!   `loams_house_written_bytes_total`, `loams_house_result_bytes_total`; the
//!   operator's `[house] metrics_namespace_labels` adds `ns` to each (one series
//!   per namespace: off by default, ruling R23.5).
//! * Admission (HS1 Task 21): `loams_house_queue_depth` (what Task 24's
//!   autoscaler reads), `loams_house_admitted_queries`,
//!   `loams_house_admitted_memory_bytes`.
//! * Workers: `loams_house_workers{state}`,
//!   `loams_house_worker_kills_total{reason}`,
//!   `loams_house_worker_boot_seconds` (histogram).
//! * `house-cache`: `loams_house_cache_hits_total`, `_misses_total` (blocks)
//!   and `_bytes_total` (bytes served to workers), plus
//!   `loams_house_cache_requests_total` and `_refused_total`.
//! * Commits: `loams_house_commit_seconds` (histogram, one per catalog
//!   commit of a batch), `loams_house_commits_total{result}`,
//!   `loams_house_commit_conflicts_total`.
//! * Compaction: `loams_house_compaction_runs_total{result}`,
//!   `loams_house_compaction_seconds` (histogram),
//!   `loams_house_compaction_files_rewritten_total`,
//!   `loams_house_compaction_bytes_rewritten_total`.
//! * Pipes and views (ruling R23.2): `loams_house_pipe_lag_records{ns,pipe}`,
//!   `loams_house_pipe_committed_records_total{ns,pipe}`,
//!   `loams_house_pipe_committed_rows_total{ns,pipe}`,
//!   `loams_house_pipe_errors{ns,pipe}`.
//! * Quotas and usage (HS1 Task 22, R22.8): `loams_house_quota_refusals_total{quota}`
//!   and `loams_house_usage_events_total{event}`.
//! * The query log: `loams_house_query_log_rows_total{result}` (`written`,
//!   `dropped`) and `loams_house_query_log_pending_rows`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::admission::{Admission, WorkerPool};
use crate::catalog::Lake;
use crate::watchdog::ExitReason;

/// The `Content-Type` of the text exposition format.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// The most a request head to the admin listener may be.
const MAX_HEAD: usize = 8 * 1024;

/// Seconds buckets of statements, commits and compactions.
pub const SECONDS_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0,
];
/// Seconds buckets of a worker's boot (HS1 R1.12: 0.2 to 2 s measured).
pub const BOOT_BUCKETS: &[f64] = &[0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0];

/// A histogram of durations, cumulative as Prometheus wants it.
#[derive(Debug)]
pub struct Histogram {
    bounds: &'static [f64],
    buckets: Vec<AtomicU64>,
    count: AtomicU64,
    sum_micros: AtomicU64,
}

/// A histogram's values at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistogramSnapshot {
    /// The upper bounds, in seconds, as text.
    pub bounds: Vec<String>,
    /// Observations at or under each bound (not cumulative).
    pub buckets: Vec<u64>,
    pub count: u64,
    /// The sum, in microseconds.
    pub sum_micros: u64,
}

impl Histogram {
    /// An empty histogram over `bounds` (seconds, ascending).
    pub fn new(bounds: &'static [f64]) -> Self {
        Self {
            bounds,
            buckets: bounds.iter().map(|_| AtomicU64::new(0)).collect(),
            count: AtomicU64::new(0),
            sum_micros: AtomicU64::new(0),
        }
    }

    /// Counts one duration.
    pub fn observe(&self, took: Duration) {
        let seconds = took.as_secs_f64();
        if let Some(at) = self.bounds.iter().position(|b| seconds <= *b) {
            self.buckets[at].fetch_add(1, Ordering::Relaxed);
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_micros.fetch_add(
            u64::try_from(took.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    /// Its values now.
    pub fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            bounds: self.bounds.iter().map(|b| b.to_string()).collect(),
            buckets: self
                .buckets
                .iter()
                .map(|b| b.load(Ordering::Relaxed))
                .collect(),
            count: self.count.load(Ordering::Relaxed),
            sum_micros: self.sum_micros.load(Ordering::Relaxed),
        }
    }
}

/// One statement, as the metrics count it.
#[derive(Clone, Debug, Default)]
pub struct StatementSample<'a> {
    pub namespace: u64,
    /// Its kind (`Query`, `Insert`, …; `Unparsed`).
    pub kind: &'a str,
    /// `ok`, `error` or `cancelled`.
    pub status: &'static str,
    pub took: Duration,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub result_bytes: u64,
}

#[derive(Debug)]
struct NamespaceSeries {
    queries: BTreeMap<(String, &'static str), u64>,
    seconds: Histogram,
    read_bytes: u64,
    written_bytes: u64,
    result_bytes: u64,
}

impl Default for NamespaceSeries {
    fn default() -> Self {
        Self {
            queries: BTreeMap::new(),
            seconds: Histogram::new(SECONDS_BUCKETS),
            read_bytes: 0,
            written_bytes: 0,
            result_bytes: 0,
        }
    }
}

/// The front's counters that no other part keeps (§49 §15). One per House,
/// shared by its surfaces, its commits and its compactions.
#[derive(Debug)]
pub struct Metrics {
    namespace_labels: std::sync::atomic::AtomicBool,
    statements: Mutex<BTreeMap<Option<u64>, NamespaceSeries>>,
    commit_seconds: Histogram,
    commits: Mutex<BTreeMap<&'static str, u64>>,
    commit_conflicts: AtomicU64,
    compaction_seconds: Histogram,
    compactions: Mutex<BTreeMap<&'static str, u64>>,
    compaction_files: AtomicU64,
    compaction_bytes: AtomicU64,
    quota_refusals: Mutex<BTreeMap<String, u64>>,
    usage_events: Mutex<BTreeMap<&'static str, u64>>,
    query_log_rows: Mutex<BTreeMap<&'static str, u64>>,
    query_log_pending: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new(false)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Metrics {
    /// Counters with a `ns` label on the statement series when
    /// `namespace_labels` (the operator's switch, §49 §15).
    pub fn new(namespace_labels: bool) -> Self {
        Self {
            namespace_labels: std::sync::atomic::AtomicBool::new(namespace_labels),
            statements: Mutex::new(BTreeMap::new()),
            commit_seconds: Histogram::new(SECONDS_BUCKETS),
            commits: Mutex::new(BTreeMap::new()),
            commit_conflicts: AtomicU64::new(0),
            compaction_seconds: Histogram::new(SECONDS_BUCKETS),
            compactions: Mutex::new(BTreeMap::new()),
            compaction_files: AtomicU64::new(0),
            compaction_bytes: AtomicU64::new(0),
            quota_refusals: Mutex::new(BTreeMap::new()),
            usage_events: Mutex::new(BTreeMap::new()),
            query_log_rows: Mutex::new(BTreeMap::new()),
            query_log_pending: AtomicU64::new(0),
        }
    }

    /// Whether the statement series carry `ns`.
    pub fn namespace_labels(&self) -> bool {
        self.namespace_labels.load(Ordering::Relaxed)
    }

    /// Turns the statement series' `ns` label on or off (the House's config,
    /// set when it starts serving; series counted before keep their labels).
    pub fn set_namespace_labels(&self, on: bool) {
        self.namespace_labels.store(on, Ordering::Relaxed);
    }

    /// A statement ended.
    pub fn statement(&self, sample: &StatementSample<'_>) {
        let key = self.namespace_labels().then_some(sample.namespace);
        let mut all = lock(&self.statements);
        let series = all.entry(key).or_default();
        *series
            .queries
            .entry((sample.kind.to_string(), sample.status))
            .or_default() += 1;
        series.seconds.observe(sample.took);
        series.read_bytes += sample.read_bytes;
        series.written_bytes += sample.written_bytes;
        series.result_bytes += sample.result_bytes;
    }

    /// A batch's catalog commit ended: `committed`, `conflict` (it is tried
    /// again on the new snapshot) or `failed`.
    pub fn commit(&self, result: &'static str, took: Duration) {
        self.commit_seconds.observe(took);
        *lock(&self.commits).entry(result).or_default() += 1;
        if result == "conflict" {
            self.commit_conflicts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A compaction ended: `committed`, `abandoned` (another commit came
    /// first), `nothing` or `failed`, having rewritten `files` of `bytes`.
    pub fn compaction(&self, result: &'static str, took: Duration, files: u64, bytes: u64) {
        self.compaction_seconds.observe(took);
        *lock(&self.compactions).entry(result).or_default() += 1;
        self.compaction_files.fetch_add(files, Ordering::Relaxed);
        self.compaction_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// A statement was refused by `quota` (HS1 Task 22).
    pub fn quota_refusal(&self, quota: &str) {
        *lock(&self.quota_refusals)
            .entry(quota.to_string())
            .or_default() += 1;
    }

    /// A usage event was handed on (`house.query`, …).
    pub fn usage_event(&self, event: &'static str) {
        *lock(&self.usage_events).entry(event).or_default() += 1;
    }

    /// Query log rows `written` to the lake, or `dropped`.
    pub fn query_log_rows(&self, result: &'static str, rows: u64) {
        *lock(&self.query_log_rows).entry(result).or_default() += rows;
    }

    /// Query log rows waiting for their flush.
    pub fn set_query_log_pending(&self, rows: u64) {
        self.query_log_pending.store(rows, Ordering::Relaxed);
    }

    /// Commits by result so far (tests).
    pub fn commits(&self) -> BTreeMap<&'static str, u64> {
        lock(&self.commits).clone()
    }
}

/// What `house-cache` has counted (HS1 Task 9), for the metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheCounters {
    /// Blocks served from the cache.
    pub hits: u64,
    /// Blocks fetched from the store.
    pub misses: u64,
    /// Body bytes served to workers.
    pub bytes: u64,
    /// Requests answered.
    pub requests: u64,
    /// Requests refused (`403`).
    pub refused: u64,
}

/// `house-cache`'s side of the metrics (it lives in another crate).
pub trait CacheMetrics: Send + Sync + std::fmt::Debug {
    fn cache_counters(&self) -> CacheCounters;
}

/// A label value, escaped as the exposition format asks.
fn label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn family(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

/// `{a="x",b="y"}`, or nothing for no labels.
fn labels(pairs: &[(&str, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", label(v)))
        .collect();
    format!("{{{}}}", inner.join(","))
}

fn histogram(out: &mut String, name: &str, pairs: &[(&str, String)], h: &HistogramSnapshot) {
    let mut cumulative = 0;
    for (bound, n) in h.bounds.iter().zip(&h.buckets) {
        cumulative += n;
        let mut with = pairs.to_vec();
        with.push(("le", bound.clone()));
        let _ = writeln!(out, "{name}_bucket{} {cumulative}", labels(&with));
    }
    let mut with = pairs.to_vec();
    with.push(("le", "+Inf".to_string()));
    let _ = writeln!(out, "{name}_bucket{} {}", labels(&with), h.count);
    let _ = writeln!(
        out,
        "{name}_sum{} {}",
        labels(pairs),
        h.sum_micros as f64 / 1e6
    );
    let _ = writeln!(out, "{name}_count{} {}", labels(pairs), h.count);
}

/// Everything `/metrics` reads.
#[derive(Clone, Debug)]
pub struct Sources {
    pub pool: WorkerPool,
    pub lake: Option<Arc<Lake>>,
    pub admission: Arc<Admission>,
    pub metrics: Arc<Metrics>,
    pub cache: Option<Arc<dyn CacheMetrics>>,
}

/// The metrics, in the text exposition format.
pub fn render(sources: &Sources) -> String {
    let mut out = String::new();
    let metrics = &sources.metrics;
    let admitted = sources.admission.stats();
    for (name, help, value) in [
        (
            "loams_house_queue_depth",
            "Statements waiting for admission (§49 §12), every namespace.",
            admitted.queued as u64,
        ),
        (
            "loams_house_admitted_queries",
            "Statements admitted and running, every namespace.",
            admitted.running as u64,
        ),
        (
            "loams_house_admitted_memory_bytes",
            "The admitted statements' max_memory_usage, summed.",
            admitted.memory,
        ),
    ] {
        family(&mut out, name, "gauge", help);
        let _ = writeln!(out, "{name} {value}");
    }

    // Statements.
    {
        let all = lock(&metrics.statements);
        let ns = |key: &Option<u64>| -> Vec<(&str, String)> {
            key.map(|n| vec![("ns", n.to_string())]).unwrap_or_default()
        };
        family(
            &mut out,
            "loams_house_queries_total",
            "counter",
            "Statements ended, by kind and status (ok, error, cancelled).",
        );
        for (key, series) in all.iter() {
            for ((kind, status), n) in &series.queries {
                let mut pairs = ns(key);
                pairs.push(("kind", kind.clone()));
                pairs.push(("status", status.to_string()));
                let _ = writeln!(out, "loams_house_queries_total{} {n}", labels(&pairs));
            }
        }
        family(
            &mut out,
            "loams_house_query_seconds",
            "histogram",
            "How long statements took, from their request to their last byte.",
        );
        for (key, series) in all.iter() {
            histogram(
                &mut out,
                "loams_house_query_seconds",
                &ns(key),
                &series.seconds.snapshot(),
            );
        }
        for (name, help, value) in [
            (
                "loams_house_read_bytes_total",
                "Bytes statements read from storage.",
                (|s: &NamespaceSeries| s.read_bytes) as fn(&NamespaceSeries) -> u64,
            ),
            (
                "loams_house_written_bytes_total",
                "Bytes statements wrote.",
                |s: &NamespaceSeries| s.written_bytes,
            ),
            (
                "loams_house_result_bytes_total",
                "Bytes of statements' results.",
                |s: &NamespaceSeries| s.result_bytes,
            ),
        ] {
            family(&mut out, name, "counter", help);
            if all.is_empty() && !metrics.namespace_labels() {
                let _ = writeln!(out, "{name} 0");
            }
            for (key, series) in all.iter() {
                let _ = writeln!(out, "{name}{} {}", labels(&ns(key)), value(series));
            }
        }
    }

    // Workers.
    let stats = sources.pool.stats();
    family(
        &mut out,
        "loams_house_workers",
        "gauge",
        "Workers of this front's pool, by state.",
    );
    for (state, n) in [
        ("idle", stats.idle_unbound),
        ("bound", stats.idle_bound),
        ("leased", stats.leased),
        ("booting", stats.booting),
        ("pinned", stats.pinned),
    ] {
        let _ = writeln!(out, "loams_house_workers{{state=\"{state}\"}} {n}");
    }
    family(
        &mut out,
        "loams_house_worker_kills_total",
        "counter",
        "Workers retired, by reason.",
    );
    for reason in ExitReason::ALL {
        let _ = writeln!(
            out,
            "loams_house_worker_kills_total{{reason=\"{}\"}} {}",
            reason.as_str(),
            stats.kills_for(reason)
        );
    }
    family(
        &mut out,
        "loams_house_worker_boot_seconds",
        "histogram",
        "How long a worker took from its start to its Ready.",
    );
    histogram(
        &mut out,
        "loams_house_worker_boot_seconds",
        &[],
        &stats.boot_seconds,
    );

    // house-cache.
    let cache = sources
        .cache
        .as_ref()
        .map(|c| c.cache_counters())
        .unwrap_or_default();
    for (name, help, value) in [
        (
            "loams_house_cache_hits_total",
            "Blocks house-cache served from its cache.",
            cache.hits,
        ),
        (
            "loams_house_cache_misses_total",
            "Blocks house-cache fetched from the store.",
            cache.misses,
        ),
        (
            "loams_house_cache_bytes_total",
            "Body bytes house-cache served to workers.",
            cache.bytes,
        ),
        (
            "loams_house_cache_requests_total",
            "Requests house-cache answered.",
            cache.requests,
        ),
        (
            "loams_house_cache_refused_total",
            "Requests house-cache refused (403).",
            cache.refused,
        ),
    ] {
        family(&mut out, name, "counter", help);
        let _ = writeln!(out, "{name} {value}");
    }

    // Commits.
    family(
        &mut out,
        "loams_house_commit_seconds",
        "histogram",
        "How long a batch's catalog commit took (each attempt).",
    );
    histogram(
        &mut out,
        "loams_house_commit_seconds",
        &[],
        &metrics.commit_seconds.snapshot(),
    );
    family(
        &mut out,
        "loams_house_commits_total",
        "counter",
        "Catalog commits of batches, by result (committed, conflict, failed).",
    );
    {
        let commits = lock(&metrics.commits);
        for result in ["committed", "conflict", "failed"] {
            let _ = writeln!(
                out,
                "loams_house_commits_total{{result=\"{result}\"}} {}",
                commits.get(result).copied().unwrap_or(0)
            );
        }
    }
    family(
        &mut out,
        "loams_house_commit_conflicts_total",
        "counter",
        "Catalog commits that lost to another commit and were tried again.",
    );
    let _ = writeln!(
        out,
        "loams_house_commit_conflicts_total {}",
        metrics.commit_conflicts.load(Ordering::Relaxed)
    );

    // Compaction.
    family(
        &mut out,
        "loams_house_compaction_runs_total",
        "counter",
        "Compactions, by result (committed, abandoned, nothing, failed).",
    );
    {
        let runs = lock(&metrics.compactions);
        for result in ["committed", "abandoned", "nothing", "failed"] {
            let _ = writeln!(
                out,
                "loams_house_compaction_runs_total{{result=\"{result}\"}} {}",
                runs.get(result).copied().unwrap_or(0)
            );
        }
    }
    family(
        &mut out,
        "loams_house_compaction_seconds",
        "histogram",
        "How long a compaction took.",
    );
    histogram(
        &mut out,
        "loams_house_compaction_seconds",
        &[],
        &metrics.compaction_seconds.snapshot(),
    );
    for (name, help, value) in [
        (
            "loams_house_compaction_files_rewritten_total",
            "Data files compactions replaced.",
            metrics.compaction_files.load(Ordering::Relaxed),
        ),
        (
            "loams_house_compaction_bytes_rewritten_total",
            "Bytes of the data files compactions replaced.",
            metrics.compaction_bytes.load(Ordering::Relaxed),
        ),
    ] {
        family(&mut out, name, "counter", help);
        let _ = writeln!(out, "{name} {value}");
    }

    // Quotas and usage (R22.8).
    family(
        &mut out,
        "loams_house_quota_refusals_total",
        "counter",
        "Statements refused by a namespace quota, by quota.",
    );
    for (quota, n) in lock(&metrics.quota_refusals).iter() {
        let _ = writeln!(
            out,
            "loams_house_quota_refusals_total{{quota=\"{}\"}} {n}",
            label(quota)
        );
    }
    family(
        &mut out,
        "loams_house_usage_events_total",
        "counter",
        "Usage events handed on, by event.",
    );
    {
        let events = lock(&metrics.usage_events);
        for event in [
            "house.query",
            "house.insert",
            "house.storage",
            "house.compaction",
        ] {
            let _ = writeln!(
                out,
                "loams_house_usage_events_total{{event=\"{event}\"}} {}",
                events.get(event).copied().unwrap_or(0)
            );
        }
    }

    // The query log.
    family(
        &mut out,
        "loams_house_query_log_rows_total",
        "counter",
        "Query log rows written to the namespaces' _house.query_log, or dropped.",
    );
    {
        let rows = lock(&metrics.query_log_rows);
        for result in ["written", "dropped"] {
            let _ = writeln!(
                out,
                "loams_house_query_log_rows_total{{result=\"{result}\"}} {}",
                rows.get(result).copied().unwrap_or(0)
            );
        }
    }
    family(
        &mut out,
        "loams_house_query_log_pending_rows",
        "gauge",
        "Query log rows waiting for their group commit.",
    );
    let _ = writeln!(
        out,
        "loams_house_query_log_pending_rows {}",
        metrics.query_log_pending.load(Ordering::Relaxed)
    );

    // Pipes and views.
    let states = sources
        .lake
        .as_ref()
        .map(|lake| lake.pipe_states().all())
        .unwrap_or_default();
    let series = |out: &mut String,
                  name: &str,
                  kind: &str,
                  help: &str,
                  value: &dyn Fn(&crate::pipes::PipeState) -> u64| {
        family(out, name, kind, help);
        for ((namespace, database, view), state) in &states {
            let _ = writeln!(
                out,
                "{name}{{ns=\"{namespace}\",pipe=\"{}\"}} {}",
                label(&format!("{database}.{view}")),
                value(state)
            );
        }
    };
    series(
        &mut out,
        "loams_house_pipe_lag_records",
        "gauge",
        "Records (objects, for S3Queue) a materialized view is behind its source, at this \
         front's last look.",
        &|s| s.lag_records,
    );
    series(
        &mut out,
        "loams_house_pipe_committed_records_total",
        "counter",
        "Records (objects, for S3Queue) this front committed for a materialized view.",
        &|s| s.committed_records,
    );
    series(
        &mut out,
        "loams_house_pipe_committed_rows_total",
        "counter",
        "Rows this front committed into a materialized view's target.",
        &|s| s.committed_rows,
    );
    series(
        &mut out,
        "loams_house_pipe_errors",
        "gauge",
        "1 while a materialized view's last run on this front failed.",
        &|s| u64::from(!s.last_error.is_empty()),
    );
    out
}

/// Serves `GET /metrics` on `listener` until the returned task is aborted.
pub fn serve(listener: TcpListener, sources: Sources) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            let sources = sources.clone();
            tokio::spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_secs(10), answer(stream, &sources)).await;
            });
        }
    })
}

async fn answer(mut stream: TcpStream, sources: &Sources) -> io::Result<()> {
    let mut head = Vec::with_capacity(512);
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
        if head.len() > MAX_HEAD {
            return respond(
                &mut stream,
                "431 Request Header Fields Too Large",
                "text/plain",
                "",
            )
            .await;
        }
    }
    let line = head.split(|&b| b == b'\r').next().unwrap_or_default();
    let line = String::from_utf8_lossy(line);
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = path.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/metrics") => respond(&mut stream, "200 OK", CONTENT_TYPE, &render(sources)).await,
        ("GET", _) => respond(&mut stream, "404 Not Found", "text/plain", "not found\n").await,
        _ => {
            respond(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain",
                "GET only\n",
            )
            .await
        }
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_escaped() {
        assert_eq!(label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn histograms_are_cumulative() {
        let h = Histogram::new(&[0.1, 1.0]);
        h.observe(Duration::from_millis(50));
        h.observe(Duration::from_millis(500));
        h.observe(Duration::from_secs(5));
        let mut out = String::new();
        histogram(&mut out, "x", &[("ns", "2".to_string())], &h.snapshot());
        assert_eq!(
            out,
            "x_bucket{ns=\"2\",le=\"0.1\"} 1\nx_bucket{ns=\"2\",le=\"1\"} 2\n\
             x_bucket{ns=\"2\",le=\"+Inf\"} 3\nx_sum{ns=\"2\"} 5.55\nx_count{ns=\"2\"} 3\n"
        );
    }

    #[test]
    fn statements_carry_ns_only_when_asked() {
        for (on, want) in [(false, None), (true, Some(7))] {
            let metrics = Metrics::new(on);
            metrics.statement(&StatementSample {
                namespace: 7,
                kind: "Query",
                status: "ok",
                took: Duration::from_millis(3),
                read_bytes: 10,
                ..StatementSample::default()
            });
            let all = lock(&metrics.statements);
            assert_eq!(all.keys().copied().collect::<Vec<_>>(), vec![want]);
            assert_eq!(all[&want].read_bytes, 10);
        }
    }
}
