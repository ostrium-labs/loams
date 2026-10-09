//! The conformance suite of [`PgControlStore`] (PG2 Task 3): the same cases
//! on the local (embedded) and the TiKV backend, as `metastore_conformance!`
//! and `kv_conformance!` do for theirs.
//!
//! [`pg_control_store_conformance!`](crate::pg_control_store_conformance)
//! expands to one `#[tokio::test]` per case for a [`Factory`]:
//!
//! ```ignore
//! loams_pg_control::pg_control_store_conformance!(
//!     loams_pg_control::store::conformance::local_factory(env!("CARGO_TARGET_TMPDIR"))
//! );
//! ```
//!
//! A TiKV factory yields no store (and the case prints a `skipped:` line)
//! when `LOAMS_TEST_PD` is unset. `undetermined_is_surfaced` and
//! `lost_ack_is_resolved_by_its_token` inject faults through `loams-kv`'s
//! fault hooks, so they need the feature `faults`.

use std::path::Path;
#[cfg(feature = "faults")]
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use loams_kv::Store;

use super::{
    KvControlStore, Page, PgControlStore, StoreError, StoreEvent, StoreOptions, Versioned,
};
use crate::model::{BranchKey, BranchPrefix, BranchRec, BranchState, Record, project_lease};

/// Expands to one `#[tokio::test]` per conformance case, each calling
/// `crate::store::conformance::<case>($factory)` with the factory
/// expression (evaluated once per case).
#[macro_export]
macro_rules! pg_control_store_conformance {
    ($factory:expr) => {
        $crate::pg_control_store_conformance!(@cases $factory;
            put_absent_then_conflict,
            cas_on_version,
            delete_requires_version,
            list_pages_in_key_order,
            lease_fences_old_holder,
            watch_sees_put_and_delete,
            undetermined_is_surfaced,
            lost_ack_is_resolved_by_its_token
        );
    };
    (@cases $factory:expr; $($case:ident),* $(,)?) => {
        $(
            #[::tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $case() {
                $crate::store::conformance::$case($factory).await;
            }
        )*
    };
}

/// Opens fresh `loams-kv` stores of one backend, each under its own root.
#[derive(Debug, Clone)]
pub struct Factory {
    kv: loams_kv::testing::Factory,
}

impl Factory {
    /// A factory over `loams-kv`'s.
    pub fn new(kv: loams_kv::testing::Factory) -> Self {
        Factory { kv }
    }

    /// A fresh `loams-kv` store, or `None` when the backend is unavailable.
    pub async fn kv(&self) -> Option<Store> {
        self.kv.store(loams_kv::testing::Spec::fresh()).await
    }

    /// A fresh control store with `options`, or `None`.
    pub async fn store(&self, options: StoreOptions) -> Option<KvControlStore> {
        Some(KvControlStore::new(self.kv().await?, options))
    }
}

/// Local stores: one redb file in a fresh directory under `base` (tests pass
/// `env!("CARGO_TARGET_TMPDIR")`).
pub fn local_factory(base: impl AsRef<Path>) -> Factory {
    Factory::new(loams_kv::testing::embedded_factory(base))
}

/// TiKV stores on the metastore's test keyspace (`loams_test_meta`), each
/// under a fresh random root, or none when `LOAMS_TEST_PD` is unset.
#[cfg(feature = "tikv")]
pub fn tikv_factory() -> Factory {
    use loams_kv::testing::{Spec, TEST_META, cluster};
    Factory::new(loams_kv::testing::Factory::new(
        loams_kv::Backend::Tikv,
        |spec: Spec| {
            Box::pin(async move {
                let cluster = cluster().await?;
                let mut config = cluster.config(TEST_META);
                config.root = spec.root;
                Some(super::tikv::open_store(config).await.expect("a TiKV store"))
            })
        },
    ))
}

/// A short poll, so watches see writes of other handles quickly.
fn options() -> StoreOptions {
    StoreOptions {
        poll: Duration::from_millis(50),
        ..StoreOptions::default()
    }
}

fn branch(project: &str, id: &str, name: &str) -> BranchRec {
    BranchRec {
        project_id: project.into(),
        id: id.into(),
        name: name.into(),
        timeline_id: [7; 16],
        parent_id: None,
        ancestor_lsn: None,
        expires_at_ms: None,
        protected: false,
        stripe_size: None,
        shards: Vec::new(),
        state: BranchState::Creating,
        created_at_ms: 1,
    }
}

