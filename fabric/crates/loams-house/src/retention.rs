//! Retention, orphan files, restore and catalog reconciliation for lake tables
//! (HS1 Task 15, design §49 §11).
//!
//! * **Snapshot expiry** ([`expire_table`]). A table keeps its snapshots for
//!   `loams_snapshot_retention` (the front's `[house] snapshot_retention_hours`,
//!   7 days by default and never under [`MIN_RETENTION`]; a table's own
//!   `history.expire.max-snapshot-age-ms` property, the Iceberg name, overrides
//!   it, under the same floor). Older snapshots are removed by a commit that
//!   requires `main` not to have moved ([`loams_house_lake::expire`]), and the
//!   files only they listed — their manifest lists, manifests and data files —
//!   are deleted after that commit. A file a kept snapshot still lists is never
//!   deleted.
//! * **Orphan files** ([`remove_orphans`]). Files under a table's `data/` and
//!   `metadata/` that nothing in its metadata names and that are older than
//!   [`ORPHAN_AGE`] (three days): the leftovers of writes whose commit failed,
//!   abandoned compactions and metadata files past the metadata log. A table
//!   whose bucket holds a newer metadata file than the catalog's pointer is left
//!   alone until it is reconciled ([`reconcile`]).
//! * **Purge** ([`purge_dropped`]). A table dropped longer ago than the
//!   retention (so `UNDROP` no longer finds it) has its files deleted and is
//!   forgotten by the catalog.
//! * **Restore** ([`restore_table`]). `RestoreTable` makes a retained snapshot
//!   `main` again (an Iceberg `set-ref`), chosen by id or as the snapshot that
//!   was current at a time. The request's idempotency key is recorded in the
//!   same commit as a table property, so a repeat within 24 hours answers the
//!   first call's operation instead of restoring again, on any front.
//! * **Reconciliation** ([`reconcile`]). After the catalog's database is
//!   restored to an earlier point (§49 §11 disaster recovery), a table whose
//!   newest metadata file in the bucket is newer than the catalog's pointer is
//!   registered again at that file (Iceberg REST `register`), and a table the
//!   catalog no longer knows is registered under the name its location carries.
//!   It is an offline step: run it with every front stopped, since a commit in
//!   flight writes its metadata file before it moves the pointer.
//!
//! The [`Scheduler`] runs expiry, orphan removal and purges over every
//! namespace of the catalog, beside compaction (§49 §6.2).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_house_lake::orphans::{self, Orphans, StoredFile};
use loams_house_lake::spec::{SnapshotReference, SnapshotRetention, TableMetadata};
use loams_house_lake::{MetadataFile, TableIdent, TableRequirement, TableUpdate, files};

use crate::catalog::{
    CatalogError, DROPPED, DROPPED_AT_MS, DROPPED_DATABASE, DROPPED_NAME, Lake, LakeTable,
};
use crate::commit::CommitIo;
use crate::ddl::{self, TableClass};
use crate::errors::{ChError, HouseError};

/// The shortest snapshot retention the House accepts (§49 §11, Q691).
pub const MIN_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// How old an unreferenced file must be before it is an orphan.
pub const ORPHAN_AGE: Duration = Duration::from_secs(3 * 24 * 3600);
/// A table's own snapshot retention, in milliseconds (Iceberg's property).
pub const RETENTION_PROPERTY: &str = "history.expire.max-snapshot-age-ms";
/// How often the [`Scheduler`] sweeps by default.
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// The table properties recording `RestoreTable`'s idempotency keys.
pub const RESTORE_KEY_PREFIX: &str = "loams.restore.";
/// How long a `RestoreTable` idempotency key is remembered.
pub const RESTORE_KEY_TTL: Duration = Duration::from_secs(24 * 3600);
/// The longest idempotency key `RestoreTable` accepts.
pub const MAX_IDEMPOTENCY_KEY: usize = 128;
/// How many times a restore is tried against concurrent commits.
pub const RESTORE_ATTEMPTS: u32 = 5;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `retention` unless it is under [`MIN_RETENTION`].
pub fn check_retention(retention: Duration) -> Result<Duration, String> {
    if retention < MIN_RETENTION {
        return Err(format!(
            "the snapshot retention is at least {} hours (asked for {} s)",
            MIN_RETENTION.as_secs() / 3600,
            retention.as_secs()
        ));
    }
    Ok(retention)
}

