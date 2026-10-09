//! LV1 plan Task 1: the reactive correctness checker (design §20 §14 item
//! 3), on the embedded store and on TiKV (`live_test!`). A seeded workload
//! of sessions and mutations is checked Transition by Transition against
//! fresh snapshot evaluations; the checker must also catch a missed
//! invalidation, and resumed sessions must converge.

use loams_kv::TxnOptions;
use loams_live::testing::TestStore;
use loams_live::testing::checker::{Report, SEED_DOCS, ViolationKind, run_reactive_checker};
use loams_live::testing::workload::{Disturbance, Workload};
use loams_live::{AppKeys, live_test};

fn first(report: &Report) -> String {
    report
        .violations
        .iter()
        .take(5)
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 10 seeds × 2 000 ops: no violation, and the checker checked something.
async fn reactive_checker_passes_seeded_workload(store: TestStore) {
    for seed in 0..10 {
        let w = Workload {
            seed,
            sessions: 4,
            tables: 3,
            ops: 2_000,
            disturb: Vec::new(),
        };
        let report = run_reactive_checker(store.fresh_root().await.store(), w).await;
        eprintln!(
            "seed {seed}: {} transitions, {} checks",
            report.transitions, report.checked
        );
        assert!(
            report.violations.is_empty(),
            "seed {seed}: {} violations, the first:\n{}",
            report.violations.len(),
            first(&report)
        );
        assert!(report.transitions > 0, "seed {seed}: no Transition");
        assert!(report.checked > 0, "seed {seed}: nothing checked");
    }
}
live_test!(reactive_checker_passes_seeded_workload);

/// With the subscription manager dropping batches of the journal (the
/// `drop_next_batch` hook), the checker reports a violation: it is not
/// vacuous.
async fn reactive_checker_detects_missed_invalidation(store: TestStore) {
    let ops = 400;
    let w = Workload {
        seed: 7,
        sessions: 3,
        tables: 2,
        ops,
        disturb: [ops / 4, ops / 2, 3 * ops / 4, ops - 1]
            .into_iter()
            .map(|at_op| Disturbance::DropInvalidation { at_op })
            .collect(),
    };
    let report = run_reactive_checker(store.store(), w).await;
    // A stale result, or a session that never caught up: not a workload
    // failure.
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKind::Stale | ViolationKind::NotCaughtUp)),
        "no reactive violation with invalidations dropped ({} transitions, {} checks): {:?}",
        report.transitions,
        report.checked,
        report.violations
    );
    assert!(
        report
            .violations
            .iter()
            .all(|v| v.kind != ViolationKind::Workload),
        "{:?}",
        report.violations
    );
}
live_test!(reactive_checker_detects_missed_invalidation);

/// Sessions disconnected mid-run resume from their last version and end up
/// matching fresh evaluations.
async fn reactive_checker_resume_converges(store: TestStore) {
    let w = Workload {
        seed: 3,
        sessions: 4,
        tables: 2,
        ops: 600,
        disturb: vec![
            Disturbance::Disconnect { at_op: 150 },
            Disturbance::Disconnect { at_op: 300 },
            Disturbance::Disconnect { at_op: 450 },
        ],
    };
    let report = run_reactive_checker(store.store(), w).await;
    assert!(
        report.violations.is_empty(),
        "{} violations, the first:\n{}",
        report.violations.len(),
        first(&report)
    );
    assert!(report.resumes >= 3, "{} resumes", report.resumes);
}
live_test!(reactive_checker_resume_converges);

/// A disturbance a later task wires is reported, not silently skipped.
async fn reactive_checker_reports_unwired_disturbances(store: TestStore) {
    let w = Workload {
        seed: 1,
        sessions: 1,
        tables: 1,
        ops: 20,
        disturb: vec![Disturbance::Deploy { at_op: 5 }],
    };
    let report = run_reactive_checker(store.store(), w).await;
    assert!(
        report
            .violations
            .iter()
            .any(|v| format!("{v:?}").contains("Deploy")),
        "{:?}",
        report.violations
    );
}
live_test!(reactive_checker_reports_unwired_disturbances);

/// Every mutation of the workload carries an idempotency key (one per op,
/// kept across its retries), so a retry after an unknown outcome replays
/// the first commit instead of applying the op twice.
async fn reactive_checker_mutations_carry_idempotency_keys(store: TestStore) {
    let root = store.fresh_root().await;
    let (ops, tables) = (60, 2);
    let w = Workload {
        seed: 5,
        sessions: 1,
        tables,
        ops,
        disturb: Vec::new(),
    };
    let report = run_reactive_checker(root.store(), w).await;
    assert!(report.violations.is_empty(), "{}", first(&report));
    let range = AppKeys::dedicated().idempotency_records();
    let records = root
        .store()
        .run(TxnOptions::new("test.idempotency_records"), move |txn| {
            let range = range.clone();
            Box::pin(async move {
                txn.scan(&range.lo, Some(&range.hi), 100_000)
                    .await
                    .map(|pairs| pairs.len())
            })
        })
        .await
        .expect("a scan")
        .value;
    assert_eq!(records, ops + tables * SEED_DOCS, "one record per mutation");
}
live_test!(reactive_checker_mutations_carry_idempotency_keys);
