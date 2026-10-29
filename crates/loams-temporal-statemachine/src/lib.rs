//! The Temporal gateway's state machines (design 74 §5.3 and §13, plan DX1
//! Task 4): one pure function, [`apply`], that takes a run's mutable state and
//! one input (a start, a worker's answer, a signal, a timeout) and returns the
//! new state, the history events to append, the effects to put in the outbox,
//! and the reply.
//!
//! It is pure: it reads no clock (the input carries `now_ms`), makes no IDs
//! (run and request IDs come in), and does no I/O. The caller commits
//! `events`, `state` and the outbox rows for `effects` in one compare-and-set
//! transaction, or nothing (D1502).
//!
//! What it writes is fixed by the corpus of real Temporal histories (DX0
//! Task 4) and by the SDKs' replayers, not by Temporal's server code (D1536):
//! - events that arrive while a workflow task is started are buffered and
//!   written after its `WorkflowTaskCompleted` (or failure), and a completion
//!   that leaves buffered events schedules the next workflow task;
//! - an activity's `ActivityTaskStarted` is written together with its
//!   outcome, carrying the time it started;
//! - a close command while events are buffered fails the workflow task with
//!   `UNHANDLED_COMMAND`, so the worker sees the new events first.

// The SDKs still send, and the corpus still records, deprecated fields such
// as `worker_version`; the gateway must carry them.
#![allow(deprecated)]

mod activity;
pub mod children;
mod commands;
mod external;
pub mod reset;
pub mod start;
pub mod update;
mod workflow_task;
mod write;

