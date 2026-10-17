//! The migration ledger and lease (UP1 Task 2), on a one-node embedded Raft
//! metastore. The TiKV backend serves the same trait calls; its
//! `pointers_with_prefix` is covered by the metastore conformance suite.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use loams_common::meta::{Consistency, MetaStore};
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_migrate::formats::{REGISTRY, Store};
use loams_migrate::ledger::{
    Error, LEASE_KEY, LEASE_TTL, LEDGER_PREFIX, MAX_CURSOR, STACK_KEY, SYSTEM_NAMESPACE,
};
use loams_migrate::{
    Class, Ledger, LedgerEntry, LedgerError, LedgerState, MigrationLease, StackRecord,
};
use loams_store::Store as ObjectStore;
use tempfile::TempDir;

struct Single {
    meta: Arc<dyn MetaStore>,
    _node: MetaNode,
    _router: Router,
    _dir: TempDir,
}

async fn metastore() -> Single {
    let router = Router::new();
    let dir = TempDir::new().expect("temp dir");
    let config = MetaConfig::new(1, dir.path(), ObjectStore::in_memory());
    let node = MetaNode::start(config, &router).await.expect("start node");
    node.initialize(vec![1]).await.expect("initialize");
    node.wait_for_leader(Duration::from_secs(20))
        .await
        .expect("a leader");
    let client = MetaClient::new(
        node.clone(),
        vec![node.clone()],
        Arc::new(SystemClock),
        MetaClientConfig::default(),
    );
    Single {
        meta: Arc::new(client),
        _node: node,
        _router: router,
        _dir: dir,
    }
}

fn entry(seq: u32, id: &str) -> LedgerEntry {
    LedgerEntry {
        seq,
        id: id.to_string(),
        release_from: "0.1.0-preview.2".to_string(),
        release_to: "0.1.0".to_string(),
        store: Store::Meta,
        class: Class::Reversible,
        state: LedgerState::Pending,
        cursor: None,
        attempts: 0,
        started_at: None,
        finished_at: None,
        error: None,
        image_digest: Some(format!("sha256:{}", "a".repeat(64))),
        code_digest: [7; 32],
    }
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

#[tokio::test]
async fn ledger_append_and_read_back() {
    let db = metastore().await;
    let ledger = Ledger::open(db.meta.clone()).await.expect("open");
    let lease = MigrationLease::acquire(db.meta.clone(), "migrate-a", LEASE_TTL)
        .await
        .expect("lease");
    assert!(ledger.entries().await.expect("entries").is_empty());
    assert_eq!(ledger.next_seq().await.expect("seq"), 1);
    let first = entry(1, "0000-adopt-preview-2");
    let mut second = entry(2, "0001-safekeeper-volume");
    // The largest entry the bounds allow still fits a pointer value.
    second.cursor = Some(vec![0xff; MAX_CURSOR]);
    second.error = Some(LedgerError::new("migration_failed", "é".repeat(400)));
    ledger.append(&lease, &first).await.expect("append 1");
    ledger.append(&lease, &second).await.expect("append 2");
    let read = ledger.entries().await.expect("entries");
    assert_eq!(read.len(), 2);
    assert_eq!(read[0], first);
    assert_eq!(read[1].cursor, second.cursor);
    assert!(read[1].error.as_ref().expect("error").message.len() <= 256);
    // Appending out of order is refused.
    let err = ledger
        .append(&lease, &entry(5, "0005-x"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::OutOfOrder {
                seq: 5,
                expected: 3
            }
        ),
        "{err}"
    );
    // A cursor over the cap is refused before anything is written.
    let mut long = entry(3, "0002-long");
    long.cursor = Some(vec![0; MAX_CURSOR + 1]);
    assert!(matches!(
        ledger.append(&lease, &long).await.unwrap_err(),
        Error::CursorTooLong { .. }
    ));
    // Updating keeps the id and sequence and checkpoints the cursor.
    let updated = ledger
        .update(&lease, 1, |e| {
            e.state = LedgerState::Running;
            e.cursor = Some(b"page-7".to_vec());
            e.attempts += 1;
            e.started_at = Some(at(1_792_000_000));
            e.id = "renamed".to_string();
        })
        .await
        .expect("update");
    assert_eq!(updated.id, "0000-adopt-preview-2");
    assert_eq!(ledger.entries().await.expect("entries")[0], updated);
    // A second open finds the same ledger.
    let again = Ledger::open_existing(db.meta.clone())
        .await
        .expect("open")
        .expect("exists");
    assert_eq!(again.entries().await.expect("entries").len(), 2);
}

#[tokio::test]
async fn ledger_update_rejected_with_stale_epoch() {
    let db = metastore().await;
    let ledger = Ledger::open(db.meta.clone()).await.expect("open");
    let old = MigrationLease::acquire(db.meta.clone(), "migrate-a", Duration::from_secs(1))
        .await
        .expect("lease");
    ledger
        .append(&old, &entry(1, "0000-a"))
        .await
        .expect("append");
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    let new = MigrationLease::acquire(db.meta.clone(), "migrate-b", LEASE_TTL)
        .await
        .expect("taken over after expiry");
    assert!(new.epoch() > old.epoch());
    let err = ledger
        .update(&old, 1, |e| e.state = LedgerState::Done)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Fenced), "{err}");
    let err = ledger.append(&old, &entry(2, "0001-b")).await.unwrap_err();
    assert!(matches!(err, Error::Fenced), "{err}");
    assert_eq!(
        ledger.entries().await.expect("entries")[0].state,
        LedgerState::Pending,
        "nothing written under the stale epoch"
    );
    ledger
        .update(&new, 1, |e| e.state = LedgerState::Done)
        .await
        .expect("the new holder writes");
}

