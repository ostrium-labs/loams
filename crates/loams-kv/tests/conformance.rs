//! The store seam's conformance (LV1 Task 20): the tikv variant behaves as
//! `loams_tikv::Tikv`, `Ts` keeps TSO layout, and the tuple codec is the
//! one `loams-tikv` exports. Task 21 adds `kv_conformance!` cases here.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use loams_kv::testing::{self, TEST_LIVE};
use loams_kv::{
    Backend, Fault, FaultPlan, FaultPoint, KvError, Store, StoreConfig, Ts, TxnError, TxnOptions,
    tuple,
};
use loams_tikv::TimestampExt;
use proptest::prelude::*;

/// A TiKV handle and a `Store` opened on the same keyspace and root.
async fn pair() -> Option<(loams_tikv::Tikv, Store)> {
    let cluster = testing::cluster().await?;
    let config = cluster.config(TEST_LIVE);
    let tikv = loams_tikv::Tikv::connect(config.clone())
        .await
        .expect("a TiKV handle");
    let store = Store::open(StoreConfig::Tikv(config))
        .await
        .expect("a TiKV store");
    Some((tikv, store))
}

fn kv(k: &str, v: &str) -> (Vec<u8>, Vec<u8>) {
    (k.as_bytes().to_vec(), v.as_bytes().to_vec())
}

#[tokio::test]
async fn tikv_store_roundtrip() {
    let Some((tikv, store)) = pair().await else {
        return;
    };
    assert_eq!(store.backend(), Backend::Tikv);
    assert_eq!(store.keyspace(), tikv.keyspace());
    assert_eq!(store.root(), tikv.root());
    assert_eq!(store.key(b"k"), tikv.key(b"k"));

    // Writes through the store, reads through the handle.
    let committed = store
        .run(TxnOptions::new("kv.test.put"), |txn| {
            Box::pin(async move {
                assert_eq!(txn.attempt(), 1);
                for (k, v) in [kv("a", "1"), kv("b", "2"), kv("c", "3"), kv("d", "4")] {
                    txn.put(&k, v).await?;
                }
                Ok(txn.start_ts())
            })
        })
        .await
        .expect("committed");
    let start = committed.value;
    assert!(committed.commit_ts > start, "commit after start");
    assert_eq!(committed.attempts, 1);

    let mut raw = tikv
        .snapshot(loams_tikv::Timestamp::from_version(committed.commit_ts.0))
        .await
        .expect("a raw snapshot");
    let mut snap = store
        .snapshot(committed.commit_ts)
        .await
        .expect("a store snapshot");
    assert_eq!(snap.ts(), committed.commit_ts);
    assert_eq!(
        snap.get(b"b").await.expect("get"),
        raw.get(b"b").await.expect("raw get")
    );
    assert_eq!(snap.get(b"b").await.expect("get"), Some(b"2".to_vec()));
    assert_eq!(
        snap.scan(b"a", Some(b"d"), 10).await.expect("scan"),
        raw.scan(b"a", Some(b"d"), 10).await.expect("raw scan")
    );
    assert_eq!(
        snap.scan(b"a", None, 10).await.expect("scan"),
        vec![kv("a", "1"), kv("b", "2"), kv("c", "3"), kv("d", "4")]
    );
    assert_eq!(
        snap.scan_reverse(b"a", None, 2)
            .await
            .expect("reverse scan"),
        raw.scan_reverse(b"a", None, 2)
            .await
            .expect("raw reverse scan")
    );
    assert_eq!(
        snap.scan_reverse(b"a", None, 2)
            .await
            .expect("reverse scan"),
        vec![kv("d", "4"), kv("c", "3")]
    );
    assert_eq!(
        snap.batch_get([b"c".as_slice(), b"zz", b"a"])
            .await
            .expect("batch get"),
        vec![kv("a", "1"), kv("c", "3")]
    );

    // A snapshot before the commit sees nothing.
    let mut before = store.snapshot(start).await.expect("an older snapshot");
    assert_eq!(before.get(b"a").await.expect("get"), None);

    // Writes through the handle, reads through the store's transaction.
    tikv.run(loams_tikv::TxnOptions::new("kv.test.raw"), |txn| {
        Box::pin(async move {
            txn.put(b"e", b"5".to_vec()).await?;
            txn.delete(b"a").await
        })
    })
    .await
    .expect("a raw commit");
    let seen = store
        .run(TxnOptions::new("kv.test.read"), |txn| {
            Box::pin(async move {
                let all = txn.scan(b"", None, 10).await?;
                let e = txn.get(b"e").await?;
                txn.lock_keys([b"e".as_slice()]).await?;
                Ok((all, e))
            })
        })
        .await
        .expect("a read")
        .value;
    assert_eq!(
        seen.0,
        vec![kv("b", "2"), kv("c", "3"), kv("d", "4"), kv("e", "5")]
    );
    assert_eq!(seen.1, Some(b"5".to_vec()));

    // insert of a present key fails with the same class as on the handle.
    let err = store
        .run(TxnOptions::new("kv.test.insert"), |txn| {
            Box::pin(async move { txn.insert(b"b", b"x".to_vec()).await })
        })
        .await
        .expect_err("the key exists");
    assert!(matches!(err, TxnError::AlreadyExists(_)), "{err:?}");

    // now() is a TSO timestamp after every commit so far.
    let now = store.now().await.expect("now");
    assert!(now > committed.commit_ts);
}

