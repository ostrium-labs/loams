//! `INSERT` into lake tables (HS1 Task 12, §49 §6.1).
//!
//! An `INSERT` whose table is a lake table of the namespace's catalog never
//! reaches chDB as written. The front ([`target`]) reads its head — the table,
//! the column list, the statement's own `SETTINGS` — and what follows it: data
//! that streams (`FORMAT f` with the request body, or `VALUES` with the tuples in
//! the body), inline `VALUES`, or a query. Then [`run`], on one worker of the
//! namespace and one House-made worker session:
//!
//! 1. **Admission.** The statement as the client wrote it (its head and `FORMAT`
//!    when its data streams) is classified and deny-checked like any other
//!    (HS1 Tasks 4–5); then every statement the front generates is too, before it
//!    runs.
//! 2. **Staging.** `CREATE TEMPORARY TABLE __loams_insert (…) ENGINE = Memory`
//!    with the table's stored columns: their ClickHouse types (held to
//!    [`views::safe_type`], R-ACL.1) and their `DEFAULT`/`MATERIALIZED`
//!    expressions, printed again from their tree ([`expr::reprint`], R11.11;
//!    the catalog's text is never spliced). The worker's user connection makes
//!    it: `readonly = 2` and `allow_ddl = 0` allow temporary tables and nothing
//!    else (R5.6, R-ACL.1).
//! 3. **Rows.** The data streams into it with `chdb_stream_insert` (R1.7), so
//!    chDB parses every input format, applies defaults for omitted columns and
//!    refuses what does not fit, exactly as ClickHouse would; or `INSERT INTO
//!    __loams_insert … SELECT …` runs the query over the namespace's pinned read
//!    views (HS1 Task 11), as any query.
//! 4. **Files.** `SELECT CAST(c, '<the Iceberg type's ClickHouse type>') AS c, …
//!    FROM __loams_insert ORDER BY <sorting key>, <insert order>` — merged
//!    first as ClickHouse merges an inserted block ([`merged_on_insert`]) —
//!    comes back as one `ArrowStream`
//!    document, which `loams_house_lake::write::DataWriter` turns into Parquet
//!    data files with Iceberg field ids and statistics, split by partition and
//!    size, as it arrives.
//! 5. **Commit.** The front uploads the files to the table's `data/` directory
//!    through its own store (the worker holds no write grant and no credentials,
//!    R1.5, R-ACL.1) and hands them to the table's [`crate::commit`] queue: group
//!    commit, deduplication and rebase-retry. The client is answered after the
//!    commit (`X-ClickHouse-Summary` with `written_rows`, `X-Loams-Snapshot-Id`),
//!    or once queued when `async_insert = 1, wait_for_async_insert = 0`.
//!
//! One statement writes at most [`DEFAULT_MAX_INSERT_BYTES`] of rows as Arrow
//! (`36` past it, ruling R12.2): the staging table and the files are held in
//! memory until the commit.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use loams_house_ipc::{Execute, InputSpec, Limits, Output, Progress, SessionRef};
use loams_house_lake::spec::{PrimitiveType, Type};
use loams_house_lake::write::DataWriter;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::admission::{Event, Outcome, WorkerLease, WorkerPool};
use crate::catalog::{Lake, LakeTable};
use crate::classify::InsertStmt;
use crate::commit::Pending;
use crate::ddl::{self, ChTable, InsertRest, TableClass, quote_name, quote_string};
use crate::errors::{ChError, HouseError};
use crate::settings::SessionLimits;
use crate::watchdog::ExitReason;
use crate::{dedup, deny, expr, gate, settings, types, views};

/// The temporary table an `INSERT`'s rows land in on its worker.
pub const STAGING: &str = "__loams_insert";
/// `loams_lake_commit_interval_ms`'s default (Shared contracts).
pub const DEFAULT_COMMIT_INTERVAL_MS: u64 = 1_000;
/// `loams_lake_commit_bytes`'s default (Shared contracts).
pub const DEFAULT_COMMIT_BYTES: u64 = 256 * 1024 * 1024;
/// `loams_lake_target_file_bytes`'s default (Shared contracts).
pub const DEFAULT_TARGET_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// The most one statement writes, as the worker's Arrow bytes (R12.2).
pub const DEFAULT_MAX_INSERT_BYTES: u64 = 1024 * 1024 * 1024;
/// The House sessions an `INSERT` uses on its worker: never a client's key
/// (those start with the user name's length).
const SESSION_PREFIX: &str = "\u{0}loams-insert/";
/// How long a staging session may idle on a worker if its `INSERT` is abandoned.
const SESSION_TIMEOUT_MS: u64 = 600_000;
/// The settings an `INSERT` into a lake table takes for itself; none reaches
/// chDB.
const CONSUMED: &[&str] = &[
    crate::read::SEQUENCED,
    "insert_deduplicate",
    "insert_deduplication_token",
    "async_insert",
    "wait_for_async_insert",
];
/// How the worker writes the rows: one uncompressed Arrow stream, strings as
/// bytes (the lake checks them for UTF-8), no dictionaries.
const ARROW_SETTINGS: &[(&str, &str)] = &[
    ("output_format_arrow_compression_method", "none"),
    ("output_format_arrow_string_as_string", "0"),
    ("output_format_arrow_fixed_string_as_fixed_byte_array", "1"),
    ("output_format_arrow_low_cardinality_as_dictionary", "0"),
];

/// Where an `INSERT`'s rows come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Data in `format` that streams after the statement (a request body).
    Stream {
        /// The data's format.
        format: String,
    },
    /// `VALUES` tuples written in the statement.
    Inline {
        /// The tuples.
        data: Vec<u8>,
    },
    /// A query.
    Select {
        /// Its text.
        text: String,
    },
}

/// An `INSERT` into a lake table, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The table's database.
    pub database: String,
    /// The table's name.
    pub name: String,
    /// The table as the catalog holds it.
    pub table: LakeTable,
    /// Its ClickHouse columns and keys.
    pub ch: ChTable,
    /// The column list, when the statement has one.
    pub columns: Option<Vec<String>>,
    /// The statement's own `SETTINGS`.
    pub settings: Vec<(String, String)>,
    /// Where the rows come from.
    pub source: Source,
    /// The statement as ClickHouse will be asked to classify it: the head and
    /// `FORMAT` when the data streams, else the whole statement.
    pub statement: String,
}

