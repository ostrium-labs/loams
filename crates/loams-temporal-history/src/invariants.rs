//! Design 74 §13.2's invariants 1–3, checked on an event list. The tests run
//! them over the corpus and over every history the state machine writes;
//! debug builds of the gateway assert them on each commit.

use std::collections::BTreeMap;

use crate::{Attributes, HistoryEvent};

/// An invariant a history breaks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Violation {
    /// Invariant 1: IDs are contiguous and increasing.
    #[error("event {found} follows {previous}")]
    NonContiguous {
        /// The previous ID.
        previous: i64,
        /// The ID found.
        found: i64,
    },
    /// Invariant 2: at most one outstanding workflow task.
    #[error("workflow task {second} scheduled while {first} is outstanding")]
    TwoWorkflowTasks {
        /// The outstanding task's scheduled event.
        first: i64,
        /// The second scheduled event.
        second: i64,
    },
    /// Invariant 3: an activity or timer gets at most one terminal event,
    /// and only after it was scheduled or started.
    #[error("{what} {id}: {problem}")]
    Terminal {
        /// `activity` or `timer`.
        what: &'static str,
        /// The scheduled event ID or the timer ID.
        id: String,
        /// What is wrong.
        problem: &'static str,
    },
}

/// Checks invariants 1–3 on `events`, which start at `first_event_id`.
pub fn check(events: &[HistoryEvent], first_event_id: i64) -> Result<(), Violation> {
    let mut previous = first_event_id - 1;
    let mut outstanding_wft: Option<i64> = None;
    // Activity scheduled event ID -> terminal seen; timer ID -> terminal seen.
    let mut activities: BTreeMap<i64, bool> = BTreeMap::new();
    let mut timers: BTreeMap<String, bool> = BTreeMap::new();
    let term = |what, id: String, problem| Violation::Terminal { what, id, problem };
    for e in events {
        if e.event_id != previous + 1 {
            return Err(Violation::NonContiguous {
                previous,
                found: e.event_id,
            });
        }
        previous = e.event_id;
        match &e.attributes {
            Some(Attributes::WorkflowTaskScheduledEventAttributes(_)) => {
                if let Some(first) = outstanding_wft {
                    return Err(Violation::TwoWorkflowTasks {
                        first,
                        second: e.event_id,
                    });
                }
                outstanding_wft = Some(e.event_id);
            }
            Some(
                Attributes::WorkflowTaskCompletedEventAttributes(_)
                | Attributes::WorkflowTaskFailedEventAttributes(_)
                | Attributes::WorkflowTaskTimedOutEventAttributes(_),
            ) => outstanding_wft = None,
            Some(Attributes::ActivityTaskScheduledEventAttributes(_)) => {
                activities.insert(e.event_id, false);
            }
            Some(
                Attributes::ActivityTaskCompletedEventAttributes(
                    loams_temporal_proto::temporal::api::history::v1::ActivityTaskCompletedEventAttributes {
                        scheduled_event_id: s,
                        ..
                    },
                )
                | Attributes::ActivityTaskFailedEventAttributes(
                    loams_temporal_proto::temporal::api::history::v1::ActivityTaskFailedEventAttributes {
                        scheduled_event_id: s,
                        ..
                    },
                )
                | Attributes::ActivityTaskTimedOutEventAttributes(
                    loams_temporal_proto::temporal::api::history::v1::ActivityTaskTimedOutEventAttributes {
                        scheduled_event_id: s,
                        ..
                    },
                )
                | Attributes::ActivityTaskCanceledEventAttributes(
                    loams_temporal_proto::temporal::api::history::v1::ActivityTaskCanceledEventAttributes {
                        scheduled_event_id: s,
                        ..
                    },
                ),
            ) => match activities.get_mut(s) {
                None => return Err(term("activity", s.to_string(), "terminal before scheduled")),
                Some(true) => return Err(term("activity", s.to_string(), "two terminal events")),
                Some(done) => *done = true,
            },
            Some(Attributes::TimerStartedEventAttributes(t)) => {
                timers.insert(t.timer_id.clone(), false);
            }
            Some(Attributes::TimerFiredEventAttributes(
                loams_temporal_proto::temporal::api::history::v1::TimerFiredEventAttributes {
                    timer_id,
                    ..
                },
            ))
            | Some(Attributes::TimerCanceledEventAttributes(
                loams_temporal_proto::temporal::api::history::v1::TimerCanceledEventAttributes {
                    timer_id,
                    ..
                },
            )) => match timers.get_mut(timer_id) {
                None => return Err(term("timer", timer_id.clone(), "terminal before started")),
                Some(true) => return Err(term("timer", timer_id.clone(), "two terminal events")),
                Some(done) => *done = true,
            },
            _ => {}
        }
    }
    Ok(())
}
