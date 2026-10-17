//! Snapshots and restores of the Loams-owned stores (UP1 Task 8).

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use bytes::Bytes;
use loams_common::meta::{
    Consistency, MetaStore, PRUNED, PointerCas, SNAPSHOT_ROOT_PREFIX, SYSTEM_NAMESPACE,
    snapshot_gc_floor,
};
use loams_migrate::snapshot::{
    self, MANIFEST_FILE, SnapshotError, SnapshotManifest, SnapshotOptions,
};
use loams_store::Store as ObjectStore;
use support::{Stack, read_dir};
use tempfile::TempDir;

/// A stack with something in every Loams-owned store.
struct Stores {
    stack: Stack,
    bucket_dir: TempDir,
    bucket: ObjectStore,
    house: TempDir,
    pg: TempDir,
    raft: TempDir,
}

impl Stores {
    async fn new() -> Stores {
        let stack = Stack::new().await;
        stack.stamp("0.1.0-preview.2").await;
        let bucket_dir = TempDir::new().expect("bucket");
        let bucket = ObjectStore::from_url(
            &format!("file://{}/", bucket_dir.path().display()),
            Vec::<(String, String)>::new(),
        )
        .expect("store");
        // A pointer naming an object, and a Git catalog document.
        let ns = stack.meta.create_namespace("app").await.expect("ns");
        bucket
            .put(
                "graphs/1/catalog/01A.json",
                Bytes::from_static(b"{\"graphs\":{}}"),
            )
            .await
            .expect("put");
        bucket
            .put(
                &format!("ns/{ns}/repos/_names/site.json"),
                Bytes::from_static(b"{\"v\":1}"),
            )
            .await
            .expect("put");
        bucket
            .put(
                "ns/1/collections/1/lance/data/big.lance",
                Bytes::from(vec![7u8; 4096]),
            )
            .await
            .expect("put");
        MetaStore::cas_pointer(
            &*stack.meta,
            PointerCas {
                namespace: ns,
                key: "graph-catalog".to_string(),
                expected: None,
                value: "graphs/1/catalog/01A.json".to_string(),
                fence: None,
                fresh: None,
            },
        )
        .await
        .into_result()
        .expect("pointer");
        // Durable.
        let durable = stack.data.path().join("durable");
        std::fs::create_dir_all(&durable).expect("mkdir");
        rusqlite::Connection::open(durable.join("default.db"))
            .expect("db")
            .execute_batch("CREATE TABLE promises (id TEXT); INSERT INTO promises VALUES ('p1');")
            .expect("seed");
        // House, pg-control, the embedded metastore's directory.
        let house = TempDir::new().expect("house");
        std::fs::write(house.path().join("catalog.sqlite"), b"house catalog").expect("w");
        std::fs::write(house.path().join("house.key"), b"secret-house-key").expect("w");
        std::fs::create_dir_all(house.path().join("spool")).expect("mkdir");
        std::fs::write(house.path().join("spool/a"), b"spooled").expect("w");
        let pg = TempDir::new().expect("pg");
        std::fs::write(pg.path().join("pg-control.redb"), b"records").expect("w");
        let raft = TempDir::new().expect("raft");
        std::fs::write(raft.path().join("meta.redb"), b"raft log").expect("w");
        Stores {
            stack,
            bucket_dir,
            bucket,
            house,
            pg,
            raft,
        }
    }

    fn cx(&self) -> loams_migrate::Cx {
        let mut cx = self.stack.cx();
        cx.bucket = Some(self.bucket.clone());
        cx.house_dir = Some(self.house.path().to_path_buf());
        cx.pg_control_dir = Some(self.pg.path().to_path_buf());
        cx
    }