/// The `INSERT` of `insert` when its table is a lake table of the catalog;
/// `None` when it is not the catalog's (a temporary table, a table function, a
/// name chDB will answer for). An external table is `497`.
pub async fn target(
    lake: &Lake,
    user: &crate::config::UserMap,
    current: &str,
    insert: &InsertStmt,
) -> Result<Option<Target>, HouseError> {
    let namespace = user.namespace;
    let Some((db, name)) = ddl::insert_target(&insert.text) else {
        return Ok(None);
    };
    let database = db.unwrap_or_else(|| current.to_string());
    if !lake.database_exists(namespace, &database).await? {
        return Ok(None);
    }
    let Some(table) = lake.load(namespace, &database, &name).await? else {
        return Ok(None);
    };
    // The identity's grants (HS1 Task 20).
    if !crate::identity::granted(user, &database, &name) {
        return Err(crate::identity::not_granted(user, &database, &name));
    }
    // HE1 R9.13: a `Distributed` table writes its local table; a view takes no
    // rows, as in ClickHouse.
    let (database, name, table) = if ddl::class_of(&table.metadata) == TableClass::View {
        let Some((local_db, local)) = ddl::distributed_target(&table) else {
            return Err(HouseError::from(ChError::not_implemented(format!(
                "INSERT INTO {database}.{name}: it is a view, which holds no rows"
            ))));
        };
        if !crate::identity::granted(user, &local_db, &local) {
            return Err(crate::identity::not_granted(user, &local_db, &local));
        }
        let table = lake
            .lake_table(
                namespace,
                &local_db,
                &local,
                "INSERT INTO a Distributed table over",
            )
            .await?;
        (local_db, local, table)
    } else {
        (database, name, table)
    };
    if ddl::class_of(&table.metadata) == TableClass::Pipe {
        return Err(HouseError::from(ChError::not_implemented(format!(
            "INSERT INTO {database}.{name}: the table is a pipe, which reads its stream; \
             write to the stream instead"
        ))));
    }
    if ddl::class_of(&table.metadata) == TableClass::External {
        return Err(HouseError::from(ChError::access_denied(format!(
            "INSERT INTO {database}.{name}: the table belongs to another writer (an external \
             table), so the House reads it and never changes it"
        ))));
    }
    let head = ddl::parse_insert(&insert.text)?;
    let (source, statement) = match (&insert.format, head.rest) {
        (Some(format), InsertRest::Nothing) => (
            Source::Stream {
                format: format.clone(),
            },
            format!("{} FORMAT {format}", insert.text),
        ),
        (None, InsertRest::Values(at)) => (
            Source::Inline {
                data: insert.text.as_bytes()[at..].to_vec(),
            },
            insert.text.clone(),
        ),
        (None, InsertRest::Select(at)) => (
            Source::Select {
                text: insert.text[at..].to_string(),
            },
            insert.text.clone(),
        ),
        _ => {
            return Err(HouseError::from(ChError::syntax_error(
                "INSERT into a lake table needs its data: FORMAT <format> with a body, VALUES, \
                 or a SELECT",
            )));
        }
    };
    if let Source::Stream { format } = &source
        && !format
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(HouseError::from(ChError::unknown_format(format!(
            "Unknown input format {format}"
        ))));
    }
    let ch = ChTable::from_lake(&database, &name, &table);
    Ok(Some(Target {
        database,
        name,
        table,
        ch,
        columns: head.columns,
        settings: head.settings,
        source,
        statement,
    }))
}

/// What an `INSERT` asked of the House, from its settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertSettings {
    /// `insert_deduplicate` (default on, as for replicated tables).
    pub deduplicate: bool,
    /// `insert_deduplication_token`.
    pub token: Option<String>,
    /// `async_insert`.
    pub async_insert: bool,
    /// `wait_for_async_insert` (default on).
    pub wait: bool,
    /// `loams_lake_commit_interval_ms`.
    pub commit_interval: Duration,
    /// `loams_lake_commit_bytes`.
    pub commit_bytes: u64,
    /// `loams_lake_target_file_bytes`.
    pub target_file_bytes: u64,
    /// `loams_dedup_window`.
    pub dedup_window: usize,
}

impl Default for InsertSettings {
    fn default() -> Self {
        Self {
            deduplicate: true,
            token: None,
            async_insert: false,
            wait: true,
            commit_interval: Duration::from_millis(DEFAULT_COMMIT_INTERVAL_MS),
            commit_bytes: DEFAULT_COMMIT_BYTES,
            target_file_bytes: DEFAULT_TARGET_FILE_BYTES,
            dedup_window: dedup::DEFAULT_WINDOW,
        }
    }
}

fn bad_value(name: &str, value: &str, kind: &str) -> HouseError {
    HouseError::from(ChError::bad_arguments(format!(
        "Cannot parse '{value}' as the value of setting {name}: expected {kind}"
    )))
}

fn boolean(name: &str, value: &str) -> Result<bool, HouseError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(bad_value(name, value, "0 or 1")),
    }
}

fn bounded(name: &str, value: &str, min: u64, max: u64) -> Result<u64, HouseError> {
    let n: u64 = value
        .parse()
        .map_err(|_| bad_value(name, value, "a whole number"))?;
    if n < min || n > max {
        return Err(HouseError::from(ChError::readonly(format!(
            "Setting {name} must be between {min} and {max}"
        ))));
    }
    Ok(n)
}

