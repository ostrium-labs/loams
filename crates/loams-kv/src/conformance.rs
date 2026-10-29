//! The seam's conformance cases (LV1 plan Task 21): the transaction
//! semantics every backend gives, held by the same tests on each.
//!
//! [`kv_conformance!`](crate::kv_conformance) expands one `#[tokio::test]`
//! per case for a [`Factory`]:
//!
//! ```ignore
//! loams_kv::kv_conformance!(embedded, loams_kv::testing::embedded_factory(env!("CARGO_TARGET_TMPDIR")));
//! loams_kv::kv_conformance!(tikv, loams_kv::testing::tikv_factory());
//! ```
//!
//! A case gets its stores from the factory, which yields `None` (the case
//! then returns, after the factory's `skipped:` line) when the backend is
//! not available. The cases cover snapshot isolation (later commits are
//! invisible, first committer wins, a locked read conflicts with a later
//! write, a committed lock makes later committers of the key conflict, inserts of present keys fail), scans, root isolation, and the
//! runner: its retries, attempt budget, deadline, fault points and commit
//! tokens, GC barriers, the paged [`Scanner`](crate::Scanner), and the
//! one-shot operations of [`ops`](crate::ops).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::testing::{Factory, Spec};
#[cfg(feature = "faults")]
use crate::{Fault, FaultPlan, FaultPoint};
use crate::{KvError, Store, Ts, TxnError, TxnOptions};

/// Expands one `#[tokio::test]` per conformance case into `mod $backend`,
/// each case calling `$factory` (an expression of type
/// [`Factory`](crate::testing::Factory), evaluated once per case).
#[macro_export]
macro_rules! kv_conformance {
    ($backend:ident, $factory:expr) => {
        mod $backend {
            #[allow(unused_imports)]
            use super::*;
            $crate::kv_conformance!(@cases $factory;
                snapshot_ignores_later_commits,
                first_committer_wins,
                locked_read_conflicts_with_write,
                locked_key_conflicts_with_later_writers,
                scan_bounds_and_order,
                insert_existing_key_fails,
                roots_are_isolated,
                read_only_commit_is_at_start_ts,
                runner_retries_conflict_and_not_applied,
                runner_stops_at_max_attempts,
                runner_does_not_retry_fatal,
                runner_deadline_passes,
                pessimistic_read_modify_write_loses_no_update,
                pessimistic_second_holder_waits,
                pessimistic_lock_wait_times_out_as_conflict,
                pessimistic_and_optimistic_conflict,
                fault_points_and_commit_tokens,
                barrier_holds_old_snapshots_and_refuses_below_safe_point,
                scanner_pages_forward,
                scanner_pages_reverse,
                scanner_sees_own_writes,
                scanner_bounds,
                one_shot_put_get_delete,
                one_shot_insert_existing_fails,
                compare_and_put_matches_and_mismatches,
                compare_and_put_absent_and_delete
            );
        }
    };
    (@cases $factory:expr; $($case:ident),* $(,)?) => {
        $(
            #[::tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $case() {
                $crate::conformance::$case($factory).await;
            }
        )*
    };
}

fn kv(k: &str, v: &str) -> (Vec<u8>, Vec<u8>) {
    (k.as_bytes().to_vec(), v.as_bytes().to_vec())
}

async fn put(store: &Store, key: &'static [u8], value: &'static [u8]) -> Ts {
    store
        .run(TxnOptions::new("kv.conformance.put"), move |txn| {
            Box::pin(async move { txn.put(key, value.to_vec()).await })
        })
        .await
        .expect("a put commits")
        .commit_ts
}

/// A write committed after a snapshot's timestamp is invisible to it, to
/// point reads and scans alike; a later snapshot sees it.
pub async fn snapshot_ignores_later_commits(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"v1").await;
    let at = store.now().await.expect("now");
    let mut snap = store.snapshot(at).await.expect("a snapshot");
    let later = put(&store, b"k", b"v2").await;
    put(&store, b"l", b"new").await;
    assert!(later > at, "the later commit is after the snapshot");
    assert_eq!(snap.ts(), at);
    assert_eq!(snap.get(b"k").await.expect("get"), Some(b"v1".to_vec()));
    assert_eq!(snap.get(b"l").await.expect("get"), None);
    assert_eq!(
        snap.scan(b"", None, 10).await.expect("scan"),
        vec![kv("k", "v1")]
    );
    assert_eq!(
        snap.batch_get([b"l".as_slice(), b"k"])
            .await
            .expect("batch get"),
        vec![kv("k", "v1")]
    );
    let mut fresh = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(fresh.get(b"k").await.expect("get"), Some(b"v2".to_vec()));
    assert_eq!(
        fresh.scan(b"", None, 10).await.expect("scan"),
        vec![kv("k", "v2"), kv("l", "new")]
    );
    // A transaction reads at its start timestamp too.
    let seen = store
        .run(TxnOptions::new("kv.conformance.read"), |txn| {
            Box::pin(async move { txn.get(b"k").await })
        })
        .await
        .expect("a read");
    assert_eq!(seen.value, Some(b"v2".to_vec()));
}

/// Two transactions write one key, both started before either commits:
/// exactly one commits and the other fails with `Conflict`.
pub async fn first_committer_wins(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"0").await;
    let both = Arc::new(tokio::sync::Barrier::new(2));
    let writer = |value: &'static [u8]| {
        let store = store.clone();
        let both = both.clone();
        async move {
            let mut opts = TxnOptions::new("kv.conformance.fcw");
            opts.max_attempts = 1;
            store
                .run(opts, move |txn| {
                    let both = both.clone();
                    Box::pin(async move {
                        txn.get(b"k").await?;
                        both.wait().await;
                        txn.put(b"k", value.to_vec()).await
                    })
                })
                .await
        }
    };
    let (a, b) = tokio::join!(tokio::spawn(writer(b"a")), tokio::spawn(writer(b"b")));
    let (a, b) = (a.expect("joined"), b.expect("joined"));
    let won = match (&a, &b) {
        (Ok(_), Err(TxnError::Conflict)) => b"a",
        (Err(TxnError::Conflict), Ok(_)) => b"b",
        other => panic!("expected one commit and one conflict, got {other:?}"),
    };
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"k").await.expect("get"), Some(won.to_vec()));
}

