//! The embedded backend's own tests (LV1 plan Task 21): the oracle across
//! restarts, GC under barriers and open snapshots, group commit, the
//! per-process handle registry, pessimistic locks, and a model check of
//! snapshot isolation against an in-memory reference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_kv::embedded::{self, Handle};
use loams_kv::testing::TempDir;
use loams_kv::{EmbeddedConfig, KvError, Mode, Store, StoreConfig, Ts, TxnError, TxnOptions};
use proptest::prelude::*;

fn tmp() -> TempDir {
    TempDir::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).expect("a temporary directory")
}

fn config(path: &Path, keyspace: &str) -> EmbeddedConfig {
    EmbeddedConfig::new(path.to_path_buf(), keyspace)
}

async fn open(path: &Path, keyspace: &str) -> Store {
    Store::open(StoreConfig::Embedded(config(path, keyspace)))
        .await
        .expect("an embedded store")
}

fn handle(store: &Store) -> &Handle {
    match store {
        Store::Embedded(h) => h,
        #[allow(unreachable_patterns)]
        _ => panic!("not an embedded store"),
    }
}

fn wall_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis(),
    )
    .expect("fits")
}

async fn put(store: &Store, key: &'static [u8], value: &'static [u8]) -> Ts {
    store
        .run(TxnOptions::new("kv.embedded.put"), move |txn| {
            Box::pin(async move { txn.put(key, value.to_vec()).await })
        })
        .await
        .expect("a put commits")
        .commit_ts
}

async fn get_at(store: &Store, at: Ts, key: &[u8]) -> Option<Vec<u8>> {
    store
        .snapshot(at)
        .await
        .expect("a snapshot")
        .get(key)
        .await
        .expect("get")
}

/// Reopening the file starts the oracle above every timestamp issued
/// before, even one far ahead of the wall clock (a snapshot at a future
/// timestamp moves the oracle past it); committed data survives.
#[tokio::test]
async fn ts_monotonic_across_restart() {
    let dir = tmp();
    let path = dir.path().join("store.redb");
    let mut issued = Vec::new();
    {
        let store = open(&path, "restart").await;
        assert!(embedded::is_open(&path));
        for _ in 0..50 {
            issued.push(store.now().await.expect("now"));
        }
        issued.push(put(&store, b"k", b"v").await);
        // Within the second a read may run ahead of the clock (row T21-14).
        let ahead = Ts::from_parts(wall_ms() + 800, 7);
        drop(store.snapshot(ahead).await.expect("a snapshot ahead"));
        let after = store.now().await.expect("now");
        assert!(after > ahead, "the oracle moved past the snapshot");
        issued.push(after);
        assert!(issued.windows(2).all(|w| w[0] < w[1]), "monotonic");
    }
    assert!(!embedded::is_open(&path), "the last handle closed the file");
    let store = open(&path, "restart").await;
    let first = store.now().await.expect("now");
    let max = issued.iter().max().copied().expect("issued");
    assert!(first > max, "{first} after a restart, {max} before");
    assert_eq!(get_at(&store, first, b"k").await, Some(b"v".to_vec()));
}

/// A client-supplied snapshot timestamp cannot move the oracle: one at
/// `u64::MAX`, or years ahead, is refused with `TsAhead`, persists no mark,
/// and the next timestamp is still on the wall clock (review fix 1).
#[tokio::test]
async fn snapshots_far_ahead_are_refused_and_leave_the_oracle_alone() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "ahead").await;
    let before = store.now().await.expect("now");
    let writes = handle(&store).stats().write_transactions;
    let years = Ts::from_parts(wall_ms() + 5 * 365 * 24 * 3_600_000, 0);
    for at in [Ts(u64::MAX), years, Ts::from_parts(Ts::MAX_PHYSICAL_MS, 0)] {
        match store.snapshot(at).await {
            Err(KvError::TsAhead { at: a, limit }) => {
                assert_eq!(a, at.0);
                assert!(limit < at.0);
            }
            other => panic!("{at}: expected TsAhead, got {other:?}"),
        }
    }
    assert_eq!(
        handle(&store).stats().write_transactions,
        writes,
        "no mark was persisted"
    );
    let after = store.now().await.expect("now");
    assert!(after > before);
    assert!(
        after.physical_ms() <= wall_ms() + 1_000,
        "the oracle stayed on the clock: {after}"
    );
    // A read a little ahead (under the limit) is still fine.
    let near = Ts::from_parts(wall_ms() + 500, 0);
    drop(store.snapshot(near).await.expect("within the limit"));
    assert!(store.now().await.expect("now") > near);
}

