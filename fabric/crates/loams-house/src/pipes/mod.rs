//! Pipes (HS1 Task 16, design §49 §9): a Loams stream lands exactly once in a
//! lake table through a materialized view.
//!
//! ```sql
//! CREATE TABLE q (k Int64, v String) ENGINE = LoamsStream('orders', 'JSONEachRow');
//! CREATE MATERIALIZED VIEW mv TO t AS SELECT k, upper(v) AS v FROM q;
//! ```
//!
//! * **The pipe** `q` is a catalog table of class `pipe`: its columns, its
//!   engine and its source ([`PipeSource`], `loams.pipe_source`), no data. It
//!   is read only by its views: a `SELECT` from it is `620`, an `INSERT` into it
//!   `48`. Formats: `JSONEachRow`, `CSV` and `CloudEvents` (§32 §5.4's `_ce_*`
//!   columns); `Avro` is `48` (a stream record is one Avro datum without its
//!   schema, which chDB's `Avro` input cannot read).
//! * **A materialized view** is recorded on its pipe (`loams.mv.<id>`, a
//!   [`MaterializedView`]): its name, its target lake table and its `SELECT`.
//!   Views over lake tables are HS1 Task 17's.
//! * **The runner** ([`PipeRunner`]), in every front, takes each view in turn:
//!   it reads the target's cursor (the table properties `loams.pipe.<view
//!   id>.<partition>`), fetches each partition's records from there (at most
//!   `loams_pipe_max_rows` rows), uploads them as one object under the target's
//!   `data/` directory, runs the view's `SELECT` over that object in a worker
//!   (the pipe's name reads it, [`PipeBatch`]) through HS1 Task 12's write path,
//!   and commits the files **with the new cursor in the same commit**: an
//!   append whose summary carries the same `loams.pipe.<view id>.<partition>`
//!   keys, which requires `main` to be the snapshot it loaded. A commit that
//!   loses checks the cursor again: moved means another runner committed
//!   these records, and this batch is abandoned (its files are orphans, HS1
//!   Task 15 removes them); unmoved means another writer's commit came first,
//!   and the append is rebuilt on the new `main`. A front that dies anywhere
//!   before the commit leaves the cursor where it was, so the records are read
//!   again; after it, they are not. The cursor lives in the table properties as
//!   well as the summaries because properties survive snapshot expiry and
//!   compaction (ruling R16.2).
//! * **Its state** — lag, last commit, last error — is kept per front and shown
//!   by `system.loams_pipes` and `ListPipes`.
//! * **`S3Queue` pipes** (HS1 Task 18) read objects under the namespace's own
//!   prefix instead of a stream, with their own runner ([`s3queue`]) and the
//!   same views, writer and cursor commit.

pub mod loams_stream;
pub mod s3queue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_house_ipc::{Execute, Limits, Output};
use loams_house_lake::{TableIdent, TableRequirement, TableUpdate, append};
use serde::{Deserialize, Serialize};

pub use loams_stream::{Bounds, EngineStreams, Fetched, StreamSource};

use crate::admission::{Outcome, WorkerLease, WorkerPool};
use crate::catalog::{CatalogError, Lake, LakeTable};
use crate::config::{EngineLink, HouseConfig, UserMap};
use crate::ddl::{
    self, ChTable, CreateTable, Oracle, PROP_CLASS, PROP_ENGINE, PROP_SETTINGS, TableClass,
    quote_name, quote_string,
};
use crate::errors::{ChError, HouseError};
use crate::insert::{InsertContext, Source, Target};

/// Engines that make a table a pipe.
pub const PIPE_ENGINES: &[&str] = &["LoamsStream", "IggyTopic", "S3Queue"];
/// Property of a pipe: its [`PipeSource`].
pub const PROP_SOURCE: &str = "loams.pipe_source";
/// Properties of a pipe: one per materialized view, `loams.mv.<id>`.
pub const MV_PREFIX: &str = "loams.mv.";
/// Properties (and summary keys) of a view's target: the cursor,
/// `loams.pipe.<view id>.<partition>` = the next offset.
pub const CURSOR_PREFIX: &str = "loams.pipe.";
/// `loams_pipe_max_rows`' default (Shared contracts).
pub const DEFAULT_MAX_ROWS: u64 = 1_000_000;
/// `loams_pipe_max_delay_ms`' default (Shared contracts).
pub const DEFAULT_MAX_DELAY_MS: u64 = 2_000;
/// The most one batch holds, in bytes of records.
pub const MAX_BATCH_BYTES: usize = 256 * 1024 * 1024;
/// One fetch's size.
pub const FETCH_BYTES: usize = 8 * 1024 * 1024;
/// How many times a batch's commit is tried against other writers' commits.
pub const COMMIT_ATTEMPTS: u32 = 10;

/// Whether an `ENGINE` makes a pipe.
pub fn is_pipe_engine(engine: &str) -> bool {
    PIPE_ENGINES.iter().any(|p| p.eq_ignore_ascii_case(engine))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

fn not_implemented(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::not_implemented(message.into()))
}

/// The format of a pipe's records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipeFormat {
    /// One JSON object per record.
    JSONEachRow,
    /// One CSV line per record.
    CSV,
    /// One CloudEvent per record, as `_ce_*` columns.
    CloudEvents,
}

impl PipeFormat {
    /// The format named in `ENGINE = LoamsStream(…)`.
    pub fn parse(name: &str) -> Result<Self, HouseError> {
        match name {
            "JSONEachRow" => Ok(Self::JSONEachRow),
            "CSV" => Ok(Self::CSV),
            "CloudEvents" => Ok(Self::CloudEvents),
            "Avro" => Err(not_implemented(
                "A LoamsStream pipe cannot read Avro: a stream record holds one Avro datum \
                 without its schema, and chDB's Avro input reads container files only",
            )),
            other => Err(HouseError::from(ChError::unknown_format(format!(
                "Unknown format {other} for a LoamsStream pipe: JSONEachRow, CSV or CloudEvents"
            )))),
        }
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Self::JSONEachRow => "JSONEachRow",
            Self::CSV => "CSV",
            Self::CloudEvents => "CloudEvents",
        }
    }

    /// The ClickHouse input format a batch is read with.
    pub fn input(self) -> &'static str {
        match self {
            Self::JSONEachRow | Self::CloudEvents => "JSONEachRow",
            Self::CSV => "CSV",
        }
    }
}

/// Where a pipe reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeSource {
    /// The stream, in the namespace's engine namespace.
    pub stream: String,
    /// Its records' format.
    pub format: PipeFormat,
    /// The consumer name the statement gave (a label: each view keeps its own
    /// cursor, ruling R16.2).
    pub consumer: Option<String>,
}

impl PipeSource {
    /// The engine as `SHOW CREATE` prints it.
    pub fn render(&self) -> String {
        let mut args = vec![quote_string(&self.stream), quote_string(self.format.name())];
        if let Some(consumer) = &self.consumer {
            args.push(quote_string(consumer));
        }
        format!("LoamsStream({})", args.join(", "))
    }
}

/// A materialized view over a pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedView {
    /// Its id, which names its cursor.
    pub id: String,
    /// Its database.
    pub database: String,
    /// Its name.
    pub name: String,
    /// The target's database.
    pub target_database: String,
    /// The target.
    pub target: String,
    /// Its `SELECT`, as written.
    pub select: String,
    /// Whether its target is its own inner table, named as the view (HE1
    /// R9.14): dropping the view drops the table.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inner: bool,
    /// A refreshable view's schedule (HE1 R9.64): it is recorded on its
    /// target, not on a source, and runs on its schedule, not on inserts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<crate::refresh::Schedule>,
}

