//! The query log (HS1 Task 23, design §49 §15): every client statement's row
//! in its namespace's own lake table, `_house.query_log`, in the tenant's
//! bucket.
//!
//! * **What is logged.** One row per statement a client sent, on either
//!   surface (ClickHouse HTTP and `loams.house.v1`), whatever it was and however
//!   it ended once its namespace was known: query id, House user and Loams
//!   principal, start and end, kind, current database, a normalized-query hash
//!   (of the text with its literals redacted, [`crate::oplog::redact`]), the
//!   full text, the ClickHouse code and message it ended with, rows and bytes
//!   read and written, result rows and bytes, the worker's peak memory and the
//!   worker's CPU time. The House's own work (pipes, views, compaction, this
//!   log's own writes) is not logged.
//! * **Writes.** The front keeps rows in memory and writes each namespace's
//!   as one group commit every [`DEFAULT_FLUSH_INTERVAL`]: Parquet files
//!   written by the front itself (`loams_house_lake::write::DataWriter`, no
//!   worker), committed through the table's commit queue like any `INSERT`. A
//!   front that dies loses at most its unflushed rows: it is a log, not a
//!   ledger. Past [`MAX_PENDING_ROWS`] waiting (a bucket that is down), the
//!   oldest are dropped and counted.
//! * **The table.** `_house.query_log`, made at the first flush: partitioned by
//!   `event_date`, sorted by `event_time`. `_house` is the House's: it is never
//!   listed, and no client statement may name it (`497`); clients read it as
//!   `system.query_log` ([`crate::system`]), `ListQueryHistory` and `GetQuery`.
//! * **Who sees what.** A user sees their own rows; `house:admin` sees the
//!   namespace's (as `ListQueryHistory`'s `NAMESPACE` scope). Rows not flushed
//!   yet are shown too, from this front's memory.
//! * **Retention.** [`DEFAULT_RETENTION`] (30 days), or the table's own
//!   [`RETENTION_PROPERTY`] (days) set through the catalog; whole days past it
//!   are removed by a `delete` snapshot ([`QueryLog::retention_round`]), and
//!   snapshot expiry ([`crate::retention`]) then deletes their files.
//!
//! [`Observer`] is the front's per-statement hub: one [`Statement`] per
//! statement, which on its end counts the metrics ([`crate::metrics`]), writes
//! the operator line ([`crate::oplog`]), ends the trace's root span
//! ([`crate::trace`]) and hands the row to the [`QueryLog`].

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arrow_array::builder::{
    Date32Builder, Int32Builder, Int64Builder, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use loams_house_ipc::{Execute, Limits, Output, Progress};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::admission::WorkerPool;
use crate::catalog::{CatalogError, Lake, LakeTable};
use crate::config::UserMap;
use crate::ddl::{self, quote_name, quote_string};
use crate::errors::{ChError, HouseError};
use crate::metrics::{Metrics, StatementSample};
use crate::oplog::{OperatorLine, OperatorLog};
use crate::trace::{OpenSpan, SpanContext, Tracer};

/// The House's own database in each namespace.
pub const HOUSE_DATABASE: &str = "_house";
/// The query log's table in it.
pub const QUERY_LOG_TABLE: &str = "query_log";
/// How often each namespace's rows are written (§49 §15).
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// How long rows are kept (§49 §15).
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// A namespace's own retention, in days: a property of its `_house.query_log`.
pub const RETENTION_PROPERTY: &str = "loams.query-log.retention-days";
/// How often old days are removed.
pub const RETENTION_SWEEP: Duration = Duration::from_secs(3600);
/// The most rows a front holds unflushed, every namespace together.
pub const MAX_PENDING_ROWS: usize = 100_000;
/// The most rows one history read returns.
pub const MAX_READ_ROWS: usize = 10_000;
/// ClickHouse's `QUERY_WAS_CANCELLED`.
pub const CANCELLED: i32 = 394;

/// The table's columns and their ClickHouse types, in order. Column names are
/// `system.query_log`'s where it has one.
pub const COLUMNS: &[(&str, &str)] = &[
    ("event_date", "Date"),
    ("event_time", "DateTime64(6, 'UTC')"),
    ("query_start_time", "DateTime64(6, 'UTC')"),
    ("query_duration_ms", "Int64"),
    ("type", "String"),
    ("query_id", "String"),
    ("user", "String"),
    ("loams_principal", "String"),
    ("loams_surface", "String"),
    ("query_kind", "String"),
    ("loams_kind", "String"),
    ("current_database", "String"),
    ("normalized_query_hash", "Int64"),
    ("query", "String"),
    ("exception_code", "Int32"),
    ("exception", "String"),
    ("read_rows", "Int64"),
    ("read_bytes", "Int64"),
    ("written_rows", "Int64"),
    ("written_bytes", "Int64"),
    ("result_rows", "Int64"),
    ("result_bytes", "Int64"),
    ("memory_usage", "Int64"),
    ("cpu_time_us", "Nullable(Int64)"),
];

/// One statement's row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// Its namespace; `None` until the caller was identified (such a
    /// statement is counted and logged for operators, never in a query log).
    pub namespace: Option<u64>,
    pub query_id: String,
    /// `http` or `api`.
    pub surface: String,
    /// The House user.
    pub user: String,
    /// The Loams principal the engine's link acted for, if any.
    pub principal: String,
    /// The House's statement kind (`Query`, `Insert`, `CreateTable`, …);
    /// `Unparsed` for one sqlparser could not read, empty before it was read.
    pub kind: String,
    pub database: String,
    /// The full text.
    pub query: String,
    /// Unix microseconds.
    pub started_us: i64,
    /// Unix microseconds.
    pub ended_us: i64,
    /// The ClickHouse code it ended with; `0` for success.
    pub code: i32,
    /// The error's message.
    pub exception: String,
    /// Whether it failed before a worker ran it.
    pub before_start: bool,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub written_rows: u64,
    pub written_bytes: u64,
    pub result_rows: u64,
    pub result_bytes: u64,
    /// The worker's peak resident set while it ran.
    pub memory_usage: u64,
    /// The worker's CPU time, measured by the front; `None` when it could not
    /// be (an in-process worker, or no worker).
    pub cpu_us: Option<u64>,
}

