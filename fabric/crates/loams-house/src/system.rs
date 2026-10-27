//! The House's own `system` tables (HS1 Task 11, FL2 Task 8, §32 §8.7).
//!
//! chDB's `system.databases`, `tables`, `columns` and `parts` describe the
//! worker — its `Memory` databases of views, its directories — and
//! `processes` and `errors` show host details (addresses, the OS user, stack
//! traces, the engine's own messages; HS1 Task 5 fix round 1). The House
//! replaces them:
//!
//! * a statement's text names `__loams_system.<t>` wherever it named
//!   `system.<t>` ([`rewrite`], with ClickHouse's lexing: strings and comments
//!   are untouched), and the worker gets a view of that name for the statement;
//! * the views have **chDB's own columns, in its order and types**, read once
//!   per process with `DESCRIBE TABLE system.<t>` ([`shape`]); the House fills
//!   what it knows and every other column holds its type's default;
//! * `databases`, `tables` and `columns` hold the namespace's catalog — its
//!   databases and lake and external tables, `ReplacingMergeTree` shown as such
//!   — plus chDB's rows for `system` (the tables the deny list allows) and
//!   `information_schema`, with their paths blanked; `tables` also shows the
//!   session's temporary tables and the namespace's materialized views (engine
//!   `MaterializedView`, HS1 Task 23), which `SHOW TABLES` lists in turn;
//! * `parts` has one row per live data file of each table's current snapshot;
//! * `processes` and `errors` are chDB's rows with the host columns blanked
//!   and the House user in place of chDB's.
//!
//! `SHOW TABLES`, `SHOW DATABASES` and `SHOW PROCESSLIST` read the same tables:
//! [`show_query`] writes them as the `SELECT` ClickHouse rewrites them into
//! (`InterpreterShowTablesQuery`), so they pass through the rewrite too.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use loams_house_ipc::{Execute, Limits, Output};

use crate::admission::WorkerLease;
use crate::catalog::{Lake, LakeTable};
use crate::classify::{Lexeme, lex};
use crate::ddl::{self, ChTable, ShowList, TableClass, quote_name, quote_string};
use crate::deny::ALLOWED_SYSTEM_TABLES;
use crate::errors::{ChError, HouseError};
use crate::read::{ReadContext, live_files};

/// The worker database the replaced `system` tables are views in.
pub const SYSTEM_VIEWS: &str = "__loams_system";

/// The `system` tables the House replaces.
pub const REPLACED: &[&str] = &[
    "databases",
    "tables",
    "columns",
    "parts",
    "processes",
    "errors",
    "loams_pipes",
    "query_log",
    // HE1 R9.62: what a clustered application's migrations read (SigNoz's
    // wait for each `ON CLUSTER` statement and each mutation), answered for a
    // one-node House.
    "clusters",
    "macros",
    "mutations",
    "distributed_ddl_queue",
];

/// `system.clusters`' columns, ClickHouse 26.9's (measured on the 26.9.2.8
/// oracle, HE1 R9.62).
pub const CLUSTERS_COLUMNS: &[(&str, &str)] = &[
    ("cluster", "String"),
    ("shard_num", "UInt32"),
    ("shard_name", "String"),
    ("shard_weight", "UInt32"),
    ("internal_replication", "UInt8"),
    ("replica_num", "UInt32"),
    ("host_name", "String"),
    ("host_address", "String"),
    ("port", "UInt16"),
    ("is_local", "UInt8"),
    ("user", "String"),
    ("default_database", "String"),
    ("errors_count", "UInt32"),
    ("slowdowns_count", "UInt32"),
    ("estimated_recovery_time", "UInt32"),
    ("database_shard_name", "String"),
    ("database_replica_name", "String"),
    ("is_shared_catalog_cluster", "UInt8"),
    ("is_active", "Nullable(UInt8)"),
    ("unsynced_after_recovery", "Nullable(UInt8)"),
    ("replication_lag", "Nullable(UInt32)"),
    ("recovery_time", "Nullable(UInt64)"),
    ("name", "String"),
];

/// `system.macros`' columns (HE1 R9.62).
pub const MACROS_COLUMNS: &[(&str, &str)] = &[("macro", "String"), ("substitution", "String")];

/// `system.mutations`' columns, ClickHouse 26.9's (HE1 R9.62).
pub const MUTATIONS_COLUMNS: &[(&str, &str)] = &[
    ("database", "String"),
    ("table", "String"),
    ("mutation_id", "String"),
    ("command", "String"),
    ("create_time", "DateTime"),
    ("finish_time", "DateTime"),
    ("block_numbers.partition_id", "Array(String)"),
    ("block_numbers.number", "Array(Int64)"),
    ("parts_in_progress_names", "Array(String)"),
    ("parts_to_do_names", "Array(String)"),
    ("parts_to_do", "Int64"),
    ("parts_postpone_reasons", "Map(String, String)"),
    ("is_done", "UInt8"),
    ("is_killed", "UInt8"),
    ("latest_failed_part", "String"),
    ("latest_fail_time", "DateTime"),
    ("latest_fail_reason", "String"),
    ("latest_fail_error_code_name", "String"),
];

/// `system.distributed_ddl_queue`'s columns, ClickHouse 26.9's (HE1 R9.62).
pub const DDL_QUEUE_COLUMNS: &[(&str, &str)] = &[
    ("entry", "String"),
    ("entry_version", "Nullable(UInt8)"),
    ("initiator_host", "Nullable(String)"),
    ("initiator_port", "Nullable(UInt16)"),
    ("cluster", "String"),
    ("query", "String"),
    ("settings", "Map(String, String)"),
    ("query_create_time", "DateTime"),
    ("host", "Nullable(String)"),
    ("port", "Nullable(UInt16)"),
    (
        "status",
        "Nullable(Enum8('Inactive' = 0, 'Active' = 1, 'Finished' = 2, 'Removing' = 3, 'Unknown' = 4))",
    ),
    ("exception_code", "Nullable(UInt16)"),
    ("exception_text", "Nullable(String)"),
    ("query_finish_time", "Nullable(DateTime)"),
    ("query_duration_ms", "Nullable(UInt64)"),
];