impl MaterializedView {
    /// The cursor key of `partition`.
    pub fn cursor_key(&self, partition: u32) -> String {
        format!("{CURSOR_PREFIX}{}.{partition}", self.id)
    }
}

/// What a pipe reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeFeed {
    /// A Loams stream (`LoamsStream`).
    Stream(PipeSource),
    /// Objects under the namespace's prefix (`S3Queue`, HS1 Task 18).
    Queue(s3queue::QueueSource),
}

impl PipeFeed {
    /// The engine as `SHOW CREATE` prints it.
    pub fn render(&self) -> String {
        match self {
            Self::Stream(source) => source.render(),
            Self::Queue(source) => source.render(),
        }
    }
}

/// A pipe as the catalog holds it, with its views.
#[derive(Debug, Clone)]
pub struct Pipe {
    /// Its database.
    pub database: String,
    /// Its name.
    pub name: String,
    /// The catalog entry.
    pub table: LakeTable,
    /// Its source.
    pub source: PipeFeed,
    /// `loams_pipe_max_rows`.
    pub max_rows: u64,
    /// `loams_pipe_max_delay_ms`.
    pub max_delay_ms: u64,
    /// Its views.
    pub views: Vec<MaterializedView>,
}

impl Pipe {
    /// The pipe a catalog entry is, unless it is not one.
    pub fn from_table(database: &str, name: &str, table: LakeTable) -> Option<Self> {
        if ddl::class_of(&table.metadata) != TableClass::Pipe {
            return None;
        }
        let properties = table.metadata.properties();
        let source = match properties.get(PROP_SOURCE) {
            Some(json) => PipeFeed::Stream(serde_json::from_str(json).ok()?),
            None => {
                PipeFeed::Queue(serde_json::from_str(properties.get(s3queue::PROP_QUEUE)?).ok()?)
            }
        };
        let settings: Vec<(String, String)> = properties
            .get(PROP_SETTINGS)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let setting = |name: &str, default: u64| {
            settings
                .iter()
                .rev()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(default)
        };
        let views = views_of(&table);
        Some(Self {
            database: database.to_string(),
            name: name.to_string(),
            max_rows: setting("loams_pipe_max_rows", DEFAULT_MAX_ROWS),
            max_delay_ms: setting("loams_pipe_max_delay_ms", DEFAULT_MAX_DELAY_MS),
            table,
            source,
            views,
        })
    }
}

/// The materialized views recorded on a table (a pipe, or a lake table they
/// read, HS1 Task 17), by name.
pub fn views_of(table: &LakeTable) -> Vec<MaterializedView> {
    let mut views: Vec<MaterializedView> = table
        .metadata
        .properties()
        .iter()
        .filter(|(k, _)| k.starts_with(MV_PREFIX))
        .filter_map(|(_, v)| serde_json::from_str(v).ok())
        .collect();
    views.sort_by(|a, b| (&a.database, &a.name).cmp(&(&b.database, &b.name)));
    views
}

/// Every pipe of a namespace.
pub async fn pipes(lake: &Lake, namespace: u64) -> Result<Vec<Pipe>, HouseError> {
    let mut out = Vec::new();
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            if let Some(table) = lake.load(namespace, &database.name, &name).await?
                && let Some(pipe) = Pipe::from_table(&database.name, &name, table)
            {
                out.push(pipe);
            }
        }
    }
    Ok(out)
}

/// The view `database.name` of a namespace, with the table it is recorded on
/// (its source: a pipe or a lake table).
async fn find_view(
    lake: &Lake,
    namespace: u64,
    database: &str,
    name: &str,
) -> Result<Option<(LakeTable, MaterializedView)>, HouseError> {
    for db in lake.databases(namespace).await? {
        for table_name in lake.table_names(namespace, &db.name).await? {
            let Some(table) = lake.load(namespace, &db.name, &table_name).await? else {
                continue;
            };
            if let Some(view) = views_of(&table)
                .into_iter()
                .chain(crate::refresh::refreshes_of(&table))
                .find(|v| v.database == database && v.name == name)
            {
                return Ok(Some((table, view)));
            }
        }
    }
    Ok(None)
}

fn number_setting(name: &str, value: &str, min: u64, max: u64) -> Result<(), HouseError> {
    match value.parse::<u64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(()),
        _ => Err(bad(format!(
            "{name} is a number from {min} to {max}, not {value:?}"
        ))),
    }
}

/// `LoamsStream('<stream>', '<format>'[, '<consumer>'])` with its `SETTINGS`.
fn stream_source(args: &[String], settings: &[(String, String)]) -> Result<PipeSource, HouseError> {
    let literal = |raw: &String| -> Result<String, HouseError> {
        if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
            Ok(ddl::unquote(raw))
        } else {
            Err(bad(format!(
                "LoamsStream's arguments are string literals, not {raw}"
            )))
        }
    };
    if !(2..=3).contains(&args.len()) {
        return Err(bad(
            "LoamsStream takes the stream, the format and, optionally, a consumer name: \
             LoamsStream('<stream>', '<format>'[, '<consumer>'])",
        ));
    }
    let stream = literal(&args[0])?;
    if stream.is_empty() {
        return Err(bad("LoamsStream's stream name is empty"));
    }
    let format = PipeFormat::parse(&literal(&args[1])?)?;
    let consumer = args.get(2).map(literal).transpose()?;
    for (name, value) in settings {
        match name.as_str() {
            "loams_pipe_max_rows" => number_setting(name, value, 1, 100_000_000)?,
            "loams_pipe_max_delay_ms" => number_setting(name, value, 1, 3_600_000)?,
            _ => {
                return Err(HouseError::from(ChError::unknown_setting(format!(
                    "Unknown setting '{name}': for a LoamsStream pipe (loams_pipe_max_rows, \
                     loams_pipe_max_delay_ms)"
                ))));
            }
        }
    }
    Ok(PipeSource {
        stream,
        format,
        consumer,
    })
}

