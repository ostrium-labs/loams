//! Activities: started by a poll; then completed, failed (and maybe
//! retried), cancelled, timed out; heartbeats in between.

use loams_temporal_history::time::{dur_ms, ms, ts};
use loams_temporal_history::{ActivityInfo, Attributes, Batch};
use loams_temporal_proto::temporal::api::common::v1::{Payloads, WorkerVersionStamp};
use loams_temporal_proto::temporal::api::enums::v1::{RetryState, TimeoutType};
use loams_temporal_proto::temporal::api::failure::v1::{Failure, TimeoutFailureInfo, failure};
use loams_temporal_proto::temporal::api::history::v1::{
    ActivityTaskCanceledEventAttributes, ActivityTaskCompletedEventAttributes,
    ActivityTaskFailedEventAttributes, ActivityTaskStartedEventAttributes,
    ActivityTaskTimedOutEventAttributes,
};

use crate::write::{Pending, arrive};
use crate::{Effect, Rejection, Reply, TimeoutKind};

fn stale(scheduled_event_id: i64) -> Rejection {
    Rejection::NotFound(format!(
        "Activity task not found (scheduled event {scheduled_event_id})."
    ))
}

/// The pending activity for a token, checked against its attempt.
fn current(
    b: &Batch<'_>,
    scheduled_event_id: i64,
    attempt: i32,
) -> Result<ActivityInfo, Rejection> {
    loams_temporal_history::activity(b.state_ref(), scheduled_event_id)
        .filter(|a| a.attempt == attempt)
        .cloned()
        .ok_or_else(|| stale(scheduled_event_id))
}

/// The started event an outcome is written with (with the time it started).
fn started_event(a: &ActivityInfo) -> Pending {
    (
        a.started_time.as_ref().map_or(0, ms),
        Attributes::ActivityTaskStartedEventAttributes(ActivityTaskStartedEventAttributes {
            scheduled_event_id: a.scheduled_event_id,
            identity: a.started_identity.clone(),
            request_id: a.started_request_id.clone(),
            attempt: a.attempt,
            last_failure: a.last_failure.clone(),
            worker_version: a.started_worker_version.clone(),
            ..Default::default()
        }),
    )
}

pub(crate) fn started(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    identity: String,
    request_id: String,
    worker_version: Option<WorkerVersionStamp>,
) -> Result<Reply, Rejection> {
    let a = current(b, scheduled_event_id, attempt)?;
    if a.started {
        if a.started_request_id == request_id {
            return Ok(started_reply(&a));
        }
        return Err(Rejection::NotFound("Activity task already started.".into()));
    }
    let now = b.now_ms();
    let a = loams_temporal_history::activity_mut(b.state(), scheduled_event_id).expect("checked");
    a.started = true;
    a.started_identity = identity;
    a.started_request_id = request_id;
    a.started_time = Some(ts(now));
    a.started_worker_version = worker_version;
    let a = a.clone();
    let stc = dur_ms(a.start_to_close_timeout.as_ref());
    if stc > 0 {
        fx.push(Effect::ArmTimeout {
            kind: TimeoutKind::StartToClose,
            event_id: scheduled_event_id,
            attempt,
            at_ms: now + stc,
        });
    }
    let hb = dur_ms(a.heartbeat_timeout.as_ref());
    if hb > 0 {
        fx.push(Effect::ArmTimeout {
            kind: TimeoutKind::Heartbeat,
            event_id: scheduled_event_id,
            attempt,
            at_ms: now + hb,
        });
    }
    Ok(started_reply(&a))
}

fn started_reply(a: &ActivityInfo) -> Reply {
    Reply::ActivityStarted {
        scheduled_event_id: a.scheduled_event_id,
        attempt: a.attempt,
        attempt_scheduled_time_ms: a.attempt_scheduled_time.as_ref().map_or(0, ms),
        started_time_ms: a.started_time.as_ref().map_or(0, ms),
        heartbeat_details: a.heartbeat_details.clone(),
    }
}

/// Removes a finished activity and writes its started event and outcome.
fn finish(b: &mut Batch<'_>, fx: &mut Vec<Effect>, a: &ActivityInfo, outcome: Attributes) {
    loams_temporal_history::take_activity(b.state(), a.scheduled_event_id);
    fx.push(Effect::ActivityDone {
        scheduled_event_id: a.scheduled_event_id,
        attempt: a.attempt,
    });
    let now = b.now_ms();
    let mut events = Vec::new();
    if a.started {
        events.push(started_event(a));
    }
    events.push((now, outcome));
    arrive(b, fx, events);
}