impl Entry {
    /// `system.query_log.type`.
    pub fn event_type(&self) -> &'static str {
        match (self.code, self.before_start) {
            (0, _) => "QueryFinish",
            (_, true) => "ExceptionBeforeStart",
            (_, false) => "ExceptionWhileProcessing",
        }
    }

    /// `system.query_log.query_kind` (ClickHouse's `IAST::QueryKind`).
    pub fn query_kind(&self) -> &'static str {
        clickhouse_kind(&self.kind)
    }

    /// `normalized_query_hash`: the first 8 bytes of SHA-256 of the redacted
    /// text, as the table's `Int64` holds them.
    pub fn normalized_hash(&self) -> i64 {
        let digest = Sha256::digest(crate::oplog::redact(&self.query).as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        i64::from_be_bytes(bytes)
    }

    /// Its duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        u64::try_from((self.ended_us - self.started_us).max(0) / 1000).unwrap_or(0)
    }

    /// `ok`, `cancelled` or `error` (the metrics' `status`).
    pub fn status(&self) -> &'static str {
        match self.code {
            0 => "ok",
            CANCELLED => "cancelled",
            _ => "error",
        }
    }
}

/// ClickHouse's `query_kind` for a House statement kind.
pub fn clickhouse_kind(kind: &str) -> &'static str {
    match kind {
        "Query" => "Select",
        "Insert" => "Insert",
        "CreateDatabase"
        | "CreateTable"
        | "CreateTableAs"
        | "CreateTemporaryTable"
        | "CreatePipe"
        | "CreateMaterializedView" => "Create",
        "Drop" | "Truncate" => "Drop",
        "AlterAddColumns" => "Alter",
        "Rename" => "Rename",
        "Undrop" => "Undrop",
        "Show" => "Show",
        "Describe" => "Describe",
        "Exists" => "Exists",
        "Use" => "Use",
        "Set" => "Set",
        "Optimize" => "Optimize",
        "KillQuery" => "KillQuery",
        "Explain" => "Explain",
        _ => "None",
    }
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn day_of(us: i64) -> i32 {
    i32::try_from(us.div_euclid(86_400_000_000)).unwrap_or(0)
}

/// `CREATE TABLE _house.query_log`, as the catalog keeps it.
pub fn create_sql() -> String {
    let columns: Vec<String> = COLUMNS
        .iter()
        .map(|(name, ty)| format!("{} {ty}", quote_name(name)))
        .collect();
    format!(
        "CREATE TABLE {}.{} ({}) ENGINE = MergeTree PARTITION BY event_date ORDER BY (event_time, \
         query_id)",
        quote_name(HOUSE_DATABASE),
        quote_name(QUERY_LOG_TABLE),
        columns.join(", ")
    )
}