/// `lock_keys(k)` in T1, then a write of `k` in T2 that commits first: T1
/// conflicts, though it never wrote `k`.
pub async fn locked_read_conflicts_with_write(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"0").await;
    let locked = Arc::new(tokio::sync::Barrier::new(2));
    let written = Arc::new(tokio::sync::Barrier::new(2));
    let t1 = {
        let store = store.clone();
        let (locked, written) = (locked.clone(), written.clone());
        tokio::spawn(async move {
            let mut opts = TxnOptions::new("kv.conformance.lock");
            opts.max_attempts = 1;
            store
                .run(opts, move |txn| {
                    let (locked, written) = (locked.clone(), written.clone());
                    Box::pin(async move {
                        let k = txn.get(b"k").await?;
                        txn.lock_keys([b"k".as_slice()]).await?;
                        locked.wait().await;
                        written.wait().await;
                        txn.put(b"other", k.unwrap_or_default()).await
                    })
                })
                .await
        })
    };
    locked.wait().await;
    put(&store, b"k", b"1").await;
    written.wait().await;
    let t1 = t1.await.expect("joined");
    assert!(matches!(t1, Err(TxnError::Conflict)), "{t1:?}");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"other").await.expect("get"), None);
}

/// What the second transaction of [`lock_holder_then`] does to the key.
#[derive(Debug, Clone, Copy)]
enum Second {
    Put,
    Lock,
}

/// T1 locks `key` (and writes `key/x`, so it is not read-only); T2 starts
/// before T1 commits and then puts or locks `key` (and writes `key/y`);
/// T1 commits first, and T2's single attempt after it. Returns T2's
/// outcome. Each round uses keys of its own: on TiKV a conflicting T2 may
/// leave a lock on its other key that a later round would wait out.
async fn lock_holder_then(store: &Store, key: &[u8], second: Second) -> Result<(), TxnError> {
    let key: Arc<[u8]> = key.into();
    let started = Arc::new(tokio::sync::Barrier::new(2));
    let ended = Arc::new(tokio::sync::Barrier::new(2));
    let t2 = {
        let store = store.clone();
        let (started, ended, key) = (started.clone(), ended.clone(), key.clone());
        tokio::spawn(async move {
            let mut opts = TxnOptions::new("kv.conformance.lock_second");
            opts.max_attempts = 1;
            store
                .run(opts, move |txn| {
                    let (started, ended, key) = (started.clone(), ended.clone(), key.clone());
                    Box::pin(async move {
                        started.wait().await;
                        ended.wait().await;
                        second.apply(txn, &key).await
                    })
                })
                .await
                .map(|_| ())
        })
    };
    started.wait().await;
    lock_once(store, &key, false)
        .await
        .expect("the locker commits");
    ended.wait().await;
    t2.await.expect("joined")
}

impl Second {
    /// Puts or locks `key`, and writes `key/y`.
    async fn apply(self, txn: &mut crate::Txn, key: &[u8]) -> Result<(), TxnError> {
        match self {
            Second::Put => txn.put(key, b"t2".to_vec()).await?,
            Second::Lock => txn.lock_keys([key]).await?,
        }
        txn.put(&[key, b"/y"].concat(), b"t2".to_vec()).await
    }
}

/// One transaction that locks `key` and writes `key/x`; with `rollback`,
/// its body then fails, so it rolls back.
async fn lock_once(store: &Store, key: &[u8], rollback: bool) -> Result<(), TxnError> {
    let key: Arc<[u8]> = key.into();
    let mut opts = TxnOptions::new("kv.conformance.lock_first");
    opts.max_attempts = 1;
    store
        .run(opts, |txn| {
            let key = key.clone();
            Box::pin(async move {
                txn.lock_keys([&*key]).await?;
                txn.put(&[&*key, b"/x"].concat(), b"t1".to_vec()).await?;
                if rollback {
                    return Err(TxnError::Fatal("roll back".into()));
                }
                Ok(())
            })
        })
        .await
        .map(|_| ())
}

/// A key T1 locked (`lock_keys`, without writing it) makes a concurrent T2
/// that writes or locks it and commits after T1 conflict, whether the key
/// is present or absent (TiKV's committed `Lock` record). A rolled-back
/// lock is no lock: a transaction after it writes the key. (One concurrent
/// with the rolled-back locker may conflict on TiKV, whose rollback leaves
/// a record, and commits on the embedded store; neither is asserted.) A
/// transaction that starts after the locker committed does not conflict.
/// A lock is no value: reads skip it, and an insert of an absent key that
/// was only locked succeeds.
pub async fn locked_key_conflicts_with_later_writers(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    for (i, second) in [Second::Put, Second::Lock].into_iter().enumerate() {
        for present in [true, false] {
            let key = format!("k{i}{}", if present { "p" } else { "a" }).into_bytes();
            let fresh = |round: &str| [key.as_slice(), round.as_bytes()].concat();
            let (committed, rolled_back) = (fresh("c"), fresh("r"));
            if present {
                for k in [&committed, &rolled_back] {
                    let k = k.clone();
                    store
                        .run(TxnOptions::new("kv.conformance.put"), move |txn| {
                            let k = k.clone();
                            Box::pin(async move { txn.put(&k, b"0".to_vec()).await })
                        })
                        .await
                        .expect("a put commits");
                }
            }
            let after_commit = lock_holder_then(&store, &committed, second).await;
            assert!(
                matches!(after_commit, Err(TxnError::Conflict)),
                "{second:?} after a committed lock (present: {present}): {after_commit:?}"
            );
            let refused = lock_once(&store, &rolled_back, true).await;
            assert!(matches!(refused, Err(TxnError::Fatal(_))), "{refused:?}");
            let mut opts = TxnOptions::new("kv.conformance.after_rollback");
            opts.max_attempts = 1;
            store
                .run(opts, |txn| {
                    let k = rolled_back.clone();
                    Box::pin(async move { second.apply(txn, &k).await })
                })
                .await
                .unwrap_or_else(|e| {
                    panic!("{second:?} after a rolled-back lock (present: {present}): {e:?}")
                });
        }
    }
    put(&store, b"present", b"t2").await;
    let mut opts = TxnOptions::new("kv.conformance.lock_only");
    opts.max_attempts = 1;
    store
        .run(opts, |txn| {
            Box::pin(async move {
                txn.lock_keys([b"fresh".as_slice(), b"present"]).await?;
                txn.put(b"locker", b"locked".to_vec()).await
            })
        })
        .await
        .expect("the locker commits");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.get(b"fresh").await.expect("get"),
        None,
        "a lock is no value"
    );
    assert_eq!(
        snap.get(b"present").await.expect("get"),
        Some(b"t2".to_vec()),
        "a lock hides no value"
    );
    assert_eq!(
        snap.scan(b"f", Some(b"g"), 10).await.expect("scan"),
        vec![],
        "scans skip locks"
    );
    // Started after the lock committed: no conflict.
    let mut opts = TxnOptions::new("kv.conformance.lock_after");
    opts.max_attempts = 1;
    store
        .run(opts, |txn| {
            Box::pin(async move {
                assert_eq!(txn.get(b"fresh").await?, None);
                txn.insert(b"fresh", b"new".to_vec()).await?;
                txn.put(b"present", b"later".to_vec()).await
            })
        })
        .await
        .expect("a later transaction writes locked keys");
}

