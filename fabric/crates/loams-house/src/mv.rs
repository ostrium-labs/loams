//! Materialized views over lake tables (HS1 Task 17, design §49 §9).
//!
//! ```sql
//! CREATE MATERIALIZED VIEW mv TO t AS SELECT k, sum(v) AS v FROM src GROUP BY k;
//! ```
//!
//! A view is recorded on its source (`loams.mv.<id>`, as a pipe's are, HS1
//! Task 16). It runs once per **committed snapshot** of the source, in order:
//! the files that snapshot added are the view's input block (a view over the
//! source's name reads exactly them, [`batch`]), the view's `SELECT` runs over
//! them in a worker, and the rows are committed into the target **with the
//! view's cursor** — the target's property `loams.mv_cursor.<id>`, the last
//! source snapshot applied, also in the snapshot's summary as
//! `loams.mv.<id>.src_snapshot` — in one commit that requires the cursor not to
//! have moved ([`crate::pipes::commit_cursor`]). So a retry, a second front or
//! a front that died half-way never applies a snapshot twice, and never skips
//! one.
//!
//! * **When.** An `INSERT` into the source runs its views after its commit and
//!   before it answers ([`ViewRunner::after_commit`]), and so do the views of
//!   their targets in turn (chains, at most [`MAX_CHAIN`] deep); the pipes'
//!   runner does the same after each batch. A background round
//!   ([`Scheduler`]) catches up what a front that died left, and inserts that
//!   did not wait (`async_insert = 1`, `wait_for_async_insert = 0`).
//! * **Errors.** As ClickHouse's default `materialized_views_ignore_errors =
//!   0` (measured with `clickhouse local` 26.7): the `INSERT` answers the view's
//!   error, its own rows committed (ClickHouse's are written too). The view
//!   then skips that block, as ClickHouse's never sees it again, unless the
//!   error is the House's own trouble (the store, the catalog, a worker that
//!   died, a limit): then the block is tried again. With
//!   `materialized_views_ignore_errors = 1` the `INSERT` answers success.
//! * **Snapshots that are not appends** (a compaction's `replace`, a
//!   `TRUNCATE`) hold no new rows: the cursor moves past them.
//! * **A new view** starts at the source's current snapshot: no backfill, as
//!   ClickHouse without `POPULATE` (which is `48`). A cursor that is no longer
//!   in the source's history (a `RestoreTable`, an expiry) moves to the current
//!   snapshot and says so in the view's last error.
//! * A block is one source commit: inserts that group commit coalesced (HS1
//!   Task 12) reach a view as one block (ruling R17.2).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use loams_house_lake::TableIdent;
use loams_house_lake::spec::{Operation, TableMetadata};

use crate::catalog::{Lake, LakeRoot, LakeTable};
use crate::ddl::{self, TableClass, quote_string};
use crate::errors::{ChError, HouseError};
use crate::pipes::{MaterializedView, PipeBatch, Ran, ViewWriter, views_of};
use crate::read::{AsOf, pin};
use crate::views::{self, Mode};

/// A target's property: the last source snapshot a view applied.
pub const CURSOR_PREFIX: &str = "loams.mv_cursor.";
/// How deep views of views run after one commit.
pub const MAX_CHAIN: usize = 8;
/// How many source snapshots one run applies at most.
pub const MAX_SNAPSHOTS_PER_RUN: usize = 64;
/// Every this many background rounds, every table with views is looked at,
/// not only those that took commits.
pub const FULL_SCAN_EVERY: u64 = 60;

/// The cursor key of a view.
pub fn cursor_key(view: &MaterializedView) -> String {
    format!("{CURSOR_PREFIX}{}", view.id)
}

/// The summary key recording the source snapshot a commit applied.
pub fn summary_key(view: &MaterializedView) -> String {
    format!("loams.mv.{}.src_snapshot", view.id)
}

/// Records a new view's cursor in its target: the source's current snapshot
/// (`0` when it has none, so its first snapshot is applied).
pub async fn start_cursor(
    lake: &Lake,
    target: &LakeTable,
    view: &MaterializedView,
    source: &LakeTable,
) -> Result<(), HouseError> {
    let at = source.metadata.current_snapshot_id().unwrap_or(0);
    lake.catalog()
        .commit(
            &target.ident,
            Vec::new(),
            vec![loams_house_lake::TableUpdate::SetProperties {
                updates: std::collections::HashMap::from([(cursor_key(view), at.to_string())]),
            }],
        )
        .await?;
    Ok(())
}