/// A commit group whose write transaction fails does not count its oracle
/// mark as persisted: after an injected sync failure the oracle's durable
/// mark is where the last successful write left it (review fix 2).
#[tokio::test]
async fn a_failed_group_write_persists_no_mark() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "io").await;
    let h = handle(&store);
    let mut txn = h.begin(Mode::Optimistic).await.expect("begin");
    txn.put(b"k", b"v".to_vec()).await.expect("put");
    let (_, durable) = h.oracle_marks();
    // Past the mark, the commit's timestamp needs a new one, written in the
    // group's own write transaction.
    while wall_ms() <= durable.physical_ms() + 5 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.fail_syncs(1);
    let err = txn.commit().await.expect_err("the sync failed");
    assert_eq!(err, TxnError::Undetermined { token: None });
    let (last, after) = h.oracle_marks();
    assert!(
        last >= durable,
        "a commit timestamp at or past the mark was allocated"
    );
    assert_eq!(after, durable, "the failed write's mark is not persisted");
}

/// A barrier at `t` keeps a snapshot at `t` readable after GC; once it is
/// gone, GC drops what only `t` saw and both a snapshot and a barrier at `t`
/// are refused.
#[tokio::test]
async fn gc_respects_barrier() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "gc").await;
    let h = handle(&store);
    put(&store, b"k", b"v0").await;
    put(&store, b"k", b"v1").await;
    let at = store.now().await.expect("now");
    put(&store, b"k", b"v2").await;
    let barrier = store
        .barrier("gc-test/1", at, Duration::from_secs(600))
        .await
        .expect("a barrier");

    // An hour on, GC stops at the barrier: v0 goes, v1 stays.
    let far = wall_ms() + 3_600_000;
    let report = h.gc_once_at(far).await.expect("a GC round");
    assert_eq!(report.safe_point, at);
    assert_eq!(report.versions_deleted, 1, "only v0 was below v1");
    assert_eq!(get_at(&store, at, b"k").await, Some(b"v1".to_vec()));

    barrier.delete().await.expect("deleted");
    let report = h.gc_once_at(far).await.expect("a GC round");
    let life = u64::try_from(embedded::DEFAULT_GC_LIFE_TIME.as_millis()).expect("fits");
    assert_eq!(report.safe_point, Ts::from_parts(far - life, 0));
    assert_eq!(report.versions_deleted, 1, "v1 went");
    match store.snapshot(at).await {
        Err(KvError::GcSafePoint { at: a, safe_point }) => {
            assert_eq!(a, at.0);
            assert_eq!(safe_point, report.safe_point.0);
        }
        other => panic!("expected GcSafePoint, got {other:?}"),
    }
    match store
        .barrier("gc-test/2", at, Duration::from_secs(60))
        .await
    {
        Err(KvError::BarrierBelowSafePoint {
            service_id,
            ts,
            min_safe_point,
        }) => {
            assert_eq!(service_id, "loams/gc-test/2");
            assert_eq!(ts, at.0);
            assert_eq!(min_safe_point, report.safe_point.0);
        }
        other => panic!("expected BarrierBelowSafePoint, got {other:?}"),
    }
}

