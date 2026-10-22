//! Compaction of lake tables (HS1 Task 13, §49 §6.2).
//!
//! A compaction reads a table's rows at one pinned snapshot, merged the way
//! `FINAL` merges them (the merge views of HS1 R11.4: Replacing keeps the
//! newest row per key and drops `is_deleted` rows, Summing sums), writes them
//! as new files sorted by the table's `ORDER BY` (the write path of HS1 Task
//! 12, [`crate::insert`]), and commits a `replace` snapshot that removes every
//! file it read ([`loams_house_lake::rewrite`]). The commit requires `main` to
//! still be the snapshot read, so a concurrent commit wins and the compaction
//! is abandoned: its files are orphans for HS1 Task 15, and no row of the
//! other commit is lost. After it, a read without `FINAL` sees merged rows, as
//! ClickHouse's does after a merge; a read with `FINAL` answers the same as
//! before.
//!
//! * `OPTIMIZE TABLE t [PARTITION p] [FINAL] [DEDUPLICATE]` ([`optimize`])
//!   compacts now and answers after the commit, trying again up to
//!   [`OPTIMIZE_ATTEMPTS`] times when it loses to another commit.
//!   `DEDUPLICATE` also drops whole-row duplicates. The House compacts the
//!   whole table whatever `PARTITION` names (ruling R13.2).
//! * The [`Scheduler`] looks at the tables that took commits since its last
//!   round and compacts one when a partition holds at least
//!   [`SMALL_FILES`] files under [`SMALL_FILE_BYTES`], or, for Replacing and
//!   Summing tables, when at least [`UNMERGED_RATIO`] of its rows would merge
//!   away.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_house_ipc::{Execute, Limits, Output};

use crate::admission::WorkerPool;
use crate::catalog::{CatalogError, Lake};
use crate::config::UserMap;
use crate::ddl::{self, ChTable, LakeEngine, quote_name};
use crate::errors::{ChError, HouseError};
use crate::insert::{InsertContext, Inserted, Source, Target};
use crate::{deny, gate};

/// Files under this size are small.
pub const SMALL_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// A partition with this many small files is compacted.
pub const SMALL_FILES: usize = 16;
/// A Replacing or Summing table whose `FINAL` would drop this share of its
/// rows is compacted.
pub const UNMERGED_RATIO: f64 = 0.2;
/// How many times `OPTIMIZE` tries before it gives up to concurrent commits.
pub const OPTIMIZE_ATTEMPTS: u32 = 3;

/// What one compaction did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compacted {
    /// Nothing to compact (no snapshot, or one file with nothing to merge).
    Nothing,
    /// The `replace` snapshot.
    Committed(i64),
    /// Another commit came first; the work is abandoned.
    Abandoned,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Compacts `database.name` in `ctx`'s namespace (see the module docs), and
/// counts it in the front's metrics (HS1 Task 23).
pub async fn compact(
    ctx: &InsertContext<'_>,
    database: &str,
    name: &str,
    deduplicate: bool,
) -> Result<Compacted, HouseError> {
    let started = std::time::Instant::now();
    let mut rewritten = (0u64, 0u64);
    let mut span = crate::trace::child("house.compaction");
    let outcome = compact_counted(ctx, database, name, deduplicate, &mut rewritten).await;
    let result = match &outcome {
        Ok(Compacted::Committed(_)) => "committed",
        Ok(Compacted::Abandoned) => "abandoned",
        Ok(Compacted::Nothing) => "nothing",
        Err(_) => "failed",
    };
    let (files, bytes) = match &outcome {
        Ok(Compacted::Committed(_)) => rewritten,
        _ => (0, 0),
    };
    ctx.lake
        .metrics()
        .compaction(result, started.elapsed(), files, bytes);
    if let Some(span) = span.as_mut() {
        span.set("loams.table", format!("{database}.{name}"));
        span.set("loams.result", result);
        span.set("loams.files", files);
        if outcome.is_err() {
            span.fail();
        }
    }
    outcome
}