/// The tables of a one-node House's cluster (HE1 R9.62) that answer no rows:
/// it has no macros configured, its mutations are done when their statement
/// returns (copy-on-write, HE1 R9.5), and an `ON CLUSTER` statement is done
/// when it returns, so no distributed DDL is ever queued.
const EMPTY_CLUSTER_TABLES: &[(&str, &[(&str, &str)])] = &[
    ("macros", MACROS_COLUMNS),
    ("mutations", MUTATIONS_COLUMNS),
    ("distributed_ddl_queue", DDL_QUEUE_COLUMNS),
];

/// `clusterAllReplicas(<cluster>, system.<t>)` read as `system.<t>` (HE1
/// R9.62): every cluster of a one-node House is that node, so the function's
/// rows are the local table's. Only that form is rewritten — a cluster name
/// (a quoted string, or a name) and a `system` table, nothing else — so
/// `clusterAllReplicas` of a user's table or of any other source stays in the
/// text and the deny list refuses it (`344`), as it refuses `cluster`,
/// `remote` and the rest. The `system` table is then held to the deny list's
/// allowed tables like any other read.
pub fn rewrite_cluster_reads(sql: &str) -> String {
    let lexemes = lex(sql);
    let raw = |i: usize| lexemes.get(i).map(|(_, r)| &sql[r.clone()]);
    let symbol =
        |i: usize, b: u8| matches!(lexemes.get(i), Some((Lexeme::Symbol(s), _)) if *s == b);
    let word_like = |i: usize| match lexemes.get(i) {
        Some((Lexeme::Word(..), _)) => true,
        Some((Lexeme::Other, r)) => {
            let text = &sql[r.clone()];
            text.len() >= 2 && (text.starts_with('`') || text.starts_with('"'))
        }
        _ => false,
    };
    let cluster_name = |i: usize| match lexemes.get(i) {
        Some((Lexeme::Other, r)) => {
            let text = &sql[r.clone()];
            text.len() >= 2 && matches!(text.as_bytes()[0], b'\'' | b'`' | b'"')
        }
        Some((Lexeme::Word(..), _)) => true,
        _ => false,
    };
    let is_system = |i: usize| {
        raw(i).is_some_and(|t| {
            let t = t.trim_matches(|c| c == '`' || c == '"');
            t == "system"
        })
    };
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut i = 0;
    while i < lexemes.len() {
        let matches = matches!(lexemes[i].0, Lexeme::Word(..))
            && raw(i).is_some_and(|w| w.eq_ignore_ascii_case("clusterAllReplicas"))
            && !(i > 0 && symbol(i - 1, b'.'))
            && symbol(i + 1, b'(')
            && cluster_name(i + 2)
            && symbol(i + 3, b',')
            && word_like(i + 4)
            && is_system(i + 4)
            && symbol(i + 5, b'.')
            && word_like(i + 6)
            && symbol(i + 7, b')');
        if matches {
            out.push_str(&sql[copied..lexemes[i].1.start]);
            out.push_str("system.");
            out.push_str(&sql[lexemes[i + 6].1.clone()]);
            copied = lexemes[i + 7].1.end;
            i += 8;
            continue;
        }
        i += 1;
    }
    out.push_str(&sql[copied..]);
    out
}

/// `system.loams_pipes` (HS1 Task 16): the House's own, one row per
/// materialized view — over a pipe or, from HS1 Task 23, a lake table — with
/// this front's view of its runs.
pub const PIPES_COLUMNS: &[(&str, &str)] = &[
    ("database", "String"),
    ("name", "String"),
    ("pipe", "String"),
    ("source", "String"),
    ("target", "String"),
    ("lag_records", "UInt64"),
    ("committed_records", "UInt64"),
    ("committed_rows", "UInt64"),
    ("last_commit_at", "Nullable(DateTime64(3, 'UTC'))"),
    ("last_error", "String"),
];

/// The databases chDB itself has, whose `system` rows are shown as they are.
const ENGINE_DATABASES: &[&str] = &["system", "information_schema", "INFORMATION_SCHEMA"];

/// One column of a chDB `system` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeColumn {
    /// Its name.
    pub name: String,
    /// Its type.
    pub ty: String,
}

/// The text with every `system.<t>` of [`REPLACED`] naming
/// `` `__loams_system`.`<t>` ``, and which tables it named.
pub fn rewrite(sql: &str) -> (String, BTreeSet<String>) {
    let lexemes = lex(sql);
    let name_at = |i: usize| -> Option<String> {
        let (lexeme, range) = lexemes.get(i)?;
        let raw = &sql[range.clone()];
        match lexeme {
            Lexeme::Word(..) => Some(raw.to_string()),
            Lexeme::Other if raw.len() >= 2 && (raw.starts_with('`') || raw.starts_with('"')) => {
                let quote = &raw[..1];
                let inner = raw.strip_prefix(quote)?.strip_suffix(quote)?;
                (!inner.contains('\\') && !inner.contains(quote)).then(|| inner.to_string())
            }
            _ => None,
        }
    };
    let dot_at = |i: usize| matches!(lexemes.get(i), Some((Lexeme::Symbol(b'.'), _)));
    let mut out = String::with_capacity(sql.len());
    let mut named = BTreeSet::new();
    let mut copied = 0;
    let mut i = 0;
    while i < lexemes.len() {
        let qualifies =
            name_at(i).as_deref() == Some("system") && dot_at(i + 1) && !(i > 0 && dot_at(i - 1));
        if qualifies && let Some(table) = name_at(i + 2).filter(|t| REPLACED.contains(&t.as_str()))
        {
            let start = lexemes[i].1.start;
            let end = lexemes[i + 2].1.end;
            out.push_str(&sql[copied..start]);
            out.push_str(&format!("`{SYSTEM_VIEWS}`.`{table}`"));
            copied = end;
            named.insert(table);
            i += 3;
            continue;
        }
        i += 1;
    }
    out.push_str(&sql[copied..]);
    (out, named)
}

