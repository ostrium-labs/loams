//! Snapshots and restores of the Loams-owned stores (design §54 §7, §8;
//! D930, D931; UP1 Task 8). The writers are stopped first (the CLI quiesces
//! the stack); a snapshot then copies:
//!
//! | Component | Method |
//! |---|---|
//! | `bucket` | the objects the metastore's pointers name, Git's mutable catalog objects and the embedded metastore's snapshot objects (small: the immutable rest is pinned, not copied) |
//! | `raft` | the embedded metastore's directory, cold (`<data-dir>/meta`) |
//! | `durable` | SQLite's backup API (`<data-dir>/durable/default.db`, the store before D2450) |
//! | `durable_local` | a copy of the local durable store (`<data-dir>/durable/default.redb`, D2450) |
//! | `house` | the House data directory |
//! | `pg_control` | `pg-control`'s data directory |
//! | `agentd` | agentd's data directory |
//!
//! TiKV (BR or a cold copy) and Postgres (branches) are UP1 Task 9.
//!
//! `snapshot.json` is written **last**: a directory without it is an
//! incomplete snapshot and is never restored. The snapshot is then recorded
//! as a GC root (`_system`'s `upgrade/snapshots/<id>`): while it is kept, no
//! GC deletes an object written at or before its time
//! ([`loams_common::meta::snapshot_gc_floor`]). Snapshot directories are mode
//! 0700 and their files 0600: they hold secrets (API key hashes, House keys,
//! SQLite contents) and never leave the host.

mod files;
mod manifest;
pub mod postgres;
pub mod tikv;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use loams_common::meta::{Consistency, PRUNED, SNAPSHOT_ROOT_PREFIX, SYSTEM_NAMESPACE};
pub use manifest::{Component, MANIFEST_FILE, SnapshotListing, SnapshotManifest};

use crate::cx::Cx;
use crate::ledger::Ledger;
use crate::migration::MigrationError;

/// The local durable store's file name (D2450), in `<data-dir>/durable`.
const DURABLE_LOCAL_FILE: &str = "default.redb";

/// `<data-dir>/durable/default.redb`, when the context has a data directory.
fn durable_local(cx: &Cx) -> Option<PathBuf> {
    cx.durable_cx()
        .ok()
        .and_then(|d| d.path().parent().map(|p| p.join(DURABLE_LOCAL_FILE)))
}

/// Why a snapshot or a restore failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// No `snapshot.json`: the snapshot did not finish (`snapshot_incomplete`).
    #[error("snapshot_incomplete: {0} has no snapshot.json; it was never finished")]
    Incomplete(PathBuf),
    /// A component's files do not match their recorded digest.
    #[error("snapshot_corrupt: {0}")]
    Corrupt(String),
    #[error("snapshot_not_found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
}

impl From<MigrationError> for SnapshotError {
    fn from(e: MigrationError) -> Self {
        SnapshotError::Io(e.to_string())
    }
}

fn io(what: impl std::fmt::Display, e: impl std::fmt::Display) -> SnapshotError {
    SnapshotError::Io(format!("{what}: {e}"))
}

/// What to snapshot besides the stores in [`Cx`].
#[derive(Clone, Debug, Default)]
pub struct SnapshotOptions {
    /// The release the stack is being upgraded to.
    pub target_release: String,
    /// The embedded metastore's directory, copied cold (`None` on TiKV).
    pub raft_dir: Option<PathBuf>,
    /// agentd's data directory, when the stack runs it.
    pub agentd_dir: Option<PathBuf>,
    /// Components another step took, recorded in the manifest as given.
    pub extra: Vec<Component>,
    /// The snapshot id to use (`None`: a new ULID): the CLI picks it when a
    /// helper (BR) writes into the snapshot directory first.
    pub id: Option<String>,
    /// TiKV's backup, taken by the CLI's helper into `<snapshot>/tikv`
    /// (UP1 Task 9): how, and at which TiKV version.
    pub tikv: Option<(tikv::TikvMethod, String)>,
    /// Loams Postgres's storage (UP1 Task 9): drain the safekeeper, then
    /// branch every timeline.
    pub postgres: Option<PostgresOptions>,
}