/// One fault at one point of the first attempt of `op`, then nothing.
struct Once {
    op: &'static str,
    point: FaultPoint,
    fault: Fault,
}

impl FaultPlan for Once {
    fn at(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault> {
        (op == self.op && point == self.point && attempt == 1).then_some(self.fault)
    }
}

/// Every fault at every point reaches the TiKV runner through the
/// adapter with the effect `loams_tikv::faults` documents: before the
/// commit, `Refuse`, `Conflict` and `LoseAck` cost one attempt (a lost
/// acknowledgement there resolves as not applied through the commit
/// token); after the commit they are unknown outcomes that the token
/// resolves as committed; `Delay` only delays. Each case's counter ends at
/// 1: no attempt applied twice.
#[tokio::test]
async fn tikv_fault_adapter_covers_every_point_and_fault() {
    let Some(store) = testing::tikv().await else {
        return;
    };
    const OP: &str = "kv.test.fault_table";
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
            let started = std::time::Instant::now();
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

            // With one attempt the fault's own class surfaces: each fault
            // maps to its own `loams_tikv` fault, not to a look-alike.
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
}

/// A snapshot below the GC read window stays readable while a barrier of
/// the store covers it, is refused once the barrier is deleted, and a
/// barrier below the cluster's safe point is refused.
#[tokio::test]
async fn tikv_barrier_holds_old_snapshots_and_refuses_below_safe_point() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    // A 62 s life time leaves a 2 s read window.
    let mut config = cluster.config(TEST_LIVE);
    config.gc_life_time = Duration::from_secs(62);
    let store = Store::open(StoreConfig::Tikv(config))
        .await
        .expect("a TiKV store");
    let put = |v: &'static [u8]| {
        store.run(TxnOptions::new("kv.test.barrier"), move |txn| {
            Box::pin(async move { txn.put(b"k", v.to_vec()).await })
        })
    };
    put(b"v1").await.expect("v1");
    let at = store.now().await.expect("now");
    put(b"v2").await.expect("v2");

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
    assert!(matches!(
        store.snapshot(at).await,
        Err(KvError::Tikv(loams_tikv::TikvError::GcSafePoint { .. }))
    ));

    // A barrier below the cluster's safe point is refused. A fresh
    // cluster's safe point is 0: one GC round moves it past version 1.
    let tikv = store.as_tikv().expect("a TiKV store").clone();
    loams_tikv::GcLoop::new(tikv, loams_tikv::GcConfig::default())
        .expect("a loop")
        .run_once()
        .await
        .expect("a GC round");
    let below = store
        .barrier("kv-test/below", Ts(1), Duration::from_secs(60))
        .await;
    match below {
        Err(KvError::Tikv(loams_tikv::TikvError::BarrierBelowSafePoint {
            min_safe_point, ..
        })) => {
            assert!(min_safe_point > 1);
        }
        Ok(b) => {
            // Never leave it to hold the safe point for other tests.
            b.delete().await.expect("deleted");
            panic!("a barrier at ts 1 was accepted");
        }
        Err(other) => panic!("expected BarrierBelowSafePoint, got {other:?}"),
    }
}

