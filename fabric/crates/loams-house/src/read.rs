//! Lake reads (HS1 Task 11, §49 §7, D766): every lake table a statement names
//! is pinned to one snapshot and read through a view the worker makes for that
//! statement alone.
//!
//! 1. **What the statement reads** comes from ClickHouse's own analysis, the
//!    worker's `EXPLAIN AST` (the deny list's trees, HS1 Task 5): every
//!    `TableIdentifier`, a bare name on the right of `IN`, the database
//!    `merge()` reads, and `information_schema`. Which tables are read with
//!    `FINAL` comes from the query tree. The user's text runs unchanged, except
//!    for `system` tables ([`crate::system`]).
//! 2. **Pinning** ([`pin`]): each name the catalog has is pinned to its current
//!    snapshot, or to the one `loams_snapshot_id` or `loams_as_of` names
//!    (Iceberg time travel), in its current metadata file. The pin is what the
//!    view reads, whatever commits land while the query runs.
//! 3. **Views** ([`crate::views`]): one per pin, in a `Memory` database named as
//!    the ClickHouse database, made on the worker's control connection before the
//!    statement; the worker drops them before its next statement, so a name the
//!    statement does not name reads nothing. The statement runs with its current
//!    database (`Execute::database`).
//!
//! A name the catalog does not have gets no view, and chDB answers it as
//! ClickHouse does: `60` for an unknown table, `81` for an unknown database. The
//! worker is bound to one namespace and only that namespace's catalog is asked,
//! so another namespace's table cannot be named at all.

use std::collections::BTreeSet;

use loams_house_ipc::Analysis;
use loams_house_lake::files::{self, LiveFile};
use loams_house_lake::spec::{SnapshotRef, TableMetadata};

use crate::admission::WorkerLease;
use crate::catalog::{Lake, LakeTable};
use crate::ddl::{ChTable, LakeEngine, TableClass};
use crate::deny::{Node, QueryTree};
use crate::errors::{ChError, HouseError};
use crate::views::{self, Mode};

/// Setting: the snapshot to read, `<id>` for every table the statement reads,
/// or `[db.]t=<id>, …` per table.
pub const SNAPSHOT_ID: &str = "loams_snapshot_id";
/// The front's own setting (never a request's: it is on no list, so a request
/// naming it is `115`) that reads every lake table a statement names in
/// [`Mode::Sequenced`]: a mutation's rewrite (HE1 R9.5).
pub const SEQUENCED: &str = "loams_internal_sequenced";
/// Setting: read every table as it was at this time (UTC `YYYY-MM-DD
/// hh:mm:ss[.fff]`, RFC 3339, or Unix milliseconds).
pub const AS_OF: &str = "loams_as_of";

/// A field of the pinned snapshot's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedField {
    /// Its name.
    pub name: String,
    /// Whether it is required (not `NULL` in any row).
    pub required: bool,
    /// Whether its Iceberg type is `long`: how a column of nanosecond times
    /// is stored (D1572; HE1 R9.61).
    pub long: bool,
}

/// One table of a statement, pinned (§49 §7): the snapshot and the metadata file
/// its view reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadPin {
    /// The table as ClickHouse shows it.
    pub table: ChTable,
    /// Its location.
    pub location: String,
    /// The metadata file the view reads (the table's current one when pinned:
    /// it holds every snapshot).
    pub metadata_file: String,
    /// The snapshot; `None` when there is none to read (a new table, or a time
    /// before its first).
    pub snapshot_id: Option<i64>,
    /// The fields of that snapshot's schema.
    pub fields: Vec<PinnedField>,
    /// Whether the snapshot is an older one than the table's current (time
    /// travel): the view then names it, and chDB reads it with the schema of its
    /// time. The current snapshot is the metadata file's own and reads the
    /// table's current schema.
    pub travel: bool,
}