use loams_temporal_history::{Batch, HistoryEvent, MutableState, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::command::v1::Command;
use loams_temporal_proto::temporal::api::common::v1::{
    Header, MeteringMetadata, Payloads, WorkerVersionStamp,
};
use loams_temporal_proto::temporal::api::failure::v1::Failure;
use loams_temporal_proto::temporal::api::protocol::v1::Message;
use loams_temporal_proto::temporal::api::sdk::v1::WorkflowTaskCompletedMetadata;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;

pub use external::HISTORY_SERVICE;
pub use loams_temporal_history as history;
pub use start::StartChain;
pub use update::{UpdateStatus, messages_for_task};
pub use workflow_task::server_failure;

/// One thing that happens to a run.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// `StartWorkflowExecution`: the run's first events. The state passed to
    /// [`apply`] is ignored.
    Start {
        /// The gateway's namespace ID.
        namespace_id: String,
        /// The new run's ID.
        run_id: String,
        /// The request as the SDK sent it.
        request: Box<StartWorkflowExecutionRequest>,
    },
    /// `SignalWithStartWorkflowExecution` with no running run: the run's
    /// first events with the signal between `WorkflowExecutionStarted` and
    /// the first `WorkflowTaskScheduled`, as Temporal writes them. The
    /// state passed to [`apply`] is ignored.
    SignalWithStart {
        /// The gateway's namespace ID.
        namespace_id: String,
        /// The new run's ID.
        run_id: String,
        /// The start part of the request.
        request: Box<StartWorkflowExecutionRequest>,
        /// The signal part.
        signal: StartSignal,
    },
    /// A poll took the outstanding workflow task.
    WorkflowTaskStarted {
        /// The task's scheduled event.
        scheduled_event_id: i64,
        /// The poller's identity.
        identity: String,
        /// The poll's request ID (a retried poll with the same ID gets the
        /// same task back).
        request_id: String,
        /// The poller's build ID stamp, if it sent one.
        worker_version: Option<WorkerVersionStamp>,
    },
    /// `RespondWorkflowTaskCompleted`.
    WorkflowTaskCompleted {
        /// The task's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The task's attempt (from the token).
        attempt: i32,
        /// The worker's identity.
        identity: String,
        /// The commands, in order.
        commands: Vec<Command>,
        /// The SDK's flags.
        sdk_metadata: Option<WorkflowTaskCompletedMetadata>,
        /// The SDK's metering data.
        metering_metadata: Option<MeteringMetadata>,
        /// The worker's build ID stamp.
        worker_version: Option<WorkerVersionStamp>,
        /// Schedule another workflow task even with nothing new.
        force_create_new_workflow_task: bool,
        /// The worker's protocol messages (update acceptances, rejections
        /// and responses), in order.
        messages: Vec<Message>,
        /// The run ID a `ContinueAsNew` command gives the next run (the
        /// caller makes one for every completion; unused otherwise).
        new_run_id: String,
    },
    /// `RespondWorkflowTaskFailed`.
    WorkflowTaskFailed {
        /// The task's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The task's attempt (from the token).
        attempt: i32,
        /// `temporal.api.enums.v1.WorkflowTaskFailedCause`.
        cause: i32,
        /// The worker's failure.
        failure: Option<Failure>,
        /// The worker's identity.
        identity: String,
    },
    /// The started workflow task's start-to-close timeout fired.
    WorkflowTaskTimedOut {
        /// The task's scheduled event.
        scheduled_event_id: i64,
        /// The task's attempt.
        attempt: i32,
    },
    /// A poll took an activity task.
    ActivityStarted {
        /// The activity's scheduled event.
        scheduled_event_id: i64,
        /// The attempt the poll took.
        attempt: i32,
        /// The poller's identity.
        identity: String,
        /// The poll's request ID.
        request_id: String,
        /// The poller's build ID stamp.
        worker_version: Option<WorkerVersionStamp>,
    },
    /// `RespondActivityTaskCompleted`.
    ActivityCompleted {
        /// The activity's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The attempt (from the token).
        attempt: i32,
        /// The result.
        result: Option<Payloads>,
        /// The worker's identity.
        identity: String,
    },
    /// `RespondActivityTaskFailed`.
    ActivityFailed {
        /// The activity's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The attempt (from the token).
        attempt: i32,
        /// The failure.
        failure: Option<Failure>,
        /// The worker's identity.
        identity: String,
        /// Heartbeat details the worker sent with the failure.
        last_heartbeat_details: Option<Payloads>,
    },
    /// `RespondActivityTaskCanceled`.
    ActivityCanceled {
        /// The activity's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The attempt (from the token).
        attempt: i32,
        /// The details.
        details: Option<Payloads>,
        /// The worker's identity.
        identity: String,
    },
    /// `RecordActivityTaskHeartbeat`.
    ActivityHeartbeat {
        /// The activity's scheduled event (from the token).
        scheduled_event_id: i64,
        /// The attempt (from the token).
        attempt: i32,
        /// The details.
        details: Option<Payloads>,
    },
    /// One of an activity's timeouts fired.
    ActivityTimedOut {
        /// The activity's scheduled event.
        scheduled_event_id: i64,
        /// The attempt the timeout was armed for.
        attempt: i32,
        /// Which timeout.
        kind: TimeoutKind,
    },
    /// `SignalWorkflowExecution`.
    Signal {
        /// The signal's name.
        name: String,
        /// Its input.
        input: Option<Payloads>,
        /// The sender's identity.
        identity: String,
        /// Its header.
        header: Option<Header>,
        /// The request ID.
        request_id: String,
    },
    /// `RequestCancelWorkflowExecution`.
    CancelRequested {
        /// The reason.
        reason: String,
        /// The requester's identity.
        identity: String,
    },
    /// `TerminateWorkflowExecution`.
    Terminate {
        /// The reason.
        reason: String,
        /// The details.
        details: Option<Payloads>,
        /// The requester's identity.
        identity: String,
    },
    /// A durable timer fired.
    TimerFired {
        /// The timer's started event.
        started_event_id: i64,
    },
    /// A child run, the next run of a continue-as-new, or a run started on
    /// a schedule: a start with its lineage (parent, previous run, first
    /// task backoff). The state passed to [`apply`] is ignored.
    StartChained {
        /// The gateway's namespace ID.
        namespace_id: String,
        /// The new run's ID.
        run_id: String,
        /// The start request.
        request: Box<StartWorkflowExecutionRequest>,
        /// Its lineage.
        chain: Box<start::StartChain>,
    },
    /// A child this run initiated has started.
    ChildStarted {
        /// The `StartChildWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
        /// The child's run ID.
        run_id: String,
        /// The child's header.
        header: Option<Header>,
    },
    /// A child could not be started (`StartChildWorkflowExecutionFailedCause`).
    ChildStartFailed {
        /// The `StartChildWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
        /// Why.
        cause: i32,
    },
    /// A child's run closed.
    ChildClosed {
        /// The `StartChildWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
        /// The child's run ID.
        run_id: String,
        /// How it closed.
        outcome: children::ChildOutcome,
    },
    /// A signal to, or cancel request of, another workflow was delivered
    /// (`failed_cause` `None`) or could not be.
    ExternalResult {
        /// Signal or cancel.
        kind: children::ExternalKind,
        /// The `…Initiated` event.
        initiated_event_id: i64,
        /// The failure cause, if it failed.
        failed_cause: Option<i32>,
    },
    /// `UpdateWorkflowExecution`: admit a workflow update. A known update
    /// ID changes nothing ([`Reply::Stale`]).
    UpdateAdmitted {
        /// The update's request.
        request: Box<loams_temporal_proto::temporal::api::update::v1::Request>,
    },
}