/// Where Loams Postgres's storage answers, and how long a drain may take.
#[derive(Clone, Debug)]
pub struct PostgresOptions {
    pub pageserver: String,
    pub safekeeper: String,
    pub drain_timeout: std::time::Duration,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Takes a snapshot of every Loams-owned store `cx` holds into
/// `<to>/<id>/`, records it as a GC root, and returns its manifest.
pub async fn snapshot(
    cx: &Cx,
    to: &Path,
    opts: &SnapshotOptions,
) -> Result<SnapshotManifest, SnapshotError> {
    let id = opts
        .id
        .clone()
        .unwrap_or_else(|| ulid::Ulid::generate().to_string());
    let created_at_ms = now_ms();
    files::private_dir(to)?;
    let dir = to.join(&id);
    files::private_dir(&dir)?;
    let mut components = Vec::new();
    if let Some(bucket) = &cx.bucket {
        components.push(snapshot_bucket(cx, bucket, &dir, opts.raft_dir.is_some()).await?);
    }
    if let Some(raft) = opts.raft_dir.as_ref().filter(|d| d.exists()) {
        components.push(files::copy_component("raft", raft, &dir.join("raft"))?);
    }
    if let Ok(durable) = cx.durable_cx()
        && durable.exists()
    {
        let target = dir.join("durable");
        files::private_dir(&target)?;
        let copy = target.join("default.db");
        {
            let source = durable.conn()?;
            let mut out = rusqlite::Connection::open(&copy).map_err(|e| io("durable", e))?;
            let backup = rusqlite::backup::Backup::new(&source, &mut out)
                .map_err(|e| io("durable backup", e))?;
            backup
                .run_to_completion(256, std::time::Duration::ZERO, None)
                .map_err(|e| io("durable backup", e))?;
        }
        files::private_file(&copy)?;
        components.push(files::describe(
            "durable",
            "sqlite_backup",
            &dir,
            "durable",
        )?);
    }
    // The local durable store (D2450): one redb file, copied cold.
    if let Some(local) = durable_local(cx).filter(|p| p.exists()) {
        let target = dir.join("durable_local");
        files::private_dir(&target)?;
        let copy = target.join(DURABLE_LOCAL_FILE);
        std::fs::copy(&local, &copy).map_err(|e| io(local.display(), e))?;
        files::private_file(&copy)?;
        components.push(files::describe(
            "durable_local",
            "copy",
            &dir,
            "durable_local",
        )?);
    }
    if let Ok(house) = cx.house_cx()
        && house.dir().join("catalog.sqlite").exists()
    {
        components.push(files::copy_component(
            "house",
            house.dir(),
            &dir.join("house"),
        )?);
    }
    if let Ok(pg) = cx.pg_control_cx()
        && files::non_empty(pg.dir())
    {
        components.push(files::copy_component(
            "pg_control",
            pg.dir(),
            &dir.join("pg-control"),
        )?);
    }
    if let Some(agentd) = opts.agentd_dir.as_ref().filter(|d| files::non_empty(d)) {
        components.push(files::copy_component(
            "agentd",
            agentd,
            &dir.join("agentd"),
        )?);
    }
    if let Some((method, version)) = &opts.tikv {
        if files::size_of(&dir.join("tikv")) == 0 {
            return Err(SnapshotError::Io(format!(
                "the TiKV {} copy is missing: the CLI's helper writes <snapshot>/tikv first",
                method.as_str()
            )));
        }
        // The helper's files are TiKV's and BR's (other users): the
        // snapshot directory above them is 0700.
        let (bytes, sha256) = files::digest(&dir.join("tikv"))?;
        let mut component = tikv::component(*method, version, bytes);
        component.sha256 = sha256;
        components.push(component);
    }
    if let Some(pg) = &opts.postgres {
        let storage = postgres::PostgresStorage::new(&pg.pageserver, &pg.safekeeper)?;
        let drained = storage
            .drain(pg.drain_timeout, std::time::Duration::from_secs(2))
            .await?;
        let branches = storage.branch_every_timeline().await?;
        let target = dir.join("postgres");
        files::private_dir(&target)?;
        let body = serde_json::to_vec_pretty(&postgres::Branches { drained, branches })
            .map_err(|e| io("postgres/branches.json", e))?;
        files::write_private(&target.join("branches.json"), &body)?;
        let mut component = files::describe("postgres", "branch", &dir, "postgres")?;
        component.method = "branch".to_string();
        components.push(component);
    }
    components.extend(opts.extra.iter().cloned());
    let (stack_record, ledger) = match Ledger::open_existing(cx.meta.clone()).await {
        Ok(Some(ledger)) => (
            ledger.stack().await.ok().flatten(),
            ledger.entries().await.unwrap_or_default(),
        ),
        _ => (None, Vec::new()),
    };
    let manifest = SnapshotManifest {
        id: id.clone(),
        created_at_ms,
        release: stack_record.as_ref().map(|s| s.release.clone()),
        target_release: opts.target_release.clone(),
        components,
        ledger,
        stack_record,
    };
    // Last: the snapshot exists once its manifest does.
    let digest = manifest.write(&dir)?;
    record_root(cx, &id, created_at_ms, &digest).await?;
    Ok(manifest)
}

/// Copies the bucket objects the stores' mutable state names.
async fn snapshot_bucket(
    cx: &Cx,
    bucket: &loams_store::Store,
    dir: &Path,
    raft: bool,
) -> Result<Component, SnapshotError> {
    let root = dir.join("bucket");
    files::private_dir(&root)?;
    let mut paths = std::collections::BTreeSet::new();
    let namespaces = cx
        .meta
        .namespaces(Consistency::Linearizable)
        .await
        .map_err(|e| io("namespaces", e))?;
    for ns in &namespaces {
        // Every pointer's target that is an object (collection manifests,
        // graph manifests and catalog, the auth directory).
        for (_, pointer) in cx
            .meta
            .pointers_with_prefix(Consistency::Linearizable, ns.id, "")
            .await
            .map_err(|e| io("pointers", e))?
        {
            if pointer.value.contains('/') && !pointer.value.contains(';') {
                paths.insert(pointer.value);
            }
        }
        // Git's mutable catalog documents and head hints.
        for info in bucket
            .list(&format!("ns/{}/repos/", ns.id))
            .await
            .map_err(|e| io("listing repositories", e))?
        {
            let p = &info.path;
            if p.contains("/_names/")
                || p.contains("/_deleting/")
                || p.contains("/_ws_names/")
                || p.ends_with("/head")
            {
                paths.insert(info.path);
            }
        }
    }
    if raft {
        for info in bucket
            .list("meta/snapshots/")
            .await
            .map_err(|e| io("listing metastore snapshots", e))?
        {
            paths.insert(info.path);
        }
    }
    for path in paths {
        let bytes = match bucket.get(&path).await {
            Ok((bytes, _)) => bytes,
            Err(loams_store::StoreError::NotFound { .. }) => continue,
            Err(e) => return Err(io(&path, e)),
        };
        let target = root.join(&path);
        if let Some(parent) = target.parent() {
            files::private_dir_all(parent)?;
        }
        files::write_private(&target, &bytes)?;
    }
    files::describe("bucket", "copy", dir, "bucket")
}

/// Records snapshot `id` as a GC root.
async fn record_root(
    cx: &Cx,
    id: &str,
    created_ms: u64,
    digest: &str,
) -> Result<(), SnapshotError> {
    let ns = match cx.meta.create_namespace(SYSTEM_NAMESPACE).await {
        Ok(id) => id,
        Err(loams_common::meta::MetaError::Rejected(
            loams_common::meta::ApplyError::NamespaceExists(id),
        )) => id,
        Err(e) => return Err(io("_system", e)),
    };
    cx.meta_cx()
        .write_new(
            ns,
            &format!("{SNAPSHOT_ROOT_PREFIX}{id}"),
            &format!("v1;{created_ms};{digest}"),
            None,
        )
        .await?;
    Ok(())
}

/// Every snapshot under `dir`, oldest first: paths, sizes and digests only,
/// never contents.
pub fn list(dir: &Path) -> Result<Vec<SnapshotListing>, SnapshotError> {
    manifest::list(dir)
}

/// Removes snapshot `id`: its GC root first (so GC may collect again), then
/// its directory.
pub async fn prune(cx: &Cx, dir: &Path, id: &str) -> Result<(), SnapshotError> {
    if let Ok(Some(ns)) = cx
        .meta
        .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
        .await
    {
        let key = format!("{SNAPSHOT_ROOT_PREFIX}{id}");
        if let Some(current) = cx
            .meta
            .pointer(Consistency::Linearizable, ns.id, &key)
            .await
            .map_err(|e| io("snapshot root", e))?
            && current.value != PRUNED
        {
            cx.meta_cx()
                .switch(ns.id, &key, Some(current.version), PRUNED, None)
                .await?;
        }
    }
    let path = dir.join(id);
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(path.display(), e)),
    }
}

