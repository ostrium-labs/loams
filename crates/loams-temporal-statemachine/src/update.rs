//! Workflow updates (Temporal's update protocol; TMP-api, D2080–D2082).
//!
//! The lifecycle, as the SDKs see it:
//! 1. `UpdateWorkflowExecution` admits the update ([`admit`]). Nothing is
//!    written to history; a workflow task is scheduled if none is
//!    outstanding.
//! 2. The next workflow task carries the update's request as a protocol
//!    message ([`messages_for_task`]); the update is then *sent*.
//! 3. The worker answers in `RespondWorkflowTaskCompleted` with protocol
//!    messages, ordered among its commands by `ProtocolMessageCommand`s
//!    ([`handle_message`]):
//!    - an `Acceptance` writes `WorkflowExecutionUpdateAccepted`;
//!    - a `Rejection` writes nothing (the outcome goes back to the caller);
//!    - a `Response` writes `WorkflowExecutionUpdateCompleted`.
//!
//!    Messages no command names are handled after every command, in order,
//!    as Temporal's server does. An update sent with the task that the
//!    worker did not answer is rejected ([`reject_unprocessed`]), with
//!    Temporal's message, so an SDK without update support fails fast.
//! 4. A workflow task that fails or times out puts its sent updates back to
//!    admitted, so the next task re-sends them ([`resend_after_failure`]).
//! 5. A run that closes aborts what is still in flight
//!    ([`loams_temporal_history::abort_updates`]).
//!
//! Where the gateway differs from Temporal's server, on purpose (D2081):
//! - Admitted updates are kept in mutable state (durable), not only in
//!   memory, so a gateway restart does not lose them.
//! - The workflow task an update needs is an ordinary one, written to
//!   history, not a "speculative" one that a rejection discards. The SDK
//!   replayers accept both shapes; the history is a few events longer.
//! - Rejected updates stay known (with their outcome), so polling a
//!   rejected update ID answers its rejection instead of `NOT_FOUND`.

use loams_temporal_history::time::ts;
use loams_temporal_history::{Attributes, Batch, UpdateInfo, UpdateState};
use loams_temporal_proto::temporal::api::failure::v1::{ApplicationFailureInfo, Failure, failure};
use loams_temporal_proto::temporal::api::history::v1::{
    WorkflowExecutionUpdateAcceptedEventAttributes, WorkflowExecutionUpdateCompletedEventAttributes,
};
use loams_temporal_proto::temporal::api::protocol::v1::{Message, message};
use loams_temporal_proto::temporal::api::update::v1::{
    Acceptance, Outcome, Rejection as UpdateRejection, Request, Response, outcome,
};
use prost::Message as _;

use crate::write::{ensure_workflow_task, wft_started};
use crate::{Effect, Rejection, Reply};

/// Temporal's `history.maxInFlightUpdates` default.
pub const MAX_IN_FLIGHT_UPDATES: usize = 10;
/// Temporal's `history.maxTotalUpdates` default.
pub const MAX_TOTAL_UPDATES: usize = 2000;

/// The `type_url` of each update protocol body.
pub const REQUEST_TYPE: &str = "type.googleapis.com/temporal.api.update.v1.Request";
/// See [`REQUEST_TYPE`].
pub const ACCEPTANCE_TYPE: &str = "type.googleapis.com/temporal.api.update.v1.Acceptance";
/// See [`REQUEST_TYPE`].
pub const REJECTION_TYPE: &str = "type.googleapis.com/temporal.api.update.v1.Rejection";
/// See [`REQUEST_TYPE`].
pub const RESPONSE_TYPE: &str = "type.googleapis.com/temporal.api.update.v1.Response";

/// Temporal's message when a worker completes a workflow task without
/// answering an update that was sent with it.
pub const UNPROCESSED_MESSAGE: &str = "Workflow Update is rejected because it wasn't processed by worker. Probably, Workflow Update is not supported by the worker.";

/// The ID of an update's request message (Temporal's `<update_id>/request`).
pub fn request_message_id(update_id: &str) -> String {
    format!("{update_id}/request")
}