/// Which snapshot a statement reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsOf {
    /// The current one of every table.
    Current,
    /// This snapshot, which every table read must have.
    Snapshot(i64),
    /// These snapshots of these tables (`[db.]t`), the current one of the rest.
    Snapshots(Vec<(Option<String>, String, i64)>),
    /// The snapshot each table had at this time (Unix milliseconds).
    Timestamp(i64),
}

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

/// The time travel the settings ask for: `36` when both are set or a value
/// cannot be read.
pub fn as_of(settings: &[(String, String)]) -> Result<AsOf, HouseError> {
    let last = |name: &str| {
        settings
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.trim().trim_matches('\'').trim())
            .filter(|v| !v.is_empty())
    };
    match (last(SNAPSHOT_ID), last(AS_OF)) {
        (None, None) => Ok(AsOf::Current),
        (Some(_), Some(_)) => Err(bad(format!(
            "Settings {SNAPSHOT_ID} and {AS_OF} cannot be used together"
        ))),
        (Some(value), None) => snapshot_setting(value),
        (None, Some(value)) => parse_time(value).map(AsOf::Timestamp).ok_or_else(|| {
            bad(format!(
                "Cannot read {AS_OF} = {value:?}: expected 'YYYY-MM-DD hh:mm:ss[.fff]' (UTC), \
                     RFC 3339 or Unix milliseconds"
            ))
        }),
    }
}