/// `CREATE TABLE … ENGINE = LoamsStream('<stream>', '<format>'[, '<consumer>'])`
/// or `S3Queue('ns/<id>/<path>', '<format>')`.
pub async fn create_pipe(
    lake: &Lake,
    namespace: u64,
    database: &str,
    stmt: &CreateTable,
    text: &str,
    oracle: &mut dyn Oracle,
) -> Result<(), HouseError> {
    let Some((engine, args)) = &stmt.engine else {
        return Err(bad("A pipe names its engine"));
    };
    match engine.as_str() {
        "LoamsStream" | "S3Queue" => {}
        "IggyTopic" => {
            return Err(not_implemented(
                "An IggyTopic pipe needs the Fabric (FL1), which this House is not deployed \
                 with (HS1 Task 18)",
            ));
        }
        other => {
            return Err(bad(format!(
                "Engine {other} is spelled LoamsStream, IggyTopic or S3Queue"
            )));
        }
    }
    let refuse = |what: &str| {
        Err(not_implemented(format!(
            "{what} is not supported on a pipe"
        )))
    };
    if stmt.or_replace {
        return refuse("CREATE OR REPLACE");
    }
    if stmt.as_select || stmt.as_other {
        return refuse("CREATE TABLE … AS");
    }
    if stmt.partition_by.is_some()
        || stmt.order_by.is_some()
        || stmt.primary_key.is_some()
        || stmt.inline_primary_key.is_some()
        || stmt.sample_by.is_some()
        || stmt.ttl.is_some()
    {
        return refuse("A storage clause (PARTITION BY, ORDER BY, PRIMARY KEY, SAMPLE BY, TTL)");
    }
    for (entry, present) in [
        ("INDEX", !stmt.indexes.is_empty()),
        ("PROJECTION", !stmt.projections.is_empty()),
        ("CONSTRAINT", !stmt.constraints.is_empty()),
    ] {
        if present {
            return refuse(&format!("{entry} in the column list"));
        }
    }
    let feed = if engine == "S3Queue" {
        PipeFeed::Queue(s3queue::QueueSource::parse(
            namespace,
            args,
            &stmt.settings,
        )?)
    } else {
        PipeFeed::Stream(stream_source(args, &stmt.settings)?)
    };
    // chDB knows no pipe engine and reads a `SETTINGS` clause against the
    // engine, so the deny list reads the columns under `Null`.
    oracle
        .check(&format!(
            "CREATE TABLE {} ({}) ENGINE = Null",
            quote_name(&stmt.name),
            stmt.columns.join(", ")
        ))
        .await?;
    let described = if stmt.columns.is_empty() {
        Vec::new()
    } else {
        oracle.describe(&stmt.columns.join(", ")).await?
    };
    for d in &described {
        if !d.column.default_kind.is_empty() {
            return refuse(&format!("A {} column", d.column.default_kind));
        }
        if crate::views::printable_type(&d.column.ch_type).is_none() {
            return Err(bad(format!(
                "Column {} of a pipe cannot be {}",
                d.column.name, d.column.ch_type
            )));
        }
    }
    let plain = CreateTable {
        engine: None,
        settings: Vec::new(),
        ..stmt.clone()
    };
    let location = lake.root().table_location(namespace, database, &stmt.name);
    let mut creation = ddl::plan_table(&plain, text, location, &described, &[])?;
    creation
        .properties
        .insert(PROP_ENGINE.to_string(), feed.render());
    creation
        .properties
        .insert(PROP_CLASS.to_string(), "pipe".to_string());
    let (key, json) = match &feed {
        PipeFeed::Stream(source) => (PROP_SOURCE, serde_json::to_string(source)),
        PipeFeed::Queue(source) => (s3queue::PROP_QUEUE, serde_json::to_string(source)),
    };
    creation
        .properties
        .insert(key.to_string(), json.map_err(|e| bad(e.to_string()))?);
    creation.properties.remove(ddl::PROP_ORDER_BY);
    if !stmt.settings.is_empty() {
        creation.properties.insert(
            PROP_SETTINGS.to_string(),
            serde_json::to_string(&stmt.settings).map_err(|e| bad(e.to_string()))?,
        );
    }
    // §49 §12: pipes per namespace (HS1 Task 21), and its `pipes` quota (§49
    // §15, HS1 Task 22).
    let cap = lake.object_caps().pipes;
    if lake.load(namespace, database, &stmt.name).await?.is_none() {
        let existing = pipes(lake, namespace).await?.len();
        if existing >= cap {
            return Err(bad(format!(
                "Too many pipes: a namespace holds at most {cap}"
            )));
        }
        if let Some(quotas) = lake.quotas() {
            quotas.check_pipes(&namespace.to_string(), existing)?;
        }
    }
    lake.create_table(namespace, database, creation, stmt.if_not_exists)
        .await?;
    Ok(())
}

/// The materialized views of a namespace, over pipes and lake tables.
async fn view_count(lake: &Lake, namespace: u64) -> Result<usize, HouseError> {
    let mut count = 0;
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            if let Some(table) = lake.load(namespace, &database.name, &name).await? {
                count += views_of(&table).len() + crate::refresh::refreshes_of(&table).len();
            }
        }
    }
    Ok(count)
}

/// `CREATE MATERIALIZED VIEW mv TO t AS SELECT … FROM q`, `q` a pipe.
pub async fn create_materialized_view(
    lake: &Lake,
    namespace: u64,
    current: &str,
    text: &str,
) -> Result<(), HouseError> {
    create_view_into(lake, namespace, current, text, false).await
}

/// [`create_materialized_view`], and with `inner` the view of an inner-table
/// materialized view, whose target is the table named as the view (HE1 R9.14).
pub async fn create_view_into(
    lake: &Lake,
    namespace: u64,
    current: &str,
    text: &str,
    inner: bool,
) -> Result<(), HouseError> {
    let stmt = ddl::parse_create_mv(text)?;
    if stmt.or_replace {
        return Err(not_implemented(
            "CREATE OR REPLACE MATERIALIZED VIEW is not supported: drop the view and create it",
        ));
    }
    if stmt.refresh {
        return create_refreshable(lake, namespace, current, &stmt).await;
    }
    if stmt.populate {
        return Err(not_implemented(
            "POPULATE is not supported (ClickHouse advises against it: rows that arrive while \
             it runs are lost); fill the target with INSERT … SELECT instead",
        ));
    }
    let Some((to_db, to)) = stmt.to.clone() else {
        return Err(not_implemented(
            "A materialized view on the House writes into a lake table: CREATE MATERIALIZED \
             VIEW … TO <table> AS SELECT …",
        ));
    };
    // A column list beside `TO` only names the target's columns (SigNoz,
    // HE1 R9.16); an `ENGINE` there is an inner table's, which `TO` excludes.
    if stmt.inner_table && (!inner && !stmt.inner_clauses.is_empty()) {
        return Err(not_implemented(
            "A materialized view with TO takes no ENGINE: its rows go to the TO table",
        ));
    }
    let db_of = |db: Option<String>| db.unwrap_or_else(|| current.to_string());
    let database = db_of(stmt.database.clone());
    let target_database = db_of(to_db);
    ddl::check_new_name("table", &stmt.name)?;
    lake.require_database(namespace, &database).await?;
    let exists = (!inner && lake.load(namespace, &database, &stmt.name).await?.is_some())
        || find_view(lake, namespace, &database, &stmt.name)
            .await?
            .is_some();
    if exists {
        return if stmt.if_not_exists {
            Ok(())
        } else {
            Err(HouseError::from(ChError::table_already_exists(format!(
                "Table {database}.{} already exists",
                stmt.name
            ))))
        };
    }
    // §49 §12: materialized views per namespace (HS1 Task 21).
    let cap = lake.object_caps().views;
    if view_count(lake, namespace).await? >= cap {
        return Err(bad(format!(
            "Too many materialized views: a namespace holds at most {cap}"
        )));
    }
    // The first name after a FROM that is one of the catalog's tables (HE1
    // R9.14: ClickHouse's trigger table, inside a subquery too).
    let mut found = None;
    for (db, name) in &stmt.candidates {
        let db = db_of(db.clone());
        if lake.database_exists(namespace, &db).await?
            && lake.load(namespace, &db, name).await?.is_some()
        {
            found = Some((Some(db), name.clone()));
            break;
        }
    }
    let Some((source_db, source)) = found.or_else(|| stmt.source.clone()) else {
        return Err(not_implemented(
            "A materialized view's SELECT reads its source first: … FROM <pipe>",
        ));
    };
    let source_db = db_of(source_db);
    let source_table = lake
        .load(namespace, &source_db, &source)
        .await?
        .ok_or_else(|| {
            HouseError::from(ChError::unknown_table(format!(
                "Table {source_db}.{source} does not exist"
            )))
        })?;
    let source_class = ddl::class_of(&source_table.metadata);
    if !matches!(source_class, TableClass::Pipe | TableClass::Lake) {
        return Err(not_implemented(
            "A materialized view reads a pipe or a lake table the House writes",
        ));
    }
    let target = lake
        .lake_table(
            namespace,
            &target_database,
            &to,
            "CREATE MATERIALIZED VIEW … TO",
        )
        .await?;
    if ddl::class_of(&target.metadata) != TableClass::Lake {
        return Err(not_implemented(
            "A materialized view writes into a lake table",
        ));
    }
    if target.ident == source_table.ident {
        return Err(bad(format!(
            "Materialized view {}.{} would read and write {target_database}.{to}",
            database, stmt.name
        )));
    }
    let view = MaterializedView {
        id: uuid::Uuid::new_v4().simple().to_string(),
        database,
        name: stmt.name.clone(),
        target_database,
        target: to,
        select: stmt.select.clone(),
        inner,
        refresh: None,
    };
    if source_class == TableClass::Lake {
        // A view over a lake table starts at the source's current snapshot
        // (no backfill, as ClickHouse without POPULATE): its cursor goes into
        // the target first, so a view is never recorded without one.
        crate::mv::start_cursor(lake, &target, &view, &source_table).await?;
    }
    lake.catalog()
        .commit(
            &source_table.ident,
            Vec::new(),
            vec![TableUpdate::SetProperties {
                updates: HashMap::from([(
                    format!("{MV_PREFIX}{}", view.id),
                    serde_json::to_string(&view).map_err(|e| bad(e.to_string()))?,
                )]),
            }],
        )
        .await?;
    Ok(())
}