#[tokio::test]
async fn embedded_store_is_a_stub_until_task_21() {
    let err = Store::open(StoreConfig::Embedded(loams_kv::EmbeddedConfig {
        path: std::path::PathBuf::from("unused"),
        keyspace: "k".into(),
        root: Vec::new(),
        gc_life_time: Duration::from_secs(600),
    }))
    .await
    .expect_err("no embedded backend yet");
    assert_eq!(err.to_string(), "embedded backend arrives in Task 21");
}

#[test]
fn ts_parts_roundtrip() {
    for (ms, logical) in [
        (0, 0),
        (1, 1),
        (1_791_474_409_383, 0),
        (1_791_474_409_383, 262_143),
        // The largest physical part tikv-client converts (its version is an i64).
        ((1 << 45) - 1, (1 << 18) - 1),
    ] {
        let ts = Ts::from_parts(ms, logical);
        assert_eq!(ts.physical_ms(), ms);
        assert_eq!(ts.logical(), logical);
        assert_eq!(ts.0, (ms << 18) | u64::from(logical), "TSO layout");
        // The same version as tikv-client's.
        let tso = loams_tikv::Timestamp::from_version(ts.0);
        assert_eq!(loams_tikv::Tikv::physical_ms(&tso), ms);
        assert_eq!(u32::try_from(tso.logical).expect("18 bits"), logical);
    }
    assert!(Ts::from_parts(5, 0) > Ts::from_parts(4, 262_143));
    assert!(Ts::from_parts(5, 1) > Ts::from_parts(5, 0));
}

fn elem() -> impl Strategy<Value = tuple::Elem<'static>> {
    let leaf = prop_oneof![
        Just(tuple::Elem::Null),
        any::<i64>().prop_map(tuple::Elem::I64),
        any::<f64>().prop_map(tuple::Elem::F64),
        any::<bool>().prop_map(tuple::Elem::Bool),
        ".*".prop_map(|s| tuple::Elem::Str(Cow::Owned(s))),
        proptest::collection::vec(any::<u8>(), 0..24)
            .prop_map(|b| tuple::Elem::Bytes(Cow::Owned(b))),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        proptest::collection::vec(inner, 0..4).prop_map(tuple::Elem::Array)
    })
}

proptest! {
    #[test]
    fn tuple_codec_reexport_is_identical(elems in proptest::collection::vec(elem(), 0..6)) {
        let mut ours = Vec::new();
        let mut theirs = Vec::new();
        for e in &elems {
            loams_kv::tuple::encode(&mut ours, e);
            loams_tikv::tuple::encode(&mut theirs, e);
        }
        prop_assert_eq!(&ours, &theirs);
        prop_assert_eq!(
            loams_kv::tuple::successor(&ours),
            loams_tikv::tuple::successor(&theirs)
        );
        let mut at = 0;
        while at < ours.len() {
            let (a, n) = loams_kv::tuple::decode(&ours[at..]).expect("decodes");
            let (b, m) = loams_tikv::tuple::decode(&theirs[at..]).expect("decodes");
            prop_assert_eq!(n, m);
            let (mut x, mut y) = (Vec::new(), Vec::new());
            loams_kv::tuple::encode(&mut x, &a);
            loams_tikv::tuple::encode(&mut y, &b);
            prop_assert_eq!(x, y);
            at += n;
        }
    }
}
