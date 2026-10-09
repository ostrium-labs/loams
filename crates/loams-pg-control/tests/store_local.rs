//! `PgControlStore`'s conformance on the local (embedded redb) backend.

loams_pg_control::pg_control_store_conformance!(
    loams_pg_control::store::conformance::local_factory(env!("CARGO_TARGET_TMPDIR"))
);

/// The local store reopens with its records.
#[tokio::test]
async fn local_store_survives_a_reopen() {
    use loams_pg_control::model::{BranchKey, BranchRec, BranchState};
    use loams_pg_control::store::local;
    use loams_pg_control::{PgControlStore, StoreOptions};

    let dir = loams_kv::testing::TempDir::new_in(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")))
        .expect("a directory");
    let path = dir.path().join("pg-control.redb");
    let rec = BranchRec {
        project_id: "prj-1".into(),
        id: "br-1".into(),
        name: "main".into(),
        timeline_id: [1; 16],
        parent_id: None,
        ancestor_lsn: Some(0x0169_AD58),
        expires_at_ms: None,
        protected: true,
        stripe_size: None,
        shards: Vec::new(),
        state: BranchState::Ready,
        created_at_ms: 1,
    };
    let v = {
        let store = local::open(&path, StoreOptions::default())
            .await
            .expect("open");
        store.api_writer().put(&rec, None).await.expect("put")
    };
    let store = local::open(&path, StoreOptions::default())
        .await
        .expect("reopen");
    let key = BranchKey {
        project_id: "prj-1".into(),
        id: "br-1".into(),
    };
    let got = store
        .get::<BranchRec>(&key)
        .await
        .expect("get")
        .expect("present");
    assert_eq!((got.record, got.version), (rec, v));
}