async fn compact_counted(
    ctx: &InsertContext<'_>,
    database: &str,
    name: &str,
    deduplicate: bool,
    rewritten: &mut (u64, u64),
) -> Result<Compacted, HouseError> {
    let lake = ctx.lake;
    let namespace = ctx.user.namespace;
    let table = lake
        .lake_table(namespace, database, name, "OPTIMIZE TABLE")
        .await?;
    let Some(snapshot) = table.metadata.current_snapshot() else {
        return Ok(Compacted::Nothing);
    };
    let snapshot_id = snapshot.snapshot_id();
    let files: Vec<_> = crate::read::live_files(lake, &table.metadata, snapshot_id)
        .await?
        .into_iter()
        .filter(|f| f.data)
        .collect();
    let ch = ChTable::from_lake(database, name, &table);
    let engine = LakeEngine::parse(&ch.engine).unwrap_or(LakeEngine::MergeTree);
    let merges = !matches!(engine, LakeEngine::MergeTree) || deduplicate;
    if files.is_empty() || (files.len() == 1 && !merges) {
        return Ok(Compacted::Nothing);
    }

    // The rows, merged as FINAL merges them, at the pinned snapshot.
    let columns: Vec<String> = ch
        .columns
        .iter()
        .filter(|c| c.stored() && c.default_kind != "MATERIALIZED")
        .map(|c| c.name.clone())
        .collect();
    let list = columns
        .iter()
        .map(|c| quote_name(c))
        .collect::<Vec<_>>()
        .join(", ");
    let select = format!(
        "SELECT {}{list} FROM {}.{} FINAL",
        if deduplicate { "DISTINCT " } else { "" },
        quote_name(database),
        quote_name(name)
    );
    let target = Target {
        database: database.to_string(),
        name: name.to_string(),
        statement: format!(
            "INSERT INTO {}.{} ({list}) {select}",
            quote_name(database),
            quote_name(name)
        ),
        table: table.clone(),
        ch,
        columns: Some(columns),
        settings: Vec::new(),
        source: Source::Select { text: select },
    };
    let pinned = [("loams_snapshot_id".to_string(), snapshot_id.to_string())];
    let pinned_ctx = InsertContext {
        settings: &pinned,
        ..*ctx
    };
    let mut lease = ctx.pool.acquire(&namespace.to_string()).await?;
    lease.account_as(compaction_of(database, name));
    let written = crate::insert::write_files(&pinned_ctx, lease, &target).await?;

    // The `replace`, on the snapshot that was read.
    let committer = lake.committer();
    let io = committer.io();
    let list = io.read(snapshot.manifest_list()).await?;
    let unreadable = |err: loams_house_lake::LakeError| {
        HouseError::from(ChError::network_error(format!(
            "the manifests of {database}.{name}: {err}"
        )))
    };
    let mut manifests = Vec::new();
    for manifest in
        loams_house_lake::rewrite::parent_manifests(&list, table.metadata.format_version())
            .map_err(unreadable)?
    {
        let bytes = io.read(&manifest.manifest_path).await?;
        manifests.push((manifest, bytes));
    }
    let removed: HashSet<String> = files.iter().map(|f| f.location.clone()).collect();
    *rewritten = (files.len() as u64, files.iter().map(|f| f.size_bytes).sum());
    let built = loams_house_lake::rewrite::replace(
        &table.metadata,
        manifests,
        &removed,
        written.data_files,
        vec![("loams.compaction".to_string(), "1".to_string())],
        now_ms(),
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
        Ok(_) => Ok(Compacted::Committed(built.snapshot_id)),
        Err(CatalogError::Conflict { .. }) => Ok(Compacted::Abandoned),
        Err(err) => Err(err.into()),
    }
}

/// A compaction's leases are `house.compaction`, not `house.query` (HS1 Task
/// 22), whether the scheduler or an `OPTIMIZE` asked for it.
fn compaction_of(database: &str, name: &str) -> crate::usage::Purpose {
    crate::usage::Purpose::Compaction {
        database: database.to_string(),
        table: name.to_string(),
    }
}

/// `OPTIMIZE TABLE` on a lake table (HS1 Task 13), parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizePlan {
    /// The table's database.
    pub database: String,
    /// The table.
    pub name: String,
    /// `DEDUPLICATE`.
    pub deduplicate: bool,
}

impl OptimizePlan {
    /// Reads an `OPTIMIZE`; `current` is the request's database.
    pub fn parse(text: &str, current: &str) -> Result<Self, HouseError> {
        let stmt = ddl::parse_optimize(text)?;
        let (database, name) = stmt.table;
        Ok(Self {
            database: database.unwrap_or_else(|| current.to_string()),
            name,
            deduplicate: stmt.deduplicate,
        })
    }
}

/// Runs an `OPTIMIZE` to its commit; answers like an `INSERT` with no rows and
/// the `replace` snapshot.
pub async fn optimize(
    ctx: &InsertContext<'_>,
    plan: &OptimizePlan,
) -> Result<Inserted, HouseError> {
    let started = std::time::Instant::now();
    for _ in 0..OPTIMIZE_ATTEMPTS {
        let snapshot_id = match compact(ctx, &plan.database, &plan.name, plan.deduplicate).await? {
            Compacted::Abandoned => continue,
            Compacted::Committed(id) => Some(id),
            Compacted::Nothing => None,
        };
        return Ok(Inserted {
            table: format!("{}.{}", plan.database, plan.name),
            written_rows: 0,
            written_bytes: 0,
            snapshot_id,
            elapsed_ns: started.elapsed().as_nanos() as u64,
        });
    }
    Err(HouseError::from(ChError::network_error(format!(
        "OPTIMIZE TABLE {}.{}: other commits came first {OPTIMIZE_ATTEMPTS} times; try again",
        plan.database, plan.name
    ))))
}

