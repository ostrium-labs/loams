//! The migration runner (UP1 Task 3): plan, run, resume, adjacency,
//! snapshots and abandon, with the test migrations of `support`.

mod support;

use std::sync::atomic::Ordering;

use loams_migrate::ledger::LEASE_TTL;
use loams_migrate::{LedgerState, Migration, MigrationLease, RunError, RunOptions, Runner};
use support::{FileMigration, Stack, path, read_dir};

const P2: &str = "0.1.0-preview.2";
const P3: &str = "0.1.0-preview.3";

fn opts() -> RunOptions {
    RunOptions::new("migrate-test")
}

fn code(result: Result<loams_migrate::Report, RunError>) -> String {
    result.expect_err("refused").code().to_string()
}

#[tokio::test]
async fn plan_lists_pending_migrations_in_order() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 3);
    let b = FileMigration::new("0002-b", 5).point_of_no_return();
    a.seed(stack.data.path());
    b.seed(stack.data.path());
    let migrations: [&dyn Migration; 2] = [&a, &b];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    let plan = runner.plan(&stack.cx(), P3).await.expect("plan");
    assert_eq!(plan.from.as_deref(), Some(P2));
    assert_eq!(plan.to, P3);
    let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["0001-a", "0002-b"]);
    assert_eq!(plan.steps[1].estimate.rows, 5);
    assert!(plan.point_of_no_return);
    // Planning wrote nothing.
    assert!(
        stack
            .ledger()
            .await
            .entries()
            .await
            .expect("entries")
            .is_empty()
    );
}

#[tokio::test]
async fn run_applies_and_records_done() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 5);
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let report = Runner::new(&migrations, &train)
        .run(&mut stack.cx(), P3, &opts())
        .await
        .expect("run");
    assert_eq!(report.applied, ["0001-a"]);
    assert!(report.stack_written);
    let ledger = stack.ledger().await;
    let entries = ledger.entries().await.expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state, LedgerState::Done);
    assert_eq!(entries[0].release_from, P2);
    assert_eq!(entries[0].release_to, P3);
    assert_eq!(entries[0].cursor, None);
    assert_eq!(entries[0].code_digest, a.code_digest());
    assert_eq!(
        ledger
            .stack()
            .await
            .expect("stack")
            .expect("stamped")
            .release,
        P3
    );
    let (current, files) = a.read_current(stack.data.path());
    assert_eq!(current, "v2");
    assert_eq!(files.get("4").map(String::as_str), Some("ITEM-4"));
    // The lease was released.
    MigrationLease::acquire(stack.meta.clone(), "someone-else", LEASE_TTL)
        .await
        .expect("free");
}

#[tokio::test]
async fn rerun_done_migration_is_noop() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 4);
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    runner.run(&mut stack.cx(), P3, &opts()).await.expect("run");
    let before = read_dir(stack.data.path());
    let entries = stack.ledger().await.entries().await.expect("entries");
    let copies = a.copies.load(Ordering::SeqCst);
    let report = runner
        .run(&mut stack.cx(), P3, &opts())
        .await
        .expect("rerun");
    assert!(report.applied.is_empty());
    assert!(!report.stack_written);
    assert_eq!(read_dir(stack.data.path()), before);
    assert_eq!(
        stack.ledger().await.entries().await.expect("entries"),
        entries
    );
    assert_eq!(a.copies.load(Ordering::SeqCst), copies);
    // A migration that finds its switch done does nothing, even called again.
    assert_eq!(
        a.apply(&mut stack.cx(), None).await.expect("apply"),
        loams_migrate::Step::Done
    );
}

#[tokio::test]
async fn resume_from_cursor_after_failure() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 6);
    a.seed(stack.data.path());
    a.fail_at.store(3, Ordering::SeqCst);
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    let err = runner.run(&mut stack.cx(), P3, &opts()).await.unwrap_err();
    assert_eq!(err.code(), "migration_failed", "{err}");
    let entry = stack.ledger().await.entries().await.expect("entries")[0].clone();
    assert_eq!(entry.state, LedgerState::Failed);
    assert_eq!(entry.error.as_ref().expect("error").code, "injected");
    assert_eq!(entry.cursor, Some(2u32.to_be_bytes().to_vec()));
    assert_eq!(a.copies.load(Ordering::SeqCst), 3, "items 0, 1 and 2");
    // The stack is not at the target while a migration failed.
    assert_eq!(
        stack
            .ledger()
            .await
            .stack()
            .await
            .expect("stack")
            .expect("p2")
            .release,
        P2
    );
    a.fail_at.store(u32::MAX, Ordering::SeqCst);
    let report = runner
        .run(&mut stack.cx(), P3, &opts())
        .await
        .expect("resume");
    assert_eq!(report.applied, ["0001-a"]);
    // Resumed at item 2: three more copies (2 again, 4 and 5 ... plus 3).
    assert_eq!(a.copies.load(Ordering::SeqCst), 3 + 4);
    let entry = stack.ledger().await.entries().await.expect("entries")[0].clone();
    assert_eq!(entry.state, LedgerState::Done);
    assert_eq!(entry.attempts, 2);
    assert_eq!(entry.error, None);
}