/// An open snapshot holds GC at its timestamp; a deleted key's versions
/// (its tombstone included) all go once GC passes them.
#[tokio::test]
async fn gc_keeps_open_snapshots_and_drops_deleted_keys() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "gc").await;
    let h = handle(&store);
    put(&store, b"k", b"v1").await;
    put(&store, b"gone", b"x").await;
    let at = store.now().await.expect("now");
    let mut held = store.snapshot(at).await.expect("a snapshot");
    put(&store, b"k", b"v2").await;
    store
        .run(TxnOptions::new("kv.embedded.delete"), |txn| {
            Box::pin(async move { txn.delete(b"gone").await })
        })
        .await
        .expect("deleted");

    let far = wall_ms() + 3_600_000;
    let report = h.gc_once_at(far).await.expect("a GC round");
    assert_eq!(report.safe_point, at, "held at the open snapshot");
    assert_eq!(report.versions_deleted, 0);
    assert_eq!(held.get(b"k").await.expect("get"), Some(b"v1".to_vec()));
    assert_eq!(held.get(b"gone").await.expect("get"), Some(b"x".to_vec()));
    drop(held);

    let report = h.gc_once_at(far).await.expect("a GC round");
    // k: v1; gone: x and its tombstone.
    assert_eq!(report.versions_deleted, 3);
    assert!(h.stats().gc_runs >= 2);
}

/// 100 concurrent commits share redb write transactions (and fsyncs).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_commit_batches_fsyncs() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "group").await;
    put(&store, b"warm", b"up").await;
    let before = handle(&store).stats().write_transactions;
    let tasks: Vec<_> = (0..100u8)
        .map(|i| {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .run(TxnOptions::new("kv.embedded.group"), move |txn| {
                        Box::pin(async move { txn.put(&[b'g', i], vec![i]).await })
                    })
                    .await
            })
        })
        .collect();
    for task in tasks {
        task.await.expect("joined").expect("committed");
    }
    let used = handle(&store).stats().write_transactions - before;
    assert!(used < 100, "100 commits took {used} write transactions");
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    assert_eq!(
        snap.scan(b"g", Some(b"h"), 200).await.expect("scan").len(),
        100
    );
}

/// Handles on one path share one database (redb locks its file); each
/// keyspace has its own id, so equal roots in two keyspaces never meet.
#[tokio::test]
async fn one_database_per_path_and_keyspaces_are_isolated() {
    let dir = tmp();
    let path = dir.path().join("store.redb");
    let a = open(&path, "ks_a").await;
    let b = open(&path, "ks_b").await;
    let a2 = open(&path, "ks_a").await;
    assert_eq!(a.keyspace(), "ks_a");
    assert_eq!(b.keyspace(), "ks_b");
    put(&a, b"k", b"a").await;
    put(&b, b"k", b"b").await;
    let now = a.now().await.expect("now");
    assert_eq!(get_at(&a2, now, b"k").await, Some(b"a".to_vec()));
    assert_eq!(get_at(&b, now, b"k").await, Some(b"b".to_vec()));
    drop((a, b));
    assert!(embedded::is_open(&path));
    drop(a2);
    assert!(!embedded::is_open(&path));

    // Ids are stable across a reopen.
    let b = open(&path, "ks_b").await;
    let now = b.now().await.expect("now");
    assert_eq!(get_at(&b, now, b"k").await, Some(b"b".to_vec()));
}

/// A config the store cannot honour is refused.
#[tokio::test]
async fn bad_configs_are_refused() {
    let dir = tmp();
    let mut short = config(&dir.path().join("store.redb"), "ks");
    short.gc_life_time = Duration::from_secs(60);
    let err = Store::open(StoreConfig::Embedded(short))
        .await
        .expect_err("a life time of one minute");
    assert!(err.to_string().contains("gc_life_time"), "{err}");
    let empty = config(&dir.path().join("store.redb"), "");
    let err = Store::open(StoreConfig::Embedded(empty))
        .await
        .expect_err("no keyspace");
    assert!(err.to_string().contains("keyspace"), "{err}");
}