/// `CREATE MATERIALIZED VIEW … REFRESH EVERY … APPEND TO t [(cols)] [EMPTY] AS
/// SELECT …` (HE1 R9.64): recorded on `t` with its schedule (see
/// [`crate::refresh`]).
async fn create_refreshable(
    lake: &Lake,
    namespace: u64,
    current: &str,
    stmt: &ddl::CreateMaterializedView,
) -> Result<(), HouseError> {
    let mut schedule = match stmt.schedule.clone() {
        Some(Ok(schedule)) => schedule,
        Some(Err(message)) => return Err(not_implemented(message)),
        None => return Err(not_implemented("Expected EVERY after REFRESH")),
    };
    schedule.empty = stmt.empty;
    if stmt.populate {
        return Err(not_implemented(
            "POPULATE is not supported (a refreshable view without EMPTY runs once at CREATE)",
        ));
    }
    let Some((to_db, to)) = stmt.to.clone() else {
        return Err(not_implemented(
            "A refreshable materialized view on the House appends into a lake table: … REFRESH \
             EVERY … APPEND TO <table> AS SELECT …",
        ));
    };
    if !stmt.inner_clauses.is_empty() {
        return Err(not_implemented(
            "A materialized view with TO takes no ENGINE: its rows go to the TO table",
        ));
    }
    let db_of = |db: Option<String>| db.unwrap_or_else(|| current.to_string());
    let database = db_of(stmt.database.clone());
    let target_database = db_of(to_db);
    ddl::check_new_name("table", &stmt.name)?;
    lake.require_database(namespace, &database).await?;
    let exists = lake.load(namespace, &database, &stmt.name).await?.is_some()
        || find_view(lake, namespace, &database, &stmt.name)
            .await?
            .is_some();
    if exists {
        return if stmt.if_not_exists {
            Ok(())
        } else {
            Err(HouseError::from(ChError::table_already_exists(format!(
                "Table {database}.{} already exists",
                stmt.name
            ))))
        };
    }
    let cap = lake.object_caps().views;
    if view_count(lake, namespace).await? >= cap {
        return Err(bad(format!(
            "Too many materialized views: a namespace holds at most {cap}"
        )));
    }
    let target = lake
        .lake_table(
            namespace,
            &target_database,
            &to,
            "CREATE MATERIALIZED VIEW … REFRESH … TO",
        )
        .await?;
    if ddl::class_of(&target.metadata) != TableClass::Lake {
        return Err(not_implemented(
            "A materialized view writes into a lake table",
        ));
    }
    let first = if schedule.empty {
        schedule
            .nominal_at(crate::retention::now_ms().div_euclid(1000))
            .to_string()
    } else {
        crate::refresh::RUN_NOW.to_string()
    };
    let view = MaterializedView {
        id: uuid::Uuid::new_v4().simple().to_string(),
        database,
        name: stmt.name.clone(),
        target_database,
        target: to,
        select: stmt.select.clone(),
        inner: false,
        refresh: Some(schedule),
    };
    lake.catalog()
        .commit(
            &target.ident,
            Vec::new(),
            vec![TableUpdate::SetProperties {
                updates: HashMap::from([
                    (
                        crate::refresh::record_key(&view),
                        serde_json::to_string(&view).map_err(|e| bad(e.to_string()))?,
                    ),
                    (crate::refresh::at_key(&view), first),
                ]),
            }],
        )
        .await?;
    Ok(())
}

/// `ALTER TABLE mv MODIFY QUERY SELECT …` (HE1 R9.12): the view's `SELECT`, which
/// must still read the view's source first (the view lives on its source, and
/// its cursor counts that source's snapshots). The cursor is kept: the new query
/// applies to the snapshots committed after it, as in ClickHouse.
pub async fn modify_materialized_view_query(
    lake: &Lake,
    namespace: u64,
    database: &str,
    name: &str,
    select: &str,
) -> Result<(), HouseError> {
    let Some((source, mut view)) = find_view(lake, namespace, database, name).await? else {
        return Err(HouseError::from(ChError::unknown_table(format!(
            "Table {database}.{name} is not a materialized view: MODIFY QUERY changes one"
        ))));
    };
    let probe = format!("CREATE MATERIALIZED VIEW loams_probe TO loams_probe AS {select}");
    let parsed = ddl::parse_create_mv(&probe)?;
    // A refreshable view reads what it likes; it stays on its target.
    if view.refresh.is_some() {
        view.select = parsed.select;
        let record = serde_json::to_string(&view).map_err(|e| bad(e.to_string()))?;
        lake.catalog()
            .commit(
                &source.ident,
                Vec::new(),
                vec![TableUpdate::SetProperties {
                    updates: HashMap::from([(crate::refresh::record_key(&view), record)]),
                }],
            )
            .await?;
        return Ok(());
    }
    // The new SELECT's source, as CREATE picks one: the first FROM name that
    // is a catalog table.
    let mut new_source = None;
    for (db, table) in &parsed.candidates {
        let db = db.clone().unwrap_or_else(|| view.database.clone());
        if lake.database_exists(namespace, &db).await?
            && let Some(loaded) = lake.load(namespace, &db, table).await?
        {
            new_source = Some(loaded);
            break;
        }
    }
    let Some(new_source) = new_source else {
        return Err(not_implemented(
            "A materialized view's SELECT reads its source first: … FROM <table>",
        ));
    };
    view.select = parsed.select;
    let record = serde_json::to_string(&view).map_err(|e| bad(e.to_string()))?;
    let key = format!("{MV_PREFIX}{}", view.id);
    if new_source.ident == source.ident {
        lake.catalog()
            .commit(
                &source.ident,
                Vec::new(),
                vec![TableUpdate::SetProperties {
                    updates: HashMap::from([(key, record)]),
                }],
            )
            .await?;
        return Ok(());
    }
    // A new source (SigNoz's `time_series_v4_1day_mv`, HE1 R9.16): the view
    // moves to it and reads from its current snapshot on, as ClickHouse's
    // `MODIFY QUERY` makes the new table the trigger.
    if !matches!(
        ddl::class_of(&new_source.metadata),
        TableClass::Pipe | TableClass::Lake
    ) {
        return Err(not_implemented(
            "A materialized view reads a pipe or a lake table the House writes",
        ));
    }
    let target = lake
        .lake_table(
            namespace,
            &view.target_database,
            &view.target,
            "MODIFY QUERY … TO",
        )
        .await?;
    if target.ident == new_source.ident {
        return Err(bad(format!(
            "Materialized view {database}.{name} would read and write {}.{}",
            view.target_database, view.target
        )));
    }
    if ddl::class_of(&new_source.metadata) == TableClass::Lake {
        crate::mv::start_cursor(lake, &target, &view, &new_source).await?;
    }
    lake.catalog()
        .commit(
            &new_source.ident,
            Vec::new(),
            vec![TableUpdate::SetProperties {
                updates: HashMap::from([(key.clone(), record)]),
            }],
        )
        .await?;
    lake.catalog()
        .commit(
            &source.ident,
            Vec::new(),
            vec![TableUpdate::RemoveProperties {
                removals: vec![key],
            }],
        )
        .await?;
    Ok(())
}

