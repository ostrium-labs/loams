//! Usage events (HS1 Task 22, design §49 §15, §27).
//!
//! The front hands every namespace's usage to a [`UsageObserver`] as a
//! [`UsageEvent`], a plain struct passed to a method call, as §27 §3.7's
//! `InvocationObserver` does. It has no wire format (no serialiser is derived
//! here), and this module opens no socket or file, keeps no buffer of events
//! and persists nothing: an event a front dies before delivering is lost. This
//! is open observability (D548); what turns events into records outside this
//! repository links its own observer, and the default ([`NoUsageObserver`])
//! does nothing.
//!
//! | Event | When | What |
//! |---|---|---|
//! | `house.query` | a worker lease ends that ran statements: one per client statement, and one per internal lease (a DDL's describe, a pipe's batch) | CPU time (measured from outside the worker), bytes and rows scanned, result bytes and rows, worker time, the ClickHouse code it ended with |
//! | `house.insert` | an `INSERT` into a lake table (HTTP, the API, pipes, materialized views) wrote rows | rows, bytes as the worker produced them |
//! | `house.storage` | every [`StorageReporter`] round (daily by default), per lake table | the bytes of its current snapshot's data files |
//! | `house.compaction` | a compaction's worker lease ends (background or `OPTIMIZE`) | CPU time, bytes scanned, worker time |
//!
//! **CPU.** The front reads a worker's CPU time itself, before and after each
//! statement: the worker's cgroup `cpu.stat` (`usage_usec`) when it has one,
//! else `/proc/<pid>/stat`. The worker never reports its own figure, so a
//! tenant's code cannot change it. An in-process worker (`--workers=inproc`)
//! shares the front's process and is not measured: its `cpu` is `None`.
//!
//! The same events feed the quotas' daily counters ([`crate::quota`]): the
//! [`Usage`] hub counts each one before it hands it on.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::catalog::Lake;
use crate::quota::Quotas;

/// How often the [`StorageReporter`] measures every lake table by default.
pub const DEFAULT_STORAGE_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// One statement's (or one lease's statements') usage: `house.query`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryUsage {
    pub namespace: String,
    /// The first statement's query id.
    pub query_id: String,
    /// The worker's CPU time; `None` when it could not be measured.
    pub cpu: Option<Duration>,
    /// Bytes read from storage.
    pub bytes_scanned: u64,
    /// Rows read from storage.
    pub rows_read: u64,
    /// Bytes of the result.
    pub result_bytes: u64,
    /// Rows of the result.
    pub result_rows: u64,
    /// How long the worker ran the statements (from `Execute` to their end).
    pub worker_time: Duration,
    /// Statements run.
    pub statements: u32,
    /// The ClickHouse code the last statement ended with; `0` for success.
    pub code: i32,
    pub at: SystemTime,
}

impl Default for QueryUsage {
    fn default() -> Self {
        Self {
            namespace: String::new(),
            query_id: String::new(),
            cpu: None,
            bytes_scanned: 0,
            rows_read: 0,
            result_bytes: 0,
            result_rows: 0,
            worker_time: Duration::ZERO,
            statements: 0,
            code: 0,
            at: SystemTime::UNIX_EPOCH,
        }
    }
}

/// An `INSERT` into a lake table: `house.insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertUsage {
    pub namespace: String,
    pub query_id: String,
    pub database: String,
    pub table: String,
    /// Rows written (a deduplicated retry writes none and sends no event).
    pub rows: u64,
    /// Their bytes as the worker produced them.
    pub bytes: u64,
    pub at: SystemTime,
}

impl Default for InsertUsage {
    fn default() -> Self {
        Self {
            namespace: String::new(),
            query_id: String::new(),
            database: String::new(),
            table: String::new(),
            rows: 0,
            bytes: 0,
            at: SystemTime::UNIX_EPOCH,
        }
    }
}