#[tokio::test]
async fn failed_migration_leaves_old_data_readable() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 6);
    a.seed(stack.data.path());
    let old = a.read_current(stack.data.path());
    a.fail_at.store(5, Ordering::SeqCst);
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    Runner::new(&migrations, &train)
        .run(&mut stack.cx(), P3, &opts())
        .await
        .unwrap_err();
    // Partial writes sit beside the old data; a reader still sees v1 whole.
    assert_eq!(a.read_current(stack.data.path()), old);
    assert_eq!(old.0, "v1");
    assert!(stack.data.path().join("0001-a/v2.partial/0").exists());
}

#[tokio::test]
async fn non_adjacent_source_refused() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 2);
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    // preview.2 → preview.4 skips preview.3.
    let err = runner
        .run(&mut stack.cx(), "0.1.0-preview.4", &opts())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "upgrade_step_not_adjacent", "{err}");
    assert!(
        err.to_string()
            .contains("0.1.0-preview.2 -> 0.1.0-preview.3 -> 0.1.0-preview.4"),
        "{err}"
    );
    // Nothing ran, nothing was written.
    assert!(
        stack
            .ledger()
            .await
            .entries()
            .await
            .expect("entries")
            .is_empty()
    );
    assert_eq!(a.read_current(stack.data.path()).0, "v1");
    // A release this binary does not know.
    assert_eq!(
        code(runner.run(&mut stack.cx(), "9.9.9", &opts()).await),
        "release_not_found"
    );
    // Adjacency on the compiled train: 0.1.0 upgrades only from preview.2.
    let compiled = loams_migrate::UpgradePath::compiled();
    assert!(compiled.check_adjacent(P2, "0.1.0").is_ok());
    assert!(compiled.check_adjacent("0.1.0-preview.1", "0.1.0").is_err());
}

#[tokio::test]
async fn patch_line_source_accepted() {
    let stack = Stack::new().await;
    let a = FileMigration::new("0001-a", 2);
    let b = FileMigration::new("0002-b", 2);
    a.seed(stack.data.path());
    b.seed(stack.data.path());
    let train = path("patch_line");
    // 0.1.1 shipped `a`: run it there first.
    stack.stamp("0.1.0").await;
    let first: [&dyn Migration; 1] = [&a];
    Runner::new(&first, &train)
        .run(&mut stack.cx(), "0.1.1", &opts())
        .await
        .expect("0.1.0 -> 0.1.1");
    // 0.1.4 carries both; 0.1.1 → 0.1.4 runs only `b`.
    let both: [&dyn Migration; 2] = [&a, &b];
    let report = Runner::new(&both, &train)
        .run(&mut stack.cx(), "0.1.4", &opts())
        .await
        .expect("0.1.1 -> 0.1.4");
    assert_eq!(report.from.as_deref(), Some("0.1.1"));
    assert_eq!(report.applied, ["0002-b"]);
    assert_eq!(a.copies.load(Ordering::SeqCst), 2, "a ran once");
    assert_eq!(
        stack
            .ledger()
            .await
            .stack()
            .await
            .expect("stack")
            .expect("stamped")
            .release,
        "0.1.4"
    );
}

#[tokio::test]
async fn patch_line_newer_or_other_line_refused() {
    let train = path("patch_line");
    for stored in ["0.1.5", "0.1.4", "0.0.9", "0.1.0-preview.9", "0.2.0"] {
        let refusal = train.check_adjacent(stored, "0.1.4").expect_err(stored);
        assert_eq!(refusal.code, "upgrade_step_not_adjacent", "{stored}");
    }
    assert!(train.check_adjacent("0.1.0", "0.1.4").is_ok());
    assert!(train.check_adjacent("0.1.3", "0.1.4").is_ok());
    // A new line's first release takes exactly the last patch.
    assert!(train.check_adjacent("0.1.4", "0.2.0").is_ok());
    assert!(train.check_adjacent("0.1.1", "0.2.0").is_err());
    // Through the runner: 0.1.5 data that no migration changed is read by
    // 0.1.4 as it is (nothing to do; UP1 ruling T14-2), and 0.1.3 data
    // cannot skip to 0.2.0.
    let stack = Stack::new().await;
    stack.stamp("0.1.5").await;
    let a = FileMigration::new("0001-a", 2);
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let report = Runner::new(&migrations, &train)
        .run(&mut stack.cx(), "0.1.4", &opts())
        .await
        .expect("nothing to do");
    assert!(report.applied.is_empty());
    let stack = Stack::new().await;
    stack.stamp("0.1.1").await;
    assert_eq!(
        code(
            Runner::new(&migrations, &train)
                .run(&mut stack.cx(), "0.2.0", &opts())
                .await
        ),
        "upgrade_step_not_adjacent"
    );
}

