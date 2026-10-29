//! DX1 Task 3's tests over DX0 Task 4's corpus: every DX1 history folds to
//! a closed run with the right status, keeps §13.2's invariants, and its
//! events are what a [`Batch`] writes for the same attributes.
//!
//! The corpus is `conformance/compute/temporal/corpus/` (or
//! `LOAMS_TEMPORAL_CORPUS`); without it the tests print why and pass.

use std::path::PathBuf;

use loams_temporal_history::{
    Batch, HistoryEvent, MutableState, WorkflowExecutionStatus, fold, invariants, time,
};
use loams_temporal_proto::temporal::api::history::v1::History;
use prost::Message as _;

/// The DX1 scenarios with the status each run ends in (corpus README).
const DX1: &[(&str, WorkflowExecutionStatus)] = &[
    ("hello", WorkflowExecutionStatus::Completed),
    (
        "two_activities_sequential",
        WorkflowExecutionStatus::Completed,
    ),
    (
        "activity_parallel_fanout_10",
        WorkflowExecutionStatus::Completed,
    ),
    ("signal_then_complete", WorkflowExecutionStatus::Completed),
    ("query_while_running", WorkflowExecutionStatus::Completed),
    ("cancel_requested", WorkflowExecutionStatus::Canceled),
    ("terminated", WorkflowExecutionStatus::Terminated),
    (
        "workflow_failed_application_error",
        WorkflowExecutionStatus::Failed,
    ),
    (
        "activity_heartbeat_details",
        WorkflowExecutionStatus::Completed,
    ),
];

fn corpus() -> Option<PathBuf> {
    let root = std::env::var_os("LOAMS_TEMPORAL_CORPUS").map_or_else(
        || {
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"))
                .join("../../conformance/compute/temporal/corpus")
        },
        PathBuf::from,
    );
    let histories = root.join("histories");
    if histories.is_dir() {
        Some(histories)
    } else {
        eprintln!("skipped: no corpus at {}", histories.display());
        None
    }
}

fn load(dir: &std::path::Path, lang: &str, scenario: &str) -> Vec<HistoryEvent> {
    let path = dir.join(lang).join(format!("{scenario}.binpb"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    History::decode(bytes.as_slice())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .events
}

#[test]
fn fold_corpus_dx1() {
    let Some(dir) = corpus() else { return };
    for lang in ["go", "ts"] {
        for (scenario, status) in DX1 {
            let events = load(&dir, lang, scenario);
            let s = fold("ns", "run", &events).unwrap_or_else(|e| panic!("{lang}/{scenario}: {e}"));
            assert_eq!(s.status, *status as i32, "{lang}/{scenario}");
            assert_eq!(
                s.next_event_id,
                events.len() as i64 + 1,
                "{lang}/{scenario}"
            );
            assert_eq!(
                s.completion_event_id,
                events.len() as i64,
                "{lang}/{scenario}"
            );
            assert!(s.workflow_task.is_none(), "{lang}/{scenario}");
            assert!(s.timers.is_empty(), "{lang}/{scenario}: {:?}", s.timers);
            assert_eq!(s.workflow_id, format!("corpus-{lang}-{scenario}"));
        }
    }
}

#[test]
fn invariants_hold_on_corpus_dx1() {
    let Some(dir) = corpus() else { return };
    for lang in ["go", "ts"] {
        for (scenario, _) in DX1 {
            let events = load(&dir, lang, scenario);
            invariants::check(&events, 1).unwrap_or_else(|e| panic!("{lang}/{scenario}: {e}"));
        }
    }
}

/// Re-pushing every corpus event's attributes, at its time, through a
/// `Batch` gives the same events (task IDs aside, which the server picks).
#[test]
fn batch_matches_corpus_shapes() {
    let Some(dir) = corpus() else { return };
    for lang in ["go", "ts"] {
        for (scenario, _) in DX1 {
            let events = load(&dir, lang, scenario);
            let mut state = MutableState {
                next_event_id: 1,
                ..Default::default()
            };
            let mut batch = Batch::new(&mut state, 0);
            for e in &events {
                let at = e.event_time.as_ref().map_or(0, time::ms);
                batch.push_at(at, e.attributes.clone().expect("attributes"));
            }
            let written = batch.finish();
            assert_eq!(written.len(), events.len());
            for (w, e) in written.iter().zip(&events) {
                assert_eq!(w.event_id, e.event_id, "{lang}/{scenario}");
                assert_eq!(
                    w.event_type, e.event_type,
                    "{lang}/{scenario} event {}",
                    e.event_id
                );
                assert_eq!(
                    w.attributes, e.attributes,
                    "{lang}/{scenario} event {}",
                    e.event_id
                );
                assert_eq!(
                    w.event_time.as_ref().map(time::ms),
                    e.event_time.as_ref().map(time::ms)
                );
            }
            assert_eq!(state.next_event_id, events.len() as i64 + 1);
        }
    }
}

#[test]
fn fold_rejects_gaps_and_events_after_close() {
    let Some(dir) = corpus() else { return };
    let mut events = load(&dir, "go", "hello");
    let mut gap = events.clone();
    gap.remove(3);
    assert!(matches!(
        fold("ns", "run", &gap),
        Err(loams_temporal_history::FoldError::NonContiguous { .. })
    ));
    let mut extra = events.last().cloned().expect("events");
    extra.event_id += 1;
    events.push(extra);
    assert!(matches!(
        fold("ns", "run", &events),
        Err(loams_temporal_history::FoldError::AfterClose(_))
    ));
}

#[test]
fn invariants_catch_two_workflow_tasks() {
    let Some(dir) = corpus() else { return };
    let events = load(&dir, "go", "hello");
    // Events 1-3 (started, task scheduled, task started), then a second
    // workflow task scheduled while the first is outstanding.
    let mut broken: Vec<HistoryEvent> = events[..3].to_vec();
    let mut second = events[1].clone();
    second.event_id = 4;
    broken.push(second);
    assert!(matches!(
        invariants::check(&broken, 1),
        Err(invariants::Violation::TwoWorkflowTasks { .. })
    ));
}