pub(crate) fn completed(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    result: Option<Payloads>,
    identity: String,
) -> Result<Reply, Rejection> {
    let a = current(b, scheduled_event_id, attempt)?;
    finish(
        b,
        fx,
        &a,
        Attributes::ActivityTaskCompletedEventAttributes(ActivityTaskCompletedEventAttributes {
            result,
            scheduled_event_id,
            identity,
            ..Default::default()
        }),
    );
    Ok(Reply::Done)
}

/// Whether the failure may be retried under the activity's policy, and the
/// retry state to record if not.
fn retry_decision(
    a: &ActivityInfo,
    failure: Option<&Failure>,
    now_ms: i64,
) -> Result<i64, RetryState> {
    let policy = a.retry_policy.clone().unwrap_or_default();
    if let Some(Failure {
        failure_info: Some(failure::FailureInfo::ApplicationFailureInfo(info)),
        ..
    }) = failure
        && (info.non_retryable || policy.non_retryable_error_types.contains(&info.r#type))
    {
        return Err(RetryState::NonRetryableFailure);
    }
    // A server failure marked non-retryable (the gateway's own, for an
    // oversized result or heartbeat, X1.R7) is final, as in Temporal's
    // `retrypolicy.IsRetryable`.
    if let Some(Failure {
        failure_info: Some(failure::FailureInfo::ServerFailureInfo(info)),
        ..
    }) = failure
        && info.non_retryable
    {
        return Err(RetryState::NonRetryableFailure);
    }
    if policy.maximum_attempts > 0 && a.attempt >= policy.maximum_attempts {
        return Err(RetryState::MaximumAttemptsReached);
    }
    let initial = dur_ms(policy.initial_interval.as_ref()).max(1) as f64;
    let coefficient = if policy.backoff_coefficient < 1.0 {
        2.0
    } else {
        policy.backoff_coefficient
    };
    let mut backoff = initial * coefficient.powi(a.attempt - 1);
    let max = dur_ms(policy.maximum_interval.as_ref()) as f64;
    if max > 0.0 && backoff > max {
        backoff = max;
    }
    let backoff = backoff as i64;
    let s2c = dur_ms(a.schedule_to_close_timeout.as_ref());
    if s2c > 0 {
        let deadline = a.scheduled_time.as_ref().map_or(0, ms) + s2c;
        if now_ms + backoff >= deadline {
            return Err(RetryState::Timeout);
        }
    }
    Ok(backoff)
}

/// Schedules the next attempt after `backoff_ms`; nothing is written to
/// history (Temporal records only the last attempt).
fn retry(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    a: &ActivityInfo,
    backoff_ms: i64,
    failure: Option<Failure>,
) {
    let now = b.now_ms();
    let next =
        loams_temporal_history::activity_mut(b.state(), a.scheduled_event_id).expect("pending");
    next.attempt += 1;
    next.started = false;
    next.started_identity.clear();
    next.started_request_id.clear();
    next.started_time = None;
    next.started_worker_version = None;
    next.attempt_scheduled_time = Some(ts(now + backoff_ms));
    next.last_failure = failure;
    let attempt = next.attempt;
    let task_queue = next.task_queue.clone();
    let s2s = dur_ms(next.schedule_to_start_timeout.as_ref());
    fx.push(Effect::ActivityDone {
        scheduled_event_id: a.scheduled_event_id,
        attempt: a.attempt,
    });
    fx.push(Effect::ScheduleActivity {
        scheduled_event_id: a.scheduled_event_id,
        attempt,
        task_queue,
        not_before_ms: now + backoff_ms,
    });
    if s2s > 0 {
        fx.push(Effect::ArmTimeout {
            kind: TimeoutKind::ScheduleToStart,
            event_id: a.scheduled_event_id,
            attempt,
            at_ms: now + backoff_ms + s2s,
        });
    }
}

pub(crate) fn failed(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    failure: Option<Failure>,
    identity: String,
    last_heartbeat_details: Option<Payloads>,
) -> Result<Reply, Rejection> {
    let mut a = current(b, scheduled_event_id, attempt)?;
    if last_heartbeat_details.is_some() {
        a.heartbeat_details = last_heartbeat_details.clone();
        if let Some(x) = loams_temporal_history::activity_mut(b.state(), scheduled_event_id) {
            x.heartbeat_details = last_heartbeat_details;
        }
    }
    match retry_decision(&a, failure.as_ref(), b.now_ms()) {
        Ok(backoff) => retry(b, fx, &a, backoff, failure),
        Err(retry_state) => finish(
            b,
            fx,
            &a,
            Attributes::ActivityTaskFailedEventAttributes(ActivityTaskFailedEventAttributes {
                failure,
                scheduled_event_id,
                identity,
                retry_state: retry_state as i32,
                ..Default::default()
            }),
        ),
    }
    Ok(Reply::Done)
}

pub(crate) fn canceled(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    details: Option<Payloads>,
    identity: String,
) -> Result<Reply, Rejection> {
    let a = current(b, scheduled_event_id, attempt)?;
    if !a.cancel_requested {
        return Err(Rejection::InvalidArgument(
            "the workflow has not requested this activity's cancellation".into(),
        ));
    }
    finish(
        b,
        fx,
        &a,
        Attributes::ActivityTaskCanceledEventAttributes(ActivityTaskCanceledEventAttributes {
            details,
            latest_cancel_requested_event_id: a.cancel_requested_event_id,
            scheduled_event_id,
            identity,
            ..Default::default()
        }),
    );
    Ok(Reply::Done)
}

pub(crate) fn heartbeat(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    details: Option<Payloads>,
) -> Result<Reply, Rejection> {
    let a = current(b, scheduled_event_id, attempt)?;
    if !a.started {
        return Err(stale(scheduled_event_id));
    }
    let now = b.now_ms();
    let x = loams_temporal_history::activity_mut(b.state(), scheduled_event_id).expect("checked");
    if details.is_some() {
        x.heartbeat_details = details;
    }
    x.last_heartbeat_time = Some(ts(now));
    let hb = dur_ms(x.heartbeat_timeout.as_ref());
    let cancel_requested = x.cancel_requested;
    if hb > 0 {
        fx.push(Effect::ArmTimeout {
            kind: TimeoutKind::Heartbeat,
            event_id: scheduled_event_id,
            attempt,
            at_ms: now + hb,
        });
    }
    Ok(Reply::Heartbeat { cancel_requested })
}

pub(crate) fn timed_out(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    kind: TimeoutKind,
) -> Result<Reply, Rejection> {
    let Ok(a) = current(b, scheduled_event_id, attempt) else {
        return Ok(Reply::Stale);
    };
    let now = b.now_ms();
    let timeout_type = match kind {
        TimeoutKind::StartToClose if a.started => TimeoutType::StartToClose,
        TimeoutKind::Heartbeat if a.started => {
            // A heartbeat re-arms its timeout; an older deadline is stale.
            let last = a
                .last_heartbeat_time
                .as_ref()
                .or(a.started_time.as_ref())
                .map_or(0, ms);
            if last + dur_ms(a.heartbeat_timeout.as_ref()) > now {
                return Ok(Reply::Stale);
            }
            TimeoutType::Heartbeat
        }
        TimeoutKind::ScheduleToStart if !a.started => TimeoutType::ScheduleToStart,
        TimeoutKind::ScheduleToClose => TimeoutType::ScheduleToClose,
        _ => return Ok(Reply::Stale),
    };
    let failure = Failure {
        message: format!(
            "activity {} timeout",
            match timeout_type {
                TimeoutType::StartToClose => "StartToClose",
                TimeoutType::Heartbeat => "Heartbeat",
                TimeoutType::ScheduleToStart => "ScheduleToStart",
                _ => "ScheduleToClose",
            }
        ),
        source: "Server".into(),
        failure_info: Some(failure::FailureInfo::TimeoutFailureInfo(
            TimeoutFailureInfo {
                timeout_type: timeout_type as i32,
                last_heartbeat_details: a.heartbeat_details.clone(),
            },
        )),
        ..Default::default()
    };
    let retryable = matches!(
        timeout_type,
        TimeoutType::StartToClose | TimeoutType::Heartbeat
    );
    let decision = if retryable {
        retry_decision(&a, None, now)
    } else {
        Err(RetryState::Timeout)
    };
    match decision {
        Ok(backoff) => retry(b, fx, &a, backoff, Some(failure)),
        Err(retry_state) => finish(
            b,
            fx,
            &a,
            Attributes::ActivityTaskTimedOutEventAttributes(ActivityTaskTimedOutEventAttributes {
                failure: Some(failure),
                scheduled_event_id,
                started_event_id: 0,
                retry_state: if retryable {
                    retry_state as i32
                } else {
                    RetryState::Timeout as i32
                },
            }),
        ),
    }
    Ok(Reply::Done)
}
