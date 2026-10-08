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
//! write, inserts of present keys fail), scans, root isolation, and the
//! runner: its retries, attempt budget, deadline, fault points and commit
//! tokens, and GC barriers.

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
                scan_bounds_and_order,
                insert_existing_key_fails,
                roots_are_isolated,
                read_only_commit_is_at_start_ts,
                runner_retries_conflict_and_not_applied,
                runner_stops_at_max_attempts,
                runner_does_not_retry_fatal,
                runner_deadline_passes,
                fault_points_and_commit_tokens,
                barrier_holds_old_snapshots_and_refuses_below_safe_point
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
async fn gc_round(store: &Store) {
    match store {
        Store::Embedded(h) => {
            h.gc_once().await.expect("a GC round");
        }
        #[cfg(feature = "tikv")]
        Store::Tikv(t) => {
            loams_tikv::GcLoop::new(t.clone(), loams_tikv::GcConfig::default())
                .expect("a loop")
                .run_once()
                .await
                .expect("a GC round");
        }
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