/// Pessimistic writers of one key queue instead of conflicting: their locks
/// are taken at once, so every run commits on its first attempt, where
/// optimistic writers started together conflict. (Reads stay at the start
/// timestamp, as on TiKV: the seam has no `get_for_update`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pessimistic_writers_queue() {
    let dir = tmp();
    let store = open(&dir.path().join("store.redb"), "pess").await;
    let both = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let tasks: Vec<_> = (0..8u8)
        .map(|i| {
            let store = store.clone();
            let both = both.clone();
            tokio::spawn(async move {
                let mut attempts = Vec::new();
                for round in 0..5u8 {
                    let both = both.clone();
                    let c = store
                        .run(TxnOptions::pessimistic("kv.embedded.pess"), move |txn| {
                            let both = both.clone();
                            Box::pin(async move {
                                if round == 0 && txn.attempt() == 1 {
                                    both.wait().await;
                                }
                                txn.lock_keys([b"n".as_slice()]).await?;
                                txn.put(b"n", vec![i, round]).await
                            })
                        })
                        .await
                        .expect("committed");
                    attempts.push(c.attempts);
                }
                attempts
            })
        })
        .collect();
    for task in tasks {
        let attempts = task.await.expect("joined");
        assert!(attempts.iter().all(|&a| a == 1), "{attempts:?}");
    }
    let stats = handle(&store).stats();
    assert_eq!(stats.conflicts, 0);
    assert!(stats.commits >= 40);
}

// ---- the model check ----

/// One step of a schedule of interleaved transactions over the keys 0..4.
#[derive(Debug, Clone)]
enum Step {
    Get(u8),
    Put(u8, u8),
    Delete(u8),
    Lock(u8),
    Scan,
    ScanReverse,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0u8..4).prop_map(Step::Get),
        (0u8..4, any::<u8>()).prop_map(|(k, v)| Step::Put(k, v)),
        (0u8..4).prop_map(Step::Delete),
        (0u8..4).prop_map(Step::Lock),
        Just(Step::Scan),
        Just(Step::ScanReverse),
    ]
}

/// What a step or a commit observed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Value(Option<u8>),
    Pairs(Vec<(u8, u8)>),
    Done,
    Committed,
    Conflict,
}

/// The reference: snapshot isolation over versions numbered by commit order.
#[derive(Debug, Default)]
struct Model {
    versions: BTreeMap<u8, Vec<(u64, Option<u8>)>>,
    commits: u64,
}

#[derive(Debug, Default)]
struct ModelTxn {
    start: u64,
    writes: BTreeMap<u8, Option<u8>>,
    locks: BTreeSet<u8>,
}

impl Model {
    fn begin(&self) -> ModelTxn {
        ModelTxn {
            start: self.commits,
            ..ModelTxn::default()
        }
    }

    fn read(&self, txn: &ModelTxn, k: u8) -> Option<u8> {
        if let Some(w) = txn.writes.get(&k) {
            return *w;
        }
        self.versions
            .get(&k)
            .and_then(|vs| vs.iter().rev().find(|(c, _)| *c <= txn.start))
            .and_then(|(_, v)| *v)
    }

    fn step(&self, txn: &mut ModelTxn, step: &Step) -> Seen {
        match *step {
            Step::Get(k) => Seen::Value(self.read(txn, k)),
            Step::Put(k, v) => {
                txn.writes.insert(k, Some(v));
                Seen::Done
            }
            Step::Delete(k) => {
                txn.writes.insert(k, None);
                Seen::Done
            }
            Step::Lock(k) => {
                txn.locks.insert(k);
                Seen::Done
            }
            Step::Scan | Step::ScanReverse => {
                let mut pairs: Vec<(u8, u8)> = (0..4)
                    .filter_map(|k| self.read(txn, k).map(|v| (k, v)))
                    .collect();
                if matches!(step, Step::ScanReverse) {
                    pairs.reverse();
                }
                Seen::Pairs(pairs)
            }
        }
    }

    fn commit(&mut self, txn: ModelTxn) -> Seen {
        let touched: BTreeSet<u8> = txn.writes.keys().copied().chain(txn.locks).collect();
        let newer = touched.iter().any(|k| {
            self.versions
                .get(k)
                .is_some_and(|vs| vs.iter().any(|(c, _)| *c > txn.start))
        });
        if newer {
            return Seen::Conflict;
        }
        if touched.is_empty() {
            return Seen::Committed;
        }
        self.commits += 1;
        for (k, v) in txn.writes {
            self.versions.entry(k).or_default().push((self.commits, v));
        }
        Seen::Committed
    }
}