/// The bytes a snapshot of these stores would take (without TiKV), for the
/// free-space check: the files it copies, measured now.
pub fn estimate_bytes(cx: &Cx, opts: &SnapshotOptions) -> u64 {
    let mut total = 0;
    if let Some(raft) = &opts.raft_dir {
        total += files::size_of(raft);
    }
    if let Ok(durable) = cx.durable_cx() {
        total += files::size_of(durable.path());
    }
    if let Some(local) = durable_local(cx) {
        total += files::size_of(&local);
    }
    if let Ok(house) = cx.house_cx() {
        total += files::size_of(house.dir());
    }
    if let Ok(pg) = cx.pg_control_cx() {
        total += files::size_of(pg.dir());
    }
    if let Some(agentd) = &opts.agentd_dir {
        total += files::size_of(agentd);
    }
    total
}

/// Copies the embedded metastore's directory `meta_dir` (with no node open
/// on it) to `<to>/raft-<ulid>`, for [`SnapshotOptions::raft_dir`]; the
/// caller removes it after the snapshot.
pub fn stage_raft(meta_dir: &Path, to: &Path) -> Result<PathBuf, SnapshotError> {
    files::private_dir(to)?;
    let staged = to.join(format!(".raft-{}", ulid::Ulid::generate()));
    files::copy_component("raft", meta_dir, &staged)?;
    Ok(staged)
}