/// The `SELECT` ClickHouse answers a `SHOW TABLES`, `SHOW DATABASES` or `SHOW
/// PROCESSLIST` with, for `current` as the current database; `None` for any
/// other statement.
pub fn show_query(
    text: &str,
    current: &str,
) -> Option<Result<(String, Option<String>), HouseError>> {
    let list = match ddl::parse_show_list(text)? {
        Ok(list) => list,
        Err(err) => return Some(Err(err)),
    };
    let like = |like: &Option<ddl::ShowLike>, column: &str| {
        like.as_ref().map(|l| {
            format!(
                "{column} {}{} {}",
                if l.not { "NOT " } else { "" },
                if l.insensitive { "ILIKE" } else { "LIKE" },
                quote_string(&l.pattern)
            )
        })
    };
    Some(Ok(match list {
        ShowList::Databases { like: l, limit } => {
            let mut q = "SELECT name FROM system.databases".to_string();
            if let Some(cond) = like(&l, "name") {
                q.push_str(&format!(" WHERE {cond}"));
            }
            q.push_str(" ORDER BY name");
            if let Some(n) = limit {
                q.push_str(&format!(" LIMIT {n}"));
            }
            (q, None)
        }
        ShowList::Tables {
            full,
            from,
            like: l,
            filter,
            limit,
        } => {
            let database = from.unwrap_or_else(|| current.to_string());
            let mut q = format!(
                "SELECT {} FROM system.tables WHERE database = {}",
                if full { "name, engine" } else { "name" },
                quote_string(&database)
            );
            if let Some(cond) = like(&l, "name") {
                q.push_str(&format!(" AND {cond}"));
            } else if let Some(expr) = filter {
                q.push_str(&format!(" AND ({expr})"));
            }
            q.push_str(" ORDER BY name");
            if let Some(n) = limit {
                q.push_str(&format!(" LIMIT {n}"));
            }
            (q, Some(database))
        }
        ShowList::Processlist => (
            "SELECT * FROM system.processes ORDER BY elapsed DESC".to_string(),
            None,
        ),
    }))
}

/// chDB's columns of `system.<table>`, asked once per process.
pub async fn shape(lease: &mut WorkerLease, table: &str) -> Result<Vec<ShapeColumn>, HouseError> {
    static SHAPES: OnceLock<Mutex<HashMap<String, Vec<ShapeColumn>>>> = OnceLock::new();
    let shapes = SHAPES.get_or_init(Default::default);
    if let Some(found) = shapes.lock().ok().and_then(|m| m.get(table).cloned()) {
        return Ok(found);
    }
    let execute = Execute {
        query_id: format!("loams-system-shape-{table}"),
        session: None,
        settings: Vec::new(),
        views: Vec::new(),
        sql: format!("DESCRIBE TABLE system.{}", quote_name(table)),
        format: "JSONCompactEachRow".to_string(),
        params: Vec::new(),
        limits: Limits::default(),
        input: None,
        output: Output::Blocks,
        database: None,
    };
    let collected = lease.run(execute).await?;
    let text = String::from_utf8_lossy(&collected.bytes);
    let mut columns = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Vec<serde_json::Value> = serde_json::from_str(line).map_err(|err| {
            HouseError::from(ChError::network_error(format!(
                "the worker's DESCRIBE of system.{table} is unreadable: {err}"
            )))
        })?;
        let text = |i: usize| {
            row.get(i)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        columns.push(ShapeColumn {
            name: text(0),
            ty: text(1),
        });
    }
    if columns.is_empty() {
        return Err(HouseError::from(ChError::network_error(format!(
            "the worker described system.{table} with no columns"
        ))));
    }
    if let Ok(mut map) = shapes.lock() {
        map.insert(table.to_string(), columns.clone());
    }
    Ok(columns)
}

/// What the House fills of a table's rows: column names, and per row a value
/// (text that `CAST` reads as the column's type, or `NULL`) for each.
#[derive(Debug, Default)]
struct Rows {
    columns: Vec<&'static str>,
    rows: Vec<Vec<Option<String>>>,
}

/// `SELECT` of every column of `shape`, the filled ones from `rows` and the
/// rest at their type's default.
fn catalog_select(shape: &[ShapeColumn], rows: &Rows) -> String {
    let select: Vec<String> = shape
        .iter()
        .map(|c| {
            let name = quote_name(&c.name);
            if rows.columns.contains(&c.name.as_str()) {
                format!("CAST(__v_{}, {}) AS {name}", c.name, quote_string(&c.ty))
            } else {
                format!("defaultValueOfTypeName({}) AS {name}", quote_string(&c.ty))
            }
        })
        .collect();
    let structure = rows
        .columns
        .iter()
        .map(|c| format!("__v_{c} Nullable(String)"))
        .collect::<Vec<_>>()
        .join(", ");
    let from = if rows.rows.is_empty() || rows.columns.is_empty() {
        let structure = if structure.is_empty() {
            "__v_ Nullable(String)".to_string()
        } else {
            structure
        };
        format!("null({})", quote_string(&structure))
    } else {
        let values: Vec<String> = rows
            .rows
            .iter()
            .map(|row| {
                let cells: Vec<String> = row
                    .iter()
                    .map(|v| {
                        v.as_deref()
                            .map_or_else(|| "NULL".to_string(), quote_string)
                    })
                    .collect();
                format!("({})", cells.join(", "))
            })
            .collect();
        format!(
            "values({}, {})",
            quote_string(&structure),
            values.join(", ")
        )
    };
    format!("SELECT {}\nFROM {from}", select.join(", "))
}

/// `SELECT` of every column of chDB's own `system.<table>`, the columns in
/// `blank` at their type's default and those in `values` replaced.
fn engine_select(
    table: &str,
    shape: &[ShapeColumn],
    blank: &[&str],
    values: &[(&str, String)],
    filter: Option<String>,
) -> String {
    let select: Vec<String> = shape
        .iter()
        .map(|c| {
            let name = quote_name(&c.name);
            if let Some((_, value)) = values.iter().find(|(n, _)| *n == c.name) {
                format!("CAST({value}, {}) AS {name}", quote_string(&c.ty))
            } else if blank.contains(&c.name.as_str()) {
                format!("defaultValueOfTypeName({}) AS {name}", quote_string(&c.ty))
            } else {
                name
            }
        })
        .collect();
    let mut out = format!("SELECT {}\nFROM system.{table}", select.join(", "));
    if let Some(filter) = filter {
        out.push_str(&format!("\nWHERE {filter}"));
    }
    out
}