/// Rows as one Arrow IPC stream, in [`COLUMNS`]' order and types (the
/// writer conforms them to the table's Arrow schema).
pub fn encode(entries: &[Entry]) -> Result<Vec<u8>, HouseError> {
    let failed = |err: arrow_schema::ArrowError| {
        HouseError::from(ChError::bad_arguments(format!(
            "the query log's rows could not be encoded: {err}"
        )))
    };
    let int = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    let text = |f: &dyn Fn(&Entry) -> &str| -> ArrayRef {
        let mut b = StringBuilder::new();
        for e in entries {
            b.append_value(f(e));
        }
        Arc::new(b.finish())
    };
    let long = |f: &dyn Fn(&Entry) -> i64| -> ArrayRef {
        let mut b = Int64Builder::with_capacity(entries.len());
        for e in entries {
            b.append_value(f(e));
        }
        Arc::new(b.finish())
    };
    let time = |f: &dyn Fn(&Entry) -> i64| -> ArrayRef {
        let mut b = TimestampMicrosecondBuilder::with_capacity(entries.len()).with_timezone("UTC");
        for e in entries {
            b.append_value(f(e));
        }
        Arc::new(b.finish())
    };
    let mut date = Date32Builder::with_capacity(entries.len());
    let mut code = Int32Builder::with_capacity(entries.len());
    let mut cpu = Int64Builder::with_capacity(entries.len());
    for e in entries {
        date.append_value(day_of(e.ended_us));
        code.append_value(e.code);
        cpu.append_option(e.cpu_us.map(int));
    }
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("event_date", Arc::new(date.finish())),
        ("event_time", time(&|e| e.ended_us)),
        ("query_start_time", time(&|e| e.started_us)),
        ("query_duration_ms", long(&|e| int(e.duration_ms()))),
        ("type", text(&|e| e.event_type())),
        ("query_id", text(&|e| &e.query_id)),
        ("user", text(&|e| &e.user)),
        ("loams_principal", text(&|e| &e.principal)),
        ("loams_surface", text(&|e| &e.surface)),
        ("query_kind", text(&|e| e.query_kind())),
        ("loams_kind", text(&|e| &e.kind)),
        ("current_database", text(&|e| &e.database)),
        ("normalized_query_hash", long(&|e| e.normalized_hash())),
        ("query", text(&|e| &e.query)),
        ("exception_code", Arc::new(code.finish())),
        ("exception", text(&|e| &e.exception)),
        ("read_rows", long(&|e| int(e.read_rows))),
        ("read_bytes", long(&|e| int(e.read_bytes))),
        ("written_rows", long(&|e| int(e.written_rows))),
        ("written_bytes", long(&|e| int(e.written_bytes))),
        ("result_rows", long(&|e| int(e.result_rows))),
        ("result_bytes", long(&|e| int(e.result_bytes))),
        ("memory_usage", long(&|e| int(e.memory_usage))),
        ("cpu_time_us", Arc::new(cpu.finish())),
    ];
    debug_assert_eq!(columns.len(), COLUMNS.len());
    let batch = RecordBatch::try_from_iter(columns).map_err(failed)?;
    let mut out = Vec::new();
    {
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(&mut out, &batch.schema()).map_err(failed)?;
        writer.write(&batch).map_err(failed)?;
        writer.finish().map_err(failed)?;
    }
    Ok(out)
}

/// A row as SQL literals in [`COLUMNS`]' order, dates as days since the epoch
/// and times as Unix microseconds (`system.query_log`'s unflushed rows).
pub fn literals(e: &Entry) -> Vec<String> {
    let int = |v: u64| i64::try_from(v).unwrap_or(i64::MAX).to_string();
    vec![
        day_of(e.ended_us).to_string(),
        e.ended_us.to_string(),
        e.started_us.to_string(),
        int(e.duration_ms()),
        quote_string(e.event_type()),
        quote_string(&e.query_id),
        quote_string(&e.user),
        quote_string(&e.principal),
        quote_string(&e.surface),
        quote_string(e.query_kind()),
        quote_string(&e.kind),
        quote_string(&e.database),
        e.normalized_hash().to_string(),
        quote_string(&e.query),
        e.code.to_string(),
        quote_string(&e.exception),
        int(e.read_rows),
        int(e.read_bytes),
        int(e.written_rows),
        int(e.written_bytes),
        int(e.result_rows),
        int(e.result_bytes),
        int(e.memory_usage),
        e.cpu_us.map_or_else(|| "NULL".to_string(), int),
    ]
}

/// What a history read keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only this House user's rows.
    pub user: Option<String>,
    /// Only this statement.
    pub query_id: Option<String>,
    /// Only statements started at or after this (Unix microseconds).
    pub since_us: Option<i64>,
    /// Only these codes: `Some(true)` successes, `Some(false)` failures.
    pub status: Option<StatusFilter>,
    /// Only statements whose text contains this, without case.
    pub text: Option<String>,
}

/// A status filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    Finished,
    Failed,
    Cancelled,
}

impl Filter {
    fn keeps(&self, e: &Entry) -> bool {
        self.user.as_ref().is_none_or(|u| *u == e.user)
            && self.query_id.as_ref().is_none_or(|q| *q == e.query_id)
            && self.since_us.is_none_or(|s| e.started_us >= s)
            && match self.status {
                None => true,
                Some(StatusFilter::Finished) => e.code == 0,
                Some(StatusFilter::Cancelled) => e.code == CANCELLED,
                Some(StatusFilter::Failed) => e.code != 0 && e.code != CANCELLED,
            }
            && self
                .text
                .as_ref()
                .is_none_or(|t| e.query.to_lowercase().contains(&t.to_lowercase()))
    }

    fn sql(&self) -> String {
        let mut cond = vec!["1".to_string()];
        if let Some(user) = &self.user {
            cond.push(format!("user = {}", quote_string(user)));
        }
        if let Some(id) = &self.query_id {
            cond.push(format!("query_id = {}", quote_string(id)));
        }
        if let Some(since) = self.since_us {
            cond.push(format!(
                "toUnixTimestamp64Micro(query_start_time) >= {since}"
            ));
        }
        match self.status {
            None => {}
            Some(StatusFilter::Finished) => cond.push("exception_code = 0".to_string()),
            Some(StatusFilter::Cancelled) => {
                cond.push(format!("exception_code = {CANCELLED}"));
            }
            Some(StatusFilter::Failed) => {
                cond.push(format!("exception_code NOT IN (0, {CANCELLED})"));
            }
        }
        if let Some(text) = &self.text {
            cond.push(format!(
                "positionCaseInsensitiveUTF8(query, {}) > 0",
                quote_string(text)
            ));
        }
        cond.join(" AND ")
    }
}