/// Whether a table should be compacted now: small files per partition, or
/// rows a merge would drop.
pub async fn needs_compaction(
    ctx: &InsertContext<'_>,
    database: &str,
    name: &str,
) -> Result<bool, HouseError> {
    let Some(table) = ctx.lake.load(ctx.user.namespace, database, name).await? else {
        return Ok(false);
    };
    if ddl::class_of(&table.metadata) != ddl::TableClass::Lake {
        return Ok(false);
    }
    let Some(snapshot) = table.metadata.current_snapshot() else {
        return Ok(false);
    };
    let files: Vec<_> = crate::read::live_files(ctx.lake, &table.metadata, snapshot.snapshot_id())
        .await?
        .into_iter()
        .filter(|f| f.data)
        .collect();
    let mut small: BTreeMap<&str, usize> = BTreeMap::new();
    for file in files.iter().filter(|f| f.size_bytes < SMALL_FILE_BYTES) {
        *small.entry(file.partition.as_str()).or_default() += 1;
    }
    if small.values().any(|n| *n >= SMALL_FILES) {
        return Ok(true);
    }
    let ch = ChTable::from_lake(database, name, &table);
    let merging = matches!(
        LakeEngine::parse(&ch.engine),
        Ok(LakeEngine::Replacing { .. } | LakeEngine::Summing { .. })
    );
    if !merging || files.len() < 2 {
        return Ok(false);
    }
    let total: u64 = files.iter().map(|f| f.record_count).sum();
    if total == 0 {
        return Ok(false);
    }
    let merged = count_final(ctx, database, name, snapshot.snapshot_id()).await?;
    Ok((total.saturating_sub(merged)) as f64 / total as f64 >= UNMERGED_RATIO)
}

/// `SELECT count() FROM t FINAL` at `snapshot_id`, through the read path.
async fn count_final(
    ctx: &InsertContext<'_>,
    database: &str,
    name: &str,
    snapshot_id: i64,
) -> Result<u64, HouseError> {
    let sql = format!(
        "SELECT count() FROM {}.{} FINAL",
        quote_name(database),
        quote_name(name)
    );
    let mut lease = ctx.pool.acquire(&ctx.user.namespace.to_string()).await?;
    lease.account_as(compaction_of(database, name));
    let result = async {
        let analysis = lease
            .analyze(&sql, &[])
            .await
            .map_err(deny::as_setting_refusal)?;
        gate::admit(&analysis, &sql, crate::classify::Expect::Read, true, None)?;
        let pinned = [("loams_snapshot_id".to_string(), snapshot_id.to_string())];
        let plan = gate::read_plan(
            Some(ctx.lake),
            ctx.user.namespace,
            database,
            ctx.user,
            &mut lease,
            &analysis,
            &sql,
            &pinned,
            None,
            None,
        )
        .await?;
        let out = lease
            .run(Execute {
                query_id: format!("loams-compaction-{}", uuid::Uuid::new_v4()),
                session: None,
                settings: Vec::new(),
                views: plan.views,
                sql: plan.sql,
                format: "TSV".to_string(),
                params: Vec::new(),
                limits: Limits::default(),
                input: None,
                output: Output::Blocks,
                database: Some(database.to_string()),
            })
            .await?;
        String::from_utf8_lossy(&out.bytes)
            .trim()
            .parse::<u64>()
            .map_err(|_| {
                HouseError::from(ChError::bad_arguments("count() did not answer a number"))
            })
    }
    .await;
    ctx.pool
        .release(lease, crate::admission::Outcome::Completed);
    result
}

/// The background compaction of tables that took commits (see the module
/// docs). It runs as the House itself: every scope, every table of the
/// table's namespace; it serves no client, so nothing is audited.
#[derive(Debug)]
pub struct Scheduler {
    lake: Arc<Lake>,
    pool: WorkerPool,
    config: crate::config::HouseConfig,
}

impl Scheduler {
    /// A scheduler over `lake`'s tables.
    pub fn new(lake: Arc<Lake>, pool: WorkerPool, config: crate::config::HouseConfig) -> Self {
        Self { lake, pool, config }
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

    /// One round: each table that took commits since the last.
    pub async fn round(&self) {
        let known = self.pool.known_settings();
        for ident in self.lake.committer().take_dirty() {
            let Some((namespace, database, name)) = Lake::names_of(&ident) else {
                continue;
            };
            let system = UserMap::system("loams-compaction", namespace);
            let ctx = InsertContext {
                lake: &self.lake,
                pool: &self.pool,
                namespace,
                database: &database,
                user: &system,
                query_id: "loams-compaction",
                params: &[],
                settings: &[],
                limits: &self.config.session_limits,
                known: &known,
                max_insert_bytes: self.config.lake_insert_max_bytes,
                pipe_batch: None,
                admitted: None,
            };
            let outcome = match needs_compaction(&ctx, &database, &name).await {
                Ok(true) => compact(&ctx, &database, &name, false).await,
                Ok(false) => continue,
                Err(err) => Err(err),
            };
            match outcome {
                Ok(Compacted::Abandoned) => self.lake.committer().mark_dirty(&ident),
                Ok(_) => {}
                Err(err) => eprintln!(
                    "loams-house: compaction of {database}.{name} (namespace {namespace}) \
                     failed: {}",
                    err.message()
                ),
            }
        }
    }
}
