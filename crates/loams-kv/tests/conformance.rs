//! The store seam's conformance (LV1 Tasks 20–21): `kv_conformance!` holds
//! the embedded and the TiKV store to the same transaction semantics; the
//! tikv variant behaves as `loams_tikv::Tikv`, `Ts` keeps TSO layout, and
//! the tuple codec is the one `loams-tikv` exports.

use std::borrow::Cow;

use loams_kv::testing::{self, TEST_LIVE};
use loams_kv::{Backend, Store, StoreConfig, Ts, TxnError, TxnOptions, tuple};
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

loams_kv::kv_conformance!(
    embedded,
    loams_kv::testing::embedded_factory(env!("CARGO_TARGET_TMPDIR"))
);
loams_kv::kv_conformance!(tikv, loams_kv::testing::tikv_factory());

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