    fn opts(&self) -> SnapshotOptions {
        SnapshotOptions {
            target_release: "0.1.0".to_string(),
            raft_dir: Some(self.raft.path().to_path_buf()),
            ..SnapshotOptions::default()
        }
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("meta").permissions().mode() & 0o777
}

#[tokio::test]
async fn snapshot_manifest_written_last() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let dir = to.path().join(&manifest.id);
    let written = std::fs::metadata(dir.join(MANIFEST_FILE))
        .expect("manifest")
        .modified()
        .expect("mtime");
    for (relative, path) in walk(&dir) {
        if relative == MANIFEST_FILE {
            continue;
        }
        let modified = std::fs::metadata(&path)
            .expect("meta")
            .modified()
            .expect("mtime");
        assert!(modified <= written, "{relative} written after the manifest");
    }
    let stores: Vec<&str> = manifest
        .components
        .iter()
        .map(|c| c.store.as_str())
        .collect();
    assert_eq!(stores, ["bucket", "raft", "durable", "house", "pg_control"]);
    assert_eq!(manifest.release.as_deref(), Some("0.1.0-preview.2"));
    // Recorded as a GC root once complete.
    let floor = snapshot_gc_floor(&*s.stack.meta).await.expect("floor");
    assert_eq!(floor, Some(manifest.created_at_ms));
}

fn walk(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        if path.is_dir() {
            for (k, v) in walk(&path) {
                out.push((format!("{name}/{k}"), v));
            }
        } else {
            out.push((name, path));
        }
    }
    out
}

#[tokio::test]
async fn restore_after_crash_mid_snapshot_refused() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let dir = to.path().join(&manifest.id);
    // A crash before the manifest: the directory has copies but no manifest.
    std::fs::remove_file(dir.join(MANIFEST_FILE)).expect("remove");
    let before = read_dir(s.house.path());
    let err = snapshot::restore(&s.cx(), &dir, Some(s.raft.path()), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, SnapshotError::Incomplete(_)), "{err}");
    assert!(err.to_string().starts_with("snapshot_incomplete"), "{err}");
    assert_eq!(read_dir(s.house.path()), before, "nothing restored");
    // And a component that does not match its digest.
    let other = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let other_dir = to.path().join(&other.id);
    std::fs::write(other_dir.join("house/catalog.sqlite"), b"tampered").expect("w");
    let err = snapshot::restore(&s.cx(), &other_dir, Some(s.raft.path()), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, SnapshotError::Corrupt(_)), "{err}");
    // A listing marks the incomplete one.
    let listed = snapshot::list(to.path()).expect("list");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|l| l.id == manifest.id && !l.complete));
}

#[tokio::test]
async fn snapshot_then_restore_round_trips_every_loams_store() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let house_before = read_dir(s.house.path());
    let pg_before = read_dir(s.pg.path());
    let raft_before = read_dir(s.raft.path());
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    // The upgrade changes everything after the snapshot.
    std::fs::write(s.house.path().join("catalog.sqlite"), b"migrated").expect("w");
    std::fs::write(s.house.path().join("FORMAT"), b"house.dir 2\n").expect("w");
    std::fs::write(s.pg.path().join("pg-control.redb"), b"new records").expect("w");
    std::fs::write(s.raft.path().join("meta.redb"), b"new raft").expect("w");
    rusqlite::Connection::open(s.stack.data.path().join("durable/default.db"))
        .expect("db")
        .execute_batch("INSERT INTO promises VALUES ('p2');")
        .expect("write");
    s.bucket
        .delete("graphs/1/catalog/01A.json")
        .await
        .expect("a replaced object is deleted");
    let report = snapshot::restore(
        &s.cx(),
        &to.path().join(&manifest.id),
        Some(s.raft.path()),
        None,
        None,
    )
    .await
    .expect("restore");
    assert_eq!(
        report.restored,
        ["bucket", "raft", "durable", "house", "pg_control"]
    );
    assert_eq!(read_dir(s.house.path()), house_before);
    assert_eq!(read_dir(s.pg.path()), pg_before);
    assert_eq!(read_dir(s.raft.path()), raft_before);
    let ids: Vec<String> =
        rusqlite::Connection::open(s.stack.data.path().join("durable/default.db"))
            .expect("db")
            .prepare("SELECT id FROM promises ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
    assert_eq!(ids, ["p1"]);
    // The pointer's object came back; the immutable data object was never
    // copied (it is pinned instead).
    let (bytes, _) = s
        .bucket
        .get("graphs/1/catalog/01A.json")
        .await
        .expect("back");
    assert_eq!(bytes.as_ref(), b"{\"graphs\":{}}");
    assert_eq!(report.objects_put_back, 1);
    let copied = read_dir(&to.path().join(&manifest.id).join("bucket"));
    assert!(
        copied.keys().any(|k| k.ends_with("_names/site.json")),
        "{copied:?}"
    );
    assert!(!copied.keys().any(|k| k.ends_with(".lance")), "{copied:?}");
    drop(s.bucket_dir);
}