async fn real_step(txn: &mut embedded::Txn, step: &Step) -> Seen {
    let pairs = |p: Vec<(Vec<u8>, Vec<u8>)>| {
        Seen::Pairs(p.into_iter().map(|(k, v)| (k[0], v[0])).collect())
    };
    match *step {
        Step::Get(k) => Seen::Value(txn.get(&[k]).await.expect("get").map(|v| v[0])),
        Step::Put(k, v) => {
            txn.put(&[k], vec![v]).await.expect("put");
            Seen::Done
        }
        Step::Delete(k) => {
            txn.delete(&[k]).await.expect("delete");
            Seen::Done
        }
        Step::Lock(k) => {
            txn.lock_keys([[k]]).await.expect("lock");
            Seen::Done
        }
        Step::Scan => pairs(txn.scan(&[], None, 10).await.expect("scan")),
        Step::ScanReverse => pairs(txn.scan_reverse(&[], None, 10).await.expect("scan")),
    }
}

/// The model check's store: one file for every case, a fresh root each, in
/// a fixed directory reset on each run (a static is never dropped, so a
/// `TempDir` would be left behind).
fn model_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("loams-kv-model-check");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the model check's directory");
        dir.join("model.redb")
    })
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime")
    })
}

/// Runs `txns` (each a list of steps), interleaved by `order` (each entry
/// picks which unfinished transaction moves next: its begin, a step or its
/// commit), on the store and on the model; returns both traces.
async fn run_schedule(txns: &[Vec<Step>], order: &[usize]) -> (Vec<Seen>, Vec<Seen>) {
    let config = EmbeddedConfig {
        root: loams_kv::testing::random_root(),
        ..EmbeddedConfig::new(model_path().clone(), "model")
    };
    let h = Handle::open(config).await.expect("a handle");
    let mut model = Model::default();
    let n = txns.len();
    // 0 = not begun; 1..=len = steps done + 1; len + 1 = before commit.
    let mut at = vec![0usize; n];
    let mut real: Vec<Option<embedded::Txn>> = (0..n).map(|_| None).collect();
    let mut reference: Vec<Option<ModelTxn>> = (0..n).map(|_| None).collect();
    let (mut got, mut want) = (Vec::new(), Vec::new());
    let mut picks = order.iter().cycle();
    loop {
        let open: Vec<usize> = (0..n).filter(|&i| at[i] <= txns[i].len() + 1).collect();
        if open.is_empty() {
            break;
        }
        let i = open[picks.next().copied().unwrap_or(0) % open.len()];
        let pos = at[i];
        if pos == 0 {
            real[i] = Some(h.begin(Mode::Optimistic).await.expect("begin"));
            reference[i] = Some(model.begin());
        } else if pos <= txns[i].len() {
            let step = &txns[i][pos - 1];
            let txn = real[i].as_mut().expect("begun");
            got.push(real_step(txn, step).await);
            want.push(model.step(reference[i].as_mut().expect("begun"), step));
        } else {
            let txn = real[i].take().expect("begun");
            got.push(match txn.commit().await {
                Ok(_) => Seen::Committed,
                Err(TxnError::Conflict) => Seen::Conflict,
                Err(e) => panic!("commit: {e:?}"),
            });
            want.push(model.commit(reference[i].take().expect("begun")));
        }
        at[i] += 1;
    }
    // The final state agrees too.
    let mut snap = h
        .snapshot(h.now().await.expect("now"))
        .await
        .expect("a snapshot");
    got.push(Seen::Pairs(
        snap.scan(&[], None, 10)
            .await
            .expect("scan")
            .into_iter()
            .map(|(k, v)| (k[0], v[0]))
            .collect(),
    ));
    let last = model.begin();
    want.push(model.step(&mut ModelTxn { ..last }, &Step::Scan));
    (got, want)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Random interleavings of up to four transactions give the reference
    /// model's reads, scans, and commit and conflict outcomes.
    #[test]
    fn model_check_against_reference(
        txns in proptest::collection::vec(proptest::collection::vec(step(), 0..6), 1..5),
        order in proptest::collection::vec(0usize..4, 1..40),
    ) {
        let (got, want) = runtime().block_on(run_schedule(&txns, &order));
        prop_assert_eq!(got, want);
    }
}