fn bkey(project: &str, id: &str) -> BranchKey {
    BranchKey {
        project_id: project.into(),
        id: id.into(),
    }
}

async fn get(store: &KvControlStore, key: &BranchKey) -> Option<Versioned<BranchRec>> {
    store.get::<BranchRec>(key).await.expect("get")
}

/// Creating an absent record works once; a second create, or a replace of
/// an absent record, is a conflict naming the current version.
pub async fn put_absent_then_conflict(factory: Factory) {
    let Some(store) = factory.store(options()).await else {
        return;
    };
    let rec = branch("prj-1", "br-1", "main");
    let key = bkey("prj-1", "br-1");
    assert_eq!(get(&store, &key).await, None);
    let v1 = store.api_writer().put(&rec, None).await.expect("create");
    assert_eq!(
        store.api_writer().put(&rec, None).await,
        Err(StoreError::Conflict { current: Some(v1) })
    );
    assert_eq!(
        get(&store, &key).await,
        Some(Versioned {
            record: rec.clone(),
            version: v1
        })
    );
    let other = branch("prj-1", "br-2", "dev");
    assert_eq!(
        store.api_writer().put(&other, Some(v1)).await,
        Err(StoreError::Conflict { current: None })
    );
    assert_eq!(get(&store, &bkey("prj-1", "br-2")).await, None);
}

/// A replace needs the current version; each write gets a larger one.
pub async fn cas_on_version(factory: Factory) {
    let Some(store) = factory.store(options()).await else {
        return;
    };
    let key = bkey("prj-1", "br-1");
    let mut rec = branch("prj-1", "br-1", "main");
    let v1 = store.api_writer().put(&rec, None).await.expect("create");
    rec.state = BranchState::Ready;
    let v2 = store
        .api_writer()
        .put(&rec, Some(v1))
        .await
        .expect("replace v1");
    assert!(v2 > v1, "{v2} > {v1}");
    let mut stale = rec.clone();
    stale.state = BranchState::Failed;
    assert_eq!(
        store.api_writer().put(&stale, Some(v1)).await,
        Err(StoreError::Conflict { current: Some(v2) })
    );
    let got = get(&store, &key).await.expect("present");
    assert_eq!((got.record.state, got.version), (BranchState::Ready, v2));

    // A re-created record never reuses a version.
    store
        .api_writer()
        .delete::<BranchRec>(&key, v2)
        .await
        .expect("delete");
    let v3 = store.api_writer().put(&rec, None).await.expect("re-create");
    assert!(v3 > v2, "{v3} > {v2}");
}

/// A delete needs the current version, and a missing record is `NotFound`.
pub async fn delete_requires_version(factory: Factory) {
    let Some(store) = factory.store(options()).await else {
        return;
    };
    let key = bkey("prj-1", "br-1");
    let v = store
        .api_writer()
        .put(&branch("prj-1", "br-1", "main"), None)
        .await
        .expect("create");
    assert_eq!(
        store
            .api_writer()
            .delete::<BranchRec>(&key, v.wrapping_sub(1))
            .await,
        Err(StoreError::Conflict { current: Some(v) })
    );
    assert!(get(&store, &key).await.is_some());
    store
        .api_writer()
        .delete::<BranchRec>(&key, v)
        .await
        .expect("delete at the current version");
    assert_eq!(get(&store, &key).await, None);
    assert_eq!(
        store.api_writer().delete::<BranchRec>(&key, v).await,
        Err(StoreError::NotFound)
    );
}

