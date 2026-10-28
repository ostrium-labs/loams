use loams_imperial::{
    Task, TaskBudget, TaskState, durable::DurableLeases, task_store::DurableTasks,
};
use loams_kv::{EmbeddedConfig, Store, StoreConfig};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

async fn setup(name: &str) -> (DurableTasks, DurableLeases, Arc<AtomicU64>) {
    let store = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "imperial",
    )))
    .await
    .unwrap();
    let now = Arc::new(AtomicU64::new(1));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    (
        DurableTasks::new(store.clone(), clock.clone()),
        DurableLeases::new(store, clock),
        now,
    )
}

fn task(id: &str) -> Task {
    Task::new(
        id,
        TaskBudget {
            tokens: 100,
            steps: 10,
            deadline_ms: 1000,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn competing_consumers_cannot_overspend_the_task_budget() {
    let (tasks, _, _) = setup("imperial-budget-atomic").await;
    tasks.create(&task("one")).await.unwrap();
    let token = tasks.claim("one", "worker", 100).await.unwrap();
    let left = tasks.clone();
    let right = tasks.clone();
    let a = token.clone();
    let b = token.clone();
    let (left, right) = tokio::join!(
        left.consume("one", &a, 60, 1),
        right.consume("one", &b, 60, 1)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let persisted = tasks.get("one").await.unwrap().unwrap();
    assert_eq!(persisted.tokens_used(), 60);
    assert_eq!(persisted.steps_used(), 1);
}

#[tokio::test]
async fn takeover_reclaims_running_task_and_fences_stale_updates() {
    let (tasks, _, clock) = setup("imperial-task-takeover").await;
    tasks.create(&task("one")).await.unwrap();
    let old = tasks.claim("one", "old", 5).await.unwrap();
    tasks.consume("one", &old, 10, 1).await.unwrap();
    clock.store(6, Ordering::SeqCst);
    let new = tasks.claim("one", "new", 10).await.unwrap();
    assert!(new.epoch() > old.epoch());
    assert!(tasks.consume("one", &old, 1, 1).await.is_err());
    tasks.finish("one", &new).await.unwrap();
    let persisted = tasks.get("one").await.unwrap().unwrap();
    assert_eq!(persisted.state(), TaskState::Done);
    assert_eq!(persisted.tokens_used(), 10);
}

#[tokio::test]
async fn task_state_does_not_overwrite_the_checkpoint() {
    let (tasks, leases, _) = setup("imperial-task-checkpoint").await;
    tasks.create(&task("one")).await.unwrap();
    let token = tasks.claim("one", "worker", 100).await.unwrap();
    leases
        .checkpoint("one", &token, 100, 1, b"saved")
        .await
        .unwrap();
    tasks.consume("one", &token, 3, 1).await.unwrap();
    assert_eq!(
        leases.read_checkpoint("one").await.unwrap(),
        Some(b"saved".to_vec())
    );
    assert_eq!(tasks.get("one").await.unwrap().unwrap().tokens_used(), 3);
}

#[tokio::test]
async fn duplicate_create_cannot_replace_a_task() {
    let (tasks, _, _) = setup("imperial-task-create").await;
    tasks.create(&task("one")).await.unwrap();
    assert!(tasks.create(&task("one")).await.is_err());
    assert_eq!(
        tasks.get("one").await.unwrap().unwrap().state(),
        TaskState::Ready
    );
}