/// `DROP VIEW` (or `DROP TABLE`) of a materialized view: whether there was one.
pub async fn drop_materialized_view(
    lake: &Lake,
    namespace: u64,
    database: &str,
    name: &str,
) -> Result<bool, HouseError> {
    let Some((source, view)) = find_view(lake, namespace, database, name).await? else {
        return Ok(false);
    };
    // A refreshable view is recorded on its target, with its last run.
    if view.refresh.is_some() {
        lake.catalog()
            .commit(
                &source.ident,
                Vec::new(),
                vec![TableUpdate::RemoveProperties {
                    removals: vec![
                        crate::refresh::record_key(&view),
                        crate::refresh::at_key(&view),
                    ],
                }],
            )
            .await?;
        lake.pipe_states().forget(namespace, database, name);
        return Ok(true);
    }
    // An inner-table view takes its table with it (HE1 R9.14).
    if view.inner {
        lake.drop_table(namespace, &view.target_database, &view.target, true)
            .await?;
    }
    lake.catalog()
        .commit(
            &source.ident,
            Vec::new(),
            vec![TableUpdate::RemoveProperties {
                removals: vec![format!("{MV_PREFIX}{}", view.id)],
            }],
        )
        .await?;
    lake.pipe_states().forget(namespace, database, name);
    Ok(true)
}

// ---------------------------------------------------------------------------
// The batch a view's SELECT reads.

/// A pipe's records for one run of a view: the view DDL that makes the pipe's
/// name read them (through `house-cache`, like a lake table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeBatch {
    /// The pipe's database.
    pub database: String,
    /// The pipe.
    pub name: String,
    /// `CREATE OR REPLACE VIEW db.q (…) SQL SECURITY NONE AS SELECT … FROM s3(…)`.
    pub view: String,
}

impl PipeBatch {
    /// The batch object at `location` read as `pipe`'s rows, through the
    /// worker's `endpoint`.
    pub fn new(
        pipe: &Pipe,
        location: &str,
        endpoint: &str,
        root: &crate::catalog::LakeRoot,
    ) -> Result<Self, HouseError> {
        let key = root
            .relative(location)
            .ok_or_else(|| bad("the batch is outside the House's store"))?;
        let url = format!(
            "{}/{}/{}",
            endpoint.trim_end_matches('/'),
            root.bucket(),
            key.trim_matches('/')
        );
        let PipeFeed::Stream(source) = &pipe.source else {
            return Err(bad("a stream batch for a pipe that reads no stream"));
        };
        Self::over(pipe, &url, source.format.input())
    }

    /// The objects `url` names (through `house-cache`) read as `pipe`'s rows
    /// in `format`.
    pub fn over(pipe: &Pipe, url: &str, format: &str) -> Result<Self, HouseError> {
        let ch = ChTable::from_lake(&pipe.database, &pipe.name, &pipe.table);
        let mut typed = Vec::new();
        let mut names = Vec::new();
        for column in &ch.columns {
            let ty = crate::views::printable_type(&column.ch_type).ok_or_else(|| {
                bad(format!(
                    "Column {} of the pipe has no printable type",
                    column.name
                ))
            })?;
            typed.push(format!("{} {ty}", quote_name(&column.name)));
            names.push(quote_name(&column.name));
        }
        if typed.is_empty() {
            return Err(bad("A pipe needs at least one column"));
        }
        let structure = typed.join(", ");
        let view = format!(
            "CREATE OR REPLACE VIEW {}.{} ({structure}) SQL SECURITY NONE AS\nSELECT {}\nFROM \
             s3({}, 'dummy', 'dummy', {}, {})\nSETTINGS input_format_skip_unknown_fields = 1",
            quote_name(&pipe.database),
            quote_name(&pipe.name),
            names.join(", "),
            quote_string(url),
            quote_string(format),
            quote_string(&structure),
        );
        Ok(Self {
            database: pipe.database.clone(),
            name: pipe.name.clone(),
            view,
        })
    }
}

/// The refusal of a direct read of a pipe.
pub fn direct_read(database: &str, name: &str) -> HouseError {
    ddl::clickhouse_error(
        620,
        "QUERY_NOT_ALLOWED",
        format!(
            "Direct select is not allowed: {database}.{name} is a pipe, read only by its \
             materialized views"
        ),
    )
}

// ---------------------------------------------------------------------------
// State.

/// What a front knows of one view's runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipeState {
    /// Records the stream holds past the cursor, at the last look.
    pub lag_records: u64,
    /// When the last batch committed, in Unix milliseconds.
    pub last_commit_ms: Option<i64>,
    /// Empty when the last run succeeded.
    pub last_error: String,
    /// Rows this front committed.
    pub committed_rows: u64,
    /// Records this front committed.
    pub committed_records: u64,
}

/// The views' states, by `(namespace, database, view)`.
#[derive(Debug, Default)]
pub struct PipeStates(Mutex<BTreeMap<(u64, String, String), PipeState>>);

impl PipeStates {
    /// One view's state.
    pub fn get(&self, namespace: u64, database: &str, name: &str) -> PipeState {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(namespace, database.to_string(), name.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    /// Every view's state this front holds, by `(namespace, database, view)`.
    pub fn all(&self) -> Vec<((u64, String, String), PipeState)> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Forgets a dropped view's state, so its metrics series ends.
    pub(crate) fn forget(&self, namespace: u64, database: &str, name: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&(
            namespace,
            database.to_string(),
            name.to_string(),
        ));
    }

    pub(crate) fn update(
        &self,
        namespace: u64,
        view: &MaterializedView,
        f: impl FnOnce(&mut PipeState),
    ) {
        let mut states = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(states
            .entry((namespace, view.database.clone(), view.name.clone()))
            .or_default());
    }
}

/// One row of `system.loams_pipes` (every materialized view) and `ListPipes`
/// (the views over pipes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeRow {
    /// The view's database.
    pub database: String,
    /// The view.
    pub name: String,
    /// `db.pipe`: its source, a pipe or a lake table.
    pub pipe: String,
    /// The source's engine.
    pub source: String,
    /// Whether the source is a pipe (not a lake table, HS1 Task 17).
    pub over_pipe: bool,
    /// `db.target`.
    pub target: String,
    /// This front's state.
    pub state: PipeState,
}