/// The signal a run is started with (`SignalWithStartWorkflowExecution`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartSignal {
    /// The signal's name.
    pub name: String,
    /// Its input.
    pub input: Option<Payloads>,
    /// The sender's identity.
    pub identity: String,
    /// Its header.
    pub header: Option<Header>,
    /// The request ID.
    pub request_id: String,
}

/// Which deadline a timeout is (design 74 §5.4's `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeoutKind {
    /// A workflow task's start-to-close.
    WorkflowTask,
    /// An activity's schedule-to-start.
    ScheduleToStart,
    /// An activity's start-to-close.
    StartToClose,
    /// An activity's schedule-to-close.
    ScheduleToClose,
    /// An activity's heartbeat.
    Heartbeat,
}

/// What the gateway must do outside the history once the transition commits
/// (each becomes an outbox row, design 74 §5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Make a workflow task available to pollers of `task_queue`.
    ScheduleWorkflowTask {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// Its attempt.
        attempt: i32,
        /// The task queue.
        task_queue: String,
    },
    /// The workflow task is finished (completed, failed or timed out): its
    /// durable task is settled.
    WorkflowTaskDone {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// Its attempt.
        attempt: i32,
    },
    /// Make an activity attempt available to pollers, not before
    /// `not_before_ms` (a retry's backoff).
    ScheduleActivity {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// The attempt.
        attempt: i32,
        /// The task queue.
        task_queue: String,
        /// The earliest dispatch time; 0 for now.
        not_before_ms: i64,
    },
    /// The activity attempt is finished (or cancelled before it started).
    ActivityDone {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// The attempt.
        attempt: i32,
    },
    /// Arm a timeout; when it fires, feed back the matching `…TimedOut`
    /// input. A later state change makes it stale.
    ArmTimeout {
        /// Which timeout.
        kind: TimeoutKind,
        /// The workflow task's or activity's scheduled event.
        event_id: i64,
        /// The attempt.
        attempt: i32,
        /// When it fires.
        at_ms: i64,
    },
    /// Start a durable timer; when it fires, feed back `TimerFired`.
    StartTimer {
        /// The timer's started event.
        started_event_id: i64,
        /// When it fires.
        fire_at_ms: i64,
    },
    /// Cancel a durable timer.
    CancelTimer {
        /// The timer's started event.
        started_event_id: i64,
    },
    /// Start the child initiated by this event (the frontend does it after
    /// the commit and answers with `ChildStarted` or `ChildStartFailed`).
    StartChild {
        /// The `StartChildWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
    },
    /// Start the run this one continued as.
    ContinuedAsNew {
        /// Its run ID.
        new_run_id: String,
    },
    /// Signal the workflow the initiated event names.
    SignalExternal {
        /// The `SignalExternalWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
    },
    /// Ask the workflow the initiated event names to cancel.
    CancelExternal {
        /// The `RequestCancelExternalWorkflowExecutionInitiated` event.
        initiated_event_id: i64,
    },
    /// The run closed with children the parent close policy acts on.
    ParentClosePolicy {
        /// The children and their policies.
        children: Vec<children::ChildRef>,
    },
    /// The run closed: settle everything it still has outstanding and wake
    /// whoever waits for the close.
    RunClosed {
        /// How it closed.
        status: WorkflowExecutionStatus,
    },
}

/// What the caller tells the requester.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Applied.
    Done,
    /// A timeout or timer for something that has moved on: nothing changed
    /// (§13.2 invariant 4). The caller acknowledges it and commits nothing.
    Stale,
    /// A poll got the workflow task.
    WorkflowTaskStarted {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// Its started event.
        started_event_id: i64,
        /// Its attempt.
        attempt: i32,
        /// The previous completed task's started event (0 if none).
        previous_started_event_id: i64,
        /// When it was scheduled.
        scheduled_time_ms: i64,
        /// When it started.
        started_time_ms: i64,
    },
    /// A poll got an activity attempt.
    ActivityStarted {
        /// Its scheduled event.
        scheduled_event_id: i64,
        /// The attempt.
        attempt: i32,
        /// When this attempt was scheduled.
        attempt_scheduled_time_ms: i64,
        /// When it started.
        started_time_ms: i64,
        /// Details from the previous attempt's heartbeats.
        heartbeat_details: Option<Payloads>,
    },
    /// A heartbeat was recorded.
    Heartbeat {
        /// The workflow asked to cancel the activity.
        cancel_requested: bool,
    },
}

