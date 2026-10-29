use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use loams_imperial::{
    Task, TaskBudget, TaskState, supervisor::Supervisor, task_store::DurableTasks,
};
use loams_kv::{EmbeddedConfig, Store, StoreConfig};

async fn setup(name: &str) -> (Supervisor, DurableTasks, Arc<AtomicU64>) {
    let store = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "imperial",
    )))
    .await
    .expect("open store");
    let now = Arc::new(AtomicU64::new(1));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    (
        Supervisor::new(store.clone(), clock.clone()),
        DurableTasks::new(store, clock),
        now,
    )
}

fn task(id: &str, deadline_ms: u64) -> Task {
    Task::new(
        id,
        TaskBudget {
            tokens: 100,
            steps: 10,
            deadline_ms,
        },
    )
    .expect("valid task")
}

#[tokio::test]
async fn only_one_leader_wins_a_concurrent_election() {
    let (supervisor, _, _) = setup("imperial-election").await;
    let mut join = tokio::task::JoinSet::new();
    for i in 0..16 {
        let supervisor = supervisor.clone();
        join.spawn(async move {
            supervisor
                .elect(&format!("candidate-{i}"), 100)
                .await
                .is_ok()
        });
    }
    let mut winners = 0;
    while let Some(result) = join.join_next().await {
        winners += usize::from(result.expect("join"));
    }
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn cancellation_revokes_the_worker_and_marks_task_atomically() {
    let (supervisor, tasks, _) = setup("imperial-cancel").await;
    let leader = supervisor.elect("leader", 100).await.expect("elect");
    tasks.create(&task("one", 1000)).await.expect("create");
    let worker = tasks.claim("one", "worker", 100).await.expect("claim");
    supervisor.cancel("one", &leader).await.expect("cancel");
    assert!(tasks.consume("one", &worker, 1, 1).await.is_err());
    assert!(tasks.claim("one", "replacement", 100).await.is_err());
    assert_eq!(
        tasks
            .get("one")
            .await
            .expect("read")
            .expect("exists")
            .state(),
        TaskState::Cancelled
    );
}

#[tokio::test]
async fn old_leader_cannot_cancel_after_new_leader_is_elected() {
    let (supervisor, tasks, clock) = setup("imperial-old-leader").await;
    let old = supervisor.elect("old", 5).await.expect("elect");
    tasks.create(&task("one", 1000)).await.expect("create");
    let worker = tasks.claim("one", "worker", 100).await.expect("claim");
    clock.store(6, Ordering::SeqCst);
    let new = supervisor.elect("new", 50).await.expect("elect new");
    assert!(supervisor.cancel("one", &old).await.is_err());
    tasks
        .consume("one", &worker, 1, 1)
        .await
        .expect("still running");
    supervisor.cancel("one", &new).await.expect("cancel new");
}

#[tokio::test]
async fn leader_can_cancel_a_task_after_its_wall_deadline() {
    let (supervisor, tasks, clock) = setup("imperial-expired-task").await;
    let leader = supervisor.elect("leader", 100).await.expect("elect");
    tasks.create(&task("one", 2)).await.expect("create");
    let worker = tasks.claim("one", "worker", 100).await.expect("claim");
    clock.store(2, Ordering::SeqCst);
    assert!(tasks.consume("one", &worker, 1, 1).await.is_err());
    supervisor
        .cancel("one", &leader)
        .await
        .expect("cancel expired task");
    assert_eq!(
        tasks
            .get("one")
            .await
            .expect("read")
            .expect("exists")
            .state(),
        TaskState::Cancelled
    );
}

#[tokio::test]
async fn leader_renewal_requires_a_changed_checkpoint_and_increased_progress() {
    let (supervisor, _, _) = setup("imperial-leader-progress").await;
    let leader = supervisor.elect("leader", 10).await.expect("elect");
    supervisor
        .checkpoint(&leader, 10, 1, b"checkpoint")
        .await
        .expect("checkpoint");
    assert!(
        supervisor
            .checkpoint(&leader, 10, 2, b"checkpoint")
            .await
            .is_err()
    );
    assert!(
        supervisor
            .checkpoint(&leader, 10, 1, b"other")
            .await
            .is_err()
    );
}