/// The namespace's materialized views — over pipes and over lake tables (HS1
/// Task 23, ruling R23.1) — with this front's state.
pub async fn rows(lake: &Lake, namespace: u64) -> Result<Vec<PipeRow>, HouseError> {
    let states = lake.pipe_states();
    let mut out = Vec::new();
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            let Some(table) = lake.load(namespace, &database.name, &name).await? else {
                continue;
            };
            let views = views_of(&table);
            if views.is_empty() {
                continue;
            }
            let ch = ChTable::from_lake(&database.name, &name, &table);
            let (source, over_pipe) = match Pipe::from_table(&database.name, &name, table) {
                Some(pipe) => (pipe.source.render(), true),
                None => (ch.engine, false),
            };
            for view in views {
                out.push(PipeRow {
                    state: states.get(namespace, &view.database, &view.name),
                    pipe: format!("{}.{name}", database.name),
                    source: source.clone(),
                    over_pipe,
                    target: format!("{}.{}", view.target_database, view.target),
                    database: view.database,
                    name: view.name,
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.database, &a.name).cmp(&(&b.database, &b.name)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// The runner.

/// The steps of a run, where tests stop a front (HS1 Task 16's kill matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The records are fetched.
    Fetched,
    /// The batch object is in the bucket.
    Uploaded,
    /// The view's rows are written as files.
    Written,
    /// The commit is about to be sent.
    BeforeCommit,
    /// The commit landed; the batch object is not yet deleted.
    Committed,
}

/// A test's hand in a run: an error stops the run there, as a front that
/// died would.
#[async_trait::async_trait]
pub trait Hooks: Send + Sync + fmt::Debug {
    /// Called at each step.
    async fn at(&self, step: Step, view: &MaterializedView) -> Result<(), HouseError>;
}

/// What one run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// No records past the cursor.
    Idle,
    /// A batch committed: records and rows.
    Committed {
        /// Records consumed.
        records: u64,
        /// Rows written into the target.
        rows: u64,
    },
    /// Another runner committed these records first.
    Abandoned,
}

/// The pipes' runner (see the module docs).
#[derive(Debug, Clone)]
pub struct PipeRunner {
    lake: Arc<Lake>,
    writer: ViewWriter,
    link: EngineLink,
    source: Arc<dyn StreamSource>,
    hooks: Option<Arc<dyn Hooks>>,
}

impl PipeRunner {
    /// A runner over `lake`'s pipes, reading `source`.
    pub fn new(
        lake: Arc<Lake>,
        pool: WorkerPool,
        config: HouseConfig,
        link: EngineLink,
        source: Arc<dyn StreamSource>,
    ) -> Self {
        Self {
            writer: ViewWriter {
                lake: Arc::clone(&lake),
                pool,
                limits: config.session_limits,
                max_insert_bytes: config.lake_insert_max_bytes,
            },
            lake,
            link,
            source,
            hooks: None,
        }
    }

    /// With a test's hooks.
    pub fn with_hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Runs every `every` until the returned task is aborted.
    pub fn spawn(self, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                self.round().await;
            }
        })
    }

    /// One round: every view of every mapped namespace, once.
    pub async fn round(&self) {
        for &namespace in self.link.namespaces.keys() {
            let pipes = match pipes(&self.lake, namespace).await {
                Ok(pipes) => pipes,
                Err(err) => {
                    eprintln!(
                        "loams-house: pipes of namespace {namespace} cannot be listed: {}",
                        err.message()
                    );
                    continue;
                }
            };
            for pipe in &pipes {
                for view in &pipe.views {
                    let _ = self.run(namespace, pipe, view).await;
                }
            }
        }
    }

    async fn hook(&self, step: Step, view: &MaterializedView) -> Result<(), HouseError> {
        match &self.hooks {
            Some(hooks) => hooks.at(step, view).await,
            None => Ok(()),
        }
    }

    /// One run of `view`, its state recorded.
    pub async fn run(
        &self,
        namespace: u64,
        pipe: &Pipe,
        view: &MaterializedView,
    ) -> Result<Ran, HouseError> {
        if !matches!(pipe.source, PipeFeed::Stream(_)) {
            // `S3Queue` pipes are the queue runner's, their state its own.
            return Ok(Ran::Idle);
        }
        let result = self.run_inner(namespace, pipe, view).await;
        let states = self.lake.pipe_states();
        states.update(namespace, view, |state| match &result {
            Ok(Ran::Committed { records, rows }) => {
                state.last_error.clear();
                state.last_commit_ms = Some(now_ms());
                state.committed_records += records;
                state.committed_rows += rows;
            }
            Ok(_) => state.last_error.clear(),
            Err(err) => state.last_error = err.message().to_string(),
        });
        result
    }

    async fn run_inner(
        &self,
        namespace: u64,
        pipe: &Pipe,
        view: &MaterializedView,
    ) -> Result<Ran, HouseError> {
        let Some(engine_namespace) = self.link.namespaces.get(&namespace) else {
            return Ok(Ran::Idle);
        };
        let PipeFeed::Stream(source) = &pipe.source else {
            return Ok(Ran::Idle);
        };
        let lake = &self.lake;
        let target = lake
            .lake_table(
                namespace,
                &view.target_database,
                &view.target,
                "a materialized view's target",
            )
            .await?;
        let cursor = cursor_of(&target, view);
        let events = source.format == PipeFormat::CloudEvents;

        // The records past the cursor.
        let bounds = self
            .source
            .partitions(engine_namespace, &source.stream)
            .await?;
        let mut data: Vec<u8> = Vec::new();
        let mut rows = 0u64;
        let mut next: BTreeMap<u32, u64> = BTreeMap::new();
        let mut lag = 0u64;
        for b in &bounds {
            let start = cursor
                .get(&b.partition)
                .copied()
                .unwrap_or(0)
                .max(b.log_start_offset);
            let mut offset = start;
            let mut high = b.high_watermark;
            while offset < high && rows < pipe.max_rows && data.len() < MAX_BATCH_BYTES {
                let fetched = self
                    .source
                    .fetch(
                        engine_namespace,
                        &source.stream,
                        b.partition,
                        offset,
                        FETCH_BYTES,
                        events,
                    )
                    .await?;
                high = high.max(fetched.high_watermark);
                if fetched.next_offset <= offset {
                    break;
                }
                // At most the rows the batch still has room for, exactly:
                // the cursor then stops at the last one taken.
                let room = (pipe.max_rows - rows) as usize;
                let taken = fetched.rows.len().min(room);
                for row in &fetched.rows[..taken] {
                    data.extend_from_slice(row);
                }
                rows += taken as u64;
                offset = if taken < fetched.rows.len() {
                    offset + taken as u64
                } else {
                    fetched.next_offset
                };
            }
            if offset > 0 || cursor.contains_key(&b.partition) {
                next.insert(b.partition, offset);
            }
            lag += high.saturating_sub(offset);
        }
        lake.pipe_states()
            .update(namespace, view, |state| state.lag_records = lag);
        let moved = next.iter().any(|(p, o)| cursor.get(p) != Some(o));
        if !moved {
            return Ok(Ran::Idle);
        }
        // A batch short of `loams_pipe_max_rows` waits for
        // `loams_pipe_max_delay_ms` after this front's last commit.
        if rows > 0 && rows < pipe.max_rows {
            let last = lake
                .pipe_states()
                .get(namespace, &view.database, &view.name)
                .last_commit_ms;
            if last.is_some_and(|at| now_ms().saturating_sub(at) < pipe.max_delay_ms as i64) {
                return Ok(Ran::Idle);
            }
        }
        self.hook(Step::Fetched, view).await?;

        let committer = lake.committer();
        let io = committer.io();
        let mut written = None;
        let mut batch_location = None;
        if rows > 0 {
            let location = format!(
                "{}/data/__loams_pipe_{}.{}",
                target.metadata.location().trim_end_matches('/'),
                uuid::Uuid::new_v4().simple(),
                if source.format == PipeFormat::CSV {
                    "csv"
                } else {
                    "jsonl"
                }
            );
            io.write(&location, bytes::Bytes::from(data)).await?;
            batch_location = Some(location.clone());
            self.hook(Step::Uploaded, view).await?;
            let root = lake.root().clone();
            let make = |endpoint: &str| PipeBatch::new(pipe, &location, endpoint, &root);
            let result = self
                .writer
                .write(namespace, view, &target, Some(&make))
                .await;
            match result {
                Ok(files) => written = Some(files),
                Err(err) => {
                    let _ = io.delete(&location).await;
                    return Err(err);
                }
            }
            self.hook(Step::Written, view).await?;
        }
        let (files, written_rows, written_bytes) = match written {
            Some(w) => (w.data_files, w.rows, w.input_bytes),
            None => (Vec::new(), 0, 0),
        };
        self.hook(Step::BeforeCommit, view).await?;
        let outcome = commit(io, &target.ident, view, &cursor, &next, files).await;
        if let Ok(Ran::Committed { .. }) = &outcome {
            self.writer
                .report_insert(namespace, view, written_rows, written_bytes);
            self.hook(Step::Committed, view).await?;
            committer.mark_dirty(&target.ident);
            // The target's own views (HS1 Task 17); their errors are theirs.
            let views = crate::mv::ViewRunner::new(self.writer.clone());
            let _ = views
                .after_commit(namespace, &view.target_database, &view.target)
                .await;
        }
        if let Some(location) = batch_location {
            let _ = io.delete(&location).await;
        }
        match outcome? {
            Ran::Committed { .. } => Ok(Ran::Committed {
                records: next
                    .iter()
                    .map(|(p, o)| o.saturating_sub(cursor.get(p).copied().unwrap_or(0)))
                    .sum(),
                rows: written_rows,
            }),
            other => Ok(other),
        }
    }
}