#[derive(Debug, Default)]
struct Pending {
    /// Rows waiting, per namespace, oldest first.
    waiting: BTreeMap<u64, Vec<Entry>>,
    /// Rows a flush is writing now, per namespace.
    flushing: BTreeMap<u64, Vec<Entry>>,
}

impl Pending {
    fn rows(&self) -> usize {
        self.waiting.values().map(Vec::len).sum::<usize>()
            + self.flushing.values().map(Vec::len).sum::<usize>()
    }
}

/// The front's writer and reader of its namespaces' query logs.
#[derive(Debug)]
pub struct QueryLog {
    lake: Arc<Lake>,
    pending: Mutex<Pending>,
    retention: Duration,
    flushing: tokio::sync::Mutex<()>,
}

impl QueryLog {
    /// A query log over `lake`'s namespaces, keeping `retention` unless a
    /// namespace's table says otherwise.
    pub fn new(lake: Arc<Lake>, retention: Duration) -> Arc<Self> {
        Arc::new(Self {
            lake,
            pending: Mutex::new(Pending::default()),
            retention,
            flushing: tokio::sync::Mutex::new(()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn metrics(&self) -> &Metrics {
        self.lake.metrics()
    }

    /// Holds a row for the next flush. Past [`MAX_PENDING_ROWS`] the oldest
    /// waiting row of the largest namespace is dropped.
    pub fn record(&self, entry: Entry) {
        let Some(namespace) = entry.namespace else {
            return;
        };
        let mut pending = self.lock();
        pending.waiting.entry(namespace).or_default().push(entry);
        if pending.rows() > MAX_PENDING_ROWS
            && let Some(rows) = pending.waiting.values_mut().max_by_key(|rows| rows.len())
            && !rows.is_empty()
        {
            rows.remove(0);
            self.metrics().query_log_rows("dropped", 1);
        }
        self.metrics().set_query_log_pending(pending.rows() as u64);
    }

    /// The rows of `namespace` not written yet, oldest first.
    pub fn pending(&self, namespace: u64) -> Vec<Entry> {
        let pending = self.lock();
        pending
            .flushing
            .get(&namespace)
            .into_iter()
            .chain(pending.waiting.get(&namespace))
            .flatten()
            .cloned()
            .collect()
    }

    /// Writes every namespace's waiting rows, one commit each. A namespace
    /// whose write fails keeps its rows for the next flush (the failure is on
    /// stderr).
    pub async fn flush(&self) {
        let _one = self.flushing.lock().await;
        let taken: Vec<(u64, Vec<Entry>)> = {
            let mut pending = self.lock();
            let taken = std::mem::take(&mut pending.waiting);
            pending.flushing = taken.clone();
            taken.into_iter().filter(|(_, r)| !r.is_empty()).collect()
        };
        for (namespace, rows) in taken {
            let n = rows.len() as u64;
            match self.write(namespace, &rows).await {
                Ok(()) => {
                    self.metrics().query_log_rows("written", n);
                    self.lock().flushing.remove(&namespace);
                }
                Err(err) => {
                    eprintln!(
                        "loams-house: the query log of namespace {namespace} was not written \
                         ({n} rows kept for the next flush): {}",
                        err.message()
                    );
                    let mut pending = self.lock();
                    let mut back = pending.flushing.remove(&namespace).unwrap_or_default();
                    back.extend(pending.waiting.remove(&namespace).unwrap_or_default());
                    pending.waiting.insert(namespace, back);
                }
            }
        }
        let pending = self.lock();
        self.metrics().set_query_log_pending(pending.rows() as u64);
    }

    /// The namespace's `_house.query_log`, made the first time.
    pub async fn table(&self, namespace: u64) -> Result<LakeTable, HouseError> {
        if let Some(table) = self
            .lake
            .load(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
        {
            return Ok(table);
        }
        self.lake.ensure_root(namespace).await?;
        match self
            .lake
            .catalog()
            .create_database(
                &Lake::namespace(namespace, HOUSE_DATABASE),
                [(
                    "comment".to_string(),
                    "The House's own tables (the query log)".to_string(),
                )]
                .into(),
            )
            .await
        {
            Ok(()) | Err(CatalogError::NamespaceExists(_)) => {}
            Err(err) => return Err(err.into()),
        }
        let text = create_sql();
        let stmt = ddl::parse_create_table(&text)?;
        let described: Vec<ddl::Described> = COLUMNS
            .iter()
            .map(|(name, ty)| ddl::Described {
                column: ddl::ChColumn {
                    name: name.to_string(),
                    ch_type: ty.to_string(),
                    ..ddl::ChColumn::default()
                },
                ttl: String::new(),
            })
            .collect();
        let location = self
            .lake
            .root()
            .table_location(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE);
        let creation = ddl::plan_table(&stmt, &text, location, &described, &[])?;
        self.lake
            .create_table(namespace, HOUSE_DATABASE, creation, true)
            .await?;
        self.lake
            .load(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
            .ok_or_else(|| {
                HouseError::from(ChError::unknown_table(
                    "the query log's table was made and is not in the catalog",
                ))
            })
    }

    async fn write(&self, namespace: u64, rows: &[Entry]) -> Result<(), HouseError> {
        let table = self.table(namespace).await?;
        let bytes = encode(rows)?;
        let lake_error = |err: loams_house_lake::LakeError| {
            HouseError::from(ChError::bad_arguments(format!(
                "the query log's files: {err}"
            )))
        };
        let mut writer = loams_house_lake::write::DataWriter::new(
            &table.metadata,
            &uuid::Uuid::new_v4().to_string(),
            crate::insert::DEFAULT_TARGET_FILE_BYTES,
            crate::insert::DEFAULT_MAX_INSERT_BYTES,
        )
        .await
        .map_err(lake_error)?;
        writer.push(&bytes).await.map_err(lake_error)?;
        let written = writer.finish().await.map_err(lake_error)?;
        let committer = self.lake.committer();
        for (location, bytes) in &written.files {
            committer.io().write(location, bytes.clone()).await?;
        }
        let (reply, answer) = oneshot::channel();
        committer.submit(
            &table.ident,
            crate::commit::Pending {
                files: written.data_files,
                key: None,
                window: 0,
                interval: Duration::ZERO,
                commit_bytes: crate::insert::DEFAULT_COMMIT_BYTES,
                reply,
            },
        );
        answer.await.map_err(|_| {
            HouseError::from(ChError::network_error(
                "the commit queue ended before the query log's commit was answered",
            ))
        })??;
        Ok(())
    }

    /// The retention a namespace's table has: its [`RETENTION_PROPERTY`], else
    /// the front's.
    pub fn retention_of(&self, table: &LakeTable) -> Duration {
        table
            .metadata
            .properties()
            .get(RETENTION_PROPERTY)
            .and_then(|days| days.trim().parse::<u64>().ok())
            .filter(|days| *days > 0)
            .map_or(self.retention, |days| {
                Duration::from_secs(days.saturating_mul(24 * 3600))
            })
    }

    /// Removes, in every namespace, the days of `_house.query_log` older than
    /// its retention as of `now`: whole files of whole days, by a `delete`
    /// snapshot. Answers the files removed.
    pub async fn retention_round(&self, now: SystemTime) -> u64 {
        let namespaces = match self.lake.namespaces().await {
            Ok(namespaces) => namespaces,
            Err(err) => {
                eprintln!(
                    "loams-house: the query log's retention found no namespaces: {}",
                    err.message()
                );
                return 0;
            }
        };
        let mut removed = 0;
        for namespace in namespaces {
            match self.expire_namespace(namespace, now).await {
                Ok(n) => removed += n,
                Err(err) => eprintln!(
                    "loams-house: the query log's retention in namespace {namespace} failed: {}",
                    err.message()
                ),
            }
        }
        removed
    }

    async fn expire_namespace(&self, namespace: u64, now: SystemTime) -> Result<u64, HouseError> {
        let Some(table) = self
            .lake
            .load(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
        else {
            return Ok(0);
        };
        let Some(snapshot) = table.metadata.current_snapshot() else {
            return Ok(0);
        };
        let now_us = now
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let keep = i64::try_from(self.retention_of(&table).as_micros()).unwrap_or(i64::MAX);
        // The first day kept: every row of an older day is past the retention.
        let first_kept = day_of(now_us.saturating_sub(keep));
        let old: HashSet<String> =
            crate::read::live_files(&self.lake, &table.metadata, snapshot.snapshot_id())
                .await?
                .into_iter()
                .filter(|f| f.data)
                .filter(|f| f.partition.parse::<i32>().is_ok_and(|day| day < first_kept))
                .map(|f| f.location)
                .collect();
        if old.is_empty() {
            return Ok(0);
        }
        let committer = self.lake.committer();
        let io = committer.io();
        let unreadable = |err: loams_house_lake::LakeError| {
            HouseError::from(ChError::network_error(format!(
                "the manifests of the query log: {err}"
            )))
        };
        let list = io.read(snapshot.manifest_list()).await?;
        let mut manifests = Vec::new();
        for manifest in
            loams_house_lake::rewrite::parent_manifests(&list, table.metadata.format_version())
                .map_err(unreadable)?
        {
            let bytes = io.read(&manifest.manifest_path).await?;
            manifests.push((manifest, bytes));
        }
        let built = loams_house_lake::rewrite::delete_files(
            &table.metadata,
            manifests,
            &old,
            vec![("loams.retention".to_string(), first_kept.to_string())],
            now_us / 1000,
        )
        .await
        .map_err(unreadable)?;
        for (location, bytes) in built.files {
            io.write(&location, bytes).await?;
        }
        match io
            .catalog
            .commit(&table.ident, built.requirements, built.updates)
            .await
        {
            Ok(_) => Ok(old.len() as u64),
            // A flush came first: the next round removes them.
            Err(CatalogError::Conflict { .. }) => Ok(0),
            Err(err) => Err(err.into()),
        }
    }

    /// Flushes every `every` and removes old days every [`RETENTION_SWEEP`],
    /// until the task is aborted.
    pub fn spawn(self: &Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut swept: Option<Instant> = None;
            loop {
                tick.tick().await;
                this.flush().await;
                if swept.is_none_or(|at| at.elapsed() >= RETENTION_SWEEP) {
                    this.retention_round(SystemTime::now()).await;
                    swept = Some(Instant::now());
                }
            }
        })
    }

    /// The DDL a worker runs to read the namespace's table as
    /// `` `_house`.`query_log` `` (a view of its current snapshot), or `None`
    /// when it has no table yet.
    pub async fn read_views(
        &self,
        namespace: u64,
        endpoint: Option<&str>,
    ) -> Result<Option<Vec<String>>, HouseError> {
        let Some(table) = self
            .lake
            .load(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
        else {
            return Ok(None);
        };
        let Some(endpoint) = endpoint else {
            return Err(HouseError::from(ChError::not_implemented(
                "the query log is a lake table, and this House's workers read no bucket \
                 (in-process workers): run process workers (--workers=process) with --store",
            )));
        };
        let pin = crate::read::pin(
            HOUSE_DATABASE,
            QUERY_LOG_TABLE,
            HOUSE_DATABASE,
            &table,
            &crate::read::AsOf::Current,
        )?;
        let source = crate::views::source(&pin, endpoint, self.lake.root())?;
        Ok(Some(vec![
            crate::views::database_ddl(HOUSE_DATABASE),
            crate::views::view_ddl(&pin, &source, crate::views::Mode::Plain)?,
        ]))
    }

    /// The namespace's rows that `filter` keeps, newest first, at most
    /// [`MAX_READ_ROWS`]: the table's, read on a worker of the namespace, and
    /// this front's unflushed ones.
    pub async fn read(
        &self,
        pool: &WorkerPool,
        namespace: u64,
        filter: &Filter,
    ) -> Result<Vec<Entry>, HouseError> {
        let mut out = self.read_table(pool, namespace, filter).await?;
        let seen: HashSet<String> = out.iter().map(|e| e.query_id.clone()).collect();
        out.extend(
            self.pending(namespace)
                .into_iter()
                .filter(|e| filter.keeps(e) && !seen.contains(&e.query_id)),
        );
        out.sort_by_key(|e| std::cmp::Reverse(e.started_us));
        out.truncate(MAX_READ_ROWS);
        Ok(out)
    }

    async fn read_table(
        &self,
        pool: &WorkerPool,
        namespace: u64,
        filter: &Filter,
    ) -> Result<Vec<Entry>, HouseError> {
        if self
            .lake
            .load(namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let mut lease = pool.acquire(&namespace.to_string()).await?;
        let result = async {
            let endpoint = lease.ready().and_then(|r| r.s3_endpoint.clone());
            let Some(views) = self.read_views(namespace, endpoint.as_deref()).await? else {
                return Ok(Vec::new());
            };
            let sql = format!(
                "SELECT query_id, user, loams_principal, loams_surface, loams_kind, \
                 current_database, query, toUnixTimestamp64Micro(query_start_time) AS started, \
                 toUnixTimestamp64Micro(event_time) AS ended, exception_code, exception, type, \
                 read_rows, read_bytes, written_rows, written_bytes, result_rows, result_bytes, \
                 memory_usage, cpu_time_us FROM {}.{} WHERE {} ORDER BY query_start_time DESC \
                 LIMIT {MAX_READ_ROWS}",
                quote_name(HOUSE_DATABASE),
                quote_name(QUERY_LOG_TABLE),
                filter.sql()
            );
            let collected = lease
                .run(Execute {
                    query_id: format!("loams-query-log-{}", uuid::Uuid::new_v4()),
                    session: None,
                    settings: Vec::new(),
                    views,
                    sql,
                    format: "JSONCompactEachRow".to_string(),
                    params: Vec::new(),
                    limits: Limits::default(),
                    input: None,
                    output: Output::Blocks,
                    database: Some(HOUSE_DATABASE.to_string()),
                })
                .await?;
            parse_rows(&collected.bytes, namespace)
        }
        .await;
        pool.release(lease, crate::admission::Outcome::Completed);
        result
    }
}

fn parse_rows(bytes: &[u8], namespace: u64) -> Result<Vec<Entry>, HouseError> {
    let text = String::from_utf8_lossy(bytes);
    let unreadable = |why: String| {
        HouseError::from(ChError::network_error(format!(
            "the query log's rows are unreadable: {why}"
        )))
    };
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Vec<serde_json::Value> =
            serde_json::from_str(line).map_err(|err| unreadable(err.to_string()))?;
        if row.len() != 20 {
            return Err(unreadable(format!("{} columns, not 20", row.len())));
        }
        let text = |i: usize| row[i].as_str().unwrap_or_default().to_string();
        // 64-bit integers come quoted (`output_format_json_quote_64bit_integers`).
        let int = |i: usize| -> i64 {
            match &row[i] {
                serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
                serde_json::Value::String(s) => s.parse().unwrap_or(0),
                _ => 0,
            }
        };
        let count = |i: usize| u64::try_from(int(i)).unwrap_or(0);
        out.push(Entry {
            namespace: Some(namespace),
            query_id: text(0),
            user: text(1),
            principal: text(2),
            surface: text(3),
            kind: text(4),
            database: text(5),
            query: text(6),
            started_us: int(7),
            ended_us: int(8),
            code: i32::try_from(int(9)).unwrap_or(0),
            exception: text(10),
            before_start: text(11) == "ExceptionBeforeStart",
            read_rows: count(12),
            read_bytes: count(13),
            written_rows: count(14),
            written_bytes: count(15),
            result_rows: count(16),
            result_bytes: count(17),
            memory_usage: count(18),
            cpu_us: (!row[19].is_null()).then(|| count(19)),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The per-statement hub.
// ---------------------------------------------------------------------------

/// The front's hub for its statements' ends (see the module docs).
#[derive(Debug)]
pub struct Observer {
    pub metrics: Arc<Metrics>,
    pub tracer: Arc<Tracer>,
    pub oplog: Option<Arc<dyn OperatorLog>>,
    pub log: Option<Arc<QueryLog>>,
}

impl Observer {
    /// A statement arrived on `surface` with `query_id`; `parent` is the
    /// caller's trace (`traceparent`).
    pub fn begin(
        self: &Arc<Self>,
        surface: &'static str,
        query_id: &str,
        parent: Option<SpanContext>,
    ) -> Arc<Statement> {
        let mut root = self.tracer.root(parent);
        if let Some(root) = root.as_mut() {
            root.set("loams.query_id", query_id.to_string());
            root.set("loams.surface", surface);
        }
        Arc::new(Statement {
            observer: Arc::clone(self),
            started: Instant::now(),
            entry: Mutex::new(Entry {
                query_id: query_id.to_string(),
                surface: surface.to_string(),
                started_us: now_us(),
                before_start: true,
                ..Entry::default()
            }),
            root: Mutex::new(root),
            done: AtomicBool::new(false),
        })
    }

    fn end(&self, entry: Entry, took: Duration, root: Option<OpenSpan>) {
        let kind = if entry.kind.is_empty() {
            "Unknown"
        } else {
            entry.kind.as_str()
        };
        self.metrics.statement(&StatementSample {
            namespace: entry.namespace.unwrap_or(0),
            kind,
            status: entry.status(),
            took,
            read_bytes: entry.read_bytes,
            written_bytes: entry.written_bytes,
            result_bytes: entry.result_bytes,
        });
        if let Some((quota, _)) = crate::quota::refusal_parts(&entry.exception) {
            self.metrics.quota_refusal(quota);
        }
        if let Some(oplog) = &self.oplog {
            oplog.record(&OperatorLine {
                at_ms: u64::try_from(entry.ended_us / 1000).unwrap_or(0),
                query_id: entry.query_id.clone(),
                surface: if entry.surface == "api" {
                    "api"
                } else {
                    "http"
                },
                namespace: entry.namespace,
                user: entry.user.clone(),
                kind: kind.to_string(),
                duration_ms: entry.duration_ms(),
                code: entry.code,
                read_rows: entry.read_rows,
                read_bytes: entry.read_bytes,
                written_rows: entry.written_rows,
                written_bytes: entry.written_bytes,
                result_rows: entry.result_rows,
                query: crate::oplog::redact(&entry.query),
            });
        }
        if let Some(mut root) = root {
            if let Some(ns) = entry.namespace {
                root.set("loams.namespace", ns);
            }
            root.set("loams.kind", kind.to_string());
            root.set("clickhouse.code", entry.code);
            root.set("loams.read_rows", entry.read_rows);
            root.set("loams.read_bytes", entry.read_bytes);
            root.set("loams.written_rows", entry.written_rows);
            root.set("loams.result_rows", entry.result_rows);
            if entry.code != 0 {
                root.fail();
            }
            root.end();
        }
        if let Some(log) = &self.log
            && entry.namespace.is_some()
        {
            log.record(entry);
        }
    }
}

/// One statement on its way (see [`Observer`]): updated as it goes, ended
/// once — by [`Statement::finish`] or when the last handle drops.
#[derive(Debug)]
pub struct Statement {
    observer: Arc<Observer>,
    started: Instant,
    entry: Mutex<Entry>,
    root: Mutex<Option<OpenSpan>>,
    done: AtomicBool,
}

impl Statement {
    /// The span its work belongs under, for [`crate::trace::scope`].
    pub fn current(&self) -> Option<crate::trace::Current> {
        let root = self.root.lock().unwrap_or_else(|e| e.into_inner());
        root.as_ref().map(|span| crate::trace::Current {
            tracer: Arc::clone(span.tracer()),
            context: span.context(),
        })
    }

    /// Changes its row.
    pub fn update(&self, change: impl FnOnce(&mut Entry)) {
        change(&mut self.entry.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Its caller, once known.
    pub fn identify(&self, user: &UserMap) {
        self.update(|e| {
            e.namespace = Some(user.namespace);
            e.user = user.user.clone();
            e.principal = user.principal.clone().unwrap_or_default();
        });
    }

    /// What it is: its kind, its current database and its text.
    pub fn describe(&self, kind: &str, database: &str, query: &str) {
        self.update(|e| {
            e.kind = kind.to_string();
            e.database = database.to_string();
            e.query = query.to_string();
        });
    }

    /// A worker started it.
    pub fn started_on_worker(&self) {
        self.update(|e| e.before_start = false);
    }

    /// Its counters (the last a worker sent, or the front's own).
    pub fn progress(&self, p: &Progress) {
        self.update(|e| {
            e.read_rows = p.rows_read;
            e.read_bytes = p.bytes_read;
            e.written_rows = e.written_rows.max(p.written_rows);
            e.written_bytes = e.written_bytes.max(p.written_bytes);
            e.result_rows = p.result_rows;
            e.result_bytes = p.result_bytes;
            e.memory_usage = e.memory_usage.max(p.peak_rss_bytes);
        });
    }

    /// The worker's CPU time for it.
    pub fn cpu(&self, cpu: Option<Duration>) {
        self.update(|e| {
            e.cpu_us = cpu.map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
        });
    }

    /// It failed with `code` and `message` (the first failure counts).
    pub fn fail(&self, code: i32, message: &str) {
        self.update(|e| {
            if e.code == 0 {
                e.code = code;
                e.exception = message.to_string();
            }
        });
    }

    /// Ends it now (a later call, or the drop, does nothing).
    pub fn finish(&self) {
        if self.done.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut entry = std::mem::take(&mut *self.entry.lock().unwrap_or_else(|e| e.into_inner()));
        entry.ended_us = now_us().max(entry.started_us);
        let root = self.root.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.observer.end(entry, self.started.elapsed(), root);
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(code: i32) -> Entry {
        Entry {
            namespace: Some(2),
            query_id: "q1".to_string(),
            surface: "http".to_string(),
            user: "alice".to_string(),
            kind: "Query".to_string(),
            database: "default".to_string(),
            query: "SELECT 'secret', 1".to_string(),
            started_us: 1_791_800_430_000_000,
            ended_us: 1_791_800_430_250_000,
            code,
            read_rows: 3,
            cpu_us: Some(1200),
            ..Entry::default()
        }
    }

    #[test]
    fn rows_encode_as_the_table_columns() {
        let bytes = encode(&[entry(0), entry(60)]).expect("encodes");
        let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
            .expect("ipc");
        let schema = reader.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        let want: Vec<&str> = COLUMNS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, want);
        let batches: Vec<RecordBatch> = reader.map(|b| b.expect("batch")).collect();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        assert_eq!(literals(&entry(0)).len(), COLUMNS.len());
    }

    #[test]
    fn kinds_types_and_statuses() {
        assert_eq!(entry(0).event_type(), "QueryFinish");
        assert_eq!(entry(0).query_kind(), "Select");
        assert_eq!(entry(0).duration_ms(), 250);
        assert_eq!(entry(CANCELLED).status(), "cancelled");
        let mut failed = entry(60);
        assert_eq!(failed.event_type(), "ExceptionWhileProcessing");
        failed.before_start = true;
        assert_eq!(failed.event_type(), "ExceptionBeforeStart");
        assert_eq!(clickhouse_kind("CreateTable"), "Create");
        // The hash ignores literals.
        let mut other = entry(0);
        other.query = "SELECT 'other', 2".to_string();
        assert_eq!(entry(0).normalized_hash(), other.normalized_hash());
    }

    #[test]
    fn create_sql_parses() {
        let stmt = ddl::parse_create_table(&create_sql()).expect("parses");
        assert_eq!(stmt.name, QUERY_LOG_TABLE);
        assert_eq!(stmt.database.as_deref(), Some(HOUSE_DATABASE));
    }

    #[test]
    fn filters() {
        let e = entry(0);
        assert!(Filter::default().keeps(&e));
        let mine = Filter {
            user: Some("bob".to_string()),
            ..Filter::default()
        };
        assert!(!mine.keeps(&e));
        let text = Filter {
            text: Some("SECRET".to_string()),
            status: Some(StatusFilter::Finished),
            ..Filter::default()
        };
        assert!(text.keeps(&e));
        assert!(!text.keeps(&entry(60)));
        assert_eq!(
            text.sql(),
            "1 AND exception_code = 0 AND positionCaseInsensitiveUTF8(query, 'SECRET') > 0"
        );
    }

    #[test]
    fn rows_parse_from_the_worker() {
        let line = r#"["q1","alice","","http","Query","default","SELECT 1","1791800430000000","1791800430250000",0,"","QueryFinish","3","10","0","0","1","8","1000",null]"#;
        let rows = parse_rows(line.as_bytes(), 2).expect("parses");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].started_us, 1_791_800_430_000_000);
        assert_eq!(rows[0].read_rows, 3);
        assert_eq!(rows[0].cpu_us, None);
        assert!(!rows[0].before_start);
    }

    #[test]
    fn a_statement_ends_once() {
        let log = Arc::new(crate::oplog::Memory::default());
        let observer = Arc::new(Observer {
            metrics: Arc::default(),
            tracer: Tracer::off(),
            oplog: Some(log.clone()),
            log: None,
        });
        let statement = observer.begin("http", "q1", None);
        statement.identify(&UserMap::dev("alice", "a", 2, false));
        statement.describe("Query", "default", "SELECT 'hunter2'");
        statement.fail(60, "Unknown table");
        statement.finish();
        drop(statement);
        let lines = log.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].code, 60);
        assert_eq!(lines[0].query, "SELECT ?");
        assert_eq!(lines[0].namespace, Some(2));
    }
}