/// The source's snapshots after `cursor` on `main`, oldest first; `None` when
/// `cursor` is not in its history.
fn pending(metadata: &TableMetadata, cursor: i64) -> Option<Vec<i64>> {
    let mut out = Vec::new();
    let mut at = metadata.current_snapshot_id();
    while let Some(id) = at {
        if id == cursor {
            out.reverse();
            return Some(out);
        }
        out.push(id);
        at = metadata
            .snapshot_by_id(id)
            .and_then(|s| s.parent_snapshot_id());
    }
    if cursor == 0 {
        out.reverse();
        return Some(out);
    }
    None
}

/// Whether an error is the House's own trouble (tried again) rather than the
/// view's (its block is skipped).
pub fn transient(err: &HouseError) -> bool {
    matches!(err.code(), 159 | 202 | 210 | 241 | 394)
}

/// The view DDL that makes `database.name` read the files `snapshot_id`
/// added, as the table's columns, through the worker's `endpoint`.
pub fn batch(
    source: &LakeTable,
    database: &str,
    name: &str,
    snapshot_id: i64,
    files: &[String],
    endpoint: &str,
    root: &LakeRoot,
) -> Result<PipeBatch, HouseError> {
    let mut pinned = pin(
        database,
        name,
        database,
        source,
        &AsOf::Snapshot(snapshot_id),
    )?;
    // The view reads the files, not the table: no snapshot setting.
    pinned.travel = false;
    let location = source.metadata.location().trim_end_matches('/');
    let key = root
        .relative(location)
        .ok_or_else(|| bad("the source is outside the House's store"))?;
    let mut relative = Vec::with_capacity(files.len());
    for file in files {
        let rest = file
            .strip_prefix(location)
            .and_then(|r| r.strip_prefix('/'))
            .ok_or_else(|| bad(format!("{file} is outside its table's location")))?;
        if rest.contains([',', '{', '}', '*', '?', '\'']) {
            return Err(bad(format!("{file} cannot be named in a read")));
        }
        relative.push(rest.to_string());
    }
    let glob = match relative.as_slice() {
        [one] => one.clone(),
        many => format!("{{{}}}", many.join(",")),
    };
    let url = format!(
        "{}/{}/{}/{glob}",
        endpoint.trim_end_matches('/'),
        root.bucket(),
        key.trim_matches('/')
    );
    let source_fn = format!("s3({}, 'dummy', 'dummy', 'Parquet')", quote_string(&url));
    Ok(PipeBatch {
        database: database.to_string(),
        name: name.to_string(),
        view: views::view_ddl(&pinned, &source_fn, Mode::Batch)?,
    })
}

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

/// Runs views over lake tables.
#[derive(Debug, Clone)]
pub struct ViewRunner {
    writer: ViewWriter,
}

impl ViewRunner {
    /// A runner writing with `writer`.
    pub fn new(writer: ViewWriter) -> Self {
        Self { writer }
    }

    fn lake(&self) -> &Arc<Lake> {
        &self.writer.lake
    }

