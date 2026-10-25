//! Tenant placements and product roots on a real store (KV1 plan KV0 Task
//! 6, review focus 4): on the embedded store always, on TiKV when
//! `LOAMS_TEST_PD` is set.

use loams_kv::tenant::{self, PRODUCT_CQL, PRODUCT_REDIS, Placement};
use loams_kv::testing::{self, TempDir};
use loams_kv::{EmbeddedConfig, Pair, Store, StoreConfig, TxnOptions};

/// The base configurations to place tenants on, each with a shared and a
/// dedicated keyspace name of its backend, under a fresh random root.
async fn bases(dir: &TempDir) -> Vec<(StoreConfig, &'static str, &'static str)> {
    let embedded = StoreConfig::Embedded(EmbeddedConfig {
        root: testing::random_root(),
        ..EmbeddedConfig::new(dir.path().join("store.redb"), "pool")
    });
    let mut out = vec![(embedded, "pool", "dedicated")];
    if let Some(cluster) = testing::cluster().await {
        let mut config = cluster.config(testing::TEST_LIVE);
        config.root = testing::random_root();
        out.push((
            StoreConfig::Tikv(config),
            testing::TEST_LIVE,
            testing::TEST_META,
        ));
    }
    out
}

async fn open(config: StoreConfig) -> Store {
    Store::open(config).await.expect("a store")
}

async fn write(store: &Store, value: &[u8]) {
    let value = value.to_vec();
    store
        .run(TxnOptions::new("kv.test.tenant"), move |txn| {
            let value = value.clone();
            Box::pin(async move {
                for k in [b"a".as_slice(), b"b", b"\x00", b"\xff"] {
                    txn.put(k, value.clone()).await?;
                }
                Ok(())
            })
        })
        .await
        .expect("written");
}

async fn all(store: &Store) -> Vec<Pair> {
    let mut snap = store
        .snapshot(store.now().await.expect("now"))
        .await
        .expect("a snapshot");
    snap.scan(b"", None, 100).await.expect("a scan")
}

#[tokio::test]
async fn tenants_are_isolated() {
    let dir = TempDir::new_in(std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))).expect("a dir");
    for (base, shared, dedicated) in bases(&dir).await {
        // `t` and `t1` share a keyspace, and `t`'s prefix is a prefix of
        // `t1`'s; `d` has a keyspace of its own.
        let placements = [
            Placement::Shared {
                keyspace: shared.into(),
                prefix: b"t".to_vec(),
            },
            Placement::Shared {
                keyspace: shared.into(),
                prefix: b"t1".to_vec(),
            },
            Placement::Shared {
                keyspace: shared.into(),
                prefix: b"t\x00".to_vec(),
            },
            Placement::Dedicated {
                keyspace: dedicated.into(),
            },
        ];
        let mut stores = Vec::new();
        for (i, placement) in placements.iter().enumerate() {
            let config = tenant::store_config(&base, placement);
            assert_eq!(config.keyspace(), placement.keyspace());
            let store = open(config).await;
            write(&store, format!("tenant-{i}").as_bytes()).await;
            stores.push(store);
        }
        for (i, store) in stores.iter().enumerate() {
            let seen = all(store).await;
            assert_eq!(
                seen.len(),
                4,
                "{:?} tenant {i} sees only its four keys",
                base.backend()
            );
            let own = format!("tenant-{i}").into_bytes();
            assert!(
                seen.iter().all(|(_, v)| *v == own),
                "tenant {i} saw {seen:?}"
            );
        }
        // The shared keyspace's base root holds the three shared tenants'
        // keys, and none of the dedicated one's.
        let pool = open(base.with_keyspace(shared)).await;
        assert_eq!(all(&pool).await.len(), 12);
    }
}

#[tokio::test]
async fn product_roots_are_disjoint() {
    let dir = TempDir::new_in(std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))).expect("a dir");
    for (base, shared, _) in bases(&dir).await {
        let tenant = tenant::store_config(
            &base,
            &Placement::Shared {
                keyspace: shared.into(),
                prefix: b"ns".to_vec(),
            },
        );
        let redis = open(tenant::product_root(&tenant, PRODUCT_REDIS)).await;
        let cql = open(tenant::product_root(&tenant, PRODUCT_CQL)).await;
        write(&redis, b"redis").await;
        write(&cql, b"cql").await;
        let r = all(&redis).await;
        let c = all(&cql).await;
        assert_eq!(r.len(), 4);
        assert!(r.iter().all(|(_, v)| v == b"redis"), "{r:?}");
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|(_, v)| v == b"cql"), "{c:?}");
        // From the tenant's root, both products, each under its tag.
        let both = all(&open(tenant).await).await;
        assert_eq!(both.len(), 8);
        assert!(both[..4].iter().all(|(k, _)| k[0] == PRODUCT_CQL));
        assert!(both[4..].iter().all(|(k, _)| k[0] == PRODUCT_REDIS));
    }
}