#[tokio::test]
async fn lease_excludes_second_holder() {
    let db = metastore().await;
    let held = MigrationLease::acquire(db.meta.clone(), "migrate-a", LEASE_TTL)
        .await
        .expect("lease");
    let err = MigrationLease::acquire(db.meta.clone(), "migrate-b", LEASE_TTL)
        .await
        .unwrap_err();
    match err {
        Error::LeaseHeld { owner } => assert_eq!(owner, "migrate-a"),
        other => panic!("{other}"),
    }
    held.renew(LEASE_TTL).await.expect("renew");
    held.release().await.expect("release");
    MigrationLease::acquire(db.meta.clone(), "migrate-b", LEASE_TTL)
        .await
        .expect("free after release");
}

#[tokio::test]
async fn lease_expires_and_is_taken_over() {
    let db = metastore().await;
    let old = MigrationLease::acquire(db.meta.clone(), "migrate-a", Duration::from_secs(1))
        .await
        .expect("lease");
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    let new = MigrationLease::acquire(db.meta.clone(), "migrate-b", LEASE_TTL)
        .await
        .expect("taken over");
    assert_eq!(new.epoch(), old.epoch() + 1);
    assert!(matches!(
        old.renew(LEASE_TTL).await.unwrap_err(),
        Error::Fenced
    ));
    let lease = db
        .meta
        .lease(Consistency::Linearizable, LEASE_KEY)
        .await
        .expect("read")
        .expect("lease");
    assert_eq!(lease.owner.as_deref(), Some("migrate-b"));
}

#[tokio::test]
async fn stack_record_and_last_entry_commit_together() {
    let db = metastore().await;
    let ledger = Ledger::open(db.meta.clone()).await.expect("open");
    let lease = MigrationLease::acquire(db.meta.clone(), "migrate-a", LEASE_TTL)
        .await
        .expect("lease");
    let target = "0.1.0";
    let record = StackRecord::current(target, REGISTRY, at(1_792_000_100), None);
    ledger
        .append(&lease, &entry(1, "0000-a"))
        .await
        .expect("append");
    ledger
        .append(&lease, &entry(2, "0001-b"))
        .await
        .expect("append");
    ledger
        .update(&lease, 1, |e| e.state = LedgerState::Done)
        .await
        .expect("done");
    assert!(!ledger.awaiting_stack(target).await.expect("check"));
    // A crash between the last entry's Done and the stack record: every entry
    // is done, the stack is not written, and a rerun is owed.
    ledger
        .update(&lease, 2, |e| e.state = LedgerState::Done)
        .await
        .expect("done");
    assert!(ledger.stack().await.expect("stack").is_none());
    assert!(ledger.awaiting_stack(target).await.expect("check"));
    // The rerun's commit point.
    ledger.set_stack(&lease, &record).await.expect("stack");
    assert_eq!(ledger.stack().await.expect("stack"), Some(record.clone()));
    assert!(!ledger.awaiting_stack(target).await.expect("check"));
    // `finish` does both, in that order.
    ledger
        .append(&lease, &entry(3, "0002-c"))
        .await
        .expect("append");
    let next = StackRecord::current("0.1.0-preview.4", REGISTRY, at(1_792_000_200), None);
    let mut third = entry(3, "0002-c");
    third.release_to = "0.1.0-preview.4".to_string();
    ledger
        .update(&lease, 3, |e| e.release_to = third.release_to.clone())
        .await
        .expect("retarget");
    ledger
        .finish(&lease, 3, at(1_792_000_201), &next)
        .await
        .expect("finish");
    let entries = ledger.entries().await.expect("entries");
    assert_eq!(entries[2].state, LedgerState::Done);
    assert_eq!(entries[2].finished_at, Some(at(1_792_000_201)));
    assert_eq!(ledger.stack().await.expect("stack"), Some(next));
}

#[tokio::test]
async fn ledger_keys_live_under_system_upgrade_prefix() {
    let db = metastore().await;
    let ledger = Ledger::open(db.meta.clone()).await.expect("open");
    let lease = MigrationLease::acquire(db.meta.clone(), "migrate-a", LEASE_TTL)
        .await
        .expect("lease");
    ledger
        .append(&lease, &entry(1, "0000-a"))
        .await
        .expect("append");
    let record = StackRecord::current("0.1.0", REGISTRY, at(1), None);
    ledger.set_stack(&lease, &record).await.expect("stack");
    let ns = db
        .meta
        .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
        .await
        .expect("read")
        .expect("_system exists");
    assert_eq!(ns.id, ledger.namespace());
    let all = db
        .meta
        .pointers_with_prefix(Consistency::Linearizable, ns.id, "")
        .await
        .expect("list");
    let keys: Vec<&str> = all.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            format!("{LEDGER_PREFIX}00000001-0000-a").as_str(),
            STACK_KEY
        ]
    );
    assert!(keys.iter().all(|k| k.starts_with("upgrade/")));
    assert!(LEASE_KEY.starts_with("_system/upgrade/"));
    // The stack record with the whole registry fits a pointer value.
    assert!(all.iter().all(|(_, p)| p.value.len() <= 1024));
}

#[tokio::test]
async fn ledger_shares_system_with_the_auth_directory() {
    let db = metastore().await;
    // The auth directory creates `_system` first on a preview.2 stack.
    let ns = db
        .meta
        .create_namespace(SYSTEM_NAMESPACE)
        .await
        .expect("create");
    let ledger = Ledger::open(db.meta.clone()).await.expect("open");
    assert_eq!(ledger.namespace(), ns);
    assert!(
        Ledger::open_existing(db.meta.clone())
            .await
            .expect("open")
            .is_some()
    );
}
