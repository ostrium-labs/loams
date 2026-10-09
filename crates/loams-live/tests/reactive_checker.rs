//! LV1 plan Task 1: the reactive correctness checker (design §20 §14 item
//! 3), on the embedded store and on TiKV (`live_test!`). A seeded workload
//! of sessions and mutations is checked Transition by Transition against
//! fresh snapshot evaluations; the checker must also catch a missed
//! invalidation, and resumed sessions must converge.

use loams_kv::TxnOptions;
use loams_live::testing::TestStore;
use loams_live::testing::checker::{Report, SEED_DOCS, ViolationKind, run_reactive_checker};
use loams_live::testing::workload::{Disturbance, Sizes, Workload};
use loams_live::{AppKeys, live_test};

/// The first violations, and where the run's dump (the writers' op logs
/// and every session's records) was written. Called only on a failure.
fn first(report: &Report) -> String {
    let violations = report
        .violations
        .iter()
        .take(5)
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("reactive-checker");
    let path = dir.join(format!(
        "seed-{}-{}-{}.log",
        report.seed,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, report.dump()))
        .map_or_else(
            |e| format!("(no dump: {e})"),
            |()| format!("dump: {}", path.display()),
        );
    format!("{violations}\n{written}")
}

/// Ticks were recorded, and every session got several Transitions with
/// updates while the writers ran, not only when it caught up at the end.
fn assert_live(report: &Report, sessions: usize) {
    assert!(report.ticks > 0, "no tick recorded");
    assert_eq!(
        report.live_updates.len(),
        sessions,
        "{:?}",
        report.live_updates
    );
    assert!(
        report.live_updates.iter().all(|n| *n >= 3),
        "update-carrying Transitions per session before the writers finished: {:?}",
        report.live_updates
    );
}

/// No violation, and the checker checked something: 10 seeds × 2 000 ops
/// on the embedded store and 2 × 2 000 on TiKV by default; the nightly TiKV
/// job runs 10 seeds from a seed offset derived from the date
/// (`LOAMS_CHECKER_SEEDS`, `LOAMS_CHECKER_OPS`, `LOAMS_CHECKER_SEED_OFFSET`).
async fn reactive_checker_passes_seeded_workload(store: TestStore) {
    let sizes = Sizes::from_process_env(store.backend()).expect("the checker's sizes");
    eprintln!("checker sizes on {:?}: {sizes:?}", store.backend());
    for seed in sizes.offset..sizes.offset + sizes.seeds {
        let w = Workload {
            seed,
            sessions: 4,
            tables: 3,
            ops: sizes.ops,
            disturb: Vec::new(),
        };
        let report = run_reactive_checker(store.fresh_root().await.store(), w).await;
        eprintln!(
            "seed {seed}: {} transitions, {} checks",
            report.transitions, report.checked
        );
        let rerun = format!(
            "seed {seed} on {:?} (rerun it alone with LOAMS_CHECKER_SEED_OFFSET={seed} \
             LOAMS_CHECKER_SEEDS=1 LOAMS_CHECKER_OPS={})",
            store.backend(),
            sizes.ops
        );
        assert!(
            report.violations.is_empty(),
            "{rerun}: {} violations, the first:\n{}",
            report.violations.len(),
            first(&report)
        );
        assert!(report.transitions > 0, "{rerun}: no Transition");
        assert!(report.checked > 0, "{rerun}: nothing checked");
        assert_live(&report, 4);
    }
}
live_test!(reactive_checker_passes_seeded_workload);

/// With the subscription manager dropping batches of the journal (the
/// `drop_next_batch` hook), the checker reports a stale result: it is not
/// vacuous. The same seed without the dropped batches passes, so the
/// violation comes from the drops.
async fn reactive_checker_detects_missed_invalidation(store: TestStore) {
    let ops = 400;
    let workload = |disturb: Vec<Disturbance>| Workload {
        seed: 7,
        sessions: 3,
        tables: 2,
        ops,
        disturb,
    };
    let drops = [ops / 4, ops / 2, 3 * ops / 4, ops - 1]
        .into_iter()
        .map(|at_op| Disturbance::DropInvalidation { at_op })
        .collect();
    let report = run_reactive_checker(store.fresh_root().await.store(), workload(drops)).await;
    assert!(
        report.subs.dropped_batches >= 1,
        "no journal batch was dropped: {:?}",
        report.subs
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Stale),
        "no stale result with invalidations dropped ({} transitions, {} checks): {:?}",
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

    let control =
        run_reactive_checker(store.fresh_root().await.store(), workload(Vec::new())).await;
    assert_eq!(control.subs.dropped_batches, 0);
    assert!(
        control.violations.is_empty(),
        "the control run (no dropped batches): {}",
        first(&control)
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
    assert_live(&report, 4);
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

/// A seed reproduces the op mix: each writer's sequence of ops (kind,
/// table, value) is the same in two runs, up to the shorter run (which
/// writer takes how many ops depends on the interleaving, which a seed
/// does not fix). The dump names every op and every session's records.
async fn reactive_checker_seed_reproduces_the_op_mix(store: TestStore) {
    let w = Workload {
        seed: 11,
        sessions: 2,
        tables: 2,
        ops: 200,
        disturb: Vec::new(),
    };
    let a = run_reactive_checker(store.fresh_root().await.store(), w.clone()).await;
    let b = run_reactive_checker(store.fresh_root().await.store(), w).await;
    assert!(a.violations.is_empty(), "{}", first(&a));
    assert!(b.violations.is_empty(), "{}", first(&b));
    assert_eq!(a.ops.len(), 200);
    let mix = |r: &Report, writer: usize| -> Vec<String> {
        r.ops
            .iter()
            .filter(|o| o.writer == writer)
            .map(|o| format!("{:?} t{} n={}", o.kind, o.table, o.n))
            .collect()
    };
    for writer in 0..8 {
        let (x, y) = (mix(&a, writer), mix(&b, writer));
        let common = x.len().min(y.len());
        assert_eq!(x[..common], y[..common], "writer {writer}");
    }
    let dump = a.dump();
    assert!(
        dump.contains("writer 0"),
        "{}",
        &dump[..dump.len().min(400)]
    );
    assert!(
        dump.contains("session 1"),
        "{}",
        &dump[..dump.len().min(400)]
    );
}
live_test!(reactive_checker_seed_reproduces_the_op_mix);