/// Forward and reverse scans over `[lo, hi)` return keys in order, respect
/// the limit and skip deleted keys, in a snapshot and in a transaction
/// (where the transaction's own writes and deletes show).
pub async fn scan_bounds_and_order(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    store
        .run(TxnOptions::new("kv.conformance.scan"), |txn| {
            Box::pin(async move {
                for (k, v) in [
                    kv("a", "1"),
                    kv("b", "2"),
                    kv("c", "3"),
                    kv("d", "4"),
                    kv("e", "5"),
                    kv("f", "6"),
                ] {
                    txn.put(&k, v).await?;
                }
                Ok(())
            })
        })
        .await
        .expect("written");
    store
        .run(TxnOptions::new("kv.conformance.scan"), |txn| {
            Box::pin(async move { txn.delete(b"c").await })
        })
        .await
        .expect("deleted");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.scan(b"b", Some(b"e"), 10).await.expect("scan"),
        vec![kv("b", "2"), kv("d", "4")]
    );
    assert_eq!(
        snap.scan(b"b", Some(b"e"), 1).await.expect("scan"),
        vec![kv("b", "2")]
    );
    assert_eq!(
        snap.scan_reverse(b"b", Some(b"e"), 10)
            .await
            .expect("reverse scan"),
        vec![kv("d", "4"), kv("b", "2")]
    );
    assert_eq!(
        snap.scan_reverse(b"b", Some(b"e"), 1)
            .await
            .expect("reverse scan"),
        vec![kv("d", "4")]
    );
    assert_eq!(
        snap.scan(b"d", None, 10).await.expect("scan"),
        vec![kv("d", "4"), kv("e", "5"), kv("f", "6")]
    );
    assert_eq!(
        snap.scan_reverse(b"", None, 2).await.expect("reverse scan"),
        vec![kv("f", "6"), kv("e", "5")]
    );
    assert!(
        snap.scan(b"c", Some(b"d"), 10)
            .await
            .expect("scan")
            .is_empty()
    );
    assert!(snap.scan(b"a", None, 0).await.expect("scan").is_empty());
    // A key that is a prefix of the bound and of other keys.
    store
        .run(TxnOptions::new("kv.conformance.scan"), |txn| {
            Box::pin(async move {
                txn.put(b"d\x00", b"40".to_vec()).await?;
                txn.put(b"d\xff", b"41".to_vec()).await
            })
        })
        .await
        .expect("written");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.scan(b"d", Some(b"d\xff"), 10).await.expect("scan"),
        vec![kv("d", "4"), (b"d\x00".to_vec(), b"40".to_vec())]
    );
    assert_eq!(
        snap.scan_reverse(b"d\x00", Some(b"e"), 10)
            .await
            .expect("reverse scan"),
        vec![
            (b"d\xff".to_vec(), b"41".to_vec()),
            (b"d\x00".to_vec(), b"40".to_vec())
        ]
    );

    // In a transaction, its own writes and deletes overlay the snapshot.
    let seen = store
        .run(TxnOptions::new("kv.conformance.scan"), |txn| {
            Box::pin(async move {
                txn.put(b"bb", b"22".to_vec()).await?;
                txn.delete(b"d").await?;
                let fwd = txn.scan(b"b", Some(b"e"), 10).await?;
                let rev = txn.scan_reverse(b"a", Some(b"e"), 2).await?;
                let limited = txn.scan(b"a", None, 3).await?;
                Ok((fwd, rev, limited))
            })
        })
        .await
        .expect("a scan")
        .value;
    assert_eq!(
        seen.0,
        vec![
            kv("b", "2"),
            kv("bb", "22"),
            (b"d\x00".to_vec(), b"40".to_vec()),
            (b"d\xff".to_vec(), b"41".to_vec())
        ]
    );
    assert_eq!(
        seen.1,
        vec![
            (b"d\xff".to_vec(), b"41".to_vec()),
            (b"d\x00".to_vec(), b"40".to_vec())
        ]
    );
    assert_eq!(seen.2, vec![kv("a", "1"), kv("b", "2"), kv("bb", "22")]);
}

/// `insert` of a key visible at the start fails with `AlreadyExists`, as
/// TiKV's `AlreadyExist`, whether the transaction read the key first or not;
/// an insert of an absent or deleted key commits.
pub async fn insert_existing_key_fails(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"v").await;
    let blind = store
        .run(TxnOptions::new("kv.conformance.insert"), |txn| {
            Box::pin(async move { txn.insert(b"k", b"x".to_vec()).await })
        })
        .await
        .expect_err("the key exists");
    assert!(matches!(blind, TxnError::AlreadyExists(_)), "{blind:?}");
    let read_first = store
        .run(TxnOptions::new("kv.conformance.insert"), |txn| {
            Box::pin(async move {
                assert!(txn.get(b"k").await?.is_some());
                txn.insert(b"k", b"x".to_vec()).await
            })
        })
        .await
        .expect_err("the key exists");
    assert!(
        matches!(read_first, TxnError::AlreadyExists(_)),
        "{read_first:?}"
    );
    store
        .run(TxnOptions::new("kv.conformance.insert"), |txn| {
            Box::pin(async move { txn.insert(b"new", b"n".to_vec()).await })
        })
        .await
        .expect("an absent key inserts");
    store
        .run(TxnOptions::new("kv.conformance.insert"), |txn| {
            Box::pin(async move { txn.delete(b"k").await })
        })
        .await
        .expect("deleted");
    store
        .run(TxnOptions::new("kv.conformance.insert"), |txn| {
            Box::pin(async move { txn.insert(b"k", b"again".to_vec()).await })
        })
        .await
        .expect("a deleted key inserts");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.batch_get([b"k".as_slice(), b"new"])
            .await
            .expect("batch get"),
        vec![kv("k", "again"), kv("new", "n")]
    );
}

