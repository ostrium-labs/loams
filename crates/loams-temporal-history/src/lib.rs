//! The Temporal gateway's history (design 74 §5.5 and §13.2, plan DX1 Task 3).
//!
//! - [`Batch`] allocates contiguous event IDs, stamps each event's time,
//!   type and task ID, and keeps [`MutableState`]'s counters in step. One
//!   batch is one compare-and-set commit.
//! - [`fold`] rebuilds a run's [`MutableState`] from its history (§5.5 rule
//!   7), so the state blob is a cache, never the only copy.
//! - [`invariants`] checks §13.2's invariants 1–3 on any event list.
//! - [`Limits`] holds Temporal's default limits (plan ruling R0.9).
//!
//! The crate is pure: it reads no clock, makes no random numbers and does no
//! I/O. Times and IDs come in as arguments.

mod batch;
mod event_type;
mod fold;
pub mod invariants;
mod limits;
pub mod time;

pub use batch::Batch;
pub use event_type::event_type;
pub use fold::{FoldError, abort_updates, fold};
pub use limits::{Limits, messages as limit_messages};
pub use loams_temporal_proto::loams::temporal::internal::v1::{
    ActivityInfo, ChildInfo, ExternalInfo, MutableState, ParentInfo, TimerInfo, UpdateInfo,
    UpdateState, WorkflowTaskInfo,
};
pub use loams_temporal_proto::temporal::api::enums::v1::{EventType, WorkflowExecutionStatus};
pub use loams_temporal_proto::temporal::api::history::v1::{
    History, HistoryEvent, history_event::Attributes,
};

/// The mutable-state format this crate writes.
pub const STATE_VERSION: u32 = 1;

/// Whether a run in `state` is still open.
pub fn is_open(state: &MutableState) -> bool {
    state.status == WorkflowExecutionStatus::Running as i32
}

/// The pending activity scheduled by `scheduled_event_id`, if any.
pub fn activity(state: &MutableState, scheduled_event_id: i64) -> Option<&ActivityInfo> {
    state
        .activities
        .iter()
        .find(|a| a.scheduled_event_id == scheduled_event_id)
}

/// [`activity`], mutable.
pub fn activity_mut(
    state: &mut MutableState,
    scheduled_event_id: i64,
) -> Option<&mut ActivityInfo> {
    state
        .activities
        .iter_mut()
        .find(|a| a.scheduled_event_id == scheduled_event_id)
}

/// Removes and returns the pending activity scheduled by `scheduled_event_id`.
pub fn take_activity(state: &mut MutableState, scheduled_event_id: i64) -> Option<ActivityInfo> {
    let i = state
        .activities
        .iter()
        .position(|a| a.scheduled_event_id == scheduled_event_id)?;
    Some(state.activities.remove(i))
}

/// The pending timer with the workflow's `timer_id`, if any.
pub fn timer<'a>(state: &'a MutableState, timer_id: &str) -> Option<&'a TimerInfo> {
    state.timers.iter().find(|t| t.timer_id == timer_id)
}

/// Removes and returns the pending timer with `timer_id`.
pub fn take_timer(state: &mut MutableState, timer_id: &str) -> Option<TimerInfo> {
    let i = state.timers.iter().position(|t| t.timer_id == timer_id)?;
    Some(state.timers.remove(i))
}

/// The workflow update with `update_id`, if the run knows it.
pub fn update<'a>(state: &'a MutableState, update_id: &str) -> Option<&'a UpdateInfo> {
    state.updates.iter().find(|u| u.update_id == update_id)
}

/// [`update`], mutable.
pub fn update_mut<'a>(state: &'a mut MutableState, update_id: &str) -> Option<&'a mut UpdateInfo> {
    state.updates.iter_mut().find(|u| u.update_id == update_id)
}

/// Whether an update in `u` is still in flight (admitted, sent or
/// accepted; Temporal's `maxInFlightUpdates` counts these).
pub fn update_in_flight(u: &UpdateInfo) -> bool {
    matches!(
        u.state(),
        UpdateState::Admitted | UpdateState::Sent | UpdateState::Accepted
    )
}

/// Temporal's message for an accepted update whose run closed before the
/// worker responded.
pub const UPDATE_CLOSED_MESSAGE: &str =
    "Workflow Update failed because the Workflow completed before the Update completed.";

/// The outcome an accepted update gets when its run closes first.
pub fn update_closed_outcome() -> loams_temporal_proto::temporal::api::update::v1::Outcome {
    use loams_temporal_proto::temporal::api::failure::v1::{
        ApplicationFailureInfo, Failure, failure,
    };
    use loams_temporal_proto::temporal::api::update::v1::outcome;
    loams_temporal_proto::temporal::api::update::v1::Outcome {
        value: Some(outcome::Value::Failure(Failure {
            message: UPDATE_CLOSED_MESSAGE.to_string(),
            source: "Server".to_string(),
            failure_info: Some(failure::FailureInfo::ApplicationFailureInfo(
                ApplicationFailureInfo {
                    r#type: "AcceptedUpdateCompletedWorkflow".to_string(),
                    non_retryable: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        })),
    }
}

/// The pending child initiated by `initiated_event_id`, if any.
pub fn child(state: &MutableState, initiated_event_id: i64) -> Option<&ChildInfo> {
    state
        .children
        .iter()
        .find(|c| c.initiated_event_id == initiated_event_id)
}

/// Removes and returns the pending child initiated by `initiated_event_id`.
pub fn take_child(state: &mut MutableState, initiated_event_id: i64) -> Option<ChildInfo> {
    let i = state
        .children
        .iter()
        .position(|c| c.initiated_event_id == initiated_event_id)?;
    Some(state.children.remove(i))
}

/// Removes and returns the pending external request initiated by
/// `initiated_event_id` from `list` (signals or cancels).
pub fn take_external(
    list: &mut Vec<ExternalInfo>,
    initiated_event_id: i64,
) -> Option<ExternalInfo> {
    let i = list
        .iter()
        .position(|x| x.initiated_event_id == initiated_event_id)?;
    Some(list.remove(i))
}

/// Merges upserted search attributes into the run's.
pub fn upsert_search_attributes(
    state: &mut MutableState,
    upsert: Option<&loams_temporal_proto::temporal::api::common::v1::SearchAttributes>,
) {
    let Some(u) = upsert else { return };
    let sa = state.search_attributes.get_or_insert_with(Default::default);
    for (k, v) in &u.indexed_fields {
        sa.indexed_fields.insert(k.clone(), v.clone());
    }
}

/// Merges an upserted memo into the run's.
pub fn upsert_memo(
    state: &mut MutableState,
    upsert: Option<&loams_temporal_proto::temporal::api::common::v1::Memo>,
) {
    let Some(u) = upsert else { return };
    let memo = state.memo.get_or_insert_with(Default::default);
    for (k, v) in &u.fields {
        memo.fields.insert(k.clone(), v.clone());
    }
}