#[tokio::test]
async fn gc_between_snapshot_and_restore_keeps_pinned_objects() {
    // The floor every GC honours (crates/loams-log's
    // `gc_between_snapshot_and_restore_keeps_pinned_objects` runs the log
    // GC against it): the oldest kept snapshot's time, and none once pruned.
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    assert_eq!(
        snapshot_gc_floor(&*s.stack.meta).await.expect("floor"),
        None
    );
    let first = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("first");
    let second = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("second");
    let floor = snapshot_gc_floor(&*s.stack.meta).await.expect("floor");
    assert_eq!(floor, Some(first.created_at_ms), "the oldest kept snapshot");
    assert!(loams_common::meta::pinned_by(floor, first.created_at_ms));
    assert!(!loams_common::meta::pinned_by(
        floor,
        first.created_at_ms + 1
    ));
    snapshot::prune(&s.cx(), to.path(), &first.id)
        .await
        .expect("prune");
    assert_eq!(
        snapshot_gc_floor(&*s.stack.meta).await.expect("floor"),
        Some(second.created_at_ms)
    );
    snapshot::prune(&s.cx(), to.path(), &second.id)
        .await
        .expect("prune");
    assert_eq!(
        snapshot_gc_floor(&*s.stack.meta).await.expect("floor"),
        None
    );
}

#[tokio::test]
async fn prune_removes_gc_root_then_files() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    snapshot::prune(&s.cx(), to.path(), &manifest.id)
        .await
        .expect("prune");
    assert!(!to.path().join(&manifest.id).exists());
    let ns = s
        .stack
        .meta
        .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
        .await
        .expect("read")
        .expect("_system");
    let root = s
        .stack
        .meta
        .pointer(
            Consistency::Linearizable,
            ns.id,
            &format!("{SNAPSHOT_ROOT_PREFIX}{}", manifest.id),
        )
        .await
        .expect("read")
        .expect("root");
    assert_eq!(root.value, PRUNED);
    // Pruning again is a no-op.
    snapshot::prune(&s.cx(), to.path(), &manifest.id)
        .await
        .expect("prune again");
}

#[tokio::test]
async fn sqlite_snapshot_uses_backup_api_and_is_consistent() {
    let s = Stores::new().await;
    let db = s.stack.data.path().join("durable/default.db");
    // A writer holds the database open in WAL mode with uncheckpointed
    // writes: the backup API still copies a consistent database.
    let writer = rusqlite::Connection::open(&db).expect("db");
    writer
        .execute_batch("PRAGMA journal_mode = WAL; INSERT INTO promises VALUES ('p2');")
        .expect("wal write");
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    writer
        .execute_batch("INSERT INTO promises VALUES ('p3');")
        .expect("after");
    let copy = to.path().join(&manifest.id).join("durable/default.db");
    let count: i64 = rusqlite::Connection::open(&copy)
        .expect("copy")
        .query_row("SELECT count(*) FROM promises", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 2, "p1 and p2, not the write after the snapshot");
    let durable = manifest
        .components
        .iter()
        .find(|c| c.store == "durable")
        .expect("durable");
    assert_eq!(durable.method, "sqlite_backup");
}

#[tokio::test]
async fn snapshot_dir_is_0700_and_files_0600() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let dir = to.path().join(&manifest.id);
    assert_eq!(mode(to.path()), 0o700);
    assert_eq!(mode(&dir), 0o700);
    for (relative, path) in walk(&dir) {
        assert_eq!(mode(&path), 0o600, "{relative}");
        let parent = path.parent().expect("parent");
        assert_eq!(mode(parent), 0o700, "{}", parent.display());
    }
}