/// Listings page through their prefix in key order, and only their prefix.
pub async fn list_pages_in_key_order(factory: Factory) {
    let Some(store) = factory.store(options()).await else {
        return;
    };
    let ids = ["br-5", "br-1", "br-7", "br-3", "br-2", "br-6", "br-4"];
    for id in ids {
        store
            .api_writer()
            .put(&branch("prj-1", id, id), None)
            .await
            .expect("create");
    }
    // A neighbour whose id extends the prefix's project id, and another.
    for (p, id) in [("prj-10", "br-0"), ("prj-0", "br-9")] {
        store
            .api_writer()
            .put(&branch(p, id, id), None)
            .await
            .expect("create");
    }
    let prefix = BranchPrefix {
        project_id: "prj-1".into(),
    };
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    let mut page = Page::first(3);
    loop {
        let (records, next) = store
            .list::<BranchRec>(&prefix, page.clone())
            .await
            .expect("list");
        sizes.push(records.len());
        seen.extend(records.into_iter().map(|v| v.record.id));
        match next {
            Some(token) => page = Page::after(3, token),
            None => break,
        }
    }
    let mut want: Vec<String> = ids.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(seen, want);
    assert_eq!(sizes, [3, 3, 1]);

    let (all, next) = store
        .list::<BranchRec>(&prefix, Page::default())
        .await
        .expect("list");
    assert_eq!((all.len(), next), (7, None));

    // A token of another prefix, or none at all, is refused.
    let (_, other_token) = store
        .list::<BranchRec>(
            &BranchPrefix {
                project_id: "prj-0".into(),
            },
            Page::first(0),
        )
        .await
        .expect("list");
    assert_eq!(other_token, None);
    for token in ["not hex", "582f70726a2d302f62722d39"] {
        let r = store
            .list::<BranchRec>(&prefix, Page::after(3, token))
            .await;
        assert!(
            matches!(r, Err(StoreError::InvalidArgument(_))),
            "{token}: {r:?}"
        );
    }
}

/// A takeover of an expired lease fences every write of the old holder;
/// a held lease is refused to another holder and renewed for its own.
pub async fn lease_fences_old_holder(factory: Factory) {
    let Some(store) = factory.store(options()).await else {
        return;
    };
    let scope = project_lease("prj-1");
    // Long enough that the calls before the expiry never outlast it on a
    // loaded TiKV (600 ms did, once in a parallel run).
    let ttl = Duration::from_secs(3);
    let a = store
        .acquire_lease(&scope, "pg-control-a", ttl)
        .await
        .expect("a takes the lease");
    assert_eq!(a.epoch(), 1);
    let held = store.acquire_lease(&scope, "pg-control-b", ttl).await;
    assert!(
        matches!(&held, Err(StoreError::Held { holder, .. }) if holder == "pg-control-a"),
        "{held:?}"
    );
    assert_eq!(
        store.acquire_lease(&scope, "pg-control-a", ttl).await,
        Ok(a.clone()),
        "a renewal keeps the epoch"
    );
    let key = bkey("prj-1", "br-1");
    let v1 = store
        .put(&branch("prj-1", "br-1", "main"), None, &a)
        .await
        .expect("a writes under its fence");

    tokio::time::sleep(ttl + Duration::from_millis(500)).await;
    let b = store
        .acquire_lease(&scope, "pg-control-b", ttl)
        .await
        .expect("b takes the expired lease");
    assert_eq!(b.epoch(), 2);

    let mut late = branch("prj-1", "br-1", "main");
    late.state = BranchState::Failed;
    assert_eq!(
        store.put(&late, Some(v1), &a).await,
        Err(StoreError::Fenced)
    );
    assert_eq!(
        store.delete::<BranchRec>(&key, v1, &a).await,
        Err(StoreError::Fenced)
    );
    assert_eq!(get(&store, &key).await.expect("present").version, v1);
    store
        .put(&late, Some(v1), &b)
        .await
        .expect("b writes under its fence");

    // Scopes are checked.
    for bad in ["e/m/node", "pg/prj-1", "e/pg/"] {
        let r = store.acquire_lease(bad, "x", ttl).await;
        assert!(
            matches!(r, Err(StoreError::InvalidArgument(_))),
            "{bad}: {r:?}"
        );
    }
    let r = store.acquire_lease(&scope, "b", Duration::ZERO).await;
    assert!(matches!(r, Err(StoreError::InvalidArgument(_))), "{r:?}");
}