/// Two roots in one keyspace never see each other's keys.
pub async fn roots_are_isolated(factory: Factory) {
    let (one, two) = (Spec::fresh(), Spec::fresh());
    let (Some(a), Some(b)) = (factory.store(one).await, factory.store(two).await) else {
        return;
    };
    assert_ne!(a.root(), b.root());
    assert_eq!(a.keyspace(), b.keyspace());
    put(&a, b"k", b"a").await;
    put(&b, b"k", b"b").await;
    put(&b, b"only-b", b"b").await;
    let read = |store: Store| async move {
        let mut snap = store
            .snapshot(store.now().await.expect("now"))
            .await
            .expect("a snapshot");
        (
            snap.get(b"k").await.expect("get"),
            snap.scan(b"", None, 10).await.expect("scan"),
            snap.scan_reverse(b"", None, 10)
                .await
                .expect("reverse scan"),
        )
    };
    let (ak, ascan, arev) = read(a.clone()).await;
    assert_eq!(ak, Some(b"a".to_vec()));
    assert_eq!(ascan, vec![kv("k", "a")]);
    assert_eq!(arev, vec![kv("k", "a")]);
    let (bk, bscan, _) = read(b.clone()).await;
    assert_eq!(bk, Some(b"b".to_vec()));
    assert_eq!(bscan, vec![kv("k", "b"), kv("only-b", "b")]);
}

/// A transaction that only reads commits at its start timestamp.
pub async fn read_only_commit_is_at_start_ts(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let c = store
        .run(TxnOptions::new("kv.conformance.ro"), |txn| {
            Box::pin(async move {
                txn.get(b"absent").await?;
                Ok(txn.start_ts())
            })
        })
        .await
        .expect("a read");
    assert_eq!(c.commit_ts, c.value);
    assert_eq!(c.attempts, 1);
    assert!(!c.earlier_unknown);
}

/// A body's `Conflict` and `NotApplied` rerun it at a new start timestamp;
/// the committing attempt's writes apply once.
pub async fn runner_retries_conflict_and_not_applied(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let starts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = starts.clone();
    let c = store
        .run(TxnOptions::new("kv.conformance.retry"), move |txn| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(txn.start_ts());
                let n = txn.get(b"n").await?.map_or(0, |v| v[0]);
                txn.put(b"n", vec![n + 1]).await?;
                match txn.attempt() {
                    1 => Err(TxnError::Conflict),
                    2 => Err(TxnError::NotApplied("try again".into())),
                    _ => Ok(txn.attempt()),
                }
            })
        })
        .await
        .expect("committed");
    assert_eq!(c.attempts, 3);
    assert_eq!(c.value, 3);
    let starts = starts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(starts.len(), 3);
    assert!(
        starts.windows(2).all(|w| w[0] < w[1]),
        "each attempt starts later: {starts:?}"
    );
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"n").await.expect("get"), Some(vec![1]));
}

/// A conflict on every attempt ends with `Conflict` after `max_attempts`.
pub async fn runner_stops_at_max_attempts(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let runs = Arc::new(AtomicU32::new(0));
    let counted = runs.clone();
    let mut opts = TxnOptions::new("kv.conformance.budget");
    opts.max_attempts = 3;
    let err = store
        .run(opts, move |txn| {
            let counted = counted.clone();
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                txn.put(b"k", b"v".to_vec()).await?;
                Err::<(), _>(TxnError::Conflict)
            })
        })
        .await
        .expect_err("never commits");
    assert_eq!(err, TxnError::Conflict);
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"k").await.expect("get"), None, "nothing applied");
}

/// `Fatal` and `AlreadyExists` from a body end the run at once.
pub async fn runner_does_not_retry_fatal(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    for error in [
        TxnError::Fatal("no".into()),
        TxnError::AlreadyExists("k".into()),
    ] {
        let runs = Arc::new(AtomicU32::new(0));
        let counted = runs.clone();
        let returned = error.clone();
        let err = store
            .run(TxnOptions::new("kv.conformance.fatal"), move |txn| {
                let counted = counted.clone();
                let returned = returned.clone();
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    txn.put(b"k", b"v".to_vec()).await?;
                    Err::<(), _>(returned)
                })
            })
            .await
            .expect_err("fails");
        assert_eq!(err, error);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "{error:?} is not retried");
    }
}

/// A body that outlasts the deadline ends with `Deadline`; so do conflicts
/// that outlast it.
pub async fn runner_deadline_passes(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let mut opts = TxnOptions::new("kv.conformance.deadline");
    opts.deadline = Duration::from_millis(200);
    let started = Instant::now();
    let err = store
        .run(opts.clone(), |txn| {
            Box::pin(async move {
                txn.put(b"k", b"v".to_vec()).await?;
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
        })
        .await
        .expect_err("too slow");
    assert_eq!(err, TxnError::Deadline);
    assert!(started.elapsed() < Duration::from_secs(3));

    opts.max_attempts = 1000;
    let err = store
        .run(opts, |_txn| {
            Box::pin(async move { Err::<(), _>(TxnError::Conflict) })
        })
        .await
        .expect_err("conflicts until the deadline");
    assert_eq!(err, TxnError::Deadline);
}

/// 16 tasks each increment one counter 100 times in pessimistic
/// transactions, reading it with `get_for_update`: the counter ends at
/// exactly 1,600 (D851; KV0 Task 5). A read at the start timestamp would
/// lose updates (row T21-15).
pub async fn pessimistic_read_modify_write_loses_no_update(factory: Factory) {
    const TASKS: u64 = 16;
    const EACH: u64 = 100;
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let mut tasks = Vec::new();
    for _ in 0..TASKS {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..EACH {
                let mut opts = TxnOptions::pessimistic("kv.conformance.pess_rmw");
                opts.max_attempts = 64;
                opts.deadline = Duration::from_secs(60);
                store
                    .run(opts, |txn| {
                        Box::pin(async move {
                            let n = txn
                                .get_for_update(b"counter")
                                .await?
                                .map_or(0, |v| u64::from_be_bytes(v.try_into().expect("8 bytes")));
                            txn.put(b"counter", (n + 1).to_be_bytes().to_vec()).await
                        })
                    })
                    .await
                    .expect("an increment commits");
            }
        }));
    }
    for task in tasks {
        task.await.expect("a task");
    }
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    let n = snap
        .get(b"counter")
        .await
        .expect("get")
        .map(|v| u64::from_be_bytes(v.try_into().expect("8 bytes")));
    assert_eq!(n, Some(TASKS * EACH), "no update lost");
}