#[tokio::test]
async fn snapshot_listing_has_no_secret_values() {
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let listed = snapshot::list(to.path()).expect("list");
    let json = serde_json::to_string(&listed).expect("json");
    for secret in ["secret-house-key", "records", "raft log", "p1", "graphs"] {
        assert!(
            !json.contains(secret),
            "the listing shows {secret:?}: {json}"
        );
    }
    assert!(json.contains("sha256"));
    // The manifest itself holds no file contents either.
    let manifest = SnapshotManifest::read(&to.path().join(&listed[0].id)).expect("read");
    let text = serde_json::to_string(&manifest).expect("json");
    assert!(!text.contains("secret-house-key"));
}

#[tokio::test]
async fn estimate_within_20_percent_of_actual() {
    let s = Stores::new().await;
    let estimate = snapshot::estimate_bytes(&s.cx(), &s.opts());
    let to = TempDir::new().expect("to");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &s.opts())
        .await
        .expect("snapshot");
    let actual: u64 = manifest
        .components
        .iter()
        .filter(|c| c.store != "bucket")
        .map(|c| c.bytes)
        .sum();
    let diff = estimate.abs_diff(actual) as f64;
    assert!(
        diff <= actual as f64 * 0.2,
        "estimate {estimate}, actual {actual}"
    );
}

/// UP1 Task 9: the CLI's helper writes TiKV's backup into `<snapshot>/tikv`
/// under the id it picked; the manifest records it with TiKV's version and
/// digest, and a restore refuses a cluster at another version (or none).
#[tokio::test]
async fn tikv_component_restores_only_at_its_version() {
    use loams_migrate::snapshot::tikv::TikvMethod;
    let s = Stores::new().await;
    let to = TempDir::new().expect("to");
    let id = "01JB0000000000000000000000".to_string();
    let opts = SnapshotOptions {
        id: Some(id.clone()),
        tikv: Some((TikvMethod::Br, "v8.5.8".to_string())),
        ..s.opts()
    };
    // Without the helper's backup, the snapshot is refused.
    let err = snapshot::snapshot(&s.cx(), to.path(), &opts)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("TiKV br copy is missing"), "{err}");
    let backup = to.path().join(&id).join("tikv");
    std::fs::create_dir_all(&backup).expect("mkdir");
    std::fs::write(backup.join("backupmeta"), b"br's meta").expect("w");
    let manifest = snapshot::snapshot(&s.cx(), to.path(), &opts)
        .await
        .expect("snapshot");
    assert_eq!(manifest.id, id);
    let tikv = manifest
        .components
        .iter()
        .find(|c| c.store == "tikv")
        .expect("a tikv component");
    assert_eq!(tikv.method, "br");
    assert_eq!(tikv.version.as_deref(), Some("v8.5.8"));
    assert_eq!(tikv.sha256.len(), 64, "{tikv:?}");
    let dir = to.path().join(&id);
    let restore = |version: Option<&'static str>| {
        let cx = s.cx();
        let dir = dir.clone();
        let raft = s.raft.path().to_path_buf();
        async move { snapshot::restore(&cx, &dir, Some(&raft), None, version).await }
    };
    let err = restore(None).await.unwrap_err();
    assert!(err.to_string().contains("--tikv-version"), "{err}");
    let err = restore(Some("v8.5.9")).await.unwrap_err();
    assert!(err.to_string().contains("component_downgrade"), "{err}");
    let report = restore(Some("8.5.8")).await.expect("restore");
    assert!(report.restored.contains(&"tikv".to_string()), "{report:?}");
    // A changed backup file is caught before anything is restored.
    std::fs::write(backup.join("backupmeta"), b"tampered").expect("w");
    let err = restore(Some("v8.5.8")).await.unwrap_err();
    assert!(matches!(err, SnapshotError::Corrupt(_)), "{err}");
}