/// The retention of one table: its own property when set (raised to the floor),
/// else `default`.
pub fn table_retention(metadata: &TableMetadata, default: Duration) -> Duration {
    metadata
        .properties()
        .get(RETENTION_PROPERTY)
        .and_then(|v| v.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms).max(MIN_RETENTION))
        .unwrap_or(default)
}

fn unreadable(what: &str, err: impl std::fmt::Display) -> HouseError {
    HouseError::from(ChError::network_error(format!("{what}: {err}")))
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// The files a snapshot lists, read through the store: its manifest list, its
/// manifests and their live files. `manifests` caches each manifest's files
/// across snapshots, which share most of theirs.
async fn snapshot_files(
    io: &CommitIo,
    metadata: &TableMetadata,
    snapshot_id: i64,
    manifests: &mut HashMap<String, Vec<String>>,
    into: &mut HashSet<String>,
) -> Result<(), HouseError> {
    let Some(snapshot) = metadata.snapshot_by_id(snapshot_id) else {
        return Ok(());
    };
    let list_location = snapshot.manifest_list().to_string();
    let list = io.read(&list_location).await?;
    into.insert(list_location);
    for manifest in files::manifests(&list, metadata.format_version())
        .map_err(|e| unreadable("a manifest list", e))?
    {
        if !manifests.contains_key(&manifest.location) {
            let bytes = io.read(&manifest.location).await?;
            let listed = files::live_files(&bytes, &manifest)
                .map_err(|e| unreadable("a manifest", e))?
                .into_iter()
                .map(|f| f.location)
                .collect();
            manifests.insert(manifest.location.clone(), listed);
        }
        into.insert(manifest.location.clone());
        into.extend(manifests[&manifest.location].iter().cloned());
    }
    Ok(())
}

/// Every file `table`'s metadata names: itself, its log, its statistics and
/// what each of its snapshots lists.
async fn referenced(io: &CommitIo, table: &LakeTable) -> Result<HashSet<String>, HouseError> {
    let metadata = &table.metadata;
    let mut named = orphans::metadata_references(metadata, &table.metadata_location);
    let mut manifests = HashMap::new();
    let ids: Vec<i64> = metadata.snapshots().map(|s| s.snapshot_id()).collect();
    for id in ids {
        snapshot_files(io, metadata, id, &mut manifests, &mut named).await?;
    }
    Ok(named)
}

/// What one table's expiry did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    /// Snapshots removed.
    pub snapshots: usize,
    /// Files deleted because only those snapshots listed them.
    pub files: usize,
}

/// Expires `table`'s snapshots older than its retention at `now_ms` (see the
/// module docs). Nothing is done to a table the House does not own, and a
/// concurrent commit makes this round leave the table alone.
pub async fn expire_table(
    lake: &Lake,
    table: &LakeTable,
    retention: Duration,
    now_ms: i64,
) -> Result<Expired, HouseError> {
    if ddl::class_of(&table.metadata) != TableClass::Lake {
        return Ok(Expired::default());
    }
    let cutoff = now_ms.saturating_sub(ms(table_retention(&table.metadata, retention)));
    let Some(expiry) = loams_house_lake::expire::expire(&table.metadata, cutoff) else {
        return Ok(Expired::default());
    };
    let committer = lake.committer();
    let io = committer.io();
    // What the expired snapshots list, read before they leave the metadata.
    let mut manifests = HashMap::new();
    let mut freed = HashSet::new();
    for id in &expiry.expired {
        snapshot_files(io, &table.metadata, *id, &mut manifests, &mut freed).await?;
    }
    let committed = match lake
        .catalog()
        .commit(&table.ident, expiry.requirements, expiry.updates)
        .await
    {
        Ok(committed) => committed,
        Err(CatalogError::Conflict { .. }) => return Ok(Expired::default()),
        Err(err) => return Err(err.into()),
    };
    // Only what no snapshot of the committed metadata still lists is freed.
    let still = referenced(io, &committed).await?;
    let mut deleted = 0;
    let mut freed: Vec<String> = freed.into_iter().filter(|f| !still.contains(f)).collect();
    freed.sort();
    for location in freed {
        io.delete(&location).await?;
        deleted += 1;
    }
    Ok(Expired {
        snapshots: expiry.expired.len(),
        files: deleted,
    })
}