fn list(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| quote_string(v))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `system` rows of chDB's own databases that the deny list lets a
/// statement read.
fn engine_rows_filter(database_column: &str, table_column: &str) -> String {
    format!(
        "{database_column} IN ({}) AND ({database_column} != 'system' OR {table_column} IN ({}))",
        list(ENGINE_DATABASES),
        list(ALLOWED_SYSTEM_TABLES)
    )
}

/// Unix milliseconds as `YYYY-MM-DD hh:mm:ss` (UTC).
fn datetime_text(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A key's columns (the bare columns and those a function of one names).
fn key_columns(expr: &str) -> BTreeSet<String> {
    ddl::key_elements(expr)
        .into_iter()
        .filter_map(|(text, column)| {
            column.or_else(|| {
                let inner = text.split_once('(')?.1.strip_suffix(')')?;
                Some(inner.trim_matches('`').to_string())
            })
        })
        .collect()
}

/// A key as `system.tables` prints it: its elements joined by `, `.
fn key_text(expr: &str) -> String {
    ddl::key_elements(expr)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every catalog table of the namespace.
async fn catalog_tables(
    lake: &Lake,
    namespace: u64,
) -> Result<Vec<(String, String, LakeTable)>, HouseError> {
    let mut out = Vec::new();
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            if let Some(table) = lake.load(namespace, &database.name, &name).await? {
                out.push((database.name.clone(), name, table));
            }
        }
    }
    Ok(out)
}

fn summary_number(table: &LakeTable, key: &str) -> Option<String> {
    let snapshot = table.metadata.current_snapshot()?;
    snapshot.summary().additional_properties.get(key).cloned()
}

/// An engine's name without its arguments, as `system.tables.engine` shows it.
fn engine_name(engine: &str) -> String {
    engine
        .split_once('(')
        .map_or(engine, |(name, _)| name)
        .trim()
        .to_string()
}

/// The engine clause `system.tables.engine_full` prints.
fn engine_full(table: &ChTable) -> String {
    if table.class == TableClass::External {
        return table.engine.clone();
    }
    let mut out = table.engine.clone();
    if !table.partition_by.is_empty() {
        out.push_str(&format!(" PARTITION BY {}", table.partition_by));
    }
    if !table.primary_key.is_empty() {
        out.push_str(&format!(" PRIMARY KEY {}", table.primary_key));
    }
    let order_by = if table.order_by.is_empty() {
        "tuple()"
    } else {
        &table.order_by
    };
    out.push_str(&format!(" ORDER BY {order_by}"));
    if !table.settings.is_empty() {
        let settings: Vec<String> = table
            .settings
            .iter()
            .map(|(k, v)| format!("{k} = {v}"))
            .collect();
        out.push_str(&format!(" SETTINGS {}", settings.join(", ")));
    }
    out
}

/// The views a statement that names `tables` (of [`REPLACED`]) needs.
pub async fn views(
    ctx: ReadContext<'_>,
    lease: &mut WorkerLease,
    tables: &BTreeSet<String>,
) -> Result<Vec<String>, HouseError> {
    let mut catalog = match ctx.lake {
        Some(lake)
            if tables.iter().any(|t| {
                !matches!(
                    t.as_str(),
                    "processes"
                        | "errors"
                        | "query_log"
                        | "macros"
                        | "mutations"
                        | "distributed_ddl_queue"
                )
            }) =>
        {
            catalog_tables(lake, ctx.namespace).await?
        }
        _ => Vec::new(),
    };
    // Materialized views are recorded on their sources (HS1 Tasks 16, 17).
    let mut materialized: Vec<(crate::pipes::MaterializedView, i64)> = catalog
        .iter()
        .flat_map(|(_, _, table)| {
            let at = table.metadata.last_updated_ms();
            crate::pipes::views_of(table)
                .into_iter()
                .chain(crate::refresh::refreshes_of(table))
                .map(move |view| (view, at))
        })
        .collect();
    materialized.sort_by(|(a, _), (b, _)| (&a.database, &a.name).cmp(&(&b.database, &b.name)));
    // Only the tables the identity may touch are listed (HS1 Task 20).
    if let Some(grants) = ctx.grants {
        catalog.retain(|(database, name, _)| grants.allows(database, name));
        materialized.retain(|(view, _)| grants.allows(&view.database, &view.name));
    }
    let mut out = vec![format!(
        "CREATE DATABASE IF NOT EXISTS {} ENGINE = Memory",
        quote_name(SYSTEM_VIEWS)
    )];
    for table in tables {
        if let Some((_, columns)) = EMPTY_CLUSTER_TABLES.iter().find(|(t, _)| t == table) {
            out.push(format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{}",
                quote_name(SYSTEM_VIEWS),
                quote_name(table),
                catalog_select(&fixed_shape(columns), &Rows::default())
            ));
            continue;
        }
        if table == "clusters" {
            out.push(format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{}",
                quote_name(SYSTEM_VIEWS),
                quote_name(table),
                clusters_view(&catalog)
            ));
            continue;
        }
        if table == "query_log" {
            out.push(format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{}",
                quote_name(SYSTEM_VIEWS),
                quote_name(table),
                query_log_view(ctx).await?
            ));
            continue;
        }
        if table == "loams_pipes" {
            out.push(format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{}",
                quote_name(SYSTEM_VIEWS),
                quote_name(table),
                pipes_view(ctx).await?
            ));
            continue;
        }
        let shape = shape(lease, table).await?;
        let body = match table.as_str() {
            "databases" => databases(ctx, &shape).await?,
            "tables" => tables_view(&shape, &catalog, &materialized),
            "columns" => columns_view(&shape, &catalog),
            "parts" => match ctx.lake {
                Some(lake) => parts_view(lake, &shape, &catalog).await?,
                None => catalog_select(&shape, &Rows::default()),
            },
            // The namespace's running statements, from the front (HS1 Task
            // 23): the caller's own, every user's with `house:admin`.
            "processes" if ctx.front.is_some() => processes_view(ctx, &shape),
            "processes" => engine_select(
                "processes",
                &shape,
                &[
                    "address",
                    "port",
                    "initial_address",
                    "initial_port",
                    "os_user",
                    "client_hostname",
                    "client_name",
                    "thread_ids",
                    "http_user_agent",
                    "http_referer",
                    "forwarded_for",
                    "quota_key",
                ],
                &[
                    ("user", quote_string(ctx.user)),
                    ("initial_user", quote_string(ctx.user)),
                ],
                None,
            ),
            "errors" => engine_select(
                "errors",
                &shape,
                &[
                    "last_error_message",
                    "last_error_format_string",
                    "last_error_trace",
                ],
                &[],
                None,
            ),
            other => {
                return Err(HouseError::from(ChError::bad_arguments(format!(
                    "system.{other} is not one the House replaces"
                ))));
            }
        };
        out.push(format!(
            "CREATE OR REPLACE VIEW {}.{} AS\n{body}",
            quote_name(SYSTEM_VIEWS),
            quote_name(table)
        ));
    }
    Ok(out)
}