/// A second pessimistic holder of a key waits for the first to commit, then
/// reads the first one's write, not the value at its own start.
pub async fn pessimistic_second_holder_waits(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"before").await;
    let (locked, wait_for_lock) = tokio::sync::oneshot::channel::<()>();
    const HOLD: Duration = Duration::from_millis(400);
    let first = tokio::spawn({
        let store = store.clone();
        async move {
            let mut locked = Some(locked);
            let mut opts = TxnOptions::pessimistic("kv.conformance.pess_first");
            opts.max_attempts = 1;
            store
                .run(opts, move |txn| {
                    let locked = locked.take();
                    Box::pin(async move {
                        assert_eq!(txn.get_for_update(b"k").await?, Some(b"before".to_vec()));
                        if let Some(locked) = locked {
                            let _ = locked.send(());
                        }
                        tokio::time::sleep(HOLD).await;
                        txn.put(b"k", b"first".to_vec()).await
                    })
                })
                .await
                .expect("the first commits")
                .commit_ts
        }
    });
    wait_for_lock.await.expect("the first holds the lock");
    let asked = Instant::now();
    // TiKV may answer a woken lock wait with a write conflict, which the
    // runner retries at a later start (on embedded the first attempt
    // commits).
    let opts = TxnOptions::pessimistic("kv.conformance.pess_second");
    let second = store
        .run(opts, |txn| {
            Box::pin(async move {
                let seen = txn.get_for_update(b"k").await?;
                let waited = asked.elapsed();
                txn.put(b"k", b"second".to_vec()).await?;
                Ok((seen, waited, txn.start_ts()))
            })
        })
        .await
        .expect("the second commits after waiting");
    let first_commit = first.await.expect("joined");
    let (seen, waited, second_start) = second.value;
    assert_eq!(
        seen,
        Some(b"first".to_vec()),
        "the latest value, not the start's"
    );
    if second.attempts == 1 {
        assert!(
            second_start < first_commit,
            "the second started before the first committed"
        );
    }
    assert!(
        waited >= HOLD / 2,
        "the second waited for the lock: {waited:?}"
    );
    assert!(second.commit_ts > first_commit);
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"k").await.expect("get"), Some(b"second".to_vec()));
}

/// A lock wait that outlasts the run's lock-wait limit fails with
/// `Conflict` (with one attempt), and the holder still commits.
pub async fn pessimistic_lock_wait_times_out_as_conflict(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"0").await;
    let (locked, wait_for_lock) = tokio::sync::oneshot::channel::<()>();
    let (done, release) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn({
        let store = store.clone();
        async move {
            let mut signals = Some((locked, release));
            let mut opts = TxnOptions::pessimistic("kv.conformance.pess_holder");
            opts.max_attempts = 1;
            opts.deadline = Duration::from_secs(30);
            store
                .run(opts, move |txn| {
                    let signals = signals.take();
                    Box::pin(async move {
                        txn.get_for_update(b"k").await?;
                        if let Some((locked, release)) = signals {
                            let _ = locked.send(());
                            let _ = release.await;
                        }
                        txn.put(b"k", b"holder".to_vec()).await
                    })
                })
                .await
        }
    });
    wait_for_lock.await.expect("the holder holds the lock");
    let mut opts = TxnOptions::pessimistic("kv.conformance.pess_waiter");
    opts.max_attempts = 1;
    opts.deadline = Duration::from_millis(1_500);
    let started = Instant::now();
    let err = store
        .run(opts, |txn| {
            Box::pin(async move {
                txn.get_for_update(b"k").await?;
                txn.put(b"k", b"waiter".to_vec()).await
            })
        })
        .await
        .expect_err("the lock is held past the limit");
    assert_eq!(err, TxnError::Conflict);
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "the wait gave up before the deadline: {:?}",
        started.elapsed()
    );
    let _ = done.send(());
    holder
        .await
        .expect("joined")
        .expect("the holder commits after the waiter gave up");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(snap.get(b"k").await.expect("get"), Some(b"holder".to_vec()));
}

/// An optimistic writer of a key a pessimistic transaction holds fails
/// with `Conflict` (one attempt); the pessimistic one commits.
pub async fn pessimistic_and_optimistic_conflict(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put(&store, b"k", b"0").await;
    let (locked, wait_for_lock) = tokio::sync::oneshot::channel::<()>();
    let (done, release) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn({
        let store = store.clone();
        async move {
            let mut signals = Some((locked, release));
            let mut opts = TxnOptions::pessimistic("kv.conformance.pess_holder");
            opts.max_attempts = 1;
            opts.deadline = Duration::from_secs(30);
            store
                .run(opts, move |txn| {
                    let signals = signals.take();
                    Box::pin(async move {
                        let seen = txn.get_for_update(b"k").await?;
                        if let Some((locked, release)) = signals {
                            let _ = locked.send(());
                            // Held until the optimistic writer finished, or
                            // for a second at most (on TiKV it may wait for
                            // the lock to go).
                            let _ = tokio::time::timeout(Duration::from_secs(1), release).await;
                        }
                        txn.put(b"k", b"pessimistic".to_vec()).await?;
                        Ok(seen)
                    })
                })
                .await
        }
    });
    wait_for_lock.await.expect("the holder holds the lock");
    let mut opts = TxnOptions::new("kv.conformance.optimistic");
    opts.max_attempts = 1;
    opts.deadline = Duration::from_secs(20);
    let err = store
        .run(opts, |txn| {
            Box::pin(async move { txn.put(b"k", b"optimistic".to_vec()).await })
        })
        .await
        .expect_err("the key is locked");
    assert_eq!(err, TxnError::Conflict);
    let _ = done.send(());
    let committed = holder.await.expect("joined").expect("the holder commits");
    assert_eq!(committed.value, Some(b"0".to_vec()));
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.get(b"k").await.expect("get"),
        Some(b"pessimistic".to_vec())
    );
}

/// One fault at one point of the first attempt of `op`, then nothing.
#[cfg(feature = "faults")]
struct Once {
    op: &'static str,
    point: FaultPoint,
    fault: Fault,
}