/// Why an input is refused. Each maps to the gRPC status the SDKs expect.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// `NOT_FOUND`: the task is not outstanding (a stale token), or the run
    /// has closed ("workflow execution already completed").
    #[error("not found: {0}")]
    NotFound(String),
    /// `INVALID_ARGUMENT`: a malformed request or command.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// `UNIMPLEMENTED`: a command of a later phase (§74 §5.8).
    #[error("unimplemented: {0}")]
    Unimplemented(String),
    /// `RESOURCE_EXHAUSTED`: a limit (X1.R7).
    #[error("resource exhausted: {0}")]
    ResourceExhausted(String),
}

/// The message Temporal's SDKs recognise for a closed run.
pub const COMPLETED: &str = "workflow execution already completed";

/// The result of [`apply`].
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// The run's new state.
    pub state: MutableState,
    /// The events to append (contiguous from the old `next_event_id`).
    pub events: Vec<HistoryEvent>,
    /// What to put in the outbox.
    pub effects: Vec<Effect>,
    /// What to tell the requester.
    pub reply: Reply,
}

impl Transition {
    /// Whether there is anything to commit.
    pub fn changes_state(&self) -> bool {
        !matches!(self.reply, Reply::Stale)
    }
}

/// Applies `input` to `state` at `now_ms`.
pub fn apply(state: &MutableState, input: Input, now_ms: i64) -> Result<Transition, Rejection> {
    let input_kind = if matches!(input, Input::Start { .. }) {
        InputKind::Start
    } else {
        InputKind::Other
    };
    let mut s = state.clone();
    let mut fx = Vec::new();
    let (reply, events) = {
        let mut b = Batch::new(&mut s, now_ms);
        let reply = dispatch(&mut b, &mut fx, input)?;
        (reply, b.finish())
    };
    if matches!(reply, Reply::Stale) {
        return Ok(Transition {
            state: state.clone(),
            events: Vec::new(),
            effects: Vec::new(),
            reply,
        });
    }
    if !matches!(input_kind, InputKind::Start)
        && loams_temporal_history::is_open(&s)
        && let Some(reason) = history_limit(state, &s)
    {
        return Ok(over_history_limit(state, reason, now_ms));
    }
    // Invariant 1 on every batch; the others need the whole history and are
    // checked by the tests over complete runs.
    debug_assert!(
        events
            .iter()
            .enumerate()
            .all(|(i, e)| e.event_id == s.next_event_id - events.len() as i64 + i as i64),
        "event IDs continue the run's history"
    );
    Ok(Transition {
        state: s,
        events,
        effects: fx,
        reply,
    })
}

enum InputKind {
    Start,
    Other,
}

/// Which of X1.R7's history limits a change on `before` that leads to
/// `after` breaks, as Temporal's `UpdateWorkflowExecutionAsActive` checks
/// them (v1.32.1 `service/history/workflow/context.go`): the size is the
/// history already written (`ExecutionStats.HistorySize`, over 50 MiB), the
/// count includes the change's own events (`nextEventID - 1`, over 51,200).
fn history_limit(before: &MutableState, after: &MutableState) -> Option<&'static str> {
    let limits = loams_temporal_history::Limits::default();
    if before.history_size_bytes > limits.history_size {
        Some(loams_temporal_history::limit_messages::HISTORY_SIZE)
    } else if after.next_event_id - 1 > limits.history_count {
        Some(loams_temporal_history::limit_messages::HISTORY_COUNT)
    } else {
        None
    }
}

/// Temporal's `forceTerminateWorkflow`: the change is discarded and the run,
/// as it was, is terminated with the limit's reason by `history-service`.
fn over_history_limit(state: &MutableState, reason: &str, now_ms: i64) -> Transition {
    let mut s = state.clone();
    let mut fx = Vec::new();
    let events = {
        let mut b = Batch::new(&mut s, now_ms);
        external::terminate(
            &mut b,
            &mut fx,
            reason.into(),
            None,
            loams_temporal_history::limit_messages::HISTORY_SERVICE.into(),
        )
        .expect("terminating an open run is always accepted");
        b.finish()
    };
    Transition {
        state: s,
        events,
        effects: fx,
        reply: Reply::Done,
    }
}