fn snapshot_setting(value: &str) -> Result<AsOf, HouseError> {
    if let Ok(id) = value.parse::<i64>() {
        return Ok(AsOf::Snapshot(id));
    }
    let mut out = Vec::new();
    for piece in value.split(',') {
        let (table, id) = piece.split_once('=').ok_or_else(|| {
            bad(format!(
                "Cannot read {SNAPSHOT_ID} = {value:?}: expected <id> or [db.]table=<id>, …"
            ))
        })?;
        let id = id
            .trim()
            .parse::<i64>()
            .map_err(|_| bad(format!("Cannot read the snapshot id in {piece:?}")))?;
        let table = table.trim();
        let (database, name) = match table.split_once('.') {
            Some((d, t)) => (Some(d.to_string()), t.to_string()),
            None => (None, table.to_string()),
        };
        if name.is_empty() {
            return Err(bad(format!("Cannot read the table in {piece:?}")));
        }
        out.push((database, name, id));
    }
    Ok(AsOf::Snapshots(out))
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DD[ T]hh:mm:ss[.fff][Z]` (UTC) or Unix milliseconds, as Unix
/// milliseconds.
fn parse_time(text: &str) -> Option<i64> {
    if let Ok(ms) = text.parse::<i64>() {
        return Some(ms);
    }
    let text = text.strip_suffix('Z').unwrap_or(text);
    let (date, time) = text.split_once([' ', 'T'])?;
    let mut d = date.split('-');
    let (year, month, day) = (
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
    );
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut t = clock.split(':');
    let (hour, minute, second) = (
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
    );
    if t.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    if !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let millis = format!("{fraction:0<3}")[..3].parse::<i64>().ok()?;
    Some(
        ((days_from_civil(year, month, day) * 24 + hour) * 60 + minute) * 60_000
            + second * 1000
            + millis,
    )
}

/// What a statement reads, from its analysis.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Table names as the explain prints them (`db.t`, `t`; a quoted name's
    /// dots are not told apart from the qualifier's, so [`resolve`] tries both).
    pub tables: Vec<String>,
    /// Names read with `FINAL`.
    pub finals: Vec<String>,
    /// Whether a `SETTINGS final = 1` clause makes every read `FINAL`.
    pub final_setting: bool,
    /// The databases `merge()` reads (`None`: the current one).
    pub merges: Vec<Option<String>>,
    /// Whether `information_schema` is read.
    pub information_schema: bool,
}

/// The functions whose right-hand side may be a bare table name.
const IN_FUNCTIONS: &[&str] = &[
    "in",
    "notIn",
    "globalIn",
    "globalNotIn",
    "nullIn",
    "notNullIn",
    "globalNullIn",
    "globalNotNullIn",
];

/// What the analysed statement reads.
pub fn references(analysis: &Analysis) -> References {
    let tree = QueryTree::from_explain(&analysis.ast, None);
    let entries = tree.entries();
    let mut out = References::default();
    let label = |at: usize| match &entries[at].node {
        Node::Other(label) => Some(label.as_str()),
        _ => None,
    };
    for (at, entry) in entries.iter().enumerate() {
        match &entry.node {
            Node::Table(name) => out.tables.push(name.clone()),
            Node::Identifier(name) => {
                // `x IN t`: the second child of `in`'s argument list.
                let Some(list) = entry.parent else { continue };
                if !label(list).is_some_and(|l| l.starts_with("ExpressionList")) {
                    continue;
                }
                let is_in = entries[list].parent.is_some_and(|f| {
                    matches!(&entries[f].node, Node::Function(names)
                        if names.iter().any(|n| IN_FUNCTIONS.contains(&n.as_str())))
                });
                let second = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.parent == Some(list))
                    .nth(1)
                    .is_some_and(|(i, _)| i == at);
                if is_in && second {
                    out.tables.push(name.clone());
                }
            }
            Node::TableFunction(names) if names.iter().any(|n| n == "merge") => {
                let args = tree.arguments(at);
                let database = match args.as_slice() {
                    [first, _, ..] => match &entries[*first].node {
                        Node::Constant(value) => Some(value.trim_matches('\'').replace("\\'", "'")),
                        Node::Identifier(name) => Some(name.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                out.merges.push(database);
            }
            _ => {}
        }
    }
    out.information_schema = out
        .tables
        .iter()
        .any(|t| t.starts_with("information_schema.") || t.starts_with("INFORMATION_SCHEMA."));
    for raw in analysis.query_tree.iter().flat_map(|t| t.lines()) {
        let line = raw.trim_start();
        if let Some(settings) = line.strip_prefix("SETTINGS ") {
            out.final_setting |= settings
                .split(' ')
                .any(|pair| matches!(pair, "final=1" | "final=true" | "final=True"));
            continue;
        }
        let Some(fields) = line
            .strip_prefix("IDENTIFIER id: ")
            .or_else(|| line.strip_prefix("TABLE id: "))
        else {
            continue;
        };
        let Some(body) = fields.strip_suffix(", final: 1") else {
            continue;
        };
        if let Some(at) = body
            .rfind("identifier: ")
            .map(|at| at + "identifier: ".len())
            .or_else(|| {
                body.rfind("table_name: ")
                    .map(|at| at + "table_name: ".len())
            })
        {
            out.finals.push(unescape(&body[at..]));
        }
    }
    out
}

/// TSV's escapes undone, for a name taken from a raw explain line.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(match next {
                    't' => '\t',
                    'n' => '\n',
                    other => other,
                });
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// How deep views may nest (HE1 R9.13), as materialized views chain (HS1 Task 17).
pub const MAX_VIEW_DEPTH: usize = 8;

/// The databases the House never serves from its catalog.
fn engine_database(database: &str) -> bool {
    matches!(
        database,
        "system" | "information_schema" | "INFORMATION_SCHEMA" | "_temporary_and_external_tables"
    )
}

/// The catalog table a printed name is, if any: `db.t` (every dot tried as the
/// qualifier, since the explain prints quoted names bare), else `t` in the
/// current database.
pub async fn resolve(
    lake: &Lake,
    namespace: u64,
    current: &str,
    printed: &str,
) -> Result<Option<(String, String, LakeTable)>, HouseError> {
    for (at, _) in printed.match_indices('.') {
        let (database, table) = (&printed[..at], &printed[at + 1..]);
        if database.is_empty() || table.is_empty() || engine_database(database) {
            continue;
        }
        if lake.database_exists(namespace, database).await?
            && let Some(loaded) = lake.load(namespace, database, table).await?
        {
            return Ok(Some((database.to_string(), table.to_string(), loaded)));
        }
    }
    if engine_database(current) || !lake.database_exists(namespace, current).await? {
        return Ok(None);
    }
    Ok(lake
        .load(namespace, current, printed)
        .await?
        .map(|loaded| (current.to_string(), printed.to_string(), loaded)))
}

/// The snapshot `as_of` picks for `database.name`: `Ok(None)` when it has none
/// to read.
fn snapshot<'a>(
    metadata: &'a TableMetadata,
    database: &str,
    current: &str,
    name: &str,
    as_of: &AsOf,
) -> Result<Option<&'a SnapshotRef>, HouseError> {
    let by_id = |id: i64| {
        metadata.snapshot_by_id(id).map(Some).ok_or_else(|| {
            bad(format!(
                "Table {database}.{name} has no snapshot {id} (loams_snapshot_id)"
            ))
        })
    };
    match as_of {
        AsOf::Current => Ok(metadata.current_snapshot()),
        AsOf::Snapshot(id) => by_id(*id),
        AsOf::Snapshots(list) => match list
            .iter()
            .find(|(d, t, _)| t == name && d.as_deref().unwrap_or(current) == database)
        {
            Some((_, _, id)) => by_id(*id),
            None => Ok(metadata.current_snapshot()),
        },
        AsOf::Timestamp(ms) => Ok(metadata
            .history()
            .iter()
            .rev()
            .find(|entry| entry.timestamp_ms <= *ms)
            .and_then(|entry| metadata.snapshot_by_id(entry.snapshot_id))),
    }
}

/// Pins a loaded table (§49 §7).
pub fn pin(
    database: &str,
    name: &str,
    current: &str,
    table: &LakeTable,
    as_of: &AsOf,
) -> Result<ReadPin, HouseError> {
    let metadata = &table.metadata;
    let chosen = snapshot(metadata, database, current, name, as_of)?;
    // The current snapshot reads the table's current schema (an `ALTER` adds
    // a column without a snapshot); an older one reads the schema of its time.
    let current_id = metadata.current_snapshot().map(|s| s.snapshot_id());
    let travel = chosen.is_some_and(|s| Some(s.snapshot_id()) != current_id);
    let schema = chosen
        .filter(|_| travel)
        .and_then(|s| s.schema_id())
        .and_then(|id| metadata.schema_by_id(id))
        .unwrap_or_else(|| metadata.current_schema());
    Ok(ReadPin {
        table: ChTable::from_lake(database, name, table),
        location: metadata.location().to_string(),
        metadata_file: table.metadata_location.clone(),
        snapshot_id: chosen.map(|s| s.snapshot_id()),
        fields: schema
            .as_struct()
            .fields()
            .iter()
            .map(|f| PinnedField {
                name: f.name.clone(),
                required: f.required,
                long: matches!(
                    *f.field_type,
                    loams_house_lake::spec::Type::Primitive(
                        loams_house_lake::spec::PrimitiveType::Long
                    )
                ),
            })
            .collect(),
        travel,
    })
}

/// The live files of a pinned snapshot, read from its manifest list and
/// manifests through the House's store.
pub async fn live_files(
    lake: &Lake,
    metadata: &TableMetadata,
    snapshot_id: i64,
) -> Result<Vec<LiveFile>, HouseError> {
    let unreadable = |err: loams_house_lake::LakeError| {
        HouseError::from(ChError::network_error(format!(
            "a manifest of {} is unreadable: {err}",
            metadata.location()
        )))
    };
    let Some(snapshot) = metadata.snapshot_by_id(snapshot_id) else {
        return Ok(Vec::new());
    };
    let list = lake.read_file(snapshot.manifest_list()).await?;
    let mut out = Vec::new();
    for manifest in files::manifests(&list, metadata.format_version()).map_err(unreadable)? {
        let bytes = lake.read_file(&manifest.location).await?;
        out.extend(files::live_files(&bytes, &manifest).map_err(unreadable)?);
    }
    Ok(out)
}

/// What a statement reads, made ready for its worker.
#[derive(Debug, Default)]
pub struct ReadPlan {
    /// The DDL its worker runs first (databases, views).
    pub views: Vec<String>,
    /// The text to run: the statement with its `system` tables replaced.
    pub sql: String,
    /// The tables it reads, pinned.
    pub pins: Vec<ReadPin>,
}

/// Who reads, and where.
#[derive(Debug, Clone, Copy)]
pub struct ReadContext<'a> {
    /// The namespace's catalog, when the House has one.
    pub lake: Option<&'a Lake>,
    /// The namespace.
    pub namespace: u64,
    /// The statement's current database.
    pub database: &'a str,
    /// The House user (what `system.processes` shows).
    pub user: &'a str,
    /// The tables the user may read (HS1 Task 20); `None`, every table of the
    /// namespace.
    pub grants: Option<&'a crate::identity::Grants>,
    /// The records a materialized view reads under its pipe's name (HS1 Task
    /// 16); a pipe read without them is `620`.
    pub batch: Option<&'a crate::pipes::PipeBatch>,
    /// What the front itself answers `system.processes` and
    /// `system.query_log` from (HS1 Task 23), for a client's statement.
    pub front: Option<Front<'a>>,
    /// Whether the reader holds `house:admin`: it sees the namespace's
    /// processes and query log, not only its own.
    pub admin: bool,
}

/// The front's own sources of `system` rows (HS1 Task 23).
#[derive(Debug, Clone, Copy)]
pub struct Front<'a> {
    /// Its running statements, both surfaces.
    pub processes: &'a crate::watchdog::Processes,
    /// Its query log, when it has a catalog.
    pub query_log: Option<&'a crate::querylog::QueryLog>,
}

/// Whether a setting is on (`1`, `true`).
fn setting_on(settings: &[(String, String)], name: &str) -> bool {
    settings
        .iter()
        .rev()
        .find(|(n, _)| n == name)
        .is_some_and(|(_, v)| matches!(v.trim(), "1" | "true" | "True" | "TRUE"))
}

/// Pins what an analysed statement reads and writes the views its worker needs
/// (§49 §7). `settings` are the statement's (session and request).
pub async fn plan(
    ctx: ReadContext<'_>,
    lease: &mut WorkerLease,
    analysis: &Analysis,
    sql: &str,
    settings: &[(String, String)],
) -> Result<ReadPlan, HouseError> {
    let refs = references(analysis);
    let (sql, system_tables) = crate::system::rewrite(sql);
    let mut plan = ReadPlan {
        sql,
        ..ReadPlan::default()
    };
    // HE1 R9.15: a lake table is read whole; `SAMPLE` would fail on its view.
    let sampled = crate::sample::rewrite(&plan.sql);
    let mut databases: BTreeSet<String> = BTreeSet::new();
    if ctx.database != "default" && !engine_database(ctx.database) {
        databases.insert(ctx.database.to_string());
    }
    let mut tables = Vec::new();
    let mut batch_views: Vec<String> = Vec::new();
    // Views over the lake views, outermost first (made last, innermost first).
    let mut plan_views: Vec<String> = Vec::new();
    let sequenced = setting_on(settings, SEQUENCED);
    if let Some(lake) = ctx.lake {
        let as_of = as_of(settings)?;
        let final_all = refs.final_setting || setting_on(settings, "final");
        let mut wanted: Vec<(String, String, LakeTable, bool)> = Vec::new();
        let mut add = |database: String, name: String, table: LakeTable, fin: bool| match wanted
            .iter_mut()
            .find(|(d, n, _, _)| *d == database && *n == name)
        {
            Some(found) => found.3 |= fin,
            None => wanted.push((database, name, table, fin)),
        };
        // HE1 R9.13: a view reads what its SELECT names, which is resolved in
        // the view's database, to a depth of [`MAX_VIEW_DEPTH`].
        let mut queue: Vec<(String, String, usize)> = refs
            .tables
            .iter()
            .map(|t| (ctx.database.to_string(), t.clone(), 0))
            .collect();
        let mut expanded: Vec<(String, String)> = Vec::new();
        let mut finals = refs.finals.clone();
        while let Some((current, printed, depth)) = queue.pop() {
            let printed = &printed;
            if let Some((database, name, table)) =
                resolve(lake, ctx.namespace, &current, printed).await?
            {
                // A table outside the user's grants is not read (HS1 Task 20),
                // nor is the House's own `_house` by name (HS1 Task 23: the
                // query log is read as `system.query_log`).
                if database == crate::querylog::HOUSE_DATABASE
                    || ctx.grants.is_some_and(|g| !g.allows(&database, &name))
                {
                    return Err(HouseError::from(ChError::access_denied(format!(
                        "{}: Not enough privileges on {database}.{name}: it is not among \
                         this identity's tables.",
                        ctx.user
                    ))));
                }
                // A materialized view's source reads its block: a pipe's
                // records (HS1 Task 16) or the files one commit added (Task
                // 17). A pipe is read no other way.
                if let Some(batch) = ctx.batch
                    && batch.database == database
                    && batch.name == name
                {
                    if !batch_views.contains(&batch.view) {
                        batch_views.push(batch.view.clone());
                    }
                    databases.insert(database);
                    continue;
                }
                if crate::ddl::class_of(&table.metadata) == TableClass::Pipe {
                    return Err(crate::pipes::direct_read(&database, &name));
                }
                if crate::ddl::class_of(&table.metadata) == TableClass::View {
                    if expanded.iter().any(|(d, n)| *d == database && *n == name) {
                        continue;
                    }
                    if depth >= MAX_VIEW_DEPTH {
                        return Err(bad(format!(
                            "View {database}.{name} nests views more than {MAX_VIEW_DEPTH} deep"
                        )));
                    }
                    let select = table
                        .metadata
                        .properties()
                        .get(crate::ddl::PROP_VIEW_SELECT)
                        .cloned()
                        .unwrap_or_default();
                    // The view's text is the catalog's: held to the deny list as a
                    // user's read is, before any worker makes it.
                    let analysis = lease
                        .analyze(&select, &[])
                        .await
                        .map_err(crate::deny::as_setting_refusal)?;
                    crate::gate::admit(
                        &analysis,
                        &select,
                        crate::classify::Expect::Read,
                        true,
                        None,
                    )?;
                    let inner = references(&analysis);
                    finals.extend(inner.finals.iter().cloned());
                    // `FROM d FINAL` of a Distributed table reads its local
                    // table with FINAL, as ClickHouse's shards do.
                    if finals.iter().any(|f| f == printed)
                        && crate::ddl::distributed_target(&table).is_some()
                    {
                        finals.extend(inner.tables.iter().cloned());
                    }
                    for t in inner.tables {
                        queue.push((database.clone(), t, depth + 1));
                    }
                    databases.insert(database.clone());
                    plan_views.push(views::user_view_ddl(&database, &name, &select));
                    expanded.push((database, name));
                    continue;
                }
                let fin = final_all || finals.iter().any(|f| f == printed);
                add(database, name, table, fin);
            }
        }
        // `merge()` reads the tables of a database by a pattern, and
        // `information_schema` lists every table: they get every view.
        let mut whole: BTreeSet<String> = BTreeSet::new();
        for database in &refs.merges {
            whole.insert(database.clone().unwrap_or_else(|| ctx.database.to_string()));
        }
        if refs.information_schema {
            for database in lake.databases(ctx.namespace).await? {
                whole.insert(database.name);
            }
        }
        for database in whole {
            if engine_database(&database)
                || database == crate::querylog::HOUSE_DATABASE
                || !lake.database_exists(ctx.namespace, &database).await?
            {
                continue;
            }
            for name in lake.table_names(ctx.namespace, &database).await? {
                if ctx.grants.is_some_and(|g| !g.allows(&database, &name)) {
                    continue;
                }
                if let Some(table) = lake.load(ctx.namespace, &database, &name).await?
                    && crate::ddl::class_of(&table.metadata) != TableClass::Pipe
                {
                    add(database.clone(), name, table, final_all);
                }
            }
        }
        for (database, name, table, fin) in wanted {
            let pin = pin(&database, &name, ctx.database, &table, &as_of)?;
            let files = match (fin, pin.snapshot_id, LakeEngine::parse(&pin.table.engine)) {
                (_, Some(id), _) if sequenced && pin.table.class != TableClass::External => {
                    live_files(lake, &table.metadata, id).await?
                }
                (
                    true,
                    Some(id),
                    Ok(LakeEngine::Replacing { .. } | LakeEngine::VersionedCollapsing { .. }),
                ) if pin.table.class != TableClass::External => {
                    live_files(lake, &table.metadata, id).await?
                }
                _ => Vec::new(),
            };
            databases.insert(database);
            tables.push((pin, fin && !sequenced, files));
        }
    }
    let endpoint = lease.ready().and_then(|ready| ready.s3_endpoint.clone());
    for database in &databases {
        if database != "default" {
            plan.views.push(views::database_ddl(database));
        }
    }
    plan.views.extend(batch_views);
    // BQ1-house: a statement that reads one lake table, directly, may push its
    // key predicates on `UInt64` columns into the view (`crate::pushdown`).
    let pushable = tables.len() == 1 && plan_views.is_empty() && ctx.batch.is_none();
    for (pin, fin, files) in tables {
        let Some(endpoint) = endpoint.as_deref() else {
            return Err(HouseError::from(ChError::not_implemented(format!(
                "Table {}.{} is a lake table, and this House's workers read no bucket \
                 (in-process workers): run process workers (--workers=process) with --store",
                pin.table.database, pin.table.name
            ))));
        };
        let root = ctx.lake.map(Lake::root).ok_or_else(|| bad("no catalog"))?;
        let source = views::source(&pin, endpoint, root)?;
        let mode = if fin {
            Mode::Final(&files)
        } else if sequenced {
            Mode::Sequenced(&files)
        } else {
            Mode::Plain
        };
        if views::carries_expressions(&pin, mode) {
            // Expression text from the catalog is printed again from its tree
            // (`crate::expr`, R11.11), and the deny list still reads the view
            // before the control connection runs it, as it reads a user's
            // statement.
            let checked = views::view_ddl(&pin, views::CHECKED_SOURCE, mode)?;
            let body = views::view_body(&checked);
            let analysis = lease
                .analyze(body, &[])
                .await
                .map_err(crate::deny::as_setting_refusal)?;
            crate::gate::admit(&analysis, body, crate::classify::Expect::Read, true, None)
                .map_err(|err| views::catalog_refusal(&pin, &err.to_string()))?;
        }
        let filters = if pushable && mode == Mode::Plain && pin.table.class != TableClass::External
        {
            crate::pushdown::key_filters(
                &plan.sql,
                &pin.table.database,
                &pin.table.name,
                ctx.database,
                &views::uint64_columns(&pin),
            )
        } else {
            Vec::new()
        };
        plan.views
            .push(views::view_ddl_filtered(&pin, &source, mode, &filters)?);
        plan.pins.push(pin);
    }
    plan.views.extend(plan_views.into_iter().rev());
    if let Some(sql) = sampled
        && !plan.pins.is_empty()
    {
        plan.sql = sql;
    }
    // HE1 R9.70: nor would `PREWHERE`.
    if !plan.pins.is_empty()
        && let Some(sql) = crate::sample::prewhere_as_where(&plan.sql)
    {
        plan.sql = sql;
    }
    // `system.query_log` reads the namespace's `_house.query_log` (HS1 Task
    // 23) through a view of its own; no client statement names it.
    if system_tables.contains("query_log")
        && let Some(log) = ctx.front.and_then(|f| f.query_log)
        && let Some(ddl) = log.read_views(ctx.namespace, endpoint.as_deref()).await?
    {
        plan.views.extend(ddl);
    }
    if !system_tables.is_empty() {
        plan.views
            .extend(crate::system::views(ctx, lease, &system_tables).await?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use loams_house_ipc::{Classification, QueryClass};

    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn time_travel_settings() {
        assert_eq!(as_of(&[]).expect("none"), AsOf::Current);
        assert_eq!(
            as_of(&pairs(&[(SNAPSHOT_ID, "42")])).expect("id"),
            AsOf::Snapshot(42)
        );
        assert_eq!(
            as_of(&pairs(&[(SNAPSHOT_ID, "t=1, db.u=2")])).expect("pairs"),
            AsOf::Snapshots(vec![
                (None, "t".to_string(), 1),
                (Some("db".to_string()), "u".to_string(), 2)
            ])
        );
        assert_eq!(
            as_of(&pairs(&[(AS_OF, "2026-10-12 10:20:30.5")])).expect("time"),
            AsOf::Timestamp(1_791_800_430_500)
        );
        assert_eq!(
            as_of(&pairs(&[(AS_OF, "'2026-10-12T10:20:30Z'")])).expect("time"),
            AsOf::Timestamp(1_791_800_430_000)
        );
        assert_eq!(
            as_of(&pairs(&[(AS_OF, "1700000000000")])).expect("ms"),
            AsOf::Timestamp(1_700_000_000_000)
        );
        assert!(as_of(&pairs(&[(AS_OF, "yesterday")])).is_err());
        assert!(as_of(&pairs(&[(SNAPSHOT_ID, "x=y")])).is_err());
        assert!(as_of(&pairs(&[(SNAPSHOT_ID, "1"), (AS_OF, "1")])).is_err());
    }

    fn analysis(ast: &str, query_tree: Option<&str>) -> Analysis {
        Analysis {
            classification: Classification {
                class: QueryClass::ReadOnly,
                statements: 1,
            },
            ast: ast.to_string(),
            query_tree: query_tree.map(str::to_string),
        }
    }

    #[test]
    fn references_from_the_explains() {
        // Measured with clickhouse-local 26.7: `SELECT * FROM t AS x FINAL JOIN
        // db.u USING a WHERE a IN v2 AND b IN (SELECT a FROM v)` and `merge`.
        let ast = "SelectWithUnionQuery (children 1)
 ExpressionList (children 1)
  SelectQuery (children 3)
   ExpressionList (children 1)
    Asterisk
   TablesInSelectQuery (children 2)
    TablesInSelectQueryElement (children 1)
     TableExpression (children 1)
      TableIdentifier t (alias x)
    TablesInSelectQueryElement (children 2)
     TableExpression (children 1)
      TableIdentifier my db.u
     TableJoin (children 1)
      ExpressionList (children 1)
       Identifier a
   Function and (children 1)
    ExpressionList (children 2)
     Function in (children 1)
      ExpressionList (children 2)
       Identifier a
       Identifier v2
     Function in (children 1)
      ExpressionList (children 2)
       Identifier b
       Subquery (children 1)
        SelectWithUnionQuery (children 1)
         ExpressionList (children 1)
          SelectQuery (children 2)
           ExpressionList (children 1)
            Identifier a
           TablesInSelectQuery (children 1)
            TablesInSelectQueryElement (children 1)
             TableExpression (children 1)
              Function merge (children 1)
               ExpressionList (children 2)
                Literal \\'sales\\'
                Literal \\'^t\\'";
        let tree = "QUERY id: 0
  JOIN TREE
    JOIN id: 3, strictness: ALL, kind: INNER
      LEFT TABLE EXPRESSION
        IDENTIFIER id: 4, alias: x, identifier: t, final: 1
      RIGHT TABLE EXPRESSION
        IDENTIFIER id: 5, identifier: my db.u
  SETTINGS max_threads=2 final=1";
        let refs = references(&analysis(ast, Some(tree)));
        assert_eq!(refs.tables, vec!["t", "my db.u", "v2"]);
        assert_eq!(refs.finals, vec!["t"]);
        assert!(refs.final_setting);
        assert_eq!(refs.merges, vec![Some("sales".to_string())]);
        assert!(!refs.information_schema);
    }
}
