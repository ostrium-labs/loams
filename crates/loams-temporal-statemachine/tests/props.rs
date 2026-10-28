//! DX1 Task 4's property test: random input sequences never break design
//! 74 §13.2's invariants 1–4, and the state always equals the fold of the
//! history written.

mod common;

use common::*;
use loams_temporal_history::{HistoryEvent, fold, invariants};
use loams_temporal_statemachine::{Input, Reply, TimeoutKind, apply};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Step {
    PollWorkflowTask,
    CompleteWithActivity,
    CompleteWithTimer,
    CompleteEmpty,
    CompleteClose,
    PollActivity(u8),
    FinishActivity(u8),
    FireTimer(u8),
    Signal,
    WftTimeout,
    ActivityTimeout(u8),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        Just(Step::PollWorkflowTask),
        Just(Step::CompleteWithActivity),
        Just(Step::CompleteWithTimer),
        Just(Step::CompleteEmpty),
        Just(Step::CompleteClose),
        any::<u8>().prop_map(Step::PollActivity),
        any::<u8>().prop_map(Step::FinishActivity),
        any::<u8>().prop_map(Step::FireTimer),
        Just(Step::Signal),
        Just(Step::WftTimeout),
        any::<u8>().prop_map(Step::ActivityTimeout),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn invariants_hold_for_random_runs(steps in proptest::collection::vec(step(), 1..60)) {
        let first = start();
        let mut s = first.state;
        let mut events: Vec<HistoryEvent> = first.events;
        let mut now = 2_000;
        let mut n = 0;
        for st in steps {
            now += 100;
            n += 1;
            let pick = |v: &[i64], i: u8| (!v.is_empty()).then(|| v[i as usize % v.len()]);
            let activities: Vec<i64> = s.activities.iter().map(|a| a.scheduled_event_id).collect();
            let input = match st {
                Step::PollWorkflowTask => match &s.workflow_task {
                    Some(w) if w.started_event_id == 0 => Input::WorkflowTaskStarted {
                        scheduled_event_id: w.scheduled_event_id,
                        identity: "w".into(),
                        request_id: format!("p{n}"),
                        worker_version: None,
                    },
                    _ => continue,
                },
                Step::CompleteWithActivity | Step::CompleteWithTimer | Step::CompleteEmpty | Step::CompleteClose => {
                    match &s.workflow_task {
                        Some(w) if w.started_event_id != 0 => {
                            let commands = match st {
                                Step::CompleteWithActivity => vec![schedule_activity(&format!("a{n}"), 2)],
                                Step::CompleteWithTimer => vec![start_timer(&format!("t{n}"), 1)],
                                Step::CompleteClose => vec![complete()],
                                _ => vec![],
                            };
                            Input::WorkflowTaskCompleted {
                                scheduled_event_id: w.scheduled_event_id,
                                attempt: w.attempt,
                                identity: "w".into(),
                                commands,
                                sdk_metadata: None,
                                metering_metadata: None,
                                worker_version: None,
                                force_create_new_workflow_task: false,
                                messages: Vec::new(),
                                new_run_id: String::new(),
                            }
                        }
                        _ => continue,
                    }
                }
                Step::PollActivity(i) => {
                    let Some(id) = pick(&activities, i) else { continue };
                    let a = loams_temporal_history::activity(&s, id).expect("listed");
                    Input::ActivityStarted {
                        scheduled_event_id: id,
                        attempt: a.attempt,
                        identity: "w".into(),
                        request_id: format!("p{n}"),
                        worker_version: None,
                    }
                }
                Step::FinishActivity(i) => {
                    let Some(id) = pick(&activities, i) else { continue };
                    let a = loams_temporal_history::activity(&s, id).expect("listed");
                    if i % 2 == 0 {
                        Input::ActivityCompleted { scheduled_event_id: id, attempt: a.attempt, result: None, identity: "w".into() }
                    } else {
                        Input::ActivityFailed { scheduled_event_id: id, attempt: a.attempt, failure: None, identity: "w".into(), last_heartbeat_details: None }
                    }
                }
                Step::FireTimer(i) => {
                    let timers: Vec<i64> = s.timers.iter().map(|t| t.started_event_id).collect();
                    // Sometimes fire a timer that does not exist (stale).
                    let id = pick(&timers, i).unwrap_or(i64::from(i));
                    Input::TimerFired { started_event_id: id }
                }
                Step::Signal => Input::Signal { name: "s".into(), input: None, identity: "c".into(), header: None, request_id: format!("s{n}") },
                Step::WftTimeout => match &s.workflow_task {
                    Some(w) => Input::WorkflowTaskTimedOut { scheduled_event_id: w.scheduled_event_id, attempt: w.attempt },
                    None => continue,
                },
                Step::ActivityTimeout(i) => {
                    let Some(id) = pick(&activities, i) else { continue };
                    let a = loams_temporal_history::activity(&s, id).expect("listed");
                    Input::ActivityTimedOut { scheduled_event_id: id, attempt: a.attempt, kind: TimeoutKind::StartToClose }
                }
            };
            // A refused input changes nothing; carry on with the next.
            if let Ok(t) = apply(&s, input, now) {
                if t.reply == Reply::Stale {
                    prop_assert!(t.events.is_empty() && t.effects.is_empty());
                }
                events.extend(t.events);
                s = t.state;
            }
        }
        prop_assert!(invariants::check(&events, 1).is_ok(), "{:?}", invariants::check(&events, 1));
        let folded = fold("ns", "run", &events).expect("history folds");
        prop_assert_eq!(folded.next_event_id, s.next_event_id);
        prop_assert_eq!(folded.status, s.status);
        // Fired timers whose event is still buffered behind a started
        // workflow task are gone from the state but not yet in history.
        let buffered_fired = s
            .buffered
            .iter()
            .filter(|e| matches!(e.attributes, Some(loams_temporal_history::Attributes::TimerFiredEventAttributes(_))))
            .count();
        prop_assert_eq!(folded.timers.len(), s.timers.len() + buffered_fired);
    }
}