impl InsertSettings {
    /// The settings of `pairs`, later ones winning.
    pub fn from_pairs(pairs: &[(String, String)]) -> Result<Self, HouseError> {
        let mut out = Self::default();
        for (name, value) in pairs {
            match name.as_str() {
                "insert_deduplicate" => out.deduplicate = boolean(name, value)?,
                "insert_deduplication_token" => {
                    out.token = (!value.is_empty()).then(|| value.clone());
                }
                "async_insert" => out.async_insert = boolean(name, value)?,
                "wait_for_async_insert" => out.wait = boolean(name, value)?,
                "loams_lake_commit_interval_ms" => {
                    out.commit_interval = Duration::from_millis(bounded(name, value, 0, 60_000)?);
                }
                "loams_lake_commit_bytes" => {
                    out.commit_bytes = bounded(name, value, 1, DEFAULT_MAX_INSERT_BYTES)?;
                }
                "loams_lake_target_file_bytes" => {
                    out.target_file_bytes =
                        bounded(name, value, 1024 * 1024, DEFAULT_MAX_INSERT_BYTES)?;
                }
                "loams_dedup_window" => {
                    out.dedup_window = bounded(name, value, 0, dedup::MAX_WINDOW as u64)? as usize;
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

/// What a write path knows about its request.
#[derive(Debug, Clone, Copy)]
pub struct InsertContext<'a> {
    /// The catalog.
    pub lake: &'a Lake,
    /// The workers.
    pub pool: &'a WorkerPool,
    /// The caller's namespace.
    pub namespace: u64,
    /// The request's current database.
    pub database: &'a str,
    /// The caller: its name, scopes and grants (HS1 Task 20).
    pub user: &'a crate::config::UserMap,
    /// The client's query id.
    pub query_id: &'a str,
    /// Query parameters (`{name:Type}`), for a `SELECT`.
    pub params: &'a [(String, String)],
    /// The session's and the request's settings, in order (Loams's own too).
    pub settings: &'a [(String, String)],
    /// The caps a setting is held to.
    pub limits: &'a SessionLimits,
    /// The engine's setting names.
    pub known: &'a HashSet<String>,
    /// The most one statement writes, as Arrow bytes.
    pub max_insert_bytes: u64,
    /// The pipe records a materialized view's `SELECT` reads (HS1 Task 16).
    pub pipe_batch: Option<&'a crate::pipes::PipeBatch>,
    /// The statement's admission (HS1 Task 21): its memory goes back once the
    /// worker's part is done, since the write then only waits for its commit.
    pub admitted: Option<&'a crate::admission::Ticket>,
}

/// What an `INSERT` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inserted {
    /// `<database>.<table>`.
    pub table: String,
    /// Rows committed (0 for a duplicate).
    pub written_rows: u64,
    /// Their size as the worker sent them (0 for a duplicate).
    pub written_bytes: u64,
    /// The snapshot holding them; `None` when nothing was written or the
    /// answer did not wait for the commit.
    pub snapshot_id: Option<i64>,
    /// Since the `INSERT` started, in nanoseconds.
    pub elapsed_ns: u64,
}

impl Inserted {
    /// The counters `X-ClickHouse-Summary` reports.
    pub fn progress(&self) -> Progress {
        Progress {
            written_rows: self.written_rows,
            written_bytes: self.written_bytes,
            elapsed_ns: self.elapsed_ns,
            ..Progress::default()
        }
    }

    /// `X-Loams-Snapshot-Id`'s value, when there is a snapshot.
    pub fn snapshot_header(&self) -> Option<String> {
        self.snapshot_id.map(|id| format!("{}={id}", self.table))
    }
}

/// Sends an `INSERT`'s streamed data to its worker: every piece as `Input`
/// frames, each also fed to `hash`, then `InputEnd`. A body that fails part-way
/// must not end its input (the worker is killed instead, HS1 Task 3 review I5).
#[async_trait]
pub trait Feed: Send {
    /// Sends the data.
    async fn feed(&mut self, lease: &mut WorkerLease, hash: &mut Sha256) -> Result<(), HouseError>;
}

/// Data already in memory (the API's, which has no request body to stream).
#[derive(Debug)]
pub struct BytesFeed(pub Vec<u8>);

#[async_trait]
impl Feed for BytesFeed {
    async fn feed(&mut self, lease: &mut WorkerLease, hash: &mut Sha256) -> Result<(), HouseError> {
        let data = std::mem::take(&mut self.0);
        hash.update(&data);
        if !data.is_empty() {
            lease.send_input(Bytes::from(data)).await?;
        }
        lease.end_input().await
    }
}

/// The `CREATE TEMPORARY TABLE` the rows land in: the table's stored columns
/// with their types and `DEFAULT`/`MATERIALIZED` expressions.
pub fn staging_ddl(ch: &ChTable) -> Result<String, HouseError> {
    let mut columns = Vec::new();
    for column in ch.columns.iter().filter(|c| c.stored()) {
        let Some(ty) = views::printable_type(&column.ch_type) else {
            return Err(refusal(
                ch,
                &format!(
                    "the type of column {} is not one the House can name",
                    column.name
                ),
            ));
        };
        let mut definition = format!("{} {ty}", quote_name(&column.name));
        match column.default_kind.as_str() {
            "" => {}
            kind @ ("DEFAULT" | "MATERIALIZED") => {
                let expression =
                    expr::reprint(&column.default_expression).map_err(|why| refusal(ch, &why))?;
                definition.push_str(&format!(" {kind} {expression}"));
            }
            other => {
                return Err(refusal(
                    ch,
                    &format!("column {} has a {other:?} default", column.name),
                ));
            }
        }
        columns.push(definition);
    }
    if columns.is_empty() {
        return Err(HouseError::from(ChError::bad_arguments(format!(
            "Table {}.{} has no columns",
            ch.database, ch.name
        ))));
    }
    // HE1 R9.12: a table's `CHECK` constraints hold on its staging table, so
    // chDB checks every row (`469 VIOLATED_CONSTRAINT`, as ClickHouse).
    for constraint in ch.constraints.iter().filter(|c| c.kind == "CHECK") {
        let expression = expr::reprint(&constraint.expression).map_err(|why| refusal(ch, &why))?;
        columns.push(format!(
            "CONSTRAINT {} CHECK {expression}",
            quote_name(&constraint.name)
        ));
    }
    Ok(format!(
        "CREATE TEMPORARY TABLE {STAGING} ({}) ENGINE = Memory",
        columns.join(", ")
    ))
}

/// The staging table's `INSERT` head, with the statement's column list.
pub fn staging_head(columns: Option<&[String]>) -> String {
    match columns {
        Some(columns) => format!(
            "INSERT INTO {STAGING} ({})",
            columns
                .iter()
                .map(|c| quote_name(c))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => format!("INSERT INTO {STAGING}"),
    }
}

/// The `SELECT` that writes the rows out: every field of the table's Iceberg
/// schema, in order, cast to the ClickHouse type chDB's Iceberg reader gives it
/// (so its Arrow type is the field's, up to what the lake conforms), merged as
/// ClickHouse merges an inserted block ([`merged_on_insert`]) and sorted by the
/// table's `ORDER BY`, then by insert order ([`ORDER`]).
pub fn output_sql(target: &Target) -> Result<String, HouseError> {
    let ch = &target.ch;
    let schema = target.table.metadata.current_schema();
    let mut columns = Vec::new();
    for field in schema.as_struct().fields() {
        let Some(column) = ch
            .columns
            .iter()
            .find(|c| c.stored() && c.name == field.name)
        else {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "Table {}.{}: the Iceberg field {} has no ClickHouse column, so the House \
                 cannot write the table",
                ch.database, ch.name, field.name
            ))));
        };
        let Some(storage) =
            views::printable_type(&types::from_iceberg(&field.field_type, field.required))
        else {
            return Err(refusal(
                ch,
                &format!(
                    "the type of field {} is not one the House can name",
                    field.name
                ),
            ));
        };
        let name = quote_name(&field.name);
        // D1572, HE1 R9.61: nanosecond times are written as
        // nanoseconds since the epoch (a `CAST` to an integer would give
        // seconds).
        let nanos = matches!(*field.field_type, Type::Primitive(PrimitiveType::Long))
            && types::ChType::parse(&column.ch_type).is_ok_and(|t| t.nanos_zone().is_some());
        let value = if nanos {
            format!("toUnixTimestamp64Nano({name})")
        } else {
            name.clone()
        };
        columns.push(format!(
            "CAST({value}, {}) AS {name}",
            quote_string(&storage)
        ));
    }
    let key = expr::key_elements(&ch.order_by).map_err(|why| refusal(ch, &why))?;
    let rows = merged_on_insert(ch, &key)?;
    let mut order: Vec<String> = key.into_iter().map(|(element, _)| element).collect();
    order.push(ORDER.to_string());
    Ok(format!(
        "SELECT {}\nFROM ({rows})\nORDER BY {}",
        columns.join(", "),
        order.join(", ")
    ))
}