#[tokio::test]
async fn point_of_no_return_needs_snapshot() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 2).point_of_no_return();
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    assert_eq!(
        code(runner.run(&mut stack.cx(), P3, &opts()).await),
        "snapshot_required"
    );
    assert_eq!(a.read_current(stack.data.path()).0, "v1");
    let mut accept = opts();
    accept.no_snapshot_i_accept_data_loss = true;
    runner
        .run(&mut stack.cx(), P3, &accept)
        .await
        .expect("with --no-snapshot-i-accept-data-loss");
    assert_eq!(a.read_current(stack.data.path()).0, "v2");
}

#[tokio::test]
async fn code_digest_mismatch_refuses_resume() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 4);
    a.seed(stack.data.path());
    a.fail_at.store(2, Ordering::SeqCst);
    let first: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    Runner::new(&first, &train)
        .run(&mut stack.cx(), P3, &opts())
        .await
        .unwrap_err();
    // Another binary whose 0001-a is a point of no return.
    let other = FileMigration::new("0001-a", 4).point_of_no_return();
    assert_ne!(other.code_digest(), a.code_digest());
    let second: [&dyn Migration; 1] = [&other];
    assert_eq!(
        code(
            Runner::new(&second, &train)
                .run(&mut stack.cx(), P3, &opts())
                .await
        ),
        "migration_mismatch"
    );
}

#[tokio::test]
async fn abandon_removes_partial_writes() {
    let stack = Stack::new().await;
    stack.stamp(P2).await;
    let a = FileMigration::new("0001-a", 6);
    a.seed(stack.data.path());
    a.fail_at.store(4, Ordering::SeqCst);
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    runner.run(&mut stack.cx(), P3, &opts()).await.unwrap_err();
    assert!(stack.data.path().join("0001-a/v2.partial").exists());
    runner
        .abandon(&mut stack.cx(), "0001-a", "migrate-abandon")
        .await
        .expect("abandon");
    assert!(!stack.data.path().join("0001-a/v2.partial").exists());
    let entry = stack.ledger().await.entries().await.expect("entries")[0].clone();
    assert_eq!(entry.state, LedgerState::Failed);
    assert_eq!(entry.cursor, None);
    assert_eq!(entry.error.expect("error").code, "abandoned");
    assert_eq!(a.read_current(stack.data.path()).0, "v1");
    // A later run starts it over and completes.
    a.fail_at.store(u32::MAX, Ordering::SeqCst);
    runner
        .run(&mut stack.cx(), P3, &opts())
        .await
        .expect("rerun");
    assert_eq!(a.read_current(stack.data.path()).0, "v2");
}

#[tokio::test]
async fn new_stack_is_stamped_and_unstamped_store_refused() {
    let stack = Stack::new().await;
    let a = FileMigration::new("0001-a", 2);
    let migrations: [&dyn Migration; 1] = [&a];
    let train = path("prerelease");
    let runner = Runner::new(&migrations, &train);
    assert_eq!(
        code(runner.run(&mut stack.cx(), P3, &opts()).await),
        "stack_unrecognised"
    );
    let mut new = opts();
    new.new_stack = true;
    let report = runner.run(&mut stack.cx(), P3, &new).await.expect("new");
    assert!(report.stack_written && report.applied.is_empty());
    assert_eq!(
        stack
            .ledger()
            .await
            .stack()
            .await
            .expect("stack")
            .expect("stamped")
            .release,
        P3
    );
}

/// UP1 Task 14 (`rollback --keep-writes`, §54 §8): an older release's
/// `migrate run` over data a newer release migrated is a no-op when every
/// newer migration is `Reversible` and done, and refuses otherwise.
#[tokio::test]
async fn older_release_runs_on_reversible_newer_data() {
    for (class_is_pnr, expect_ok) in [(false, true), (true, false)] {
        let stack = Stack::new().await;
        stack.stamp(P2).await;
        let a = FileMigration::new("0001-a", 2);
        let a = if class_is_pnr {
            a.point_of_no_return()
        } else {
            a
        };
        a.seed(stack.data.path());
        let migrations: [&dyn Migration; 1] = [&a];
        let train = path("prerelease");
        let runner = Runner::new(&migrations, &train);
        let mut accept = opts();
        accept.no_snapshot_i_accept_data_loss = true;
        runner
            .run(&mut stack.cx(), P3, &accept)
            .await
            .expect("to preview.3");
        // preview.2's migrate over preview.3's data.
        let older = runner.run(&mut stack.cx(), P2, &opts()).await;
        if expect_ok {
            let report = older.expect("reversible: nothing to do");
            assert!(report.applied.is_empty() && !report.stack_written);
        } else {
            assert_eq!(code(older), "format_unreadable");
        }
        // The data stays at preview.3 either way.
        let stamped = stack
            .ledger()
            .await
            .stack()
            .await
            .expect("stack")
            .expect("stamped");
        assert_eq!(stamped.release, P3);
    }
    assert_eq!(
        loams_migrate::path::compare_releases("0.1.0-preview.10", "0.1.0-preview.9"),
        Some(std::cmp::Ordering::Greater)
    );
    assert_eq!(
        loams_migrate::path::compare_releases("0.1.0", "0.1.0-preview.9"),
        Some(std::cmp::Ordering::Greater)
    );
}