/// Deletes `ident`'s orphan files as of `now_ms` (see the module docs):
/// `Some(n)` deleted, `None` when the table is left alone because the bucket
/// holds a newer metadata file than the catalog's pointer.
pub async fn remove_orphans(
    lake: &Lake,
    ident: &TableIdent,
    orphan_age: Duration,
    now_ms: i64,
) -> Result<Option<usize>, HouseError> {
    let committer = lake.committer();
    let io = committer.io();
    let catalog = lake.catalog();
    // The listing comes first: every file in it was written before the table
    // was loaded, so a commit landing in between names only newer files.
    let location = match catalog.load_table(ident).await {
        Ok(table) => table.metadata.location().to_string(),
        Err(CatalogError::NoSuchTable(_)) => return Ok(Some(0)),
        Err(err) => return Err(err.into()),
    };
    let listed = io.list(&location).await?;
    let table = match catalog.load_table(ident).await {
        Ok(table) => table,
        Err(CatalogError::NoSuchTable(_)) => return Ok(Some(0)),
        Err(err) => return Err(err.into()),
    };
    if ddl::class_of(&table.metadata) != TableClass::Lake || table.metadata.location() != location {
        return Ok(Some(0));
    }
    let current = MetadataFile::parse(&table.metadata_location)
        .map_err(|e| unreadable("the table's metadata file name", e))?;
    let named = referenced(io, &table).await?;
    let cutoff = u64::try_from(now_ms.saturating_sub(ms(orphan_age))).unwrap_or(0);
    match orphans::orphans(&location, &listed, &named, &current, cutoff) {
        Orphans::NewerMetadata(file) => {
            eprintln!(
                "loams-house: {} has metadata {file} newer than the catalog's pointer; \
                 its orphan files wait until the catalog is reconciled",
                crate::catalog::table_text(ident)
            );
            Ok(None)
        }
        Orphans::Files(found) => {
            let n = found.len();
            for file in found {
                io.delete(&file).await?;
            }
            Ok(Some(n))
        }
    }
}

/// Purges namespace `namespace`'s tables dropped longer ago than `retention`
/// at `now_ms`: their files are deleted, then the catalog forgets them, then
/// the metadata files go. Answers how many were purged.
pub async fn purge_dropped(
    lake: &Lake,
    namespace: u64,
    retention: Duration,
    now_ms: i64,
) -> Result<usize, HouseError> {
    let catalog = lake.catalog();
    let dropped = match catalog
        .list_tables(&Lake::dropped_namespace(namespace))
        .await
    {
        Ok(tables) => tables,
        Err(CatalogError::NoSuchNamespace(_)) => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let committer = lake.committer();
    let io = committer.io();
    let mut purged = 0;
    for ident in dropped {
        let table = match catalog.load_table(&ident).await {
            Ok(table) => table,
            Err(CatalogError::NoSuchTable(_)) => continue,
            Err(err) => return Err(err.into()),
        };
        let at = table
            .metadata
            .properties()
            .get(DROPPED_AT_MS)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if now_ms.saturating_sub(at) <= ms(retention)
            || ddl::class_of(&table.metadata) != TableClass::Lake
        {
            continue;
        }
        let location = table.metadata.location().to_string();
        let listed = io.list(&location).await?;
        let (metadata_files, others): (Vec<&StoredFile>, Vec<&StoredFile>) = listed
            .iter()
            .partition(|f| f.location.ends_with(".metadata.json"));
        for file in others {
            io.delete(&file.location).await?;
        }
        match catalog.drop_table(&ident).await {
            Ok(()) | Err(CatalogError::NoSuchTable(_)) => {}
            Err(err) => return Err(err.into()),
        }
        for file in metadata_files {
            io.delete(&file.location).await?;
        }
        purged += 1;
    }
    Ok(purged)
}

/// What one sweep did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    /// Snapshots expired.
    pub expired_snapshots: usize,
    /// Files freed by those expiries.
    pub freed_files: usize,
    /// Orphan files deleted.
    pub orphan_files: usize,
    /// Tables whose orphans wait for a reconciliation.
    pub waiting_tables: usize,
    /// Dropped tables purged.
    pub purged_tables: usize,
    /// Tables whose sweep failed (logged; the next sweep tries again).
    pub failed_tables: usize,
}