/// One lake table's bytes on one day: `house.storage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageUsage {
    pub namespace: String,
    pub database: String,
    pub table: String,
    /// The data files of its current snapshot (`total-files-size`).
    pub bytes: u64,
    /// The UTC day measured, in days since the epoch.
    pub day: u64,
    pub at: SystemTime,
}

/// A compaction's worker time: `house.compaction`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionUsage {
    pub namespace: String,
    pub database: String,
    pub table: String,
    /// The worker's CPU time; `None` when it could not be measured.
    pub cpu: Option<Duration>,
    pub bytes_scanned: u64,
    pub worker_time: Duration,
    pub at: SystemTime,
}

/// One usage event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageEvent {
    Query(QueryUsage),
    Insert(InsertUsage),
    Storage(StorageUsage),
    Compaction(CompactionUsage),
}

impl UsageEvent {
    /// Its name (§49 §15): `house.query`, `house.insert`, `house.storage`,
    /// `house.compaction`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Query(_) => "house.query",
            Self::Insert(_) => "house.insert",
            Self::Storage(_) => "house.storage",
            Self::Compaction(_) => "house.compaction",
        }
    }

    /// The namespace it is for.
    pub fn namespace(&self) -> &str {
        match self {
            Self::Query(e) => &e.namespace,
            Self::Insert(e) => &e.namespace,
            Self::Storage(e) => &e.namespace,
            Self::Compaction(e) => &e.namespace,
        }
    }

    /// When it happened.
    pub fn at(&self) -> SystemTime {
        match self {
            Self::Query(e) => e.at,
            Self::Insert(e) => e.at,
            Self::Storage(e) => e.at,
            Self::Compaction(e) => e.at,
        }
    }
}

/// Receives every usage event. Called on the task that ended the work (a
/// lease's drop, an insert's answer, the storage round): it must return
/// quickly and must not block.
pub trait UsageObserver: Send + Sync + std::fmt::Debug {
    fn on_usage(&self, event: &UsageEvent);
}

/// Observes nothing: the open-source default.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoUsageObserver;

impl UsageObserver for NoUsageObserver {
    fn on_usage(&self, _event: &UsageEvent) {}
}

/// Keeps every event in memory (tests).
#[derive(Debug, Default)]
pub struct Recorder(Mutex<Vec<UsageEvent>>);