/// The staged rows' position in the `INSERT`: the files are sorted by the
/// table's key and then by it, as ClickHouse's writer sorts stably, so rows of
/// one key stay in the order they were inserted (the last of equal versions
/// wins a `ReplacingMergeTree`, R9.4). It is read with `max_threads = 1`
/// ([`OUTPUT_SETTINGS`]), so the staging table's blocks come in order.
const ORDER: &str = "__loams_order";

/// The settings the output `SELECT` runs with beyond [`ARROW_SETTINGS`].
const OUTPUT_SETTINGS: &[(&str, &str)] = &[("max_threads", "1")];

/// The staged rows, merged within the `INSERT` as ClickHouse merges an
/// inserted block (`optimize_on_insert`, on by default; measured on the
/// 26.9.2.8 oracle, R9.4): a `ReplacingMergeTree` keeps per key the row of the
/// greatest version, the last inserted among equals (and keeps `is_deleted`
/// rows, which only `FINAL` drops); a `SummingMergeTree` sums per key, keeps
/// the first row's other columns, and drops a key whose sums are all zero; a
/// `VersionedCollapsingMergeTree` cancels rows of opposite sign per key and
/// version as `FINAL` does ([`views`]); a `MergeTree` keeps every row.
fn merged_on_insert(ch: &ChTable, key: &[(String, Option<String>)]) -> Result<String, HouseError> {
    // Named, not `*`: `*` leaves out `MATERIALIZED` columns.
    let staged: Vec<String> = ch
        .columns
        .iter()
        .filter(|c| c.stored())
        .map(|c| quote_name(&c.name))
        .collect();
    let staged = staged.join(", ");
    let rows = format!("SELECT {staged}, rowNumberInAllBlocks() AS {ORDER} FROM {STAGING}");
    let elements: Vec<String> = key.iter().map(|(element, _)| element.clone()).collect();
    let by = if elements.is_empty() {
        "tuple()".to_string()
    } else {
        elements.join(", ")
    };
    let engine = ddl::LakeEngine::parse(&ch.engine).unwrap_or(ddl::LakeEngine::MergeTree);
    Ok(match engine {
        ddl::LakeEngine::MergeTree => rows,
        ddl::LakeEngine::Replacing { ver, .. } => {
            let mut order = Vec::new();
            if let Some(ver) = ver {
                order.push(format!("{} DESC", quote_name(&ver)));
            }
            order.push(format!("{ORDER} DESC"));
            format!(
                "SELECT * FROM ({rows}) ORDER BY {} LIMIT 1 BY {by}",
                order.join(", ")
            )
        }
        ddl::LakeEngine::Summing { columns: summed } => {
            let key_columns: Vec<&str> = key.iter().filter_map(|(_, c)| c.as_deref()).collect();
            let mut select = Vec::new();
            let mut sums = Vec::new();
            for column in ch.columns.iter().filter(|c| c.stored()) {
                let name = quote_name(&column.name);
                let is_summed = match &summed {
                    Some(list) => list.iter().any(|s| s == &column.name),
                    None => {
                        !key_columns.contains(&column.name.as_str())
                            && views::is_numeric(&column.ch_type)
                    }
                };
                if key_columns.contains(&column.name.as_str()) {
                    select.push(name);
                } else if is_summed {
                    select.push(format!("sumWithOverflow({name}) AS {name}"));
                    sums.push(format!("{name} = 0"));
                } else {
                    select.push(format!("argMin({name}, __loams_position) AS {name}"));
                }
            }
            // The group's first position, under another name than the rows'
            // so that no aggregate reads another's alias.
            select.push(format!("min(__loams_position) AS {ORDER}"));
            let having = if sums.is_empty() {
                String::new()
            } else {
                format!(" HAVING NOT ({})", sums.join(" AND "))
            };
            format!(
                "SELECT {} FROM (SELECT {staged}, rowNumberInAllBlocks() AS __loams_position \
                 FROM {STAGING}) GROUP BY {by}{having}",
                select.join(", ")
            )
        }
        // HE1 R9.14: a Null table's rows are stored for its materialized views
        // (reads never see them).
        ddl::LakeEngine::Null => rows,
        // HE1 R14.13: an inserted block of a CoalescingMergeTree is merged as
        // ClickHouse merges it: per sorting key, each other column's last
        // value that is not NULL (`argMax` skips NULLs).
        ddl::LakeEngine::Coalescing => {
            let key_columns: Vec<&str> = key.iter().filter_map(|(_, c)| c.as_deref()).collect();
            let mut select = Vec::new();
            for column in ch.columns.iter().filter(|c| c.stored()) {
                let name = quote_name(&column.name);
                if key_columns.contains(&column.name.as_str()) {
                    select.push(name);
                } else {
                    select.push(format!("argMax({name}, __loams_position) AS {name}"));
                }
            }
            select.push(format!("max(__loams_position) AS {ORDER}"));
            format!(
                "SELECT {} FROM (SELECT {staged}, rowNumberInAllBlocks() AS __loams_position \
                 FROM {STAGING}) GROUP BY {by}",
                select.join(", ")
            )
        }
        // HE1 R9.14: an inserted block of an AggregatingMergeTree is merged as
        // ClickHouse merges it (`optimize_on_insert`): per sorting key, states
        // with their function's `-MergeState`, simple columns with their
        // function, the first row's value of the rest.
        ddl::LakeEngine::Aggregating => {
            let key_columns: Vec<&str> = key.iter().filter_map(|(_, c)| c.as_deref()).collect();
            let mut select = Vec::new();
            for column in ch.columns.iter().filter(|c| c.stored()) {
                let name = quote_name(&column.name);
                let refused = || refusal(ch, &format!("the type of column {}", column.name));
                if key_columns.contains(&column.name.as_str()) {
                    select.push(name);
                    continue;
                }
                select.push(match crate::types::ChType::parse(&column.ch_type)? {
                    crate::types::ChType::AggregateState => {
                        let (function, params) =
                            views::state_function(&column.ch_type).ok_or_else(refused)?;
                        format!("{function}MergeState{params}({name}) AS {name}")
                    }
                    crate::types::ChType::SimpleAggregate(function, _) => {
                        let (function, params) =
                            views::aggregate_function(&function).ok_or_else(refused)?;
                        format!("{function}{params}({name}) AS {name}")
                    }
                    _ => format!("argMin({name}, __loams_position) AS {name}"),
                });
            }
            select.push(format!("min(__loams_position) AS {ORDER}"));
            format!(
                "SELECT {} FROM (SELECT {staged}, rowNumberInAllBlocks() AS __loams_position \
                 FROM {STAGING}) GROUP BY {by}",
                select.join(", ")
            )
        }
        ddl::LakeEngine::VersionedCollapsing { sign, version } => {
            let sign = quote_name(&sign);
            let group = if elements.is_empty() {
                quote_name(&version)
            } else {
                format!("{by}, {}", quote_name(&version))
            };
            format!(
                "SELECT * FROM (SELECT *, row_number() OVER (PARTITION BY {group}, \
                 __loams_balance ORDER BY {ORDER} DESC) AS __loams_rank FROM (SELECT *, \
                 sum({sign}) OVER (PARTITION BY {group} ORDER BY {ORDER} ROWS BETWEEN \
                 UNBOUNDED PRECEDING AND CURRENT ROW) AS __loams_balance, sum({sign}) OVER \
                 (PARTITION BY {group}) AS __loams_total FROM ({rows})) WHERE {sign} = \
                 sign(__loams_total) AND {sign} * __loams_balance BETWEEN 1 AND \
                 abs(__loams_total)) WHERE __loams_rank = 1"
            )
        }
    })
}