/// A shape from a fixed column list.
fn fixed_shape(columns: &[(&str, &str)]) -> Vec<ShapeColumn> {
    columns
        .iter()
        .map(|(name, ty)| ShapeColumn {
            name: (*name).to_string(),
            ty: (*ty).to_string(),
        })
        .collect()
}

/// The clusters a one-node House answers (HE1 R9.62): one row, this node, for
/// each cluster an `ON CLUSTER` clause of the namespace's stored `CREATE`
/// statements names (the House accepts any name there, D344). The node is
/// local and named `localhost` at `127.0.0.1`, so an application looking for
/// other hosts of its cluster finds none.
fn clusters_view(catalog: &[(String, String, LakeTable)]) -> String {
    let names: BTreeSet<String> = catalog
        .iter()
        .filter_map(|(database, name, table)| {
            ddl::on_cluster_of(&ChTable::from_lake(database, name, table).ddl)
        })
        .collect();
    let rows = Rows {
        columns: vec![
            "cluster",
            "shard_num",
            "shard_weight",
            "replica_num",
            "host_name",
            "host_address",
            "port",
            "is_local",
            "user",
            "name",
        ],
        rows: names
            .into_iter()
            .map(|cluster| {
                vec![
                    Some(cluster.clone()),
                    Some("1".to_string()),
                    Some("1".to_string()),
                    Some("1".to_string()),
                    Some("localhost".to_string()),
                    Some("127.0.0.1".to_string()),
                    Some("9000".to_string()),
                    Some("1".to_string()),
                    Some("default".to_string()),
                    Some(cluster),
                ]
            })
            .collect(),
    };
    catalog_select(&fixed_shape(CLUSTERS_COLUMNS), &rows)
}

/// `system.processes` from the front's process list (HS1 Task 23).
fn processes_view(ctx: ReadContext<'_>, shape: &[ShapeColumn]) -> String {
    let mut rows = Rows {
        columns: vec![
            "is_initial_query",
            "user",
            "query_id",
            "initial_user",
            "initial_query_id",
            "elapsed",
            "is_cancelled",
            "query",
        ],
        rows: Vec::new(),
    };
    if let Some(front) = ctx.front {
        for p in front.processes.list(ctx.namespace) {
            if !ctx.admin && p.user != ctx.user {
                continue;
            }
            rows.rows.push(vec![
                Some("1".to_string()),
                Some(p.user.clone()),
                Some(p.query_id.clone()),
                Some(p.user),
                Some(p.query_id),
                Some(format!("{:.6}", p.elapsed.as_secs_f64())),
                Some(u8::from(p.cancelled).to_string()),
                Some(p.query),
            ]);
        }
    }
    catalog_select(shape, &rows)
}

/// `system.query_log`'s columns: ClickHouse's names and types where it has
/// them, then the House's own (HS1 Task 23).
pub const QUERY_LOG_COLUMNS: &[(&str, &str)] = &[
    ("type", "String"),
    ("event_date", "Date"),
    ("event_time", "DateTime('UTC')"),
    ("event_time_microseconds", "DateTime64(6, 'UTC')"),
    ("query_start_time", "DateTime('UTC')"),
    ("query_start_time_microseconds", "DateTime64(6, 'UTC')"),
    ("query_duration_ms", "UInt64"),
    ("read_rows", "UInt64"),
    ("read_bytes", "UInt64"),
    ("written_rows", "UInt64"),
    ("written_bytes", "UInt64"),
    ("result_rows", "UInt64"),
    ("result_bytes", "UInt64"),
    ("memory_usage", "UInt64"),
    ("current_database", "String"),
    ("query", "String"),
    ("normalized_query_hash", "UInt64"),
    ("query_kind", "String"),
    ("is_initial_query", "UInt8"),
    ("user", "String"),
    ("query_id", "String"),
    ("initial_user", "String"),
    ("initial_query_id", "String"),
    ("exception_code", "Int32"),
    ("exception", "String"),
    ("ProfileEvents", "Map(String, UInt64)"),
    ("loams_principal", "String"),
    ("loams_surface", "String"),
];