/// Makes a view's batch from the worker's bucket endpoint.
pub type MakeBatch<'a> = dyn Fn(&str) -> Result<PipeBatch, HouseError> + Sync + 'a;

/// Writes a materialized view's `SELECT` over a batch as files of its target
/// (HS1 Tasks 16 and 17): the pipes' runner and the views over lake tables
/// share it.
#[derive(Debug, Clone)]
pub struct ViewWriter {
    /// The catalog.
    pub lake: Arc<Lake>,
    /// The workers.
    pub pool: WorkerPool,
    /// The caps a statement's settings are held to.
    pub limits: crate::settings::SessionLimits,
    /// The most one write takes, as Arrow bytes.
    pub max_insert_bytes: u64,
}

impl ViewWriter {
    /// `house.insert` for `rows` (`bytes` as the worker produced them) a pipe
    /// or a materialized view committed into `database.table` (HS1 Task 22).
    pub fn report_insert(&self, namespace: u64, view: &MaterializedView, rows: u64, bytes: u64) {
        if rows == 0 {
            return;
        }
        if let Some(usage) = self.pool.usage() {
            usage.emit(crate::usage::UsageEvent::Insert(
                crate::usage::InsertUsage {
                    namespace: namespace.to_string(),
                    query_id: String::new(),
                    database: view.target_database.clone(),
                    table: view.target.clone(),
                    rows,
                    bytes,
                    at: std::time::SystemTime::now(),
                },
            ));
        }
    }

    /// The view's `SELECT` over the batch, written as files of `target`.
    /// `batch` makes the batch's view from the worker's bucket endpoint.
    pub async fn write(
        &self,
        namespace: u64,
        view: &MaterializedView,
        target: &LakeTable,
        batch: Option<&MakeBatch<'_>>,
    ) -> Result<loams_house_lake::write::Written, HouseError> {
        let mut lease = self.pool.acquire(&namespace.to_string()).await?;
        let endpoint = lease.ready().and_then(|r| r.s3_endpoint.clone());
        let Some(endpoint) = endpoint else {
            self.pool.release(lease, Outcome::Completed);
            return Err(not_implemented(
                "Pipes need process workers that read the bucket (--workers=process with \
                 --store)",
            ));
        };
        // A refreshable view reads its tables as they are (no batch).
        let batch = match batch.map(|make| make(&endpoint)).transpose() {
            Ok(batch) => batch,
            Err(err) => {
                self.pool.release(lease, Outcome::Completed);
                return Err(err);
            }
        };
        let system = UserMap::system("loams-pipe", namespace);
        let known = self.pool.known_settings();
        let ch = ChTable::from_lake(&view.target_database, &view.target, target);
        // ClickHouse matches a view's columns to its target's by name.
        let names = match self
            .output_names(&mut lease, namespace, view, &system, batch.as_ref())
            .await
        {
            Ok(names) => names,
            Err(err) => {
                self.pool.release(lease, Outcome::Completed);
                return Err(err);
            }
        };
        let stored: HashSet<&str> = ch
            .columns
            .iter()
            .filter(|c| c.stored() && c.default_kind != "MATERIALIZED")
            .map(|c| c.name.as_str())
            .collect();
        let columns: Vec<String> = names
            .into_iter()
            .filter(|n| stored.contains(n.as_str()))
            .collect();
        if columns.is_empty() {
            self.pool.release(lease, Outcome::Completed);
            return Err(bad(format!(
                "No column of materialized view {}.{}'s SELECT is a column of {}.{}",
                view.database, view.name, view.target_database, view.target
            )));
        }
        let list = columns
            .iter()
            .map(|c| quote_name(c))
            .collect::<Vec<_>>()
            .join(", ");
        let select = format!("SELECT {list} FROM ({})", view.select);
        let insert = Target {
            database: view.target_database.clone(),
            name: view.target.clone(),
            statement: format!(
                "INSERT INTO {}.{} ({list}) {select}",
                quote_name(&view.target_database),
                quote_name(&view.target)
            ),
            table: target.clone(),
            ch,
            columns: Some(columns),
            settings: Vec::new(),
            source: Source::Select { text: select },
        };
        let query_id = format!("loams-pipe-{}", uuid::Uuid::new_v4());
        let ctx = InsertContext {
            lake: &self.lake,
            pool: &self.pool,
            namespace,
            database: &view.database,
            user: &system,
            query_id: &query_id,
            params: &[],
            settings: &[],
            limits: &self.limits,
            known: &known,
            max_insert_bytes: self.max_insert_bytes,
            pipe_batch: batch.as_ref(),
            admitted: None,
        };
        crate::insert::write_files(&ctx, lease, &insert).await
    }