/// `497`: the table's catalog properties hold what the House will not run.
fn refusal(ch: &ChTable, why: &str) -> HouseError {
    HouseError::from(ChError::access_denied(format!(
        "Table {}.{} cannot be written: {why}",
        ch.database, ch.name
    )))
}

/// The settings the worker's `INSERT` statement runs with: the request's, less
/// the front's own and those the House takes for the lake write.
fn engine_settings(pairs: &[(String, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .filter(|(name, _)| !settings::is_front_only(name) && !CONSUMED.contains(&name.as_str()))
        .cloned()
        .collect()
}

/// Why a write stopped, and whether its worker can be trusted again.
struct Stop {
    error: HouseError,
    /// The worker is mid-statement (a body cut short, an output abandoned): it is
    /// killed, not released.
    kill: bool,
}

impl From<HouseError> for Stop {
    fn from(error: HouseError) -> Self {
        Self { error, kill: false }
    }
}

/// Runs an `INSERT` into a lake table on `lease` (see the module docs); the
/// lease is released or killed here. `feed` sends a streamed source's data.
///
/// Rows written are a `house.insert` usage event (HS1 Task 22), whichever
/// surface or pipe wrote them.
pub async fn run(
    ctx: &InsertContext<'_>,
    lease: WorkerLease,
    target: Target,
    feed: Option<&mut (dyn Feed + '_)>,
) -> Result<Inserted, HouseError> {
    let (database, table) = (target.database.clone(), target.name.clone());
    let inserted = write_and_commit(ctx, lease, target, feed).await?;
    if inserted.written_rows > 0
        && let Some(usage) = ctx.pool.usage()
    {
        usage.emit(crate::usage::UsageEvent::Insert(
            crate::usage::InsertUsage {
                namespace: ctx.namespace.to_string(),
                query_id: ctx.query_id.to_string(),
                database,
                table,
                rows: inserted.written_rows,
                bytes: inserted.written_bytes,
                at: std::time::SystemTime::now(),
            },
        ));
    }
    Ok(inserted)
}

async fn write_and_commit(
    ctx: &InsertContext<'_>,
    lease: WorkerLease,
    target: Target,
    feed: Option<&mut (dyn Feed + '_)>,
) -> Result<Inserted, HouseError> {
    let started = Instant::now();
    let written = write_staged(ctx, lease, &target, feed).await?;
    let Staged {
        written,
        hash,
        settings,
    } = written;
    let table = format!("{}.{}", target.database, target.name);
    if written.rows == 0 {
        return Ok(Inserted {
            table,
            written_rows: 0,
            written_bytes: 0,
            snapshot_id: None,
            elapsed_ns: started.elapsed().as_nanos() as u64,
        });
    }

    // The files go up from the front: the worker holds no write grant and no
    // credentials (R1.5, R-ACL.1).
    let committer = ctx.lake.committer();
    for (location, bytes) in &written.files {
        committer.io().write(location, bytes.clone()).await?;
    }
    let key = match (settings.deduplicate, &settings.token) {
        (false, _) => None,
        (true, Some(token)) => Some(dedup::token_key(token)?),
        (true, None) => Some(dedup::hash_key(hash)),
    };
    // The commit's span (HS1 Task 23): from the queue to the answer, which
    // includes the group commit's interval.
    let mut span = crate::trace::child("house.commit");
    if let Some(span) = span.as_mut() {
        span.set("loams.table", table.clone());
        span.set("loams.files", written.files.len() as u64);
        span.set("loams.rows", written.rows);
    }
    let (reply, answer) = oneshot::channel();
    committer.submit(
        &target.table.ident,
        Pending {
            files: written.data_files.clone(),
            key,
            window: settings.dedup_window,
            interval: settings.commit_interval,
            commit_bytes: settings.commit_bytes,
            reply,
        },
    );
    if settings.async_insert && !settings.wait {
        // Answered once queued; a crash before the commit loses the rows, as in
        // ClickHouse (§49 §6.1).
        return Ok(Inserted {
            table,
            written_rows: written.rows,
            written_bytes: written.input_bytes,
            snapshot_id: None,
            elapsed_ns: started.elapsed().as_nanos() as u64,
        });
    }
    let committed = answer.await.map_err(|_| {
        HouseError::from(ChError::network_error(
            "the commit queue ended before the commit was answered",
        ))
    })?;
    if let Some(mut span) = span {
        match &committed {
            Ok(c) => {
                span.set("loams.snapshot_id", c.snapshot_id);
                span.set("loams.duplicate", c.duplicate);
            }
            Err(_) => span.fail(),
        }
        span.end();
    }
    let committed = committed?;
    let (rows, bytes) = if committed.duplicate {
        (0, 0)
    } else {
        (written.rows, written.input_bytes)
    };
    Ok(Inserted {
        table,
        written_rows: rows,
        written_bytes: bytes,
        snapshot_id: Some(committed.snapshot_id),
        elapsed_ns: started.elapsed().as_nanos() as u64,
    })
}

/// The worker's part of a write ([`stage`]) on `lease`, which is released, or
/// killed when it was left mid-statement.
async fn write_staged(
    ctx: &InsertContext<'_>,
    mut lease: WorkerLease,
    target: &Target,
    feed: Option<&mut (dyn Feed + '_)>,
) -> Result<Staged, HouseError> {
    if let Err(err) = ctx.lake.committer().io().store() {
        ctx.pool.release(lease, Outcome::Completed);
        return Err(err);
    }
    let session = format!("{SESSION_PREFIX}{}", uuid::Uuid::new_v4());
    match stage(ctx, &mut lease, target, &session, feed).await {
        Ok(written) => {
            ctx.pool.release(lease, Outcome::Completed);
            if let Some(ticket) = ctx.admitted {
                ticket.release_memory();
            }
            Ok(written)
        }
        Err(stop) => {
            if stop.kill || close_session(ctx, &mut lease, &session).await.is_err() {
                ctx.pool.kill(lease, ExitReason::Cancel);
            } else {
                ctx.pool.release(lease, Outcome::Completed);
            }
            Err(stop.error)
        }
    }
}

/// A target's rows written as data files and uploaded, not committed (HS1
/// Task 13's compaction commits them as a `replace`).
pub async fn write_files(
    ctx: &InsertContext<'_>,
    lease: WorkerLease,
    target: &Target,
) -> Result<loams_house_lake::write::Written, HouseError> {
    let staged = write_staged(ctx, lease, target, None).await?;
    let committer = ctx.lake.committer();
    for (location, bytes) in &staged.written.files {
        committer.io().write(location, bytes.clone()).await?;
    }
    Ok(staged.written)
}

/// What the worker's part of a write gave.
struct Staged {
    written: loams_house_lake::write::Written,
    hash: Sha256,
    settings: InsertSettings,
}

fn execute(
    ctx: &InsertContext<'_>,
    session: &str,
    close: bool,
    sql: String,
    format: &str,
) -> Execute {
    Execute {
        query_id: ctx.query_id.to_string(),
        session: Some(SessionRef {
            key: session.to_string(),
            timeout_ms: SESSION_TIMEOUT_MS,
            close,
        }),
        settings: Vec::new(),
        views: Vec::new(),
        sql,
        format: format.to_string(),
        params: Vec::new(),
        limits: Limits::default(),
        input: None,
        output: Output::Blocks,
        database: None,
    }
}

/// Admits a statement the front generated: one statement ClickHouse parses,
/// nothing denied anywhere in it.
async fn admit_generated(
    lease: &mut WorkerLease,
    sql: &str,
    params: &[(String, String)],
) -> Result<(), HouseError> {
    let analysis = lease
        .analyze(sql, params)
        .await
        .map_err(deny::as_setting_refusal)?;
    gate::admit_ddl(&analysis, sql)
}

/// The worker's part: admission, staging, rows, files.
async fn stage(
    ctx: &InsertContext<'_>,
    lease: &mut WorkerLease,
    target: &Target,
    session: &str,
    feed: Option<&mut (dyn Feed + '_)>,
) -> Result<Staged, Stop> {
    // The statement as the client wrote it, as every statement is admitted.
    let statement =
        deny::rewrite_host_functions(&target.statement, &gate::host_values(&ctx.user.user));
    let analysis = lease
        .analyze(&statement, ctx.params)
        .await
        .map_err(deny::as_setting_refusal)?;
    let expect = crate::classify::Stmt::Insert(InsertStmt {
        text: String::new(),
        format: None,
        table: None,
    })
    .expect()
    .unwrap_or(crate::classify::Expect::Read);
    // HE1 R14.5: what it selects may call the namespace's own views.
    let views = gate::view_calls(Some(ctx.lake), ctx.namespace, ctx.database, &analysis).await?;
    gate::admit_views(&analysis, &statement, expect, false, None, &views)?;

    // Its settings: the session's, the request's, then its own.
    for (name, value) in &target.settings {
        settings::check(name, value, ctx.limits, ctx.known)?;
    }
    let mut pairs = ctx.settings.to_vec();
    pairs.extend(target.settings.iter().cloned());
    let insert_settings = InsertSettings::from_pairs(&pairs)?;
    let engine = engine_settings(&pairs);

    // Staging.
    let ddl = staging_ddl(&target.ch)?;
    let output = output_sql(target)?;
    admit_generated(lease, &ddl, &[]).await?;
    admit_generated(lease, &output, &[]).await?;
    lease.run(execute(ctx, session, false, ddl, "TSV")).await?;

    // Rows.
    let mut hash = Sha256::new();
    let head = staging_head(target.columns.as_deref());
    match &target.source {
        Source::Select { text } => {
            let text = deny::rewrite_host_functions(text, &gate::host_values(&ctx.user.user));
            let internal = format!("{head}\n{text}");
            let mut analysis = lease
                .analyze(&internal, ctx.params)
                .await
                .map_err(deny::as_setting_refusal)?;
            gate::admit_views(&analysis, &internal, expect, false, None, &views)?;
            // Which tables the query reads with `FINAL` is in its query tree,
            // which ClickHouse builds for a query but not for an `INSERT`: the
            // query alone gives it (HS1 Task 13), so a Replacing or Summing
            // table it reads `FINAL` reads merged.
            let query = lease
                .analyze(&text, ctx.params)
                .await
                .map_err(deny::as_setting_refusal)?;
            gate::admit_views(
                &query,
                &text,
                crate::classify::Expect::Read,
                true,
                None,
                &views,
            )?;
            analysis.query_tree = query.query_tree;
            let plan = gate::read_plan(
                Some(ctx.lake),
                ctx.namespace,
                ctx.database,
                ctx.user,
                lease,
                &analysis,
                &internal,
                &pairs,
                ctx.pipe_batch,
                None,
            )
            .await?;
            let mut insert = execute(ctx, session, false, plan.sql, "TSV");
            insert.settings = engine.clone();
            insert.views = plan.views;
            insert.params = ctx.params.to_vec();
            insert.database = Some(ctx.database.to_string());
            lease.run(insert).await?;
        }
        data => {
            let format = match data {
                Source::Stream { format } => format.as_str(),
                _ => "Values",
            };
            hash.update(format.as_bytes());
            hash.update(b"\n");
            hash.update(head.as_bytes());
            hash.update(b"\n");
            let mut insert = execute(ctx, session, false, String::new(), "TSV");
            insert.settings = engine.clone();
            insert.input = Some(InputSpec {
                insert: head.clone(),
                format: format.to_string(),
            });
            lease.start(insert).await?;
            let fed = match (data, feed) {
                (Source::Inline { data }, _) => {
                    BytesFeed(data.clone()).feed(lease, &mut hash).await
                }
                (_, Some(feed)) => feed.feed(lease, &mut hash).await,
                (_, None) => BytesFeed(Vec::new()).feed(lease, &mut hash).await,
            };
            if let Err(error) = fed {
                return Err(Stop { error, kill: true });
            }
            drain(lease).await?;
        }
    }

    // Files.
    let mut out = execute(ctx, session, true, output, "ArrowStream");
    out.settings = ARROW_SETTINGS
        .iter()
        .chain(OUTPUT_SETTINGS)
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    out.output = Output::Document {
        max_buffered_bytes: ctx.max_insert_bytes,
    };
    let prefix = uuid::Uuid::new_v4().to_string();
    let mut writer = DataWriter::new(
        &target.table.metadata,
        &prefix,
        insert_settings.target_file_bytes,
        ctx.max_insert_bytes,
    )
    .await
    .map_err(|err| Stop::from(lake_error(err)))?;
    lease.start(out).await?;
    let hashes_rows = matches!(target.source, Source::Select { .. });
    loop {
        match lease.next_event().await? {
            Event::Chunk(chunk) => {
                if hashes_rows {
                    hash.update(&chunk.bytes);
                }
                if let Err(err) = writer.push(&chunk.bytes).await {
                    return Err(Stop {
                        error: lake_error(err),
                        kill: true,
                    });
                }
            }
            Event::Progress(_) => {}
            Event::Done(_) => break,
        }
    }
    let written = writer.finish().await.map_err(lake_error)?;
    Ok(Staged {
        written,
        hash,
        settings: insert_settings,
    })
}

/// Reads a statement's events to its end, keeping none.
async fn drain(lease: &mut WorkerLease) -> Result<Progress, HouseError> {
    loop {
        if let Event::Done(stats) = lease.next_event().await? {
            return Ok(stats);
        }
    }
}

/// Ends a write's worker session after a failure, so its staging table goes.
async fn close_session(
    ctx: &InsertContext<'_>,
    lease: &mut WorkerLease,
    session: &str,
) -> Result<(), HouseError> {
    lease
        .run(execute(ctx, session, true, "SELECT 1".to_string(), "TSV"))
        .await
        .map(|_| ())
}

/// The lake's refusal as a ClickHouse error.
fn lake_error(err: loams_house_lake::LakeError) -> HouseError {
    use loams_house_lake::LakeError;
    match err {
        LakeError::TooLarge(cap) => HouseError::from(ChError::bad_arguments(format!(
            "The INSERT's rows are more than the {cap} bytes one statement may write to a lake \
             table: split it into smaller inserts"
        ))),
        LakeError::Data(why) => HouseError::from(ChError::bad_arguments(format!(
            "The rows cannot be written to the lake table: {why}"
        ))),
        other => HouseError::from(ChError::bad_arguments(format!(
            "The rows cannot be written to the lake table: {other}"
        ))),
    }
}

/// `CREATE TABLE … AS SELECT` of a lake table (§49 §6.3): the table, made by
/// the catalog's own `CREATE TABLE` with the query's columns when the statement
/// names none, then [`run`]'s `INSERT … SELECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAs {
    /// The whole statement, for admission.
    pub statement: String,
    /// `CREATE TABLE [IF NOT EXISTS] [db.]t [ON CLUSTER c]` and its column list
    /// if it has one.
    pub head: String,
    /// The clauses between the column list and `AS` (`ENGINE`, `ORDER BY`, …).
    pub clauses: String,
    /// Whether the statement names its columns.
    pub has_columns: bool,
    /// The query.
    pub select: String,
    /// `EMPTY AS`: no rows.
    pub empty: bool,
    /// The table's database.
    pub database: String,
    /// The table's name.
    pub name: String,
    /// `IF NOT EXISTS`.
    pub if_not_exists: bool,
    /// An inner-table materialized view (HE1 R9.14): once the table is made,
    /// the view `name TO name AS <select>` over its source.
    pub inner_view: bool,
}

impl CreateAs {
    /// `CREATE MATERIALIZED VIEW mv [(columns)] ENGINE = … AS SELECT …` with no
    /// `TO` (HE1 R9.14): ClickHouse makes an inner table and reads the view's
    /// name as that table. The House makes a lake table named as the view, with
    /// the `SELECT`'s columns (or the ones written) and the clauses written,
    /// then the view into it; `POPULATE` is refused as for any view.
    pub fn inner_view(text: &str, current: &str) -> Result<Self, HouseError> {
        let mv = ddl::parse_create_mv(text)?;
        if mv.populate {
            return Err(HouseError::from(ChError::not_implemented(
                "POPULATE is not supported (ClickHouse advises against it: rows that arrive while \
                 it runs are lost); fill the view's table with INSERT … SELECT instead",
            )));
        }
        if mv.or_replace || mv.refresh {
            return Err(HouseError::from(ChError::not_implemented(
                "CREATE OR REPLACE and REFRESH materialized views are not on the House's surface",
            )));
        }
        let database = mv.database.clone().unwrap_or_else(|| current.to_string());
        let head = format!(
            "CREATE TABLE {}{}.{}",
            if mv.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            quote_name(&database),
            quote_name(&mv.name)
        );
        Ok(Self {
            statement: text.to_string(),
            head: match &mv.inner_columns {
                Some(columns) => format!("{head} {columns}"),
                None => head,
            },
            clauses: mv.inner_clauses.clone(),
            has_columns: mv.inner_columns.is_some(),
            select: mv.select.clone(),
            empty: true,
            database,
            name: mv.name,
            if_not_exists: mv.if_not_exists,
            inner_view: true,
        })
    }

    /// Reads a `CREATE TABLE … AS SELECT`; `current` is the request's database.
    pub fn parse(text: &str, current: &str) -> Result<Self, HouseError> {
        let create = ddl::parse_create_table(text)?;
        let Some((as_at, select_at)) = create.as_at.filter(|_| create.as_select) else {
            return Err(HouseError::from(ChError::syntax_error(
                "Expected AS SELECT at the end of CREATE TABLE",
            )));
        };
        if create.or_replace {
            return Err(HouseError::from(ChError::not_implemented(
                "CREATE OR REPLACE TABLE … AS SELECT is not served on lake tables",
            )));
        }
        let clauses_at = create.clauses_at.min(as_at);
        Ok(Self {
            statement: text.to_string(),
            head: text[..clauses_at].trim_end().to_string(),
            clauses: text[clauses_at..as_at].trim().to_string(),
            has_columns: !create.columns.is_empty(),
            select: text[select_at..].trim().to_string(),
            empty: create.empty_as,
            database: create.database.unwrap_or_else(|| current.to_string()),
            name: create.name,
            if_not_exists: create.if_not_exists,
            inner_view: false,
        })
    }
}

/// Runs a `CREATE TABLE … AS SELECT` (see [`CreateAs`]) on `lease`, which is
/// released or killed here. The table stays if its rows fail, as in
/// ClickHouse.
pub async fn create_as(
    ctx: &InsertContext<'_>,
    mut lease: WorkerLease,
    plan: CreateAs,
) -> Result<Inserted, HouseError> {
    let nothing = || Inserted {
        table: format!("{}.{}", plan.database, plan.name),
        written_rows: 0,
        written_bytes: 0,
        snapshot_id: None,
        elapsed_ns: 0,
    };
    let created = async {
        // Nothing is made when its rows could not be written.
        ctx.lake.committer().io().store()?;
        // The statement as written, as any catalog DDL is admitted.
        let statement =
            deny::rewrite_host_functions(&plan.statement, &gate::host_values(&ctx.user.user));
        let analysis = lease
            .analyze(&statement, ctx.params)
            .await
            .map_err(deny::as_setting_refusal)?;
        let views =
            gate::view_calls(Some(ctx.lake), ctx.namespace, ctx.database, &analysis).await?;
        gate::admit_ddl_views(&analysis, &statement, &views)?;
        if plan.has_columns {
            return Ok(format!("{} {}", plan.head, plan.clauses));
        }
        // The query's columns, as ClickHouse names and types them, read through
        // the namespace's pinned views.
        let select = deny::rewrite_host_functions(&plan.select, &gate::host_values(&ctx.user.user));
        let analysis = lease
            .analyze(&select, ctx.params)
            .await
            .map_err(deny::as_setting_refusal)?;
        gate::admit_views(
            &analysis,
            &select,
            crate::classify::Expect::Read,
            true,
            None,
            &views,
        )?;
        let reads = gate::read_plan(
            Some(ctx.lake),
            ctx.namespace,
            ctx.database,
            ctx.user,
            &mut lease,
            &analysis,
            &select,
            ctx.settings,
            None,
            None,
        )
        .await?;
        let mut describe = Execute {
            query_id: ctx.query_id.to_string(),
            session: None,
            settings: engine_settings(ctx.settings),
            views: reads.views,
            sql: format!(
                "DESCRIBE TABLE (\n{}\n) SETTINGS describe_compact_output = 1",
                reads.sql
            ),
            format: "JSONCompactEachRow".to_string(),
            params: ctx.params.to_vec(),
            limits: Limits::default(),
            input: None,
            output: Output::Blocks,
            database: Some(ctx.database.to_string()),
        };
        describe
            .settings
            .retain(|(name, _)| name != "describe_compact_output");
        let described = lease.run(describe).await?;
        let mut columns = Vec::new();
        for line in String::from_utf8_lossy(&described.bytes).lines() {
            let (name, ty): (String, String) = serde_json::from_str(line).map_err(|_| {
                HouseError::from(ChError::bad_arguments(
                    "the query's columns could not be read",
                ))
            })?;
            let Some(ty) = views::printable_type(&ty) else {
                return Err(HouseError::from(ChError::not_implemented(format!(
                    "column {name} of the query has type {ty}, which a lake table cannot hold"
                ))));
            };
            columns.push(format!("{} {ty}", quote_name(&name)));
        }
        Ok(format!(
            "{} ({}) {}",
            plan.head,
            columns.join(", "),
            plan.clauses
        ))
    }
    .await;
    // The catalog's own DDL takes its own worker.
    ctx.pool.release(lease, Outcome::Completed);
    let create = created?;
    let exists = ctx
        .lake
        .database_exists(ctx.namespace, &plan.database)
        .await?
        && ctx
            .lake
            .load(ctx.namespace, &plan.database, &plan.name)
            .await?
            .is_some();
    if exists && plan.if_not_exists {
        return Ok(nothing());
    }
    let crate::classify::Classified::Known { stmt, .. } = crate::classify::classify(&create)?
    else {
        return Err(HouseError::from(ChError::syntax_error(
            "the table's CREATE TABLE could not be read",
        )));
    };
    match gate::catalog_answer(
        Some(ctx.lake),
        ctx.pool,
        ctx.user,
        ctx.database,
        &stmt,
        &create,
        None,
        ctx.query_id,
    )
    .await?
    {
        gate::CatalogOutcome::Done => {}
        _ => {
            return Err(HouseError::from(ChError::syntax_error(
                "the table's CREATE TABLE was not a lake table's",
            )));
        }
    }
    if plan.inner_view {
        let view = format!(
            "CREATE MATERIALIZED VIEW {db}.{name} TO {db}.{name} AS {}",
            plan.select,
            db = quote_name(&plan.database),
            name = quote_name(&plan.name),
        );
        if let Err(err) =
            crate::pipes::create_view_into(ctx.lake, ctx.namespace, ctx.database, &view, true).await
        {
            // No inner table without its view.
            let _ = ctx
                .lake
                .drop_table(ctx.namespace, &plan.database, &plan.name, true)
                .await;
            return Err(err);
        }
    }
    if plan.empty {
        return Ok(nothing());
    }
    let insert = InsertStmt {
        text: format!(
            "INSERT INTO {}.{}\n{}",
            quote_name(&plan.database),
            quote_name(&plan.name),
            plan.select
        ),
        format: None,
        table: None,
    };
    let Some(target) = target(ctx.lake, ctx.user, ctx.database, &insert).await? else {
        return Err(HouseError::from(ChError::unknown_table(format!(
            "Table {}.{} does not exist",
            plan.database, plan.name
        ))));
    };
    let lease = ctx.pool.acquire(&ctx.namespace.to_string()).await?;
    run(ctx, lease, target, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_settings_parse_and_cap() {
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let s = InsertSettings::from_pairs(&pairs(&[
            ("insert_deduplicate", "0"),
            ("insert_deduplication_token", "t1"),
            ("async_insert", "1"),
            ("wait_for_async_insert", "false"),
            ("loams_lake_commit_interval_ms", "0"),
            ("loams_dedup_window", "2"),
            ("insert_deduplicate", "1"),
        ]))
        .expect("parses");
        assert!(s.deduplicate, "later wins");
        assert_eq!(s.token.as_deref(), Some("t1"));
        assert!(s.async_insert && !s.wait);
        assert_eq!(s.commit_interval, Duration::ZERO);
        assert_eq!(s.dedup_window, 2);
        for bad in [
            ("loams_lake_commit_interval_ms", "60001"),
            ("loams_dedup_window", "10001"),
            ("loams_lake_target_file_bytes", "10"),
            ("async_insert", "maybe"),
        ] {
            assert!(
                InsertSettings::from_pairs(&pairs(&[bad])).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn staging_head_quotes_names() {
        assert_eq!(staging_head(None), "INSERT INTO __loams_insert");
        assert_eq!(
            staging_head(Some(&["a".to_string(), "my`c".to_string()])),
            "INSERT INTO __loams_insert (a, `my\\`c`)"
        );
    }

    #[test]
    fn engine_settings_keep_only_the_engines() {
        let pairs: Vec<(String, String)> = [
            ("input_format_skip_unknown_fields", "1"),
            ("insert_deduplicate", "0"),
            ("insert_deduplication_token", "x"),
            ("async_insert", "1"),
            ("loams_dedup_window", "3"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(
            engine_settings(&pairs),
            vec![(
                "input_format_skip_unknown_fields".to_string(),
                "1".to_string()
            )]
        );
    }
}