/// `system.query_log` (HS1 Task 23): the namespace's `_house.query_log` (the
/// view [`crate::read::plan`] makes for it) and this front's rows not written
/// yet; the caller's own rows, every user's with `house:admin`.
async fn query_log_view(ctx: ReadContext<'_>) -> Result<String, HouseError> {
    use crate::querylog::{COLUMNS, HOUSE_DATABASE, QUERY_LOG_TABLE};
    let mut parts: Vec<String> = Vec::new();
    let log = ctx.front.and_then(|f| f.query_log);
    if log.is_some()
        && let Some(lake) = ctx.lake
        && lake
            .load(ctx.namespace, HOUSE_DATABASE, QUERY_LOG_TABLE)
            .await?
            .is_some()
    {
        let list: Vec<String> = COLUMNS
            .iter()
            .map(|(name, _)| format!("{} AS __{name}", quote_name(name)))
            .collect();
        parts.push(format!(
            "SELECT {} FROM {}.{}",
            list.join(", "),
            quote_name(HOUSE_DATABASE),
            quote_name(QUERY_LOG_TABLE)
        ));
    }
    // The rows not written yet, as literals: dates as days, times as
    // microseconds.
    let pending = log.map(|l| l.pending(ctx.namespace)).unwrap_or_default();
    let raw = |ty: &str| match ty {
        "Date" => "Int32".to_string(),
        t if t.starts_with("DateTime64") => "Int64".to_string(),
        t => t.to_string(),
    };
    let structure: Vec<String> = COLUMNS
        .iter()
        .map(|(name, ty)| format!("__v_{name} {}", raw(ty)))
        .collect();
    let converted: Vec<String> = COLUMNS
        .iter()
        .map(|(name, ty)| match *ty {
            "Date" => format!("addDays(toDate('1970-01-01'), __v_{name}) AS __{name}"),
            t if t.starts_with("DateTime64") => {
                format!("fromUnixTimestamp64Micro(__v_{name}, 'UTC') AS __{name}")
            }
            _ => format!("__v_{name} AS __{name}"),
        })
        .collect();
    let source = if pending.is_empty() {
        format!("null({})", quote_string(&structure.join(", ")))
    } else {
        let rows: Vec<String> = pending
            .iter()
            .map(|e| format!("({})", crate::querylog::literals(e).join(", ")))
            .collect();
        format!(
            "values({}, {})",
            quote_string(&structure.join(", ")),
            rows.join(", ")
        )
    };
    parts.push(format!("SELECT {} FROM {source}", converted.join(", ")));
    let filter = if ctx.admin {
        String::new()
    } else {
        format!("\nWHERE __user = {}", quote_string(ctx.user))
    };
    let typed = |expr: &str, name: &str| -> String {
        let ty = QUERY_LOG_COLUMNS
            .iter()
            .find(|(n, _)| *n == name)
            .map_or("String", |(_, t)| *t);
        format!("CAST({expr}, {}) AS {}", quote_string(ty), quote_name(name))
    };
    let select = [
        typed("__type", "type"),
        typed("__event_date", "event_date"),
        typed("toDateTime(__event_time, 'UTC')", "event_time"),
        typed("__event_time", "event_time_microseconds"),
        typed("toDateTime(__query_start_time, 'UTC')", "query_start_time"),
        typed("__query_start_time", "query_start_time_microseconds"),
        typed("greatest(__query_duration_ms, 0)", "query_duration_ms"),
        typed("greatest(__read_rows, 0)", "read_rows"),
        typed("greatest(__read_bytes, 0)", "read_bytes"),
        typed("greatest(__written_rows, 0)", "written_rows"),
        typed("greatest(__written_bytes, 0)", "written_bytes"),
        typed("greatest(__result_rows, 0)", "result_rows"),
        typed("greatest(__result_bytes, 0)", "result_bytes"),
        typed("greatest(__memory_usage, 0)", "memory_usage"),
        typed("__current_database", "current_database"),
        typed("__query", "query"),
        typed(
            "reinterpretAsUInt64(__normalized_query_hash)",
            "normalized_query_hash",
        ),
        typed("__query_kind", "query_kind"),
        typed("1", "is_initial_query"),
        typed("__user", "user"),
        typed("__query_id", "query_id"),
        typed("__user", "initial_user"),
        typed("__query_id", "initial_query_id"),
        typed("__exception_code", "exception_code"),
        typed("__exception", "exception"),
        typed(
            "map('OSCPUVirtualTimeMicroseconds', toUInt64(greatest(ifNull(__cpu_time_us, 0), 0)))",
            "ProfileEvents",
        ),
        typed("__loams_principal", "loams_principal"),
        typed("__loams_surface", "loams_surface"),
    ];
    Ok(format!(
        "SELECT {}\nFROM ({}){filter}",
        select.join(", "),
        parts.join("\nUNION ALL\n")
    ))
}

async fn pipes_view(ctx: ReadContext<'_>) -> Result<String, HouseError> {
    let shape: Vec<ShapeColumn> = PIPES_COLUMNS
        .iter()
        .map(|(name, ty)| ShapeColumn {
            name: name.to_string(),
            ty: ty.to_string(),
        })
        .collect();
    let mut rows = Rows {
        columns: PIPES_COLUMNS.iter().map(|(name, _)| *name).collect(),
        rows: Vec::new(),
    };
    if let Some(lake) = ctx.lake {
        for row in crate::pipes::rows(lake, ctx.namespace).await? {
            if ctx
                .grants
                .is_some_and(|grants| !grants.allows(&row.database, &row.name))
            {
                continue;
            }
            let state = &row.state;
            rows.rows.push(vec![
                Some(row.database),
                Some(row.name),
                Some(row.pipe),
                Some(row.source),
                Some(row.target),
                Some(state.lag_records.to_string()),
                Some(state.committed_records.to_string()),
                Some(state.committed_rows.to_string()),
                state
                    .last_commit_ms
                    .map(|ms| format!("{}.{:03}", datetime_text(ms), ms.rem_euclid(1000))),
                Some(state.last_error.clone()),
            ]);
        }
    }
    Ok(catalog_select(&shape, &rows))
}

async fn databases(ctx: ReadContext<'_>, shape: &[ShapeColumn]) -> Result<String, HouseError> {
    let mut rows = Rows {
        columns: vec!["name", "engine", "engine_full", "comment"],
        rows: Vec::new(),
    };
    match ctx.lake {
        Some(lake) => {
            for database in lake.databases(ctx.namespace).await? {
                if ctx
                    .grants
                    .is_some_and(|grants| !grants.mentions_database(&database.name))
                {
                    continue;
                }
                rows.rows.push(vec![
                    Some(database.name),
                    Some("Atomic".to_string()),
                    Some("Atomic".to_string()),
                    Some(database.comment),
                ]);
            }
        }
        None => rows.rows.push(vec![
            Some("default".to_string()),
            Some("Atomic".to_string()),
            Some("Atomic".to_string()),
            Some(String::new()),
        ]),
    }
    Ok(format!(
        "{}\nUNION ALL\n{}",
        catalog_select(shape, &rows),
        engine_select(
            "databases",
            shape,
            &["data_path", "metadata_path"],
            &[],
            Some(format!("name IN ({})", list(ENGINE_DATABASES))),
        )
    ))
}

