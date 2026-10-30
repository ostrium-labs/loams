//! The raw store's behaviour, on the in-memory backend always and on TiKV
//! when `LOAMS_TEST_PD` names a cluster (API v2, `storage.enable-ttl = true`).
//! Every case runs under a fresh random root, in its own keyspace
//! (`loams_test_raw`), so raw keys never meet the transactional tests'.

use std::time::Duration;

use loams_kv::{RawStore, RawTtl};

/// The keyspace the TiKV cases use; raw data never shares one with the
/// transactional tests.
const TEST_RAW: &str = "loams_test_raw";

async fn stores() -> Vec<RawStore> {
    let mut stores = vec![RawStore::memory(loams_kv::testing::random_root())];
    if let Some(cluster) = loams_tikv::testing::cluster().await {
        let config = loams_kv::TikvConfig {
            root: loams_kv::testing::random_root(),
            pd_http: Some(cluster.pd_http.clone()),
            ..loams_kv::TikvConfig::new(cluster.pd.clone(), TEST_RAW)
        };
        stores.push(RawStore::open(&config).await.expect("a raw store on TiKV"));
    }
    stores
}

#[tokio::test]
async fn get_put_delete() {
    for s in stores().await {
        assert_eq!(s.get(b"a").await.unwrap(), None, "{s:?}");
        s.put(b"a", b"1".to_vec(), None).await.unwrap();
        assert_eq!(s.get(b"a").await.unwrap(), Some(b"1".to_vec()), "{s:?}");
        s.put(b"a", b"2".to_vec(), None).await.unwrap();
        assert_eq!(s.get(b"a").await.unwrap(), Some(b"2".to_vec()), "{s:?}");
        s.delete(b"a").await.unwrap();
        assert_eq!(s.get(b"a").await.unwrap(), None, "{s:?}");
        // Deleting an absent key is not an error.
        s.delete(b"a").await.unwrap();
    }
}

#[tokio::test]
async fn batches_leave_absent_keys_out() {
    for s in stores().await {
        let pairs: Vec<_> = (0..20u8).map(|i| (vec![b'k', i], vec![i])).collect();
        s.batch_put(pairs, None).await.unwrap();
        let mut got = s
            .batch_get([vec![b'k', 3], vec![b'k', 99], vec![b'k', 7]])
            .await
            .unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![(vec![b'k', 3], vec![3]), (vec![b'k', 7], vec![7])],
            "{s:?}"
        );
        s.batch_delete([vec![b'k', 3], vec![b'k', 7]])
            .await
            .unwrap();
        assert!(
            s.batch_get([vec![b'k', 3], vec![b'k', 7]])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.get(&[b'k', 4]).await.unwrap(), Some(vec![4]));
    }
}

#[tokio::test]
async fn compare_and_swap_is_conditional() {
    for s in stores().await {
        // Absent expected: creates.
        let (seen, ok) = s.compare_and_swap(b"c", None, b"1".to_vec()).await.unwrap();
        assert_eq!((seen, ok), (None, true), "{s:?}");
        // Absent expected on a present key: refused, with the value seen.
        let (seen, ok) = s.compare_and_swap(b"c", None, b"x".to_vec()).await.unwrap();
        assert_eq!((seen.as_deref(), ok), (Some(&b"1"[..]), false), "{s:?}");
        // Wrong expected: refused.
        let (_, ok) = s
            .compare_and_swap(b"c", Some(b"0"), b"x".to_vec())
            .await
            .unwrap();
        assert!(!ok, "{s:?}");
        // Right expected: swaps.
        let (_, ok) = s
            .compare_and_swap(b"c", Some(b"1"), b"2".to_vec())
            .await
            .unwrap();
        assert!(ok, "{s:?}");
        assert_eq!(s.get(b"c").await.unwrap(), Some(b"2".to_vec()));
    }
}

#[tokio::test]
async fn concurrent_increments_through_cas_lose_none() {
    for s in stores().await {
        s.put(b"n", b"0".to_vec(), None).await.unwrap();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let s = s.clone();
                tokio::spawn(async move {
                    for _ in 0..25 {
                        loop {
                            let cur = s.get(b"n").await.unwrap().unwrap();
                            let n: u64 = std::str::from_utf8(&cur).unwrap().parse().unwrap();
                            let next = (n + 1).to_string().into_bytes();
                            if s.compare_and_swap(b"n", Some(&cur), next).await.unwrap().1 {
                                break;
                            }
                        }
                    }
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(s.get(b"n").await.unwrap(), Some(b"200".to_vec()), "{s:?}");
    }
}

#[tokio::test]
async fn ttl_expires_and_cas_clears_it() {
    for s in stores().await {
        s.put(b"p", b"v".to_vec(), None).await.unwrap();
        assert_eq!(s.ttl(b"p").await.unwrap(), RawTtl::Persistent, "{s:?}");
        assert_eq!(s.ttl(b"none").await.unwrap(), RawTtl::Absent, "{s:?}");
        s.put(b"t", b"v".to_vec(), Some(Duration::from_millis(1200)))
            .await
            .unwrap();
        match s.ttl(b"t").await.unwrap() {
            RawTtl::Expires(left) => assert!(left <= Duration::from_secs(2), "{left:?}"),
            other => panic!("{s:?}: expected a TTL, got {other:?}"),
        }
        // A CAS writes without a TTL (the module docs' rule).
        s.put(b"u", b"1".to_vec(), Some(Duration::from_secs(60)))
            .await
            .unwrap();
        assert!(
            s.compare_and_swap(b"u", Some(b"1"), b"2".to_vec())
                .await
                .unwrap()
                .1
        );
        assert_eq!(s.ttl(b"u").await.unwrap(), RawTtl::Persistent, "{s:?}");
        // 1.2 s rounds up to 2 s on TiKV; past it the key reads as absent.
        tokio::time::sleep(Duration::from_millis(3100)).await;
        assert_eq!(s.get(b"t").await.unwrap(), None, "{s:?}");
        assert_eq!(s.ttl(b"t").await.unwrap(), RawTtl::Absent, "{s:?}");
    }
}

#[tokio::test]
async fn roots_are_isolated_and_scans_stay_inside() {
    for s in stores().await {
        let other = s.with_root(loams_kv::testing::random_root());
        s.put(b"a1", b"s".to_vec(), None).await.unwrap();
        s.put(b"a2", b"s".to_vec(), None).await.unwrap();
        s.put(b"b1", b"s".to_vec(), None).await.unwrap();
        other.put(b"a1", b"o".to_vec(), None).await.unwrap();
        assert_eq!(s.get(b"a1").await.unwrap(), Some(b"s".to_vec()), "{s:?}");
        assert_eq!(
            other.get(b"a1").await.unwrap(),
            Some(b"o".to_vec()),
            "{s:?}"
        );
        let keys: Vec<Vec<u8>> = s
            .scan(b"a", Some(b"b"), 10)
            .await
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec![b"a1".to_vec(), b"a2".to_vec()], "{s:?}");
        let all = s.scan(b"", None, 10).await.unwrap();
        assert_eq!(
            all.len(),
            3,
            "{s:?}: a scan to the root's end stays in the root"
        );
    }
}