#[cfg(feature = "faults")]
impl FaultPlan for Once {
    fn at(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault> {
        (op == self.op && point == self.point && attempt == 1).then_some(self.fault)
    }
}

/// Every fault at every point has the effect the table of `faults.rs`
/// gives: before the commit, `Refuse`, `Conflict` and `LoseAck` cost one
/// attempt (a lost acknowledgement there resolves as not applied through
/// the commit token); after the commit they are unknown outcomes that the
/// token resolves as committed; `Delay` only delays. Each case's counter
/// ends at 1: no attempt applied twice. With one attempt, each fault's own
/// error class surfaces. Needs the `faults` feature.
#[allow(clippy::too_many_lines)]
pub async fn fault_points_and_commit_tokens(factory: Factory) {
    #[cfg(not(feature = "faults"))]
    {
        let _ = factory;
        eprintln!("skipped: fault_points_and_commit_tokens needs the faults feature");
    }
    #[cfg(feature = "faults")]
    {
        let Some(store) = factory.store(Spec::fresh()).await else {
            return;
        };
        const OP: &str = "kv.conformance.fault_table";
        let delay = Duration::from_millis(60);
        let points = [
            FaultPoint::BeforeBegin,
            FaultPoint::BeforePrewrite,
            FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit,
        ];
        let faults = [
            Fault::Conflict,
            Fault::LoseAck,
            Fault::Refuse,
            Fault::Delay(delay),
        ];
        for (p, &point) in points.iter().enumerate() {
            for (f, &fault) in faults.iter().enumerate() {
                let case = format!("{point:?} × {fault:?}");
                let key = vec![b'c', u8::try_from(p * 4 + f).expect("16 cases")];
                let faulty = store.clone().with_faults(Arc::new(Once {
                    op: OP,
                    point,
                    fault,
                }));
                let started = Instant::now();
                let body_key = key.clone();
                let c = faulty
                    .run(TxnOptions::new(OP).with_token(), move |txn| {
                        let key = body_key.clone();
                        Box::pin(async move {
                            let n = txn.get(&key).await?.map_or(0, |v| v[0]);
                            txn.put(&key, vec![n + 1]).await
                        })
                    })
                    .await
                    .unwrap_or_else(|e| panic!("{case}: {e:?}"));
                let (attempts, unknown) = match (point, fault) {
                    (_, Fault::Delay(_)) => (1, false),
                    (FaultPoint::AfterCommit, _) => (1, true),
                    _ => (2, false),
                };
                assert_eq!(c.attempts, attempts, "{case}: attempts");
                assert_eq!(c.earlier_unknown, unknown, "{case}: earlier_unknown");
                if matches!(fault, Fault::Delay(_)) {
                    assert!(started.elapsed() >= delay, "{case}: delayed");
                }
                let mut snap = store
                    .snapshot(store.now().await.expect("now"))
                    .await
                    .expect("a snapshot");
                assert_eq!(
                    snap.get(&key).await.expect("get"),
                    Some(vec![1]),
                    "{case}: applied once"
                );

                let strict = vec![b's', key[1]];
                let mut opts = TxnOptions::new(OP).with_token();
                opts.max_attempts = 1;
                let one = faulty
                    .run(opts, move |txn| {
                        let key = strict.clone();
                        Box::pin(async move { txn.put(&key, vec![1]).await })
                    })
                    .await;
                match (point, fault, one) {
                    (_, Fault::Delay(_), Ok(c)) | (FaultPoint::AfterCommit, _, Ok(c)) => {
                        assert_eq!(c.attempts, 1, "{case}: one attempt");
                    }
                    (_, Fault::Conflict, Err(TxnError::Conflict)) => {}
                    (
                        FaultPoint::BeforeBegin,
                        Fault::Refuse | Fault::LoseAck,
                        Err(TxnError::NotApplied(m)),
                    ) => {
                        assert!(m.contains("refused before begin"), "{case}: {m}");
                    }
                    (_, Fault::Refuse, Err(TxnError::NotApplied(m))) => {
                        assert!(m.contains("refused before"), "{case}: {m}");
                    }
                    (_, Fault::LoseAck, Err(TxnError::NotApplied(m))) => {
                        assert!(m.contains("token is absent"), "{case}: {m}");
                    }
                    (_, _, other) => panic!("{case}: one attempt gave {other:?}"),
                }
            }
        }
        // Without a token, an unknown outcome stays undetermined.
        let faulty = store.clone().with_faults(Arc::new(Once {
            op: OP,
            point: FaultPoint::AfterCommit,
            fault: Fault::LoseAck,
        }));
        let err = faulty
            .run(TxnOptions::new(OP), |txn| {
                Box::pin(async move { txn.put(b"untokened", b"v".to_vec()).await })
            })
            .await
            .expect_err("no token to resolve");
        assert_eq!(err, TxnError::Undetermined { token: None });
    }
}

/// Whether `e` refuses a read below the GC safe point, on either backend.
fn is_gc_safe_point(e: &KvError) -> bool {
    match e {
        KvError::GcSafePoint { .. } => true,
        #[cfg(feature = "tikv")]
        KvError::Tikv(loams_tikv::TikvError::GcSafePoint { .. }) => true,
        _ => false,
    }
}

/// The safe point a barrier refusal names, on either backend.
fn barrier_refusal(e: &KvError) -> Option<u64> {
    match e {
        KvError::BarrierBelowSafePoint { min_safe_point, .. } => Some(*min_safe_point),
        #[cfg(feature = "tikv")]
        KvError::Tikv(loams_tikv::TikvError::BarrierBelowSafePoint { min_safe_point, .. }) => {
            Some(*min_safe_point)
        }
        _ => None,
    }
}

/// One GC round of the backend: the store's own GC on embedded, the
/// cluster GC loop on TiKV.
// Without `tikv` the early return is the last statement (clippy's needless_return).
#[cfg_attr(not(feature = "tikv"), allow(clippy::needless_return))]
async fn gc_round(store: &Store) {
    if let Some(h) = store.as_embedded() {
        h.gc_once().await.expect("a GC round");
        return;
    }
    #[cfg(feature = "tikv")]
    if let Some(t) = store.as_tikv() {
        loams_tikv::GcLoop::new(t.clone(), loams_tikv::GcConfig::default())
            .expect("a loop")
            .run_once()
            .await
            .expect("a GC round");
    }
}

/// A snapshot below the GC read window stays readable while a barrier of
/// the store covers it, open or taken anew; it is refused once the barrier
/// is deleted; a barrier below the safe point is refused.
pub async fn barrier_holds_old_snapshots_and_refuses_below_safe_point(factory: Factory) {
    // A 62 s life time leaves a 2 s read window.
    let spec = Spec {
        gc_life_time: Some(Duration::from_secs(62)),
        ..Spec::fresh()
    };
    let Some(store) = factory.store(spec).await else {
        return;
    };
    put(&store, b"k", b"v1").await;
    let at = store.now().await.expect("now");
    put(&store, b"k", b"v2").await;

    let name = format!("kv-test/{:x}", at.0);
    let barrier = store
        .barrier(&name, at, Duration::from_secs(120))
        .await
        .expect("a barrier");
    assert_eq!(barrier.service_id(), format!("loams/{name}"));
    assert_eq!(barrier.ts(), at);
    let mut held = store.snapshot(at).await.expect("inside the window");
    tokio::time::sleep(Duration::from_millis(2_500)).await;

    // Past the window, under the barrier: an open and a new snapshot read.
    let held_read = held.get(b"k").await;
    let late = match store.snapshot(at).await {
        Ok(mut snap) => snap.get(b"k").await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    barrier.delete().await.expect("deleted");
    assert_eq!(held_read.expect("covered"), Some(b"v1".to_vec()));
    assert_eq!(late.expect("covered"), Some(b"v1".to_vec()));

    // Without the barrier both refuse.
    assert!(matches!(held.get(b"k").await, Err(TxnError::Fatal(_))));
    match store.snapshot(at).await {
        Err(e) => assert!(is_gc_safe_point(&e), "{e:?}"),
        Ok(_) => panic!("a snapshot below the window without a barrier"),
    }

    // A barrier below the safe point is refused. A fresh store's safe
    // point is 0: one GC round moves it past version 1.
    gc_round(&store).await;
    match store
        .barrier("kv-test/below", Ts(1), Duration::from_secs(60))
        .await
    {
        Err(e) => {
            let min = barrier_refusal(&e)
                .unwrap_or_else(|| panic!("expected BarrierBelowSafePoint, got {e:?}"));
            assert!(min > 1);
        }
        Ok(b) => {
            // Never leave it to hold the safe point for other tests.
            b.delete().await.expect("deleted");
            panic!("a barrier at ts 1 was accepted");
        }
    }
}

/// Writes `pairs` in one transaction.
async fn put_all(store: &Store, pairs: Vec<(Vec<u8>, Vec<u8>)>) {
    store
        .run(TxnOptions::new("kv.conformance.put_all"), move |txn| {
            let pairs = pairs.clone();
            Box::pin(async move {
                for (k, v) in pairs {
                    txn.put(&k, v).await?;
                }
                Ok(())
            })
        })
        .await
        .expect("written");
}

fn numbered(n: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..n)
        .map(|i| {
            (
                format!("k{i:02}").into_bytes(),
                format!("v{i}").into_bytes(),
            )
        })
        .collect()
}

/// Every page of `scanner`, until it is exhausted, and a check that it
/// stays exhausted.
async fn pages(mut scanner: crate::Scanner<'_>) -> Vec<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::new();
    while let Some(page) = scanner.next_page().await.expect("a page") {
        assert!(
            page.len() <= scanner.page_size(),
            "a page over the page size"
        );
        out.push(page);
    }
    assert_eq!(scanner.next_page().await.expect("a page"), None);
    assert_eq!(scanner.next().await.expect("a pair"), None);
    out
}

/// Every pair of `scanner` through `next`, which holds at most one page.
async fn pairs(mut scanner: crate::Scanner<'_>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    while let Some(pair) = scanner.next().await.expect("a pair") {
        assert!(scanner.buffered() < scanner.page_size());
        out.push(pair);
    }
    out
}