    /// Runs every view of `database.name` until it has applied the table's
    /// current snapshot, then the views of their targets, at most
    /// [`MAX_CHAIN`] deep. Answers the first error a view gave; the others
    /// still run.
    pub async fn after_commit(
        &self,
        namespace: u64,
        database: &str,
        name: &str,
    ) -> Result<(), HouseError> {
        let mut first: Option<HouseError> = None;
        let mut level = vec![(database.to_string(), name.to_string())];
        for _ in 0..MAX_CHAIN {
            let mut next = Vec::new();
            for (db, table) in level {
                let Some(source) = self.lake().load(namespace, &db, &table).await? else {
                    continue;
                };
                if ddl::class_of(&source.metadata) != TableClass::Lake {
                    continue;
                }
                for view in views_of(&source) {
                    match self.run(namespace, &db, &table, &view).await {
                        Ok(applied) => {
                            if applied > 0 {
                                next.push((view.target_database.clone(), view.target.clone()));
                            }
                        }
                        Err(err) => {
                            if first.is_none() {
                                first = Some(err);
                            }
                            // Rows a view did commit before failing still feed
                            // its target's views.
                            next.push((view.target_database.clone(), view.target.clone()));
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort();
            next.dedup();
            level = next;
        }
        first.map_or(Ok(()), Err)
    }

    /// Applies `view`'s pending source snapshots, in order; answers how many
    /// committed rows-bearing or cursor-only commits it made.
    pub async fn run(
        &self,
        namespace: u64,
        database: &str,
        name: &str,
        view: &MaterializedView,
    ) -> Result<usize, HouseError> {
        let result = self.run_inner(namespace, database, name, view).await;
        let states = self.lake().pipe_states();
        states.update(namespace, view, |state| match &result {
            Ok(n) if *n > 0 => {
                state.last_error.clear();
                state.last_commit_ms = Some(crate::retention::now_ms());
            }
            Ok(_) => {}
            Err(err) => state.last_error = err.message().to_string(),
        });
        result
    }

    async fn run_inner(
        &self,
        namespace: u64,
        database: &str,
        name: &str,
        view: &MaterializedView,
    ) -> Result<usize, HouseError> {
        let lake = Arc::clone(self.lake());
        let committer = lake.committer();
        let io = committer.io();
        let key = cursor_key(view);
        let mut applied = 0;
        for _ in 0..MAX_SNAPSHOTS_PER_RUN {
            let Some(source) = lake.load(namespace, database, name).await? else {
                return Ok(applied);
            };
            let target = lake
                .lake_table(
                    namespace,
                    &view.target_database,
                    &view.target,
                    "a materialized view's target",
                )
                .await?;
            let read: BTreeMap<String, String> = target
                .metadata
                .properties()
                .get(&key)
                .map(|v| BTreeMap::from([(key.clone(), v.clone())]))
                .unwrap_or_default();
            let current = source.metadata.current_snapshot_id().unwrap_or(0);
            let Some(cursor) = read.get(&key).and_then(|v| v.parse::<i64>().ok()) else {
                // No cursor (a view recorded by a front that died before its
                // cursor, which cannot happen in this order, or a table
                // registered from elsewhere): start now.
                crate::pipes::commit_cursor(
                    io,
                    &target.ident,
                    &read,
                    &BTreeMap::from([(key.clone(), current.to_string())]),
                    Vec::new(),
                    Vec::new(),
                )
                .await?;
                return Ok(applied);
            };
            let Some(pending) = pending(&source.metadata, cursor) else {
                crate::pipes::commit_cursor(
                    io,
                    &target.ident,
                    &read,
                    &BTreeMap::from([(key.clone(), current.to_string())]),
                    Vec::new(),
                    Vec::new(),
                )
                .await?;
                return Err(bad(format!(
                    "Materialized view {}.{}: snapshot {cursor} of {database}.{name} is no longer \
                     in its history (a restore or an expiry); the view goes on from snapshot \
                     {current}",
                    view.database, view.name
                )));
            };
            let Some(&snapshot_id) = pending.first() else {
                return Ok(applied);
            };
            let next = BTreeMap::from([(key.clone(), snapshot_id.to_string())]);
            let files = self.added(&source, snapshot_id).await?;
            let mut written_counts = (0, 0);
            let outcome = if files.is_empty() {
                crate::pipes::commit_cursor(io, &target.ident, &read, &next, Vec::new(), Vec::new())
                    .await?
            } else {
                let root = lake.root().clone();
                let make = |endpoint: &str| {
                    batch(
                        &source,
                        database,
                        name,
                        snapshot_id,
                        &files,
                        endpoint,
                        &root,
                    )
                };
                match self
                    .writer
                    .write(namespace, view, &target, Some(&make))
                    .await
                {
                    Ok(written) => {
                        written_counts = (written.rows, written.input_bytes);
                        crate::pipes::commit_cursor(
                            io,
                            &target.ident,
                            &read,
                            &next,
                            vec![(summary_key(view), snapshot_id.to_string())],
                            written.data_files,
                        )
                        .await?
                    }
                    Err(err) if transient(&err) => return Err(err),
                    Err(err) => {
                        // The view's own error: its block is skipped, as
                        // ClickHouse's view never sees it again.
                        crate::pipes::commit_cursor(
                            io,
                            &target.ident,
                            &read,
                            &next,
                            Vec::new(),
                            Vec::new(),
                        )
                        .await?;
                        return Err(err);
                    }
                }
            };
            match outcome {
                Ran::Committed { .. } => {
                    applied += 1;
                    self.writer
                        .report_insert(namespace, view, written_counts.0, written_counts.1);
                    committer.mark_dirty(&target.ident);
                }
                // Another front applied it: go on from where it got to.
                Ran::Abandoned | Ran::Idle => {}
            }
        }
        Ok(applied)
    }

    /// The data files `snapshot_id` added to `source`; none for a snapshot
    /// that is not an append.
    async fn added(&self, source: &LakeTable, snapshot_id: i64) -> Result<Vec<String>, HouseError> {
        let metadata = &source.metadata;
        let Some(snapshot) = metadata.snapshot_by_id(snapshot_id) else {
            return Ok(Vec::new());
        };
        if snapshot.summary().operation != Operation::Append {
            return Ok(Vec::new());
        }
        let unreadable = |err: loams_house_lake::LakeError| {
            HouseError::from(ChError::network_error(format!(
                "a manifest of {} is unreadable: {err}",
                metadata.location()
            )))
        };
        let lake = self.lake();
        let list = lake.read_file(snapshot.manifest_list()).await?;
        let mut out = Vec::new();
        for manifest in loams_house_lake::files::manifests_added_by(
            &list,
            metadata.format_version(),
            snapshot_id,
        )
        .map_err(unreadable)?
        {
            let bytes = lake.read_file(&manifest.location).await?;
            out.extend(
                loams_house_lake::files::added_files(&bytes, &manifest, snapshot_id)
                    .map_err(unreadable)?
                    .into_iter()
                    .map(|f| f.location),
            );
        }
        Ok(out)
    }
}

/// Every this many background rounds, the targets of refreshable views are
/// listed again (HE1 R9.64), so a view a front created is found within
/// about this many rounds.
pub const REFRESH_SCAN_EVERY: u64 = 3;

/// The background catch-up of views over lake tables (see the module docs),
/// and the runs of refreshable views ([`crate::refresh`]).
#[derive(Debug)]
pub struct Scheduler {
    runner: ViewRunner,
    refresh: crate::refresh::RefreshRunner,
    /// The targets of refreshable views at the last listing.
    targets: tokio::sync::Mutex<Vec<(u64, String, String)>>,
}

impl Scheduler {
    /// A catch-up with `runner`.
    pub fn new(runner: ViewRunner) -> Self {
        let refresh = crate::refresh::RefreshRunner::new(runner.writer.clone());
        Self {
            runner,
            refresh,
            targets: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Runs every `every` until the returned task is aborted.
    pub fn spawn(self, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut round: u64 = 0;
            loop {
                tick.tick().await;
                self.round(round.is_multiple_of(FULL_SCAN_EVERY)).await;
                self.refresh_round(
                    round.is_multiple_of(REFRESH_SCAN_EVERY),
                    crate::retention::now_ms().div_euclid(1000),
                )
                .await;
                round += 1;
            }
        })
    }

    /// One round: the tables that took commits since the last, or, with
    /// `full`, every table of every namespace.
    pub async fn round(&self, full: bool) {
        let lake = self.runner.lake();
        let mut tables: Vec<(u64, String, String)> = lake
            .committer()
            .take_views_dirty()
            .iter()
            .filter_map(Lake::names_of)
            .collect();
        if full {
            match lake.namespaces().await {
                Ok(namespaces) => {
                    for namespace in namespaces {
                        match sources(lake, namespace).await {
                            Ok(found) => tables
                                .extend(found.into_iter().map(|(db, name)| (namespace, db, name))),
                            Err(err) => eprintln!(
                                "loams-house: views of namespace {namespace} cannot be listed: \
                                 {}",
                                err.message()
                            ),
                        }
                    }
                }
                Err(err) => eprintln!(
                    "loams-house: the views' catch-up cannot list namespaces: {}",
                    err.message()
                ),
            }
        }
        tables.sort();
        tables.dedup();
        for (namespace, database, name) in tables {
            if let Err(err) = self.runner.after_commit(namespace, &database, &name).await {
                eprintln!(
                    "loams-house: a materialized view of {database}.{name} (namespace \
                     {namespace}) failed: {}",
                    err.message()
                );
            }
        }
    }
}

impl Scheduler {
    /// One round of refreshable views: each that is due at `now_s` runs, and
    /// the views of its target after it. With `scan`, their targets are listed
    /// again first.
    pub async fn refresh_round(&self, scan: bool, now_s: i64) {
        let lake = self.runner.lake();
        let mut targets = self.targets.lock().await;
        if scan {
            let mut found = Vec::new();
            match lake.namespaces().await {
                Ok(namespaces) => {
                    for namespace in namespaces {
                        match crate::refresh::targets(lake, namespace).await {
                            Ok(t) => {
                                found.extend(t.into_iter().map(|(db, name)| (namespace, db, name)))
                            }
                            Err(err) => eprintln!(
                                "loams-house: refreshable views of namespace {namespace} cannot \
                                 be listed: {}",
                                err.message()
                            ),
                        }
                    }
                    *targets = found;
                }
                Err(err) => eprintln!(
                    "loams-house: refreshable views cannot list namespaces: {}",
                    err.message()
                ),
            }
        }
        for (namespace, database, name) in targets.iter() {
            match self
                .refresh
                .run_due(*namespace, database, name, now_s)
                .await
            {
                Ok(runs) => {
                    if runs
                        .iter()
                        .any(|r| matches!(r, crate::refresh::Refreshed::Appended(_)))
                        && let Err(err) = self.runner.after_commit(*namespace, database, name).await
                    {
                        eprintln!(
                            "loams-house: a materialized view of {database}.{name} (namespace \
                             {namespace}) failed: {}",
                            err.message()
                        );
                    }
                }
                Err(err) => eprintln!(
                    "loams-house: a refreshable view into {database}.{name} (namespace \
                     {namespace}) failed: {}",
                    err.message()
                ),
            }
        }
    }
}

/// The lake tables of a namespace that views read.
pub async fn sources(lake: &Lake, namespace: u64) -> Result<Vec<(String, String)>, HouseError> {
    let mut out = Vec::new();
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            if let Some(table) = lake.load(namespace, &database.name, &name).await?
                && ddl::class_of(&table.metadata) == TableClass::Lake
                && !views_of(&table).is_empty()
            {
                out.push((database.name.clone(), name));
            }
        }
    }
    Ok(out)
}

/// Runs the views of `database.name` after an `INSERT` committed into it (see
/// the module docs): their error is the `INSERT`'s unless `settings` set
/// `materialized_views_ignore_errors`.
#[allow(clippy::too_many_arguments)]
pub async fn after_insert(
    lake: &Arc<Lake>,
    pool: &crate::admission::WorkerPool,
    limits: crate::settings::SessionLimits,
    max_insert_bytes: u64,
    namespace: u64,
    database: &str,
    name: &str,
    settings: &[(String, String)],
) -> Result<(), HouseError> {
    let runner = ViewRunner::new(ViewWriter {
        lake: Arc::clone(lake),
        pool: pool.clone(),
        limits,
        max_insert_bytes,
    });
    match runner.after_commit(namespace, database, name).await {
        Err(err) if !ignore_errors(settings) => Err(err),
        _ => Ok(()),
    }
}

/// Whether the request asked that a failing view not fail its `INSERT`.
pub fn ignore_errors(settings: &[(String, String)]) -> bool {
    settings
        .iter()
        .rev()
        .find(|(k, _)| k == "materialized_views_ignore_errors")
        .is_some_and(|(_, v)| matches!(v.trim(), "1" | "true" | "True" | "TRUE"))
}

/// The table a source's identifier names, for messages.
pub fn source_text(table: &TableIdent) -> String {
    crate::catalog::table_text(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_and_settings() {
        assert!(transient(&HouseError::from(ChError::network_error("x"))));
        assert!(!transient(&HouseError::from(ChError::bad_arguments("x"))));
        let pairs = |v: &str| {
            vec![(
                "materialized_views_ignore_errors".to_string(),
                v.to_string(),
            )]
        };
        assert!(ignore_errors(&pairs("1")));
        assert!(!ignore_errors(&pairs("0")));
        assert!(!ignore_errors(&[]));
    }
}