/// Admits an update (`UpdateWorkflowExecution`). An update ID the run
/// already knows is a retry: nothing changes ([`Reply::Stale`]), and the
/// caller reads the update's stage from the state.
pub(crate) fn admit(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    request: Request,
) -> Result<Reply, Rejection> {
    let update_id = request
        .meta
        .as_ref()
        .map(|m| m.update_id.clone())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Rejection::InvalidArgument("Update id is not set on request.".into()))?;
    if request.input.as_ref().is_none_or(|i| i.name.is_empty()) {
        return Err(Rejection::InvalidArgument(
            "Update name is not set on request.".into(),
        ));
    }
    let s = b.state_ref();
    if loams_temporal_history::update(s, &update_id).is_some() {
        return Ok(Reply::Stale);
    }
    let in_flight = s
        .updates
        .iter()
        .filter(|u| loams_temporal_history::update_in_flight(u))
        .count();
    if in_flight >= MAX_IN_FLIGHT_UPDATES {
        return Err(Rejection::ResourceExhausted(format!(
            "limit on number of concurrent in-flight updates has been reached ({MAX_IN_FLIGHT_UPDATES})"
        )));
    }
    if s.updates.len() >= MAX_TOTAL_UPDATES {
        return Err(Rejection::ResourceExhausted(format!(
            "limit on number of total updates has been reached ({MAX_TOTAL_UPDATES})"
        )));
    }
    let now = b.now_ms();
    b.state().updates.push(UpdateInfo {
        update_id,
        state: UpdateState::Admitted as i32,
        request: Some(request),
        admitted_time: Some(ts(now)),
        ..Default::default()
    });
    // A started task finishes first; its completion schedules the next.
    if !wft_started(b) {
        ensure_workflow_task(b, fx);
    }
    Ok(Reply::Done)
}

/// Marks the admitted updates as sent with the workflow task that just
/// started (`scheduled_event_id`).
pub(crate) fn mark_sent(b: &mut Batch<'_>, scheduled_event_id: i64) {
    for u in &mut b.state().updates {
        if u.state() == UpdateState::Admitted {
            u.state = UpdateState::Sent as i32;
            u.sent_in_wft = scheduled_event_id;
        }
    }
}

/// The protocol messages a poll answer for the workflow task scheduled at
/// `scheduled_event_id` and started at `started_event_id` carries: one
/// request per update sent with it, sequenced after the last event before
/// the task started (as Temporal's server sequences them).
pub fn messages_for_task(
    state: &loams_temporal_history::MutableState,
    scheduled_event_id: i64,
    started_event_id: i64,
) -> Vec<Message> {
    state
        .updates
        .iter()
        .filter(|u| u.state() == UpdateState::Sent && u.sent_in_wft == scheduled_event_id)
        .map(|u| Message {
            id: request_message_id(&u.update_id),
            protocol_instance_id: u.update_id.clone(),
            sequencing_id: Some(message::SequencingId::EventId(started_event_id - 1)),
            body: Some(prost_types::Any {
                type_url: REQUEST_TYPE.to_string(),
                value: u.request.clone().unwrap_or_default().encode_to_vec(),
            }),
        })
        .collect()
}

fn bad_message(why: impl Into<String>) -> Rejection {
    Rejection::InvalidArgument(why.into())
}

fn decode<M: prost::Message + Default>(body: &prost_types::Any) -> Result<M, Rejection> {
    M::decode(body.value.as_ref())
        .map_err(|e| bad_message(format!("malformed {} message: {e}", body.type_url)))
}

/// Handles one protocol message of a completed workflow task.
pub(crate) fn handle_message(b: &mut Batch<'_>, m: Message) -> Result<(), Rejection> {
    let body = m
        .body
        .as_ref()
        .ok_or_else(|| bad_message(format!("protocol message {} has no body", m.id)))?;
    let update_id = m.protocol_instance_id.clone();
    let known = loams_temporal_history::update(b.state_ref(), &update_id)
        .cloned()
        .ok_or_else(|| {
            bad_message(format!(
                "protocol message {} names update {update_id}, which the workflow does not have",
                m.id
            ))
        })?;
    match body.type_url.as_str() {
        ACCEPTANCE_TYPE => {
            let a: Acceptance = decode(body)?;
            if !matches!(known.state(), UpdateState::Sent | UpdateState::Admitted) {
                return Err(bad_message(format!(
                    "update {update_id} cannot be accepted in state {:?}",
                    known.state()
                )));
            }
            let id = b.push(Attributes::WorkflowExecutionUpdateAcceptedEventAttributes(
                WorkflowExecutionUpdateAcceptedEventAttributes {
                    protocol_instance_id: update_id.clone(),
                    accepted_request_message_id: if a.accepted_request_message_id.is_empty() {
                        request_message_id(&update_id)
                    } else {
                        a.accepted_request_message_id
                    },
                    accepted_request_sequencing_event_id: a.accepted_request_sequencing_event_id,
                    accepted_request: a.accepted_request.or(known.request.clone()),
                },
            ));
            let u = loams_temporal_history::update_mut(b.state(), &update_id).expect("known");
            u.state = UpdateState::Accepted as i32;
            u.accepted_event_id = id;
        }
        REJECTION_TYPE => {
            let r: UpdateRejection = decode(body)?;
            if !matches!(known.state(), UpdateState::Sent | UpdateState::Admitted) {
                return Err(bad_message(format!(
                    "update {update_id} cannot be rejected in state {:?}",
                    known.state()
                )));
            }
            let u = loams_temporal_history::update_mut(b.state(), &update_id).expect("known");
            u.state = UpdateState::Rejected as i32;
            u.outcome = Some(Outcome {
                value: Some(outcome::Value::Failure(r.failure.unwrap_or_default())),
            });
        }
        RESPONSE_TYPE => {
            let r: Response = decode(body)?;
            if known.state() != UpdateState::Accepted {
                return Err(bad_message(format!(
                    "update {update_id} must be accepted before it completes (state {:?})",
                    known.state()
                )));
            }
            let id = b.push(Attributes::WorkflowExecutionUpdateCompletedEventAttributes(
                WorkflowExecutionUpdateCompletedEventAttributes {
                    meta: Some(r.meta.unwrap_or_else(|| {
                        known
                            .request
                            .as_ref()
                            .and_then(|q| q.meta.clone())
                            .unwrap_or_default()
                    })),
                    accepted_event_id: known.accepted_event_id,
                    outcome: r.outcome,
                },
            ));
            let u = loams_temporal_history::update_mut(b.state(), &update_id).expect("known");
            u.state = UpdateState::Completed as i32;
            u.completed_event_id = id;
        }
        other => {
            return Err(bad_message(format!(
                "unknown protocol message body type {other}"
            )));
        }
    }
    Ok(())
}

