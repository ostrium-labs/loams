use loams_imperial::durable::DurableLeases;
use loams_kv::{EmbeddedConfig, Store, StoreConfig};
use std::sync::Arc;
async fn leases(name: &str) -> DurableLeases {
    let kv = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "imperial",
    )))
    .await
    .expect("operation succeeds");
    DurableLeases::new(kv, Arc::new(|| 100))
}
#[tokio::test]
async fn concurrent_claims_have_one_winner() {
    let store = leases("imperial-one-winner").await;
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..16 {
        let store = store.clone();
        tasks.spawn(async move {
            store
                .claim("task", &format!("worker-{n}"), 1000)
                .await
                .is_ok()
        });
    }
    let mut wins = 0;
    while let Some(v) = tasks.join_next().await {
        wins += usize::from(v.expect("operation succeeds"));
    }
    assert_eq!(wins, 1);
}
#[tokio::test]
async fn revoked_writer_cannot_change_checkpoint() {
    let store = leases("imperial-fenced-write").await;
    let old = store
        .claim("task", "old", 1000)
        .await
        .expect("operation succeeds");
    store
        .checkpoint("task", &old, 1000, 1, b"first")
        .await
        .expect("operation succeeds");
    store
        .revoke("task", &old)
        .await
        .expect("operation succeeds");
    let new = store
        .claim("task", "new", 1000)
        .await
        .expect("operation succeeds");
    assert!(new.epoch() > old.epoch());
    assert!(
        store
            .checkpoint("task", &old, 1000, 2, b"zombie")
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read_checkpoint("task")
            .await
            .expect("operation succeeds"),
        Some(b"first".to_vec())
    );
}
#[tokio::test]
async fn renewal_requires_changed_checkpoint_and_progress() {
    let store = leases("imperial-progress").await;
    let token = store
        .claim("task", "a", 1000)
        .await
        .expect("operation succeeds");
    store
        .checkpoint("task", &token, 1000, 1, b"first")
        .await
        .expect("operation succeeds");
    assert!(
        store
            .checkpoint("task", &token, 1000, 2, b"first")
            .await
            .is_err()
    );
    assert!(
        store
            .checkpoint("task", &token, 1000, 1, b"second")
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read_checkpoint("task")
            .await
            .expect("operation succeeds"),
        Some(b"first".to_vec())
    );
}
#[tokio::test]
async fn task_names_do_not_alias() {
    let store = leases("imperial-names").await;
    let a = store
        .claim("a", "worker", 1000)
        .await
        .expect("operation succeeds");
    let b = store
        .claim("a/b", "worker", 1000)
        .await
        .expect("operation succeeds");
    assert!(
        store
            .checkpoint("a/b", &a, 1000, 1, b"wrong-task")
            .await
            .is_err()
    );
    store
        .checkpoint("a", &a, 1000, 1, b"a")
        .await
        .expect("operation succeeds");
    store
        .checkpoint("a/b", &b, 1000, 1, b"b")
        .await
        .expect("operation succeeds");
    assert_eq!(
        store
            .read_checkpoint("a")
            .await
            .expect("operation succeeds"),
        Some(b"a".to_vec())
    );
}