/// Expiry and orphan removal for every live table of `namespace`, then the
/// purge of its dropped tables, as of `now_ms`.
pub async fn sweep_namespace(
    lake: &Lake,
    namespace: u64,
    now_ms: i64,
) -> Result<Swept, HouseError> {
    let retention = lake.retention();
    let mut swept = Swept::default();
    // The House's own `_house` (the query log, HS1 Task 23) is swept too: it
    // takes a snapshot every few seconds.
    let mut databases: Vec<String> = lake
        .databases(namespace)
        .await?
        .into_iter()
        .map(|d| d.name)
        .collect();
    databases.push(crate::querylog::HOUSE_DATABASE.to_string());
    for database in databases {
        for name in lake.table_names(namespace, &database).await? {
            let Some(table) = lake.load(namespace, &database, &name).await? else {
                continue;
            };
            // One table's failure (an unreadable manifest, a store error) is
            // logged and the sweep goes on with the others.
            let result = async {
                let expired = expire_table(lake, &table, retention, now_ms).await?;
                let orphans = remove_orphans(lake, &table.ident, ORPHAN_AGE, now_ms).await?;
                Ok::<_, HouseError>((expired, orphans))
            }
            .await;
            match result {
                Ok((expired, orphans)) => {
                    swept.expired_snapshots += expired.snapshots;
                    swept.freed_files += expired.files;
                    match orphans {
                        Some(n) => swept.orphan_files += n,
                        None => swept.waiting_tables += 1,
                    }
                }
                Err(err) => {
                    swept.failed_tables += 1;
                    eprintln!(
                        "loams-house: the retention sweep of {}.{name} (namespace {namespace}) \
                         failed: {}",
                        database,
                        err.message()
                    );
                }
            }
        }
    }
    swept.purged_tables = purge_dropped(lake, namespace, retention, now_ms).await?;
    Ok(swept)
}

/// The background sweep over every namespace of the catalog.
#[derive(Debug)]
pub struct Scheduler {
    lake: Arc<Lake>,
}

impl Scheduler {
    /// A sweep over `lake`'s namespaces.
    pub fn new(lake: Arc<Lake>) -> Self {
        Self { lake }
    }

    /// Sweeps every `every` until the returned task is aborted.
    pub fn spawn(self, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                self.round(now_ms()).await;
            }
        })
    }

    /// One sweep as of `now_ms`; failures are logged and the next namespace
    /// goes on.
    pub async fn round(&self, now_ms: i64) {
        let namespaces = match self.lake.namespaces().await {
            Ok(namespaces) => namespaces,
            Err(err) => {
                eprintln!(
                    "loams-house: the retention sweep cannot list namespaces: {}",
                    err.message()
                );
                return;
            }
        };
        for namespace in namespaces {
            if let Err(err) = sweep_namespace(&self.lake, namespace, now_ms).await {
                eprintln!(
                    "loams-house: the retention sweep of namespace {namespace} failed: {}",
                    err.message()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

/// The point `RestoreTable` goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePoint {
    /// This snapshot.
    Snapshot(i64),
    /// The snapshot `main` pointed at at this time, in Unix milliseconds.
    Timestamp(i64),
}

/// A restore's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The operation's id (`op-` and 26 hex characters).
    pub operation_id: String,
    /// The snapshot that is `main` now.
    pub snapshot_id: i64,
    /// When the restore committed, in Unix milliseconds.
    pub at_ms: i64,
    /// Whether this answers an earlier call with the same idempotency key.
    pub repeated: bool,
}

/// `36` unless `key` is empty or a short run of `[A-Za-z0-9_-]`.
pub fn check_idempotency_key(key: &str) -> Result<(), HouseError> {
    if key.len() > MAX_IDEMPOTENCY_KEY
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(HouseError::from(ChError::bad_arguments(format!(
            "idempotency_key is at most {MAX_IDEMPOTENCY_KEY} characters of A-Z, a-z, 0-9, \
             '_' and '-' (a ULID fits)"
        ))));
    }
    Ok(())
}

fn recorded(metadata: &TableMetadata, key: &str, now_ms: i64) -> Option<Restored> {
    let value = metadata
        .properties()
        .get(&format!("{RESTORE_KEY_PREFIX}{key}"))?;
    let mut parts = value.split(' ');
    let operation_id = parts.next()?.to_string();
    let snapshot_id = parts.next()?.parse().ok()?;
    let at_ms: i64 = parts.next()?.parse().ok()?;
    (now_ms.saturating_sub(at_ms) <= ms(RESTORE_KEY_TTL)).then_some(Restored {
        operation_id,
        snapshot_id,
        at_ms,
        repeated: true,
    })
}

fn operation_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("op-{}", &hex[..26])
}

