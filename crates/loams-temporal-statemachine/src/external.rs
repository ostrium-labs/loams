//! Inputs from outside the workflow: signals, cancel requests, terminate,
//! and fired timers.

use loams_temporal_history::{Attributes, Batch, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::common::v1::{Header, Payloads};
use loams_temporal_proto::temporal::api::history::v1::{
    TimerFiredEventAttributes, WorkflowExecutionCancelRequestedEventAttributes,
    WorkflowExecutionSignaledEventAttributes, WorkflowExecutionTerminatedEventAttributes,
};

use crate::write::{arrive, close, flush};
use crate::{Effect, Rejection, Reply};

pub(crate) fn signal(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    name: String,
    input: Option<Payloads>,
    identity: String,
    header: Option<Header>,
    request_id: String,
) -> Result<Reply, Rejection> {
    let limits = loams_temporal_history::Limits::default();
    if b.state_ref().signal_count >= limits.pending_signals as i64 {
        return Err(Rejection::ResourceExhausted(
            "signal count limit exceeded for this workflow".into(),
        ));
    }
    b.state().signal_count += 1;
    let now = b.now_ms();
    arrive(
        b,
        fx,
        vec![(
            now,
            Attributes::WorkflowExecutionSignaledEventAttributes(
                WorkflowExecutionSignaledEventAttributes {
                    signal_name: name,
                    input,
                    identity,
                    header: Some(header.unwrap_or_default()),
                    request_id,
                    ..Default::default()
                },
            ),
        )],
    );
    Ok(Reply::Done)
}

pub(crate) fn cancel_requested(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    reason: String,
    identity: String,
) -> Result<Reply, Rejection> {
    if b.state_ref().cancel_requested {
        // Already requested: nothing new to tell the workflow.
        return Ok(Reply::Done);
    }
    b.state().cancel_requested = true;
    let now = b.now_ms();
    arrive(
        b,
        fx,
        vec![(
            now,
            Attributes::WorkflowExecutionCancelRequestedEventAttributes(
                WorkflowExecutionCancelRequestedEventAttributes {
                    cause: reason,
                    identity,
                    ..Default::default()
                },
            ),
        )],
    );
    Ok(Reply::Done)
}

pub(crate) fn terminate(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    reason: String,
    details: Option<Payloads>,
    identity: String,
) -> Result<Reply, Rejection> {
    // Events waiting behind a started workflow task are written first.
    flush(b);
    let id = b.push(Attributes::WorkflowExecutionTerminatedEventAttributes(
        WorkflowExecutionTerminatedEventAttributes {
            reason,
            details,
            identity,
        },
    ));
    close(b, fx, id, WorkflowExecutionStatus::Terminated);
    Ok(Reply::Done)
}

pub(crate) fn timer_fired(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    started_event_id: i64,
) -> Result<Reply, Rejection> {
    let Some(t) = b
        .state_ref()
        .timers
        .iter()
        .find(|t| t.started_event_id == started_event_id)
        .cloned()
    else {
        return Ok(Reply::Stale);
    };
    loams_temporal_history::take_timer(b.state(), &t.timer_id);
    let now = b.now_ms();
    arrive(
        b,
        fx,
        vec![(
            now,
            Attributes::TimerFiredEventAttributes(TimerFiredEventAttributes {
                timer_id: t.timer_id,
                started_event_id,
            }),
        )],
    );
    Ok(Reply::Done)
}