fn sizes(pages: &[Vec<(Vec<u8>, Vec<u8>)>]) -> Vec<usize> {
    pages.iter().map(Vec::len).collect()
}

/// A forward scanner returns the range in key order, in full pages and a
/// short last one, with no empty page when the range divides evenly.
pub async fn scanner_pages_forward(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let all = numbered(25);
    put_all(&store, all.clone()).await;
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    let got = pages(snap.scanner(b"", None, 10)).await;
    assert_eq!(sizes(&got), [10, 10, 5]);
    assert_eq!(got.concat(), all);
    let got = pages(snap.scanner(b"k05", Some(b"k25"), 10)).await;
    assert_eq!(
        sizes(&got),
        [10, 10],
        "an even range has no empty last page"
    );
    assert_eq!(got.concat(), all[5..]);
    assert_eq!(
        pairs(snap.scanner(b"k03", Some(b"k17"), 4)).await,
        all[3..17]
    );
    // A page size of 0 is read as 1.
    let got = pages(snap.scanner(b"k20", None, 0)).await;
    assert_eq!(sizes(&got), [1, 1, 1, 1, 1]);
}

/// A reverse scanner returns the range in descending key order, paged.
pub async fn scanner_pages_reverse(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let all = numbered(25);
    put_all(&store, all.clone()).await;
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    let reversed: Vec<_> = all.iter().rev().cloned().collect();
    let got = pages(snap.scanner_reverse(b"", None, 10)).await;
    assert_eq!(sizes(&got), [10, 10, 5]);
    assert_eq!(got.concat(), reversed);
    let got = pages(snap.scanner_reverse(b"k05", Some(b"k25"), 10)).await;
    assert_eq!(sizes(&got), [10, 10]);
    assert_eq!(got.concat(), reversed[..20]);
    let want: Vec<_> = all[3..17].iter().rev().cloned().collect();
    assert_eq!(
        pairs(snap.scanner_reverse(b"k03", Some(b"k17"), 4)).await,
        want
    );
}

/// In a transaction, a scanner sees the transaction's own writes and
/// deletes on every page, forward and reverse.
pub async fn scanner_sees_own_writes(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    put_all(&store, numbered(12)).await;
    let (fwd, rev, want) = store
        .run(TxnOptions::new("kv.conformance.scanner"), |txn| {
            Box::pin(async move {
                txn.delete(b"k00").await?;
                txn.delete(b"k05").await?;
                txn.delete(b"k06").await?;
                txn.put(b"k05a", b"new".to_vec()).await?;
                txn.put(b"k99", b"last".to_vec()).await?;
                txn.put(b"k03", b"changed".to_vec()).await?;
                let want = txn.scan(b"", None, 100).await?;
                let mut fwd = Vec::new();
                {
                    let mut scanner = txn.scanner(b"", None, 3);
                    while let Some(page) = scanner.next_page().await? {
                        fwd.extend(page);
                    }
                }
                let mut rev = Vec::new();
                {
                    let mut scanner = txn.scanner_reverse(b"", None, 2);
                    while let Some(pair) = scanner.next().await? {
                        rev.push(pair);
                    }
                }
                Ok((fwd, rev, want))
            })
        })
        .await
        .expect("a scan")
        .value;
    assert_eq!(want.len(), 11, "12 keys, 3 deleted, 2 new");
    assert_eq!(want.first(), Some(&kv("k01", "v1")));
    assert!(want.contains(&kv("k03", "changed")));
    assert!(want.contains(&kv("k05a", "new")));
    assert_eq!(fwd, want);
    let reversed: Vec<_> = want.iter().rev().cloned().collect();
    assert_eq!(rev, reversed);
}