fn chosen(
    metadata: &TableMetadata,
    database: &str,
    table: &str,
    point: RestorePoint,
) -> Result<i64, HouseError> {
    match point {
        RestorePoint::Snapshot(id) => metadata
            .snapshot_by_id(id)
            .map(|s| s.snapshot_id())
            .ok_or_else(|| {
                HouseError::from(ChError::bad_arguments(format!(
                    "Table {database}.{table} has no snapshot {id}: it never existed, or it is \
                     older than the table's snapshot retention"
                )))
            }),
        RestorePoint::Timestamp(at) => metadata
            .history()
            .iter()
            .rev()
            .find(|entry| entry.timestamp_ms <= at)
            .filter(|entry| metadata.snapshot_by_id(entry.snapshot_id).is_some())
            .map(|entry| entry.snapshot_id)
            .ok_or_else(|| {
                HouseError::from(ChError::bad_arguments(format!(
                    "Table {database}.{table} has no snapshot that was current at {at} ms: the \
                     time is before its first write, or older than its snapshot retention"
                )))
            }),
    }
}

/// `RestoreTable`: makes the snapshot `point` names `main` of
/// `database.table` (see the module docs). `key`, when not empty, makes a
/// repeat within 24 hours answer the first call.
pub async fn restore_table(
    lake: &Lake,
    namespace: u64,
    database: &str,
    table: &str,
    point: RestorePoint,
    key: &str,
    now_ms: i64,
) -> Result<Restored, HouseError> {
    check_idempotency_key(key)?;
    for _ in 0..RESTORE_ATTEMPTS {
        let loaded = lake
            .lake_table(namespace, database, table, "RestoreTable")
            .await?;
        let metadata = &loaded.metadata;
        if !key.is_empty()
            && let Some(first) = recorded(metadata, key, now_ms)
        {
            return Ok(first);
        }
        let snapshot_id = chosen(metadata, database, table, point)?;
        let operation_id = operation_id();
        let mut updates = vec![TableUpdate::SetSnapshotRef {
            ref_name: loams_house_lake::append::MAIN.to_string(),
            reference: SnapshotReference::new(
                snapshot_id,
                SnapshotRetention::branch(None, None, None),
            ),
        }];
        if !key.is_empty() {
            updates.push(TableUpdate::SetProperties {
                updates: HashMap::from([(
                    format!("{RESTORE_KEY_PREFIX}{key}"),
                    format!("{operation_id} {snapshot_id} {now_ms}"),
                )]),
            });
        }
        // Keys past their 24 hours are forgotten in the same commit.
        let stale: Vec<String> = metadata
            .properties()
            .keys()
            .filter_map(|k| {
                let old = k.strip_prefix(RESTORE_KEY_PREFIX)?;
                (old != key && recorded(metadata, old, now_ms).is_none()).then(|| k.clone())
            })
            .collect();
        if !stale.is_empty() {
            updates.push(TableUpdate::RemoveProperties { removals: stale });
        }
        let requirements = vec![TableRequirement::RefSnapshotIdMatch {
            r#ref: loams_house_lake::append::MAIN.to_string(),
            snapshot_id: metadata.current_snapshot_id(),
        }];
        match lake
            .catalog()
            .commit(&loaded.ident, requirements, updates)
            .await
        {
            Ok(_) => {
                return Ok(Restored {
                    operation_id,
                    snapshot_id,
                    at_ms: now_ms,
                    repeated: false,
                });
            }
            Err(CatalogError::Conflict { .. }) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(HouseError::from(ChError::network_error(format!(
        "RestoreTable {database}.{table}: other commits came first {RESTORE_ATTEMPTS} times; \
         try again"
    ))))
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

/// What [`reconcile`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// Tables registered again at a newer metadata file, as `db.table` (or
    /// the dropped tables' namespace and name).
    pub updated: Vec<String>,
    /// Tables the catalog did not know, registered.
    pub registered: Vec<String>,
    /// Table locations that could not be registered (their name is taken), by
    /// their newest metadata file.
    pub skipped: Vec<String>,
}

/// The newest metadata file among `listed`, by version.
fn newest_metadata(listed: &[StoredFile]) -> Option<(MetadataFile, String)> {
    listed
        .iter()
        .filter(|f| f.location.ends_with(".metadata.json") && f.location.contains("/metadata/"))
        .filter_map(|f| {
            MetadataFile::parse(&f.location)
                .ok()
                .map(|name| (name, f.location.clone()))
        })
        .max_by_key(|(name, _)| name.version())
}

/// Where a table belongs, from its newest metadata: the dropped tables'
/// namespace when it was dropped, else `live`.
fn home(namespace: u64, newest: &TableMetadata, live: Option<TableIdent>) -> Option<TableIdent> {
    if let Some(at) = newest
        .properties()
        .get(DROPPED_AT_MS)
        .and_then(|v| v.parse::<u64>().ok())
    {
        return Some(TableIdent::new(
            Lake::dropped_namespace(namespace),
            Lake::dropped_name(&newest.uuid(), at),
        ));
    }
    live
}

/// The live name a known table goes back to: its own when it is live; the one
/// its dropped properties name when the catalog has it as dropped.
fn live_name(namespace: u64, known: &LakeTable) -> Option<TableIdent> {
    if known.ident.namespace().as_ref().last().map(String::as_str) != Some(DROPPED) {
        return Some(known.ident.clone());
    }
    let properties = known.metadata.properties();
    Some(Lake::table_ident(
        namespace,
        properties.get(DROPPED_DATABASE)?,
        properties.get(DROPPED_NAME)?,
    ))
}

async fn read_metadata(io: &CommitIo, location: &str) -> Result<TableMetadata, HouseError> {
    let bytes = io.read(location).await?;
    loams_house_lake::metadata::from_json(location, &bytes).map_err(|e| unreadable(location, e))
}

/// Creates the namespace `target` goes in, if it is missing.
async fn ensure_parent(lake: &Lake, namespace: u64, target: &TableIdent) -> Result<(), HouseError> {
    lake.ensure_root(namespace).await?;
    match lake
        .catalog()
        .create_database(target.namespace(), HashMap::new())
        .await
    {
        Ok(()) | Err(CatalogError::NamespaceExists(_)) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Re-registers `known` at `newest` under `target`.
async fn repoint(
    lake: &Lake,
    namespace: u64,
    known: &LakeTable,
    target: &TableIdent,
    newest: &str,
) -> Result<bool, HouseError> {
    let catalog = lake.catalog();
    if *target != known.ident {
        ensure_parent(lake, namespace, target).await?;
        match catalog.register_table(target, newest).await {
            Ok(_) => {}
            Err(CatalogError::TableExists(_)) => return Ok(false),
            Err(err) => return Err(err.into()),
        }
        catalog.drop_table(&known.ident).await?;
        return Ok(true);
    }
    catalog.drop_table(&known.ident).await?;
    if let Err(err) = catalog.register_table(target, newest).await {
        // Put the old pointer back rather than lose the table.
        catalog
            .register_table(target, &known.metadata_location)
            .await?;
        return Err(err.into());
    }
    Ok(true)
}

/// Reconciles namespace `namespace`'s tables with the bucket (see the module
/// docs). Every front must be stopped.
pub async fn reconcile(lake: &Lake, namespace: u64) -> Result<Reconciled, HouseError> {
    let catalog = lake.catalog();
    let committer = lake.committer();
    let io = committer.io();
    let mut out = Reconciled::default();

    // The tables the catalog knows, live and dropped.
    let mut known: Vec<LakeTable> = Vec::new();
    let mut namespaces = vec![Lake::dropped_namespace(namespace)];
    namespaces.extend(
        lake.databases(namespace)
            .await?
            .into_iter()
            .map(|d| Lake::namespace(namespace, &d.name))
            .chain([Lake::namespace(namespace, crate::querylog::HOUSE_DATABASE)]),
    );
    for ns in &namespaces {
        let tables = match catalog.list_tables(ns).await {
            Ok(tables) => tables,
            Err(CatalogError::NoSuchNamespace(_)) => continue,
            Err(err) => return Err(err.into()),
        };
        for ident in tables {
            match catalog.load_table(&ident).await {
                Ok(table) if ddl::class_of(&table.metadata) != TableClass::External => {
                    known.push(table)
                }
                Ok(_) | Err(CatalogError::NoSuchTable(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    let known_locations: HashSet<String> = known
        .iter()
        .map(|t| t.metadata.location().trim_end_matches('/').to_string())
        .collect();

    // Known tables whose newest metadata is past the pointer.
    for table in &known {
        let listed = io
            .list(&format!(
                "{}/metadata",
                table.metadata.location().trim_end_matches('/')
            ))
            .await?;
        let Some((name, newest)) = newest_metadata(&listed) else {
            continue;
        };
        let pointer = MetadataFile::parse(&table.metadata_location)
            .map_err(|e| unreadable("the table's metadata file name", e))?;
        if name.version() <= pointer.version() || newest == table.metadata_location {
            continue;
        }
        let metadata = read_metadata(io, &newest).await?;
        let Some(target) = home(namespace, &metadata, live_name(namespace, table)) else {
            out.skipped.push(newest);
            continue;
        };
        if repoint(lake, namespace, table, &target, &newest).await? {
            out.updated.push(crate::catalog::table_text(&target));
        } else {
            out.skipped.push(newest);
        }
    }

    // Table locations the catalog does not know.
    let tree = format!("{}/ns/{namespace}/house", lake.root());
    let mut unknown: HashMap<String, Vec<StoredFile>> = HashMap::new();
    for file in io.list(&tree).await? {
        let Some((dir, _)) = file.location.split_once("/metadata/") else {
            continue;
        };
        if !known_locations.contains(dir) {
            unknown.entry(dir.to_string()).or_default().push(file);
        }
    }
    let mut dirs: Vec<_> = unknown.into_iter().collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    for (dir, listed) in dirs {
        let Some((_, newest)) = newest_metadata(&listed) else {
            continue;
        };
        let metadata = read_metadata(io, &newest).await?;
        if metadata.location().trim_end_matches('/') != dir
            || ddl::class_of(&metadata) == TableClass::External
        {
            out.skipped.push(newest);
            continue;
        }
        // `<tree>/<database>/<table>-<32 hex>`: the names it was created with.
        let guess = dir
            .strip_prefix(&format!("{tree}/"))
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(database, leaf)| {
                let (table, id) = leaf.rsplit_once('-')?;
                (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| (database.to_string(), table.to_string()))
            });
        let live = guess
            .as_ref()
            .map(|(database, table)| Lake::table_ident(namespace, database, table));
        let Some(target) = home(namespace, &metadata, live) else {
            out.skipped.push(newest);
            continue;
        };
        ensure_parent(lake, namespace, &target).await?;
        match catalog.register_table(&target, &newest).await {
            Ok(_) => out.registered.push(crate::catalog::table_text(&target)),
            Err(CatalogError::TableExists(_)) => out.skipped.push(newest),
            Err(err) => return Err(err.into()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_floor_and_keys() {
        assert!(check_retention(Duration::from_secs(3600)).is_err());
        assert_eq!(
            check_retention(MIN_RETENTION).expect("floor"),
            MIN_RETENTION
        );
        assert!(check_idempotency_key("").is_ok());
        assert!(check_idempotency_key("01J9Z3Q4W5E6R7T8Y9U0I1O2P3").is_ok());
        assert!(check_idempotency_key("a b").is_err());
        assert!(check_idempotency_key(&"x".repeat(MAX_IDEMPOTENCY_KEY + 1)).is_err());
        let id = operation_id();
        assert_eq!(id.len(), 29);
        assert!(id.starts_with("op-"));
    }
}