/// `CREATE MATERIALIZED VIEW` as `system.tables.create_table_query` shows it.
fn view_ddl(view: &crate::pipes::MaterializedView) -> String {
    let refresh = view
        .refresh
        .as_ref()
        .map(|r| format!(" {}", r.render()))
        .unwrap_or_default();
    format!(
        "CREATE MATERIALIZED VIEW {}.{}{refresh} TO {}.{} AS {}",
        quote_name(&view.database),
        quote_name(&view.name),
        quote_name(&view.target_database),
        quote_name(&view.target),
        view.select
    )
}

fn tables_view(
    shape: &[ShapeColumn],
    catalog: &[(String, String, LakeTable)],
    materialized: &[(crate::pipes::MaterializedView, i64)],
) -> String {
    let mut rows = Rows {
        columns: vec![
            "database",
            "name",
            "uuid",
            "engine",
            "is_temporary",
            "metadata_modification_time",
            "create_table_query",
            "engine_full",
            "partition_key",
            "sorting_key",
            "primary_key",
            "total_rows",
            "total_bytes",
            "parts",
            "active_parts",
            "comment",
            "has_own_data",
            "table",
            "as_select",
        ],
        rows: Vec::new(),
    };
    for (database, name, table) in catalog {
        let ch = ChTable::from_lake(database, name, table);
        let primary = if ch.primary_key.is_empty() {
            &ch.order_by
        } else {
            &ch.primary_key
        };
        let files = summary_number(table, "total-data-files").or(Some("0".to_string()));
        rows.rows.push(vec![
            Some(database.clone()),
            Some(name.clone()),
            Some(table.metadata.uuid().to_string()),
            Some(engine_name(&ch.engine)),
            Some("0".to_string()),
            Some(datetime_text(table.metadata.last_updated_ms())),
            Some(ch.ddl.clone()),
            Some(engine_full(&ch)),
            Some(ch.partition_by.clone()),
            Some(key_text(&ch.order_by)),
            Some(key_text(primary)),
            summary_number(table, "total-records").or(Some("0".to_string())),
            summary_number(table, "total-files-size").or(Some("0".to_string())),
            files.clone(),
            files,
            Some(ch.comment.clone()),
            Some("1".to_string()),
            Some(name.clone()),
            Some(String::new()),
        ]);
    }
    // A view with `TO` holds no data: ClickHouse's sizes are NULL.
    for (view, at) in materialized {
        rows.rows.push(vec![
            Some(view.database.clone()),
            Some(view.name.clone()),
            Some(
                uuid::Uuid::parse_str(&view.id)
                    .unwrap_or_default()
                    .hyphenated()
                    .to_string(),
            ),
            Some("MaterializedView".to_string()),
            Some("0".to_string()),
            Some(datetime_text(*at)),
            Some(view_ddl(view)),
            Some(String::new()),
            Some(String::new()),
            Some(String::new()),
            Some(String::new()),
            None,
            None,
            None,
            None,
            Some(String::new()),
            Some("0".to_string()),
            Some(view.name.clone()),
            Some(view.select.clone()),
        ]);
    }
    format!(
        "{}\nUNION ALL\n{}",
        catalog_select(shape, &rows),
        engine_select(
            "tables",
            shape,
            &["data_paths", "metadata_path"],
            &[],
            Some(format!(
                "({}) OR is_temporary",
                engine_rows_filter("database", "name")
            )),
        )
    )
}

fn columns_view(shape: &[ShapeColumn], catalog: &[(String, String, LakeTable)]) -> String {
    let mut rows = Rows {
        columns: vec![
            "database",
            "table",
            "name",
            "type",
            "position",
            "default_kind",
            "default_expression",
            "comment",
            "is_in_partition_key",
            "is_in_sorting_key",
            "is_in_primary_key",
            "compression_codec",
        ],
        rows: Vec::new(),
    };
    for (database, name, table) in catalog {
        let ch = ChTable::from_lake(database, name, table);
        let partition = key_columns(&ch.partition_by);
        let sorting = key_columns(&ch.order_by);
        let primary = if ch.primary_key.is_empty() {
            sorting.clone()
        } else {
            key_columns(&ch.primary_key)
        };
        let flag =
            |set: &BTreeSet<String>, column: &str| Some(u8::from(set.contains(column)).to_string());
        for (position, column) in ch.columns.iter().enumerate() {
            rows.rows.push(vec![
                Some(database.clone()),
                Some(name.clone()),
                Some(column.name.clone()),
                Some(column.ch_type.clone()),
                Some((position + 1).to_string()),
                Some(column.default_kind.clone()),
                Some(column.default_expression.clone()),
                Some(column.comment.clone()),
                flag(&partition, &column.name),
                flag(&sorting, &column.name),
                flag(&primary, &column.name),
                Some(if column.codec.is_empty() {
                    String::new()
                } else {
                    format!("CODEC({})", column.codec)
                }),
            ]);
        }
    }
    format!(
        "{}\nUNION ALL\n{}",
        catalog_select(shape, &rows),
        engine_select(
            "columns",
            shape,
            &[],
            &[],
            Some(engine_rows_filter("database", "table")),
        )
    )
}