/// The failure Temporal gives an update the worker did not answer.
pub fn unprocessed_failure() -> Failure {
    Failure {
        message: UNPROCESSED_MESSAGE.to_string(),
        source: "Server".to_string(),
        failure_info: Some(failure::FailureInfo::ApplicationFailureInfo(
            ApplicationFailureInfo {
                r#type: "UnprocessedUpdate".to_string(),
                non_retryable: true,
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Rejects the updates sent with the workflow task `scheduled_event_id`
/// that its completion left unanswered.
pub(crate) fn reject_unprocessed(b: &mut Batch<'_>, scheduled_event_id: i64) {
    for u in &mut b.state().updates {
        if u.state() == UpdateState::Sent && u.sent_in_wft == scheduled_event_id {
            u.state = UpdateState::Rejected as i32;
            u.outcome = Some(Outcome {
                value: Some(outcome::Value::Failure(unprocessed_failure())),
            });
        }
    }
}

/// A failed or timed-out workflow task's sent updates go back to admitted,
/// to be sent again with the next task.
pub(crate) fn resend_after_failure(b: &mut Batch<'_>, scheduled_event_id: i64) {
    for u in &mut b.state().updates {
        if u.state() == UpdateState::Sent && u.sent_in_wft == scheduled_event_id {
            u.state = UpdateState::Admitted as i32;
            u.sent_in_wft = 0;
        }
    }
}

/// Whether updates wait for a workflow task (admitted, not yet sent).
pub(crate) fn has_admitted(b: &Batch<'_>) -> bool {
    b.state_ref()
        .updates
        .iter()
        .any(|u| u.state() == UpdateState::Admitted)
}

/// Where an update stands, as `UpdateWorkflowExecution` and
/// `PollWorkflowExecutionUpdate` report it.
// The outcome is moved out once per call; boxing it buys nothing here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateStatus {
    /// Admitted or sent: the worker has not answered.
    Admitted,
    /// Accepted; `accepted_event_id` names its event.
    Accepted,
    /// Completed with this outcome (a response, a rejection, or the
    /// failure of an accepted update whose run closed). A completed one's
    /// outcome is in the history event `completed_event_id` (0 otherwise).
    Completed {
        /// The outcome, when it is kept in the state.
        outcome: Option<Outcome>,
        /// The `WorkflowExecutionUpdateCompleted` event, if one was written.
        completed_event_id: i64,
    },
    /// The run closed before the worker accepted it.
    Aborted,
}

/// The status of `update_id` in `state`; `None` if the run does not know it.
pub fn status(
    state: &loams_temporal_history::MutableState,
    update_id: &str,
) -> Option<UpdateStatus> {
    let u = loams_temporal_history::update(state, update_id)?;
    Some(match u.state() {
        UpdateState::Admitted | UpdateState::Sent | UpdateState::Unspecified => {
            UpdateStatus::Admitted
        }
        UpdateState::Accepted => UpdateStatus::Accepted,
        UpdateState::Completed => UpdateStatus::Completed {
            outcome: None,
            completed_event_id: u.completed_event_id,
        },
        UpdateState::Rejected => UpdateStatus::Completed {
            outcome: u.outcome.clone(),
            completed_event_id: 0,
        },
        UpdateState::Aborted => match &u.outcome {
            Some(o) => UpdateStatus::Completed {
                outcome: Some(o.clone()),
                completed_event_id: 0,
            },
            None => UpdateStatus::Aborted,
        },
    })
}