    /// The names of the view's `SELECT`'s columns over the batch.
    async fn output_names(
        &self,
        lease: &mut WorkerLease,
        namespace: u64,
        view: &MaterializedView,
        user: &UserMap,
        batch: Option<&PipeBatch>,
    ) -> Result<Vec<String>, HouseError> {
        let sql = format!("DESCRIBE TABLE ({})", view.select);
        let analysis = lease
            .analyze(&view.select, &[])
            .await
            .map_err(crate::deny::as_setting_refusal)?;
        crate::gate::admit(
            &analysis,
            &view.select,
            crate::classify::Expect::Read,
            true,
            None,
        )?;
        let plan = crate::read::plan(
            crate::read::ReadContext {
                lake: Some(&self.lake),
                namespace,
                database: &view.database,
                user: &user.user,
                grants: None,
                batch,
                front: None,
                admin: false,
            },
            lease,
            &analysis,
            &sql,
            &[],
        )
        .await?;
        let out = lease
            .run(Execute {
                query_id: format!("loams-pipe-describe-{}", uuid::Uuid::new_v4()),
                session: None,
                settings: Vec::new(),
                views: plan.views,
                sql: plan.sql,
                format: "TSV".to_string(),
                params: Vec::new(),
                limits: Limits::default(),
                input: None,
                output: Output::Blocks,
                database: Some(view.database.clone()),
            })
            .await?;
        Ok(String::from_utf8_lossy(&out.bytes)
            .lines()
            .filter_map(|line| line.split('\t').next())
            .filter(|name| !name.is_empty())
            .map(unescape_tsv)
            .collect())
    }
}

fn unescape_tsv(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The view's cursor in its target: partition → next offset.
pub fn cursor_of(target: &LakeTable, view: &MaterializedView) -> BTreeMap<u32, u64> {
    let prefix = format!("{CURSOR_PREFIX}{}.", view.id);
    target
        .metadata
        .properties()
        .iter()
        .filter_map(|(k, v)| {
            let partition = k.strip_prefix(&prefix)?.parse().ok()?;
            Some((partition, v.parse().ok()?))
        })
        .collect()
}

/// Commits `files` with the pipe's cursor moved from `read` to `next` (see
/// the module docs).
async fn commit(
    io: &crate::commit::CommitIo,
    table: &TableIdent,
    view: &MaterializedView,
    read: &BTreeMap<u32, u64>,
    next: &BTreeMap<u32, u64>,
    files: Vec<loams_house_lake::spec::DataFile>,
) -> Result<Ran, HouseError> {
    let strings = |cursor: &BTreeMap<u32, u64>| -> BTreeMap<String, String> {
        cursor
            .iter()
            .map(|(p, o)| (view.cursor_key(*p), o.to_string()))
            .collect()
    };
    let summary: Vec<(String, String)> = strings(next).into_iter().collect();
    commit_cursor(io, table, &strings(read), &strings(next), summary, files).await
}

/// Commits `files` into `table` with the table properties `next` set, on the
/// condition that those properties still read `read` (a key missing from
/// `read` must be missing). `summary` goes into the new snapshot's summary.
/// A lost commit reloads: properties that moved mean another runner did this
/// work (or this commit landed and its answer was lost: then they read `next`);
/// unmoved means another writer committed, and the append is rebuilt.
pub(crate) async fn commit_cursor(
    io: &crate::commit::CommitIo,
    table: &TableIdent,
    read: &BTreeMap<String, String>,
    next: &BTreeMap<String, String>,
    summary: Vec<(String, String)>,
    files: Vec<loams_house_lake::spec::DataFile>,
) -> Result<Ran, HouseError> {
    let keys: Vec<&String> = read.keys().chain(next.keys()).collect();
    let current = |metadata: &loams_house_lake::spec::TableMetadata| -> BTreeMap<String, String> {
        keys.iter()
            .filter_map(|k| {
                metadata
                    .properties()
                    .get(k.as_str())
                    .map(|v| ((*k).clone(), v.clone()))
            })
            .collect()
    };
    let mut ambiguous = false;
    for _ in 0..COMMIT_ATTEMPTS {
        let loaded = io.catalog.load_table(table).await?;
        let metadata = &loaded.metadata;
        let now = current(metadata);
        if now != *read {
            return Ok(if ambiguous && now == *next {
                Ran::Committed {
                    records: 0,
                    rows: 0,
                }
            } else {
                Ran::Abandoned
            });
        }
        let set_cursor = TableUpdate::SetProperties {
            updates: next.clone().into_iter().collect(),
        };
        let (requirements, updates, uploads) = if files.is_empty() {
            (
                vec![TableRequirement::RefSnapshotIdMatch {
                    r#ref: append::MAIN.to_string(),
                    snapshot_id: metadata.current_snapshot_id(),
                }],
                vec![set_cursor],
                Vec::new(),
            )
        } else {
            let parent_list = match metadata.current_snapshot() {
                Some(snapshot) => Some(io.read(snapshot.manifest_list()).await?),
                None => None,
            };
            let built = append::append_files(
                metadata,
                parent_list.as_deref(),
                files.clone(),
                summary.clone(),
                now_ms(),
            )
            .await
            .map_err(|err| bad(format!("the view's commit could not be built: {err}")))?;
            let mut updates = built.updates;
            updates.push(set_cursor);
            (built.requirements, updates, built.files)
        };
        for (location, bytes) in uploads {
            io.write(&location, bytes).await?;
        }
        match io.catalog.commit(table, requirements, updates).await {
            Ok(_) => {
                return Ok(Ran::Committed {
                    records: 0,
                    rows: 0,
                });
            }
            Err(CatalogError::Conflict { .. }) => {
                let jitter = (uuid::Uuid::new_v4().as_u128() as u64) % 20;
                tokio::time::sleep(Duration::from_millis(jitter)).await;
            }
            Err(CatalogError::Unavailable(_)) => ambiguous = true,
            Err(err) => return Err(err.into()),
        }
    }
    Err(HouseError::from(ChError::network_error(format!(
        "the view's commit into {} lost to other commits {COMMIT_ATTEMPTS} times; it is tried \
         again",
        crate::catalog::table_text(table)
    ))))
}

/// The engine link's stream source.
pub fn engine_source(link: &EngineLink) -> Result<Arc<dyn StreamSource>, HouseError> {
    Ok(Arc::new(EngineStreams::new(&link.url, link.key.clone())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_and_formats() {
        assert!(is_pipe_engine("LoamsStream"));
        assert!(!is_pipe_engine("MergeTree"));
        assert_eq!(PipeFormat::parse("CSV").expect("csv").input(), "CSV");
        assert_eq!(PipeFormat::parse("Avro").expect_err("avro").code(), 48);
        assert_eq!(PipeFormat::parse("Native").expect_err("native").code(), 73);
        let source = PipeSource {
            stream: "orders".to_string(),
            format: PipeFormat::CloudEvents,
            consumer: Some("c1".to_string()),
        };
        assert_eq!(
            source.render(),
            "LoamsStream('orders', 'CloudEvents', 'c1')"
        );
        assert_eq!(unescape_tsv("a\\tb"), "a\tb");
    }
}