async fn parts_view(
    lake: &Lake,
    shape: &[ShapeColumn],
    catalog: &[(String, String, LakeTable)],
) -> Result<String, HouseError> {
    let mut rows = Rows {
        columns: vec![
            "partition",
            "partition_id",
            "name",
            "active",
            "rows",
            "bytes_on_disk",
            "data_compressed_bytes",
            "database",
            "table",
            "engine",
            "disk_name",
            "min_block_number",
            "max_block_number",
            "level",
        ],
        rows: Vec::new(),
    };
    for (database, name, table) in catalog {
        let Some(snapshot) = table.metadata.current_snapshot() else {
            continue;
        };
        let engine = engine_name(&ChTable::from_lake(database, name, table).engine);
        for file in live_files(lake, &table.metadata, snapshot.snapshot_id()).await? {
            if !file.data {
                continue;
            }
            let file_name = file
                .location
                .rsplit_once('/')
                .map_or(file.location.as_str(), |(_, n)| n);
            let partition_id = if file.partition == "tuple()" {
                "all".to_string()
            } else {
                file.partition.trim_matches('\'').to_string()
            };
            rows.rows.push(vec![
                Some(file.partition.clone()),
                Some(partition_id),
                Some(file_name.to_string()),
                Some("1".to_string()),
                Some(file.record_count.to_string()),
                Some(file.size_bytes.to_string()),
                Some(file.size_bytes.to_string()),
                Some(database.clone()),
                Some(name.clone()),
                Some(engine.clone()),
                Some("default".to_string()),
                Some(file.sequence_number.to_string()),
                Some(file.sequence_number.to_string()),
                Some("0".to_string()),
            ]);
        }
    }
    Ok(catalog_select(shape, &rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HE1 R9.62: only `clusterAllReplicas(<cluster>, system.<t>)` is read
    /// locally; every other use stays for the deny list to refuse.
    #[test]
    fn cluster_reads_of_system_tables_are_local() {
        for (sql, want) in [
            (
                "SELECT count(*) FROM clusterAllReplicas('cluster', system.tables) WHERE database = 'd'",
                "SELECT count(*) FROM system.tables WHERE database = 'd'",
            ),
            (
                "SELECT 1 FROM ClusterAllReplicas( c , `system` . `parts` ) AS p",
                "SELECT 1 FROM system.`parts` AS p",
            ),
            (
                "SELECT * FROM clusterAllReplicas('c', default.t)",
                "SELECT * FROM clusterAllReplicas('c', default.t)",
            ),
            (
                "SELECT * FROM clusterAllReplicas('c', t)",
                "SELECT * FROM clusterAllReplicas('c', t)",
            ),
            (
                "SELECT * FROM clusterAllReplicas('c', system.tables, rand())",
                "SELECT * FROM clusterAllReplicas('c', system.tables, rand())",
            ),
            (
                "SELECT * FROM clusterAllReplicas('c', system, tables)",
                "SELECT * FROM clusterAllReplicas('c', system, tables)",
            ),
            (
                "SELECT 'clusterAllReplicas(''c'', system.tables)'",
                "SELECT 'clusterAllReplicas(''c'', system.tables)'",
            ),
            (
                "SELECT * FROM remote('h', system.tables)",
                "SELECT * FROM remote('h', system.tables)",
            ),
        ] {
            assert_eq!(rewrite_cluster_reads(sql), want, "{sql}");
        }
    }

    #[test]
    fn rewrite_takes_only_qualified_system_tables() {
        let (sql, named) = rewrite(
            "SELECT name, 'system.tables' FROM system.tables JOIN `system`.\"columns\" USING (x) \
             -- system.parts\nWHERE system . databases.name != '' AND t.system.parts = 1 \
             AND system.settings.name = ''",
        );
        assert_eq!(
            sql,
            "SELECT name, 'system.tables' FROM `__loams_system`.`tables` JOIN \
             `__loams_system`.`columns` USING (x) -- system.parts\nWHERE \
             `__loams_system`.`databases`.name != '' AND t.system.parts = 1 AND \
             system.settings.name = ''"
        );
        assert_eq!(
            named.into_iter().collect::<Vec<_>>(),
            vec!["columns", "databases", "tables"]
        );
    }

    #[test]
    fn show_forms_are_clickhouses_selects() {
        let q = |text: &str| show_query(text, "db").map(|r| r.expect("reads").0);
        assert_eq!(
            q("SHOW TABLES").as_deref(),
            Some("SELECT name FROM system.tables WHERE database = 'db' ORDER BY name")
        );
        assert_eq!(
            q("show full tables from x not ilike 'a%' limit 3").as_deref(),
            Some(
                "SELECT name, engine FROM system.tables WHERE database = 'x' AND name NOT ILIKE \
                 'a%' ORDER BY name LIMIT 3"
            )
        );
        assert_eq!(
            q("SHOW TABLES IN x WHERE engine = 'View' LIMIT 1").as_deref(),
            Some(
                "SELECT name FROM system.tables WHERE database = 'x' AND (engine = 'View') ORDER \
                 BY name LIMIT 1"
            )
        );
        assert_eq!(
            q("SHOW DATABASES LIKE '%a%'").as_deref(),
            Some("SELECT name FROM system.databases WHERE name LIKE '%a%' ORDER BY name")
        );
        assert_eq!(
            q("SHOW PROCESSLIST").as_deref(),
            Some("SELECT * FROM system.processes ORDER BY elapsed DESC")
        );
        assert_eq!(q("SHOW TEMPORARY TABLES"), None);
        assert_eq!(q("SHOW SETTINGS LIKE 'max%'"), None);
        assert!(show_query("SHOW TABLES LIMIT x", "db").is_some_and(|r| r.is_err()));
    }

    #[test]
    fn catalog_rows_fill_chdbs_columns() {
        let shape = vec![
            ShapeColumn {
                name: "name".to_string(),
                ty: "String".to_string(),
            },
            ShapeColumn {
                name: "data_path".to_string(),
                ty: "String".to_string(),
            },
            ShapeColumn {
                name: "total_rows".to_string(),
                ty: "Nullable(UInt64)".to_string(),
            },
        ];
        let rows = Rows {
            columns: vec!["name", "total_rows"],
            rows: vec![vec![Some("a'b".to_string()), None]],
        };
        assert_eq!(
            catalog_select(&shape, &rows),
            "SELECT CAST(__v_name, 'String') AS name, defaultValueOfTypeName('String') AS \
             data_path, CAST(__v_total_rows, 'Nullable(UInt64)') AS total_rows\nFROM \
             values('__v_name Nullable(String), __v_total_rows Nullable(String)', ('a\\'b', NULL))"
        );
        assert!(
            catalog_select(&shape, &Rows::default())
                .ends_with("FROM null('__v_ Nullable(String)')")
        );
        assert_eq!(datetime_text(1_791_800_430_500), "2026-10-12 10:20:30");
        assert_eq!(datetime_text(0), "1970-01-01 00:00:00");
        assert_eq!(
            key_columns("(k, toYYYYMM(ts), a + b)"),
            BTreeSet::from(["k".to_string(), "ts".to_string()])
        );
    }
}