impl Recorder {
    /// Everything observed so far.
    pub fn events(&self) -> Vec<UsageEvent> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl UsageObserver for Recorder {
    fn on_usage(&self, event: &UsageEvent) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

/// The front's usage hub: counts each event against the quotas, then hands it
/// to the observer.
#[derive(Debug)]
pub struct Usage {
    observer: Arc<dyn UsageObserver>,
    quotas: Arc<Quotas>,
    /// Counts each event by name (`loams_house_usage_events_total`, HS1
    /// Task 23).
    metrics: Option<Arc<crate::metrics::Metrics>>,
}

impl Usage {
    /// A hub over `observer` and `quotas`.
    pub fn new(observer: Arc<dyn UsageObserver>, quotas: Arc<Quotas>) -> Arc<Self> {
        Arc::new(Self {
            observer,
            quotas,
            metrics: None,
        })
    }

    /// A hub that also counts each event in `metrics` (HS1 Task 23).
    pub fn with_metrics(
        observer: Arc<dyn UsageObserver>,
        quotas: Arc<Quotas>,
        metrics: Arc<crate::metrics::Metrics>,
    ) -> Arc<Self> {
        Arc::new(Self {
            observer,
            quotas,
            metrics: Some(metrics),
        })
    }

    /// Its quotas.
    pub fn quotas(&self) -> &Arc<Quotas> {
        &self.quotas
    }

    /// Counts and hands on one event.
    pub fn emit(&self, event: UsageEvent) {
        self.quotas.account(&event, event.at());
        if let Some(metrics) = &self.metrics {
            metrics.usage_event(event.kind());
        }
        self.observer.on_usage(&event);
    }

    /// A worker lease ended after running `measured`'s statements.
    pub fn lease_ended(&self, namespace: &str, measured: &Measured) {
        if measured.statements == 0 {
            return;
        }
        let at = SystemTime::now();
        let event = match &measured.purpose {
            Purpose::Query => UsageEvent::Query(QueryUsage {
                namespace: namespace.to_string(),
                query_id: measured.query_id.clone(),
                cpu: measured.cpu,
                bytes_scanned: measured.bytes_scanned,
                rows_read: measured.rows_read,
                result_bytes: measured.result_bytes,
                result_rows: measured.result_rows,
                worker_time: measured.worker_time,
                statements: measured.statements,
                code: measured.code,
                at,
            }),
            Purpose::Compaction { database, table } => UsageEvent::Compaction(CompactionUsage {
                namespace: namespace.to_string(),
                database: database.clone(),
                table: table.clone(),
                cpu: measured.cpu,
                bytes_scanned: measured.bytes_scanned,
                worker_time: measured.worker_time,
                at,
            }),
        };
        self.emit(event);
    }
}

/// What a lease's statements are for, which names their event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    /// A client's statement, or the House's own on its behalf: `house.query`.
    #[default]
    Query,
    /// Compacting a table: `house.compaction`.
    Compaction { database: String, table: String },
}

/// What a lease measured of its statements (the pool's [`WorkerLease`]
/// keeps it; [`Usage::lease_ended`] reports it).
///
/// [`WorkerLease`]: crate::admission::WorkerLease
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    pub purpose: Purpose,
    /// The first statement's query id.
    pub query_id: String,
    pub statements: u32,
    /// CPU time summed over the statements; `None` once one could not be
    /// measured at all.
    pub cpu: Option<Duration>,
    pub bytes_scanned: u64,
    pub rows_read: u64,
    pub result_bytes: u64,
    pub result_rows: u64,
    pub worker_time: Duration,
    pub code: i32,
}

/// Measures every lake table's bytes and sends one `house.storage` per table,
/// and each namespace's total to the storage quota.
#[derive(Debug)]
pub struct StorageReporter {
    lake: Arc<Lake>,
    usage: Arc<Usage>,
}

impl StorageReporter {
    /// A reporter over `lake`'s tables.
    pub fn new(lake: Arc<Lake>, usage: Arc<Usage>) -> Self {
        Self { lake, usage }
    }