/// Restores the embedded metastore's directory from snapshot `from`, with no
/// node open on it. Refuses an incomplete or corrupt snapshot first.
pub fn restore_raft(from: &Path, meta_dir: &Path) -> Result<bool, SnapshotError> {
    let manifest = SnapshotManifest::read(from)?;
    manifest.check(from)?;
    match manifest.components.iter().find(|c| c.store == "raft") {
        Some(component) => {
            files::replace_dir(&from.join(&component.path), meta_dir)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// What a restore put back.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RestoreReport {
    pub id: String,
    pub restored: Vec<String>,
    /// Bucket objects that were missing and were put back.
    pub objects_put_back: usize,
    /// The Postgres branches the snapshot made: the CLI points each
    /// timeline's computes back at its branch (or calls `pg-control`'s
    /// `RestoreBranch`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub postgres_branches: Vec<postgres::SnapshotBranch>,
}

/// Restores every Loams-owned component of the snapshot at `from` (a
/// snapshot directory). The stack must be stopped. Refuses an incomplete or
/// corrupt snapshot before changing anything.
///
/// TiKV is restored by the CLI's helper (BR into a fresh cluster) before
/// this runs: `tikv_version` is that cluster's version, and a snapshot with
/// a `tikv` component is refused without it or at another version
/// (`component_downgrade`).
pub async fn restore(
    cx: &Cx,
    from: &Path,
    raft_dir: Option<&Path>,
    agentd_dir: Option<&Path>,
    tikv_version: Option<&str>,
) -> Result<RestoreReport, SnapshotError> {
    let manifest = SnapshotManifest::read(from)?;
    manifest.check(from)?;
    if let Some(taken) = manifest.components.iter().find(|c| c.store == "tikv") {
        let taken = tikv::component_version(taken).unwrap_or_default();
        let cluster = tikv_version.ok_or_else(|| {
            SnapshotError::Io(format!(
                "the snapshot holds TiKV at {taken}: restore it with BR into a fresh cluster \
                 at that version, then pass --tikv-version"
            ))
        })?;
        tikv::check_restore_target(taken, cluster)?;
    }
    let mut report = RestoreReport {
        id: manifest.id.clone(),
        ..RestoreReport::default()
    };
    for component in &manifest.components {
        let source = from.join(&component.path);
        match component.store.as_str() {
            "bucket" => {
                let Some(bucket) = &cx.bucket else {
                    return Err(SnapshotError::Io(
                        "restoring the bucket needs --bucket".into(),
                    ));
                };
                for (path, file) in files::walk(&source)? {
                    let bytes = std::fs::read(&file).map_err(|e| io(file.display(), e))?;
                    match bucket.put_if_absent(&path, Bytes::from(bytes)).await {
                        Ok(_) => report.objects_put_back += 1,
                        Err(loams_store::StoreError::AlreadyExists { .. }) => {}
                        Err(e) => return Err(io(&path, e)),
                    }
                }
            }
            // The embedded metastore is restored by `restore_raft` while
            // no node has it open; a caller that already did passes `None`.
            "raft" => match raft_dir {
                Some(target) => files::replace_dir(&source, target)?,
                None => continue,
            },
            "durable" => {
                let durable = cx.durable_cx()?;
                let db = durable.path();
                if let Some(parent) = db.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(parent.display(), e))?;
                }
                let staged = durable.new_path();
                std::fs::copy(source.join("default.db"), &staged)
                    .map_err(|e| io(staged.display(), e))?;
                for suffix in ["-wal", "-shm"] {
                    let mut stale = db.as_os_str().to_owned();
                    stale.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(stale));
                }
                std::fs::rename(&staged, db).map_err(|e| io(db.display(), e))?;
            }
            "durable_local" => {
                let target = durable_local(cx).ok_or_else(|| {
                    SnapshotError::Io("restoring the durable store needs --data-dir".into())
                })?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(parent.display(), e))?;
                }
                let mut staged = target.as_os_str().to_owned();
                staged.push(".restore-new");
                let staged = PathBuf::from(staged);
                std::fs::copy(source.join(DURABLE_LOCAL_FILE), &staged)
                    .map_err(|e| io(staged.display(), e))?;
                std::fs::rename(&staged, &target).map_err(|e| io(target.display(), e))?;
            }
            "house" => files::replace_dir(&source, cx.house_cx()?.dir())?,
            "pg_control" => files::replace_dir(&source, cx.pg_control_cx()?.dir())?,
            "agentd" => {
                let target = agentd_dir.ok_or_else(|| {
                    SnapshotError::Io("restoring agentd needs its directory".into())
                })?;
                files::replace_dir(&source, target)?;
            }
            // Restored by the CLI's helper; its version was checked above.
            "tikv" => {}
            "postgres" => {
                let file = source.join("branches.json");
                let body = std::fs::read(&file).map_err(|e| io(file.display(), e))?;
                let branches: postgres::Branches =
                    serde_json::from_slice(&body).map_err(|e| io(file.display(), e))?;
                report.postgres_branches = branches.branches;
            }
            _ => continue,
        }
        report.restored.push(component.store.clone());
    }
    Ok(report)
}
