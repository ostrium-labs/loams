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
    .expect("operation succeeds");
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
    .expect("operation succeeds")
}

#[tokio::test]
async fn competing_consumers_cannot_overspend_the_task_budget() {
    let (tasks, _, _) = setup("imperial-budget-atomic").await;
    tasks
        .create(&task("one"))
        .await
        .expect("operation succeeds");
    let token = tasks
        .claim("one", "worker", 100)
        .await
        .expect("operation succeeds");
    let left = tasks.clone();
    let right = tasks.clone();
    let a = token.clone();
    let b = token.clone();
    let (left, right) = tokio::join!(
        left.consume("one", &a, 60, 1),
        right.consume("one", &b, 60, 1)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let persisted = tasks
        .get("one")
        .await
        .expect("operation succeeds")
        .expect("operation succeeds");
    assert_eq!(persisted.tokens_used(), 60);
    assert_eq!(persisted.steps_used(), 1);
}

#[tokio::test]
async fn takeover_reclaims_running_task_and_fences_stale_updates() {
    let (tasks, _, clock) = setup("imperial-task-takeover").await;
    tasks
        .create(&task("one"))
        .await
        .expect("operation succeeds");
    let old = tasks
        .claim("one", "old", 5)
        .await
        .expect("operation succeeds");
    tasks
        .consume("one", &old, 10, 1)
        .await
        .expect("operation succeeds");
    clock.store(6, Ordering::SeqCst);
    let new = tasks
        .claim("one", "new", 10)
        .await
        .expect("operation succeeds");
    assert!(new.epoch() > old.epoch());
    assert!(tasks.consume("one", &old, 1, 1).await.is_err());
    tasks.finish("one", &new).await.expect("operation succeeds");
    let persisted = tasks
        .get("one")
        .await
        .expect("operation succeeds")
        .expect("operation succeeds");
    assert_eq!(persisted.state(), TaskState::Done);
    assert_eq!(persisted.tokens_used(), 10);
}

#[tokio::test]
async fn task_state_does_not_overwrite_the_checkpoint() {
    let (tasks, leases, _) = setup("imperial-task-checkpoint").await;
    tasks
        .create(&task("one"))
        .await
        .expect("operation succeeds");
    let token = tasks
        .claim("one", "worker", 100)
        .await
        .expect("operation succeeds");
    leases
        .checkpoint("one", &token, 100, 1, b"saved")
        .await
        .expect("operation succeeds");
    tasks
        .consume("one", &token, 3, 1)
        .await
        .expect("operation succeeds");
    assert_eq!(
        leases
            .read_checkpoint("one")
            .await
            .expect("operation succeeds"),
        Some(b"saved".to_vec())
    );
    assert_eq!(
        tasks
            .get("one")
            .await
            .expect("operation succeeds")
            .expect("operation succeeds")
            .tokens_used(),
        3
    );
}

#[tokio::test]
async fn duplicate_create_cannot_replace_a_task() {
    let (tasks, _, _) = setup("imperial-task-create").await;
    tasks
        .create(&task("one"))
        .await
        .expect("operation succeeds");
    assert!(tasks.create(&task("one")).await.is_err());
    assert_eq!(
        tasks
            .get("one")
            .await
            .expect("operation succeeds")
            .expect("operation succeeds")
            .state(),
        TaskState::Ready
    );
}