/// A watch sends what its prefix holds, `Synced`, then each put and delete
/// under its prefix, and nothing of other prefixes; another handle's writes
/// reach it too.
pub async fn watch_sees_put_and_delete(factory: Factory) {
    let Some(kv) = factory.kv().await else {
        return;
    };
    let store = KvControlStore::new(kv.clone(), options());
    let other_handle = KvControlStore::new(kv, options());
    let v0 = store
        .api_writer()
        .put(&branch("prj-1", "br-0", "main"), None)
        .await
        .expect("create");
    let prefix = BranchRec::encode_prefix(&BranchPrefix {
        project_id: "prj-1".into(),
    })
    .expect("a prefix");
    let key = |id: &str| BranchRec::encode_key(&bkey("prj-1", id)).expect("a key");
    let mut events = store.watch(&prefix);
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(10), events.next())
            .await
            .expect("an event within 10 s")
            .expect("the watch does not end")
    };
    assert_eq!(
        next().await,
        StoreEvent::Put {
            key: key("br-0"),
            version: v0
        }
    );
    assert_eq!(next().await, StoreEvent::Synced);

    store
        .api_writer()
        .put(&branch("prj-2", "br-9", "main"), None)
        .await
        .expect("a write elsewhere");
    let v1 = store
        .api_writer()
        .put(&branch("prj-1", "br-1", "dev"), None)
        .await
        .expect("create");
    assert_eq!(
        next().await,
        StoreEvent::Put {
            key: key("br-1"),
            version: v1
        }
    );
    store
        .api_writer()
        .delete::<BranchRec>(&bkey("prj-1", "br-1"), v1)
        .await
        .expect("delete");
    assert_eq!(next().await, StoreEvent::Delete { key: key("br-1") });

    let v2 = other_handle
        .api_writer()
        .put(&branch("prj-1", "br-2", "qa"), None)
        .await
        .expect("another handle's create");
    assert_eq!(
        next().await,
        StoreEvent::Put {
            key: key("br-2"),
            version: v2
        }
    );
}

/// The fault plan of the injected cases: `fault` at `point` of the first
/// attempt of every `pg.put`.
#[cfg(feature = "faults")]
#[derive(Debug)]
struct FirstPut {
    point: loams_kv::FaultPoint,
    fault: loams_kv::Fault,
}

#[cfg(feature = "faults")]
impl loams_kv::FaultPlan for FirstPut {
    fn at(&self, op: &str, point: loams_kv::FaultPoint, attempt: u32) -> Option<loams_kv::Fault> {
        (op == super::kv::OP_PUT && point == self.point && attempt == 1).then_some(self.fault)
    }
}

/// A lost acknowledgement the store cannot resolve (no commit token) is
/// `Undetermined`, never success or a conflict, whether or not the write
/// applied.
pub async fn undetermined_is_surfaced(factory: Factory) {
    #[cfg(not(feature = "faults"))]
    {
        let _ = factory;
        eprintln!("skipped: undetermined_is_surfaced needs the faults feature");
    }
    #[cfg(feature = "faults")]
    {
        use loams_kv::{Fault, FaultPoint};
        let Some(kv) = factory.kv().await else {
            return;
        };
        let plain = KvControlStore::new(kv.clone(), options());
        let untokened = StoreOptions {
            commit_tokens: false,
            ..options()
        };
        for (point, id, applied) in [
            (FaultPoint::AfterCommit, "br-1", true),
            (FaultPoint::BeforeCommit, "br-2", false),
        ] {
            let faulty = KvControlStore::new(
                kv.clone().with_faults(Arc::new(FirstPut {
                    point,
                    fault: Fault::LoseAck,
                })),
                untokened.clone(),
            );
            let r = faulty
                .api_writer()
                .put(&branch("prj-1", id, id), None)
                .await;
            assert_eq!(r, Err(StoreError::Undetermined), "{point:?}");
            assert_eq!(
                get(&plain, &bkey("prj-1", id)).await.is_some(),
                applied,
                "{point:?}: applied"
            );
        }
    }
}

/// With commit tokens (the default), a lost acknowledgement is resolved: the
/// write reports the version it committed, applied once.
pub async fn lost_ack_is_resolved_by_its_token(factory: Factory) {
    #[cfg(not(feature = "faults"))]
    {
        let _ = factory;
        eprintln!("skipped: lost_ack_is_resolved_by_its_token needs the faults feature");
    }
    #[cfg(feature = "faults")]
    {
        use loams_kv::{Fault, FaultPoint};
        let Some(kv) = factory.kv().await else {
            return;
        };
        let plain = KvControlStore::new(kv.clone(), options());
        for (point, id) in [
            (FaultPoint::AfterCommit, "br-1"),
            (FaultPoint::BeforeCommit, "br-2"),
        ] {
            let faulty = KvControlStore::new(
                kv.clone().with_faults(Arc::new(FirstPut {
                    point,
                    fault: Fault::LoseAck,
                })),
                options(),
            );
            let v = faulty
                .api_writer()
                .put(&branch("prj-1", id, id), None)
                .await
                .unwrap_or_else(|e| panic!("{point:?}: {e}"));
            let got = get(&plain, &bkey("prj-1", id)).await.expect("applied");
            assert_eq!(got.version, v, "{point:?}");
        }
    }
}