    /// Runs a round now and every `every` after, until the task is aborted.
    pub fn spawn(self, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                self.round(SystemTime::now()).await;
            }
        })
    }

    /// One round as of `at`: every table of every namespace. A namespace that
    /// cannot be listed is skipped (and keeps its last total), with a line on
    /// stderr.
    pub async fn round(&self, at: SystemTime) {
        let namespaces = match self.lake.namespaces().await {
            Ok(namespaces) => namespaces,
            Err(err) => {
                eprintln!(
                    "loams-house: the storage round cannot list namespaces: {}",
                    err.message()
                );
                return;
            }
        };
        for namespace in namespaces {
            match self.namespace(namespace, at).await {
                Ok(total) => self.usage.quotas().stored(&namespace.to_string(), total),
                Err(err) => eprintln!(
                    "loams-house: the storage round of namespace {namespace} failed: {}",
                    err.message()
                ),
            }
        }
    }

    async fn namespace(
        &self,
        namespace: u64,
        at: SystemTime,
    ) -> Result<u64, crate::errors::HouseError> {
        let day = crate::quota::utc_day(at);
        let mut measured = Vec::new();
        for database in self.lake.databases(namespace).await? {
            for name in self.lake.table_names(namespace, &database.name).await? {
                let Some(table) = self.lake.load(namespace, &database.name, &name).await? else {
                    continue;
                };
                // An external table's files are not the namespace's.
                if crate::ddl::class_of(&table.metadata) == crate::ddl::TableClass::External {
                    continue;
                }
                let bytes = table
                    .metadata
                    .current_snapshot()
                    .and_then(|s| {
                        s.summary()
                            .additional_properties
                            .get("total-files-size")
                            .and_then(|n| n.parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                measured.push((database.name.clone(), name, bytes));
            }
        }
        let mut total = 0u64;
        for (database, table, bytes) in measured {
            total = total.saturating_add(bytes);
            self.usage.emit(UsageEvent::Storage(StorageUsage {
                namespace: namespace.to_string(),
                database,
                table,
                bytes,
                day,
                at,
            }));
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_name_themselves_as_the_plan_does() {
        let query = UsageEvent::Query(QueryUsage::default());
        let insert = UsageEvent::Insert(InsertUsage::default());
        assert_eq!(query.kind(), "house.query");
        assert_eq!(insert.kind(), "house.insert");
        let storage = UsageEvent::Storage(StorageUsage {
            namespace: "2".to_string(),
            database: "default".to_string(),
            table: "t".to_string(),
            bytes: 1,
            day: 1,
            at: SystemTime::UNIX_EPOCH,
        });
        assert_eq!(storage.kind(), "house.storage");
        assert_eq!(storage.namespace(), "2");
    }

    #[test]
    fn a_lease_reports_once_by_purpose_and_feeds_the_quotas() {
        let recorder = Arc::new(Recorder::default());
        let quotas = Quotas::unlimited();
        let usage = Usage::new(recorder.clone(), quotas.clone());
        usage.lease_ended("2", &Measured::default());
        assert!(recorder.events().is_empty(), "no statement, no event");
        usage.lease_ended(
            "2",
            &Measured {
                query_id: "q1".to_string(),
                statements: 1,
                cpu: Some(Duration::from_millis(20)),
                bytes_scanned: 800,
                ..Measured::default()
            },
        );
        usage.lease_ended(
            "2",
            &Measured {
                purpose: Purpose::Compaction {
                    database: "default".to_string(),
                    table: "t".to_string(),
                },
                statements: 2,
                cpu: Some(Duration::from_millis(5)),
                bytes_scanned: 50,
                ..Measured::default()
            },
        );
        let events = recorder.events();
        let kinds: Vec<&str> = events.iter().map(UsageEvent::kind).collect();
        assert_eq!(kinds, ["house.query", "house.compaction"]);
        // The quotas count queries, not the House's own compaction.
        let used = quotas.usage("2", std::time::Instant::now(), SystemTime::now());
        assert_eq!(used.scanned_today, 800);
        assert_eq!(used.cpu_today, Duration::from_millis(20));
    }

    /// The front's CPU readings: `/proc/<pid>/stat`'s `utime + stime` after a
    /// command name with spaces and parentheses, and a cgroup's `usage_usec`.
    #[test]
    fn cpu_is_read_from_outside_the_worker() {
        let stat = "1234 (a b) c) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 9 0 100";
        assert_eq!(
            crate::watchdog::proc_stat_cpu(stat),
            Some(Duration::from_millis(3_000))
        );
        assert_eq!(crate::watchdog::proc_stat_cpu("garbage"), None);
        let own = std::fs::read_to_string("/proc/self/stat").expect("own stat");
        assert!(crate::watchdog::proc_stat_cpu(&own).is_some(), "{own}");
        let dir = std::env::temp_dir().join(format!("loams-cpu-stat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("cpu.stat"),
            "usage_usec 1500\nuser_usec 1000\nsystem_usec 500\n",
        )
        .expect("write");
        assert_eq!(
            crate::watchdog::cgroup_cpu(&dir),
            Some(Duration::from_micros(1_500))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The event types carry no serialiser: they are handed over, not sent.
    #[test]
    fn usage_events_have_no_wire_format() {
        let text = include_str!("usage.rs");
        let derive = ["Serial", "ize"].concat();
        let serde = ["se", "rde"].concat();
        assert!(!text.contains(&derive), "no {derive} derive in usage.rs");
        assert!(
            !text.contains(&format!("use {serde}")),
            "no {serde} in usage.rs"
        );
    }
}
