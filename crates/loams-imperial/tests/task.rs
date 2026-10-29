use loams_imperial::{Lease, Task, TaskBudget, TaskError, TaskState};

fn budget() -> TaskBudget {
    TaskBudget {
        tokens: 100,
        steps: 5,
        deadline_ms: 50,
    }
}

#[test]
fn only_a_leased_worker_can_start_and_finish_a_ready_task() {
    let mut lease = Lease::new();
    let mut task = Task::new("task", budget()).expect("operation succeeds");
    let a = lease.claim("a", 1, 10).expect("operation succeeds");
    task.start(&lease, &a, 2).expect("operation succeeds");
    let b = lease.claim("b", 11, 10).expect("operation succeeds");
    assert_eq!(task.finish(&lease, &a, 12), Err(TaskError::Lease));
    task.finish(&lease, &b, 12).expect("operation succeeds");
    assert_eq!(task.state(), TaskState::Done);
    assert_eq!(
        task.start(&lease, &b, 12),
        Err(TaskError::InvalidTransition)
    );
}

#[test]
fn budget_is_checked_before_mutating_usage() {
    let mut lease = Lease::new();
    let token = lease.claim("a", 0, 100).expect("operation succeeds");
    let mut task = Task::new("task", budget()).expect("operation succeeds");
    task.start(&lease, &token, 1).expect("operation succeeds");
    task.consume(&lease, &token, 2, 40, 2)
        .expect("operation succeeds");
    assert_eq!(
        task.consume(&lease, &token, 3, 61, 1),
        Err(TaskError::BudgetExceeded)
    );
    assert_eq!(task.tokens_used(), 40);
    assert_eq!(task.steps_used(), 2);
    task.consume(&lease, &token, 4, 60, 3)
        .expect("operation succeeds");
    assert_eq!(
        task.consume(&lease, &token, 5, 0, 1),
        Err(TaskError::BudgetExceeded)
    );
}

#[test]
fn wall_deadline_is_hard_and_state_survives_serialization() {
    let mut lease = Lease::new();
    let token = lease.claim("a", 0, 100).expect("operation succeeds");
    let mut task = Task::new("task", budget()).expect("operation succeeds");
    task.start(&lease, &token, 1).expect("operation succeeds");
    let mut task: Task =
        serde_json::from_slice(&serde_json::to_vec(&task).expect("operation succeeds"))
            .expect("operation succeeds");
    assert_eq!(
        task.consume(&lease, &token, 50, 1, 1),
        Err(TaskError::Deadline)
    );
    assert_eq!(task.finish(&lease, &token, 50), Err(TaskError::Deadline));
}

#[test]
fn blocked_tasks_can_be_requeued_but_terminal_states_cannot() {
    let mut lease = Lease::new();
    let token = lease.claim("a", 0, 100).expect("operation succeeds");
    let mut task = Task::new("task", budget()).expect("operation succeeds");
    task.start(&lease, &token, 1).expect("operation succeeds");
    task.block(&lease, &token, 2).expect("operation succeeds");
    assert_eq!(
        task.consume(&lease, &token, 3, 1, 1),
        Err(TaskError::InvalidTransition)
    );
    task.requeue(&lease, &token, 3).expect("operation succeeds");
    task.start(&lease, &token, 4).expect("operation succeeds");
    task.fail(&lease, &token, 5).expect("operation succeeds");
    assert_eq!(task.state(), TaskState::Failed);
    assert_eq!(
        task.requeue(&lease, &token, 6),
        Err(TaskError::InvalidTransition)
    );
}

#[test]
fn counter_overflow_is_a_budget_failure() {
    let mut lease = Lease::new();
    let token = lease.claim("a", 0, 100).expect("operation succeeds");
    let mut task = Task::new(
        "task",
        TaskBudget {
            tokens: u64::MAX,
            steps: 2,
            deadline_ms: 20,
        },
    )
    .expect("operation succeeds");
    task.start(&lease, &token, 1).expect("operation succeeds");
    task.consume(&lease, &token, 2, u64::MAX, 1)
        .expect("operation succeeds");
    assert_eq!(
        task.consume(&lease, &token, 3, 1, 1),
        Err(TaskError::BudgetExceeded)
    );
    assert_eq!(task.tokens_used(), u64::MAX);
}
