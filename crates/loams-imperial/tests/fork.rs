use loams_imperial::{Task, TaskBudget, task_store::DurableTasks};
use loams_kv::{EmbeddedConfig, Store, StoreConfig};
use std::sync::Arc;

async fn setup(name: &str) -> DurableTasks {
    let store = Store::open(StoreConfig::Embedded(EmbeddedConfig::in_memory(
        name, "imperial",
    )))
    .await
    .expect("open store");
    DurableTasks::new(store, Arc::new(|| 1))
}
fn task(id: &str) -> Task {
    Task::new(
        id,
        TaskBudget {
            tokens: 100,
            steps: 10,
            deadline_ms: 100,
        },
    )
    .expect("valid task")
}
fn child(tokens: u64, steps: u64) -> TaskBudget {
    TaskBudget {
        tokens,
        steps,
        deadline_ms: 100,
    }
}

#[tokio::test]
async fn concurrent_forks_cannot_exceed_the_root_budget() {
    let tasks = setup("imperial-fork-budget").await;
    tasks.create(&task("root")).await.expect("create");
    let token = tasks.claim("root", "worker", 100).await.expect("claim");
    let (a, b) = tokio::join!(
        tasks.fork("root", &token, "a", child(60, 2)),
        tasks.fork("root", &token, "b", child(60, 2)),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let root = tasks.get("root").await.expect("read").expect("exists");
    assert_eq!(root.tokens_reserved(), 60);
    assert!(tasks.consume("root", &token, 41, 1).await.is_err());
    tasks
        .consume("root", &token, 40, 1)
        .await
        .expect("remaining budget");
}

#[tokio::test]
async fn duplicate_child_creation_does_not_reserve_budget_twice() {
    let tasks = setup("imperial-fork-id").await;
    tasks.create(&task("root")).await.expect("create");
    let token = tasks.claim("root", "worker", 100).await.expect("claim");
    tasks
        .fork("root", &token, "child", child(20, 2))
        .await
        .expect("fork");
    assert!(
        tasks
            .fork("root", &token, "child", child(20, 2))
            .await
            .is_err()
    );
    assert!(
        tasks
            .fork("root", &token, "root", child(20, 2))
            .await
            .is_err()
    );
    let root = tasks.get("root").await.expect("read").expect("exists");
    assert_eq!(root.tokens_reserved(), 20);
    assert_eq!(root.steps_reserved(), 2);
}

#[tokio::test]
async fn default_fanout_and_depth_are_bounded() {
    let tasks = setup("imperial-fork-limits").await;
    tasks.create(&task("root")).await.expect("create");
    let root = tasks.claim("root", "worker", 100).await.expect("claim");
    tasks
        .fork("root", &root, "a", child(80, 6))
        .await
        .expect("fork a");
    tasks
        .fork("root", &root, "b", child(10, 1))
        .await
        .expect("fork b");
    assert!(tasks.fork("root", &root, "c", child(1, 1)).await.is_err());
    let a = tasks.claim("a", "a-worker", 100).await.expect("claim a");
    tasks
        .fork("a", &a, "a1", child(70, 5))
        .await
        .expect("fork a1");
    let a1 = tasks.claim("a1", "a1-worker", 100).await.expect("claim a1");
    tasks
        .fork("a1", &a1, "a2", child(60, 4))
        .await
        .expect("fork a2");
    let a2 = tasks.claim("a2", "a2-worker", 100).await.expect("claim a2");
    assert!(tasks.fork("a2", &a2, "a3", child(50, 3)).await.is_err());
}

#[tokio::test]
async fn stale_parent_cannot_allocate_a_child() {
    let tasks = setup("imperial-fork-fenced").await;
    tasks.create(&task("root")).await.expect("create");
    let token = tasks.claim("root", "worker", 100).await.expect("claim");
    // A token for another task never authorizes this parent.
    tasks.create(&task("other")).await.expect("create other");
    let other = tasks
        .claim("other", "worker", 100)
        .await
        .expect("claim other");
    assert!(
        tasks
            .fork("root", &other, "child", child(1, 1))
            .await
            .is_err()
    );
    tasks
        .fork("root", &token, "child", child(1, 1))
        .await
        .expect("valid fork");
}