fn dispatch(b: &mut Batch<'_>, fx: &mut Vec<Effect>, input: Input) -> Result<Reply, Rejection> {
    if !matches!(
        input,
        Input::Start { .. } | Input::SignalWithStart { .. } | Input::StartChained { .. }
    ) && !loams_temporal_history::is_open(b.state_ref())
    {
        return match input {
            // A late timeout for a closed run changes nothing.
            Input::WorkflowTaskTimedOut { .. }
            | Input::ActivityTimedOut { .. }
            | Input::TimerFired { .. } => Ok(Reply::Stale),
            _ => Err(Rejection::NotFound(COMPLETED.into())),
        };
    }
    match input {
        Input::Start {
            namespace_id,
            run_id,
            request,
        } => start::start(b, fx, &namespace_id, &run_id, *request, None, None),
        Input::StartChained {
            namespace_id,
            run_id,
            request,
            chain,
        } => start::start(b, fx, &namespace_id, &run_id, *request, None, Some(*chain)),
        Input::ChildStarted {
            initiated_event_id,
            run_id,
            header,
        } => children::child_started(b, fx, initiated_event_id, run_id, header, None),
        Input::ChildStartFailed {
            initiated_event_id,
            cause,
        } => children::child_started(b, fx, initiated_event_id, String::new(), None, Some(cause)),
        Input::ChildClosed {
            initiated_event_id,
            run_id,
            outcome,
        } => children::child_closed(b, fx, initiated_event_id, run_id, outcome),
        Input::ExternalResult {
            kind,
            initiated_event_id,
            failed_cause,
        } => children::external_result(b, fx, kind, initiated_event_id, failed_cause),
        Input::SignalWithStart {
            namespace_id,
            run_id,
            request,
            signal,
        } => start::start(b, fx, &namespace_id, &run_id, *request, Some(signal), None),
        Input::WorkflowTaskStarted {
            scheduled_event_id,
            identity,
            request_id,
            worker_version,
        } => workflow_task::started(
            b,
            fx,
            scheduled_event_id,
            identity,
            request_id,
            worker_version,
        ),
        Input::WorkflowTaskCompleted {
            scheduled_event_id,
            attempt,
            identity,
            commands,
            sdk_metadata,
            metering_metadata,
            worker_version,
            force_create_new_workflow_task,
            messages,
            new_run_id,
        } => workflow_task::completed(
            b,
            fx,
            workflow_task::Completion {
                scheduled_event_id,
                attempt,
                identity,
                commands,
                sdk_metadata,
                metering_metadata,
                worker_version,
                force_create_new_workflow_task,
                messages,
                new_run_id,
            },
        ),
        Input::WorkflowTaskFailed {
            scheduled_event_id,
            attempt,
            cause,
            failure,
            identity,
        } => workflow_task::failed(b, fx, scheduled_event_id, attempt, cause, failure, identity),
        Input::WorkflowTaskTimedOut {
            scheduled_event_id,
            attempt,
        } => workflow_task::timed_out(b, fx, scheduled_event_id, attempt),
        Input::ActivityStarted {
            scheduled_event_id,
            attempt,
            identity,
            request_id,
            worker_version,
        } => activity::started(
            b,
            fx,
            scheduled_event_id,
            attempt,
            identity,
            request_id,
            worker_version,
        ),
        Input::ActivityCompleted {
            scheduled_event_id,
            attempt,
            result,
            identity,
        } => activity::completed(b, fx, scheduled_event_id, attempt, result, identity),
        Input::ActivityFailed {
            scheduled_event_id,
            attempt,
            failure,
            identity,
            last_heartbeat_details,
        } => activity::failed(
            b,
            fx,
            scheduled_event_id,
            attempt,
            failure,
            identity,
            last_heartbeat_details,
        ),
        Input::ActivityCanceled {
            scheduled_event_id,
            attempt,
            details,
            identity,
        } => activity::canceled(b, fx, scheduled_event_id, attempt, details, identity),
        Input::ActivityHeartbeat {
            scheduled_event_id,
            attempt,
            details,
        } => activity::heartbeat(b, fx, scheduled_event_id, attempt, details),
        Input::ActivityTimedOut {
            scheduled_event_id,
            attempt,
            kind,
        } => activity::timed_out(b, fx, scheduled_event_id, attempt, kind),
        Input::Signal {
            name,
            input,
            identity,
            header,
            request_id,
        } => external::signal(b, fx, name, input, identity, header, request_id),
        Input::CancelRequested { reason, identity } => {
            external::cancel_requested(b, fx, reason, identity)
        }
        Input::Terminate {
            reason,
            details,
            identity,
        } => external::terminate(b, fx, reason, details, identity),
        Input::TimerFired { started_event_id } => external::timer_fired(b, fx, started_event_id),
        Input::UpdateAdmitted { request } => update::admit(b, fx, *request),
    }
}