/// Scanner bounds: an empty range, a range of one key, an exclusive end on
/// a key, and page boundaries on keys that are prefixes of the next ones.
pub async fn scanner_bounds(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let keys: Vec<(Vec<u8>, Vec<u8>)> =
        [&b"a"[..], b"a\x00", b"a\x00\x00", b"a\x01", b"b", b"b\xff"]
            .iter()
            .enumerate()
            .map(|(i, k)| (k.to_vec(), vec![b'0' + u8::try_from(i).expect("small")]))
            .collect();
    put_all(&store, keys.clone()).await;
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    // Empty: nothing in the range, and start == end.
    assert!(pages(snap.scanner(b"c", None, 2)).await.is_empty());
    assert!(pages(snap.scanner(b"a", Some(b"a"), 2)).await.is_empty());
    assert!(
        pages(snap.scanner_reverse(b"a", Some(b"a"), 2))
            .await
            .is_empty()
    );
    assert!(
        pages(snap.scanner_reverse(b"", Some(b"a"), 2))
            .await
            .is_empty()
    );
    // One key.
    assert_eq!(
        pages(snap.scanner(b"b", Some(b"b\x00"), 2)).await,
        [vec![keys[4].clone()]]
    );
    assert_eq!(
        pages(snap.scanner_reverse(b"b", Some(b"b\x00"), 2)).await,
        [vec![keys[4].clone()]]
    );
    // The end is exclusive, a page boundary falls on every key, and a key
    // that is a prefix of the next is not skipped or repeated.
    assert_eq!(pairs(snap.scanner(b"a", Some(b"b"), 1)).await, keys[..4]);
    let want: Vec<_> = keys[..4].iter().rev().cloned().collect();
    assert_eq!(pairs(snap.scanner_reverse(b"a", Some(b"b"), 1)).await, want);
    assert_eq!(
        pages(snap.scanner(b"a\x00", None, 2)).await.concat(),
        keys[1..]
    );
    let want: Vec<_> = keys[1..].iter().rev().cloned().collect();
    assert_eq!(
        pages(snap.scanner_reverse(b"a\x00", None, 2))
            .await
            .concat(),
        want
    );
}

/// The one-shot `put`, `get` and `delete` (KV0 Task 2): each commits on its
/// own, later commits at later timestamps, and a delete of an absent key
/// commits too.
pub async fn one_shot_put_get_delete(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    assert_eq!(store.get(b"k").await.expect("get"), None);
    let first = store.put(b"k", b"v1".to_vec()).await.expect("put");
    assert_eq!(store.get(b"k").await.expect("get"), Some(b"v1".to_vec()));
    let second = store.put(b"k", b"v2".to_vec()).await.expect("put");
    assert!(second > first, "a later put commits later");
    assert_eq!(store.get(b"k").await.expect("get"), Some(b"v2".to_vec()));
    // The one-shot writes are ordinary commits: a snapshot between them sees
    // the first.
    let mut snap = store.snapshot(first).await.expect("a snapshot");
    assert_eq!(snap.get(b"k").await.expect("get"), Some(b"v1".to_vec()));
    let deleted = store.delete(b"k").await.expect("delete");
    assert!(deleted > second);
    assert_eq!(store.get(b"k").await.expect("get"), None);
    store
        .delete(b"absent")
        .await
        .expect("deleting an absent key commits");
    // Values over 2 MiB are refused, as in a transaction.
    let err = store
        .put(b"big", vec![0u8; crate::MAX_VALUE_BYTES + 1])
        .await
        .expect_err("too big");
    assert!(matches!(err, TxnError::Fatal(_)), "{err:?}");
}

/// A one-shot insert of a present key fails with `AlreadyExists` and leaves
/// the value; of an absent or deleted key it commits.
pub async fn one_shot_insert_existing_fails(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    store
        .insert(b"k", b"first".to_vec())
        .await
        .expect("an absent key inserts");
    let err = store
        .insert(b"k", b"second".to_vec())
        .await
        .expect_err("the key exists");
    assert!(matches!(err, TxnError::AlreadyExists(_)), "{err:?}");
    assert_eq!(store.get(b"k").await.expect("get"), Some(b"first".to_vec()));
    store.delete(b"k").await.expect("delete");
    store
        .insert(b"k", b"again".to_vec())
        .await
        .expect("a deleted key inserts");
    assert_eq!(store.get(b"k").await.expect("get"), Some(b"again".to_vec()));
}

/// `compare_and_put` writes when the value matches and reports the current
/// value, writing nothing, when it does not.
pub async fn compare_and_put_matches_and_mismatches(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    let put = store.put(b"k", b"a".to_vec()).await.expect("put");
    let swapped = store
        .compare_and_put(b"k", Some(b"a"), Some(b"b"))
        .await
        .expect("a cas")
        .expect("the value matched");
    assert!(swapped > put);
    assert_eq!(
        store
            .compare_and_put(b"k", Some(b"a"), Some(b"c"))
            .await
            .expect("a cas"),
        Err(Some(b"b".to_vec())),
        "a stale expectation reports the current value"
    );
    assert_eq!(
        store
            .compare_and_put(b"k", None, Some(b"c"))
            .await
            .expect("a cas"),
        Err(Some(b"b".to_vec())),
        "expecting absence of a present key mismatches"
    );
    assert_eq!(store.get(b"k").await.expect("get"), Some(b"b".to_vec()));
    // A mismatch wrote nothing: no commit after the swap changed the key.
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("snap");
    assert_eq!(snap.get(b"k").await.expect("get"), Some(b"b".to_vec()));
    // An empty value is a value, not absence.
    store.put(b"e", Vec::new()).await.expect("put");
    assert_eq!(
        store
            .compare_and_put(b"e", None, Some(b"x"))
            .await
            .expect("a cas"),
        Err(Some(Vec::new()))
    );
    store
        .compare_and_put(b"e", Some(b""), Some(b"x"))
        .await
        .expect("a cas")
        .expect("the empty value matched");
    assert_eq!(store.get(b"e").await.expect("get"), Some(b"x".to_vec()));
}

/// `compare_and_put` with `expected: None` creates an absent key once, and
/// with `new: None` deletes a matching key.
pub async fn compare_and_put_absent_and_delete(factory: Factory) {
    let Some(store) = factory.store(Spec::fresh()).await else {
        return;
    };
    store
        .compare_and_put(b"k", None, Some(b"x"))
        .await
        .expect("a cas")
        .expect("absent matched");
    assert_eq!(
        store
            .compare_and_put(b"k", None, Some(b"y"))
            .await
            .expect("a cas"),
        Err(Some(b"x".to_vec()))
    );
    store
        .compare_and_put(b"k", Some(b"x"), None)
        .await
        .expect("a cas")
        .expect("matched, deleted");
    assert_eq!(store.get(b"k").await.expect("get"), None);
    assert_eq!(
        store
            .compare_and_put(b"k", Some(b"x"), Some(b"z"))
            .await
            .expect("a cas"),
        Err(None),
        "a deleted key reports absence"
    );
    store
        .compare_and_put(b"never", None, None)
        .await
        .expect("a cas")
        .expect("absent matched absent");
    assert_eq!(store.get(b"never").await.expect("get"), None);
}
