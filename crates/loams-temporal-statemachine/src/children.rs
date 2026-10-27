//! Child workflows, continue-as-new, signals to and cancel requests of
//! other workflows, and search-attribute and memo upserts (TMP-api group D,
//! D2087).
//!
//! The commands write their `…Initiated` (or closing) event and an effect
//! the frontend carries out after the commit: start the child or the next
//! run, signal or cancel the other workflow. The answer comes back as an
//! input ([`child_started`], [`child_closed`], [`external_result`]), which
//! writes the event the SDKs wait for, as Temporal's history shows it (the
//! corpus's `child_workflow`, `child_workflow_cancelled` and
//! `continue_as_new_3`).

use loams_temporal_history::time::dur;
use loams_temporal_history::{Attributes, Batch, ChildInfo, ExternalInfo, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::command::v1::{
    ContinueAsNewWorkflowExecutionCommandAttributes, ModifyWorkflowPropertiesCommandAttributes,
    RequestCancelExternalWorkflowExecutionCommandAttributes,
    SignalExternalWorkflowExecutionCommandAttributes, StartChildWorkflowExecutionCommandAttributes,
    UpsertWorkflowSearchAttributesCommandAttributes,
};
use loams_temporal_proto::temporal::api::common::v1::{
    Header, Payloads, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::{
    ContinueAsNewInitiator, ParentClosePolicy, RetryState, TaskQueueKind,
};
use loams_temporal_proto::temporal::api::failure::v1::Failure;
use loams_temporal_proto::temporal::api::history::v1::{
    ChildWorkflowExecutionCanceledEventAttributes, ChildWorkflowExecutionCompletedEventAttributes,
    ChildWorkflowExecutionFailedEventAttributes, ChildWorkflowExecutionStartedEventAttributes,
    ChildWorkflowExecutionTerminatedEventAttributes, ChildWorkflowExecutionTimedOutEventAttributes,
    ExternalWorkflowExecutionCancelRequestedEventAttributes,
    ExternalWorkflowExecutionSignaledEventAttributes,
    RequestCancelExternalWorkflowExecutionFailedEventAttributes,
    RequestCancelExternalWorkflowExecutionInitiatedEventAttributes,
    SignalExternalWorkflowExecutionFailedEventAttributes,
    SignalExternalWorkflowExecutionInitiatedEventAttributes,
    StartChildWorkflowExecutionFailedEventAttributes,
    StartChildWorkflowExecutionInitiatedEventAttributes,
    UpsertWorkflowSearchAttributesEventAttributes, WorkflowExecutionContinuedAsNewEventAttributes,
    WorkflowPropertiesModifiedEventAttributes,
};
use loams_temporal_proto::temporal::api::sdk::v1::UserMetadata;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;

use crate::write::{arrive, close};
use crate::{Effect, Rejection, Reply};

/// How a child run ended, as its parent records it.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ChildOutcome {
    /// `ChildWorkflowExecutionCompleted`.
    Completed(Option<Payloads>),
    /// `ChildWorkflowExecutionFailed`.
    Failed(Option<Failure>, RetryState),
    /// `ChildWorkflowExecutionCanceled`.
    Canceled(Option<Payloads>),
    /// `ChildWorkflowExecutionTimedOut`.
    TimedOut(RetryState),
    /// `ChildWorkflowExecutionTerminated`.
    Terminated,
}

/// Which request to another workflow an answer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalKind {
    /// `SignalExternalWorkflowExecution`.
    Signal,
    /// `RequestCancelExternalWorkflowExecution`.
    Cancel,
}

/// A child the parent close policy acts on when its parent closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRef {
    /// The child's namespace name (empty: the parent's).
    pub namespace: String,
    /// The child's namespace ID, if known.
    pub namespace_id: String,
    /// The child's workflow ID.
    pub workflow_id: String,
    /// The child's run ID (empty if it never started).
    pub run_id: String,
    /// `temporal.api.enums.v1.ParentClosePolicy`.
    pub policy: i32,
}

fn invalid(why: impl Into<String>) -> Rejection {
    Rejection::InvalidArgument(why.into())
}

fn execution(c: &ChildInfo) -> WorkflowExecution {
    WorkflowExecution {
        workflow_id: c.workflow_id.clone(),
        run_id: c.run_id.clone(),
    }
}

fn wf_type(c: &ChildInfo) -> Option<WorkflowType> {
    Some(WorkflowType {
        name: c.workflow_type.clone(),
    })
}

/// `StartChildWorkflowExecution`.
pub(crate) fn start_child(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    a: StartChildWorkflowExecutionCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
) -> Result<(), Rejection> {
    if a.workflow_id.is_empty() {
        return Err(invalid("Required field WorkflowId is not set."));
    }
    let workflow_type = a
        .workflow_type
        .clone()
        .filter(|t| !t.name.is_empty())
        .ok_or_else(|| invalid("Required field WorkflowType is not set."))?;
    let s = b.state_ref();
    let same_namespace = a.namespace.is_empty() || a.namespace == s.namespace_name;
    let namespace = if a.namespace.is_empty() {
        s.namespace_name.clone()
    } else {
        a.namespace.clone()
    };
    let namespace_id = if same_namespace {
        s.namespace_id.clone()
    } else {
        String::new()
    };
    let task_queue = a
        .task_queue
        .as_ref()
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| s.task_queue.clone());
    let task_timeout = a.workflow_task_timeout.or(s.workflow_task_timeout);
    let parent_close_policy = match a.parent_close_policy {
        0 => ParentClosePolicy::Terminate as i32,
        p => p,
    };
    let id = b.push_with_metadata(
        md,
        Attributes::StartChildWorkflowExecutionInitiatedEventAttributes(
            StartChildWorkflowExecutionInitiatedEventAttributes {
                namespace: namespace.clone(),
                namespace_id: namespace_id.clone(),
                workflow_id: a.workflow_id.clone(),
                workflow_type: Some(workflow_type.clone()),
                task_queue: Some(TaskQueue {
                    name: task_queue,
                    kind: TaskQueueKind::Normal as i32,
                    normal_name: String::new(),
                }),
                input: a.input,
                workflow_execution_timeout: Some(a.workflow_execution_timeout.unwrap_or(dur(0))),
                workflow_run_timeout: Some(a.workflow_run_timeout.unwrap_or(dur(0))),
                workflow_task_timeout: task_timeout,
                parent_close_policy,
                control: a.control,
                workflow_task_completed_event_id: completed_id,
                workflow_id_reuse_policy: a.workflow_id_reuse_policy,
                retry_policy: a.retry_policy,
                cron_schedule: a.cron_schedule,
                header: Some(a.header.unwrap_or_default()),
                memo: a.memo,
                search_attributes: a.search_attributes,
                #[allow(deprecated)]
                inherit_build_id: a.inherit_build_id,
                priority: a.priority,
                ..Default::default()
            },
        ),
    );
    b.state().children.push(ChildInfo {
        initiated_event_id: id,
        namespace,
        namespace_id,
        workflow_id: a.workflow_id,
        workflow_type: workflow_type.name,
        parent_close_policy,
        ..Default::default()
    });
    fx.push(Effect::StartChild {
        initiated_event_id: id,
    });
    Ok(())
}

/// `ContinueAsNewWorkflowExecution`: closes the run; the frontend starts the
/// next one with `new_run_id`.
pub(crate) fn continue_as_new(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    a: ContinueAsNewWorkflowExecutionCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
    new_run_id: &str,
) -> Result<(), Rejection> {
    if new_run_id.is_empty() {
        return Err(invalid("continue-as-new needs a new run ID"));
    }
    let s = b.state_ref();
    let workflow_type = a
        .workflow_type
        .clone()
        .filter(|t| !t.name.is_empty())
        .unwrap_or_else(|| WorkflowType {
            name: s.workflow_type.clone(),
        });
    let task_queue = a
        .task_queue
        .as_ref()
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| s.task_queue.clone());
    let run_timeout = a
        .workflow_run_timeout
        .or(s.workflow_run_timeout)
        .unwrap_or(dur(0));
    let task_timeout = a.workflow_task_timeout.or(s.workflow_task_timeout);
    let initiator = match a.initiator {
        0 => ContinueAsNewInitiator::Workflow as i32,
        i => i,
    };
    let id = b.push_with_metadata(
        md,
        Attributes::WorkflowExecutionContinuedAsNewEventAttributes(
            WorkflowExecutionContinuedAsNewEventAttributes {
                new_execution_run_id: new_run_id.to_string(),
                workflow_type: Some(workflow_type),
                task_queue: Some(TaskQueue {
                    name: task_queue,
                    kind: TaskQueueKind::Normal as i32,
                    normal_name: String::new(),
                }),
                input: a.input,
                workflow_run_timeout: Some(run_timeout),
                workflow_task_timeout: task_timeout,
                workflow_task_completed_event_id: completed_id,
                backoff_start_interval: a.backoff_start_interval,
                initiator,
                failure: a.failure,
                last_completion_result: a.last_completion_result,
                header: Some(a.header.unwrap_or_default()),
                memo: a.memo,
                search_attributes: a.search_attributes,
                #[allow(deprecated)]
                inherit_build_id: a.inherit_build_id,
                initial_versioning_behavior: a.initial_versioning_behavior,
            },
        ),
    );
    close(b, fx, id, WorkflowExecutionStatus::ContinuedAsNew);
    fx.push(Effect::ContinuedAsNew {
        new_run_id: new_run_id.to_string(),
    });
    Ok(())
}

fn target_namespace(b: &Batch<'_>, namespace: &str) -> (String, String) {
    let s = b.state_ref();
    if namespace.is_empty() || namespace == s.namespace_name {
        (s.namespace_name.clone(), s.namespace_id.clone())
    } else {
        (namespace.to_string(), String::new())
    }
}

/// `SignalExternalWorkflowExecution`.
pub(crate) fn signal_external(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    a: SignalExternalWorkflowExecutionCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
) -> Result<(), Rejection> {
    let execution = a
        .execution
        .clone()
        .filter(|e| !e.workflow_id.is_empty())
        .ok_or_else(|| invalid("Required field Execution.WorkflowId is not set."))?;
    if a.signal_name.is_empty() {
        return Err(invalid("Required field SignalName is not set."));
    }
    let (namespace, namespace_id) = target_namespace(b, &a.namespace);
    let id = b.push_with_metadata(
        md,
        Attributes::SignalExternalWorkflowExecutionInitiatedEventAttributes(
            SignalExternalWorkflowExecutionInitiatedEventAttributes {
                workflow_task_completed_event_id: completed_id,
                namespace: namespace.clone(),
                namespace_id: namespace_id.clone(),
                workflow_execution: Some(execution.clone()),
                signal_name: a.signal_name,
                input: a.input,
                control: a.control,
                child_workflow_only: a.child_workflow_only,
                header: Some(a.header.unwrap_or_default()),
            },
        ),
    );
    b.state().external_signals.push(ExternalInfo {
        initiated_event_id: id,
        namespace,
        namespace_id,
        workflow_id: execution.workflow_id,
        run_id: execution.run_id,
    });
    fx.push(Effect::SignalExternal {
        initiated_event_id: id,
    });
    Ok(())
}

/// `RequestCancelExternalWorkflowExecution`.
pub(crate) fn cancel_external(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    a: RequestCancelExternalWorkflowExecutionCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
) -> Result<(), Rejection> {
    if a.workflow_id.is_empty() {
        return Err(invalid("Required field WorkflowId is not set."));
    }
    let (namespace, namespace_id) = target_namespace(b, &a.namespace);
    let execution = WorkflowExecution {
        workflow_id: a.workflow_id,
        run_id: a.run_id,
    };
    let id = b.push_with_metadata(
        md,
        Attributes::RequestCancelExternalWorkflowExecutionInitiatedEventAttributes(
            RequestCancelExternalWorkflowExecutionInitiatedEventAttributes {
                workflow_task_completed_event_id: completed_id,
                namespace: namespace.clone(),
                namespace_id: namespace_id.clone(),
                workflow_execution: Some(execution.clone()),
                control: a.control,
                child_workflow_only: a.child_workflow_only,
                reason: a.reason,
            },
        ),
    );
    b.state().external_cancels.push(ExternalInfo {
        initiated_event_id: id,
        namespace,
        namespace_id,
        workflow_id: execution.workflow_id,
        run_id: execution.run_id,
    });
    fx.push(Effect::CancelExternal {
        initiated_event_id: id,
    });
    Ok(())
}

/// `UpsertWorkflowSearchAttributes`.
pub(crate) fn upsert_search_attributes(
    b: &mut Batch<'_>,
    a: UpsertWorkflowSearchAttributesCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
) {
    loams_temporal_history::upsert_search_attributes(b.state(), a.search_attributes.as_ref());
    b.push_with_metadata(
        md,
        Attributes::UpsertWorkflowSearchAttributesEventAttributes(
            UpsertWorkflowSearchAttributesEventAttributes {
                workflow_task_completed_event_id: completed_id,
                search_attributes: a.search_attributes,
            },
        ),
    );
}

/// `ModifyWorkflowProperties` (the memo).
pub(crate) fn modify_properties(
    b: &mut Batch<'_>,
    a: ModifyWorkflowPropertiesCommandAttributes,
    md: Option<UserMetadata>,
    completed_id: i64,
) {
    loams_temporal_history::upsert_memo(b.state(), a.upserted_memo.as_ref());
    b.push_with_metadata(
        md,
        Attributes::WorkflowPropertiesModifiedEventAttributes(
            WorkflowPropertiesModifiedEventAttributes {
                workflow_task_completed_event_id: completed_id,
                upserted_memo: a.upserted_memo,
            },
        ),
    );
}

/// The child started (or the start failed: `failed_cause` is a
/// `StartChildWorkflowExecutionFailedCause`).
pub(crate) fn child_started(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    initiated_event_id: i64,
    run_id: String,
    header: Option<Header>,
    failed_cause: Option<i32>,
) -> Result<Reply, Rejection> {
    let Some(c) = loams_temporal_history::child(b.state_ref(), initiated_event_id).cloned() else {
        return Ok(Reply::Stale);
    };
    if c.started_event_id != 0 {
        return Ok(Reply::Stale);
    }
    let now = b.now_ms();
    if let Some(cause) = failed_cause {
        loams_temporal_history::take_child(b.state(), initiated_event_id);
        arrive(
            b,
            fx,
            vec![(
                now,
                Attributes::StartChildWorkflowExecutionFailedEventAttributes(
                    StartChildWorkflowExecutionFailedEventAttributes {
                        namespace: c.namespace.clone(),
                        namespace_id: c.namespace_id.clone(),
                        workflow_id: c.workflow_id.clone(),
                        workflow_type: wf_type(&c),
                        cause,
                        initiated_event_id,
                        ..Default::default()
                    },
                ),
            )],
        );
        return Ok(Reply::Done);
    }
    // The started event's ID is known only once it is written (it may wait
    // behind a started workflow task); mark the child started now with its
    // run, and record the event ID when the event is written.
    {
        let child = b
            .state()
            .children
            .iter_mut()
            .find(|x| x.initiated_event_id == initiated_event_id)
            .expect("found above");
        child.run_id = run_id.clone();
        child.started_event_id = -1;
    }
    arrive(
        b,
        fx,
        vec![(
            now,
            Attributes::ChildWorkflowExecutionStartedEventAttributes(
                ChildWorkflowExecutionStartedEventAttributes {
                    namespace: c.namespace.clone(),
                    namespace_id: c.namespace_id.clone(),
                    initiated_event_id,
                    workflow_execution: Some(WorkflowExecution {
                        workflow_id: c.workflow_id.clone(),
                        run_id,
                    }),
                    workflow_type: wf_type(&c),
                    header: Some(header.unwrap_or_default()),
                },
            ),
        )],
    );
    Ok(Reply::Done)
}

/// The child's run closed.
pub(crate) fn child_closed(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    initiated_event_id: i64,
    run_id: String,
    outcome: ChildOutcome,
) -> Result<Reply, Rejection> {
    let Some(mut c) = loams_temporal_history::child(b.state_ref(), initiated_event_id).cloned()
    else {
        return Ok(Reply::Stale);
    };
    if c.started_event_id == 0 {
        // The close overtook the started answer: record the start first.
        child_started(b, fx, initiated_event_id, run_id, None, None)?;
        c = loams_temporal_history::child(b.state_ref(), initiated_event_id)
            .cloned()
            .expect("still pending");
    }
    loams_temporal_history::take_child(b.state(), initiated_event_id);
    let ns = c.namespace.clone();
    let ns_id = c.namespace_id.clone();
    let ex = Some(execution(&c));
    let ty = wf_type(&c);
    // The started event ID is filled in when the events are written
    // (`write::link_child_started`); 0 here.
    let started_event_id = c.started_event_id.max(0);
    let event = match outcome {
        ChildOutcome::Completed(result) => {
            Attributes::ChildWorkflowExecutionCompletedEventAttributes(
                ChildWorkflowExecutionCompletedEventAttributes {
                    result,
                    namespace: ns,
                    namespace_id: ns_id,
                    workflow_execution: ex,
                    workflow_type: ty,
                    initiated_event_id,
                    started_event_id,
                },
            )
        }
        ChildOutcome::Failed(failure, retry_state) => {
            Attributes::ChildWorkflowExecutionFailedEventAttributes(
                ChildWorkflowExecutionFailedEventAttributes {
                    failure,
                    namespace: ns,
                    namespace_id: ns_id,
                    workflow_execution: ex,
                    workflow_type: ty,
                    initiated_event_id,
                    started_event_id,
                    retry_state: retry_state as i32,
                },
            )
        }
        ChildOutcome::Canceled(details) => {
            Attributes::ChildWorkflowExecutionCanceledEventAttributes(
                ChildWorkflowExecutionCanceledEventAttributes {
                    details,
                    namespace: ns,
                    namespace_id: ns_id,
                    workflow_execution: ex,
                    workflow_type: ty,
                    initiated_event_id,
                    started_event_id,
                },
            )
        }
        ChildOutcome::TimedOut(retry_state) => {
            Attributes::ChildWorkflowExecutionTimedOutEventAttributes(
                ChildWorkflowExecutionTimedOutEventAttributes {
                    namespace: ns,
                    namespace_id: ns_id,
                    workflow_execution: ex,
                    workflow_type: ty,
                    initiated_event_id,
                    started_event_id,
                    retry_state: retry_state as i32,
                },
            )
        }
        ChildOutcome::Terminated => Attributes::ChildWorkflowExecutionTerminatedEventAttributes(
            ChildWorkflowExecutionTerminatedEventAttributes {
                namespace: ns,
                namespace_id: ns_id,
                workflow_execution: ex,
                workflow_type: ty,
                initiated_event_id,
                started_event_id,
            },
        ),
    };
    let now = b.now_ms();
    arrive(b, fx, vec![(now, event)]);
    Ok(Reply::Done)
}

/// The other workflow was signalled or asked to cancel (`failed_cause`
/// `None`), or could not be (a `SignalExternalWorkflowExecutionFailedCause`
/// or `CancelExternalWorkflowExecutionFailedCause`).
pub(crate) fn external_result(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    kind: ExternalKind,
    initiated_event_id: i64,
    failed_cause: Option<i32>,
) -> Result<Reply, Rejection> {
    let list = match kind {
        ExternalKind::Signal => &mut b.state().external_signals,
        ExternalKind::Cancel => &mut b.state().external_cancels,
    };
    let Some(x) = loams_temporal_history::take_external(list, initiated_event_id) else {
        return Ok(Reply::Stale);
    };
    let ex = Some(WorkflowExecution {
        workflow_id: x.workflow_id.clone(),
        run_id: x.run_id.clone(),
    });
    let event = match (kind, failed_cause) {
        (ExternalKind::Signal, None) => {
            Attributes::ExternalWorkflowExecutionSignaledEventAttributes(
                ExternalWorkflowExecutionSignaledEventAttributes {
                    initiated_event_id,
                    namespace: x.namespace,
                    namespace_id: x.namespace_id,
                    workflow_execution: ex,
                    ..Default::default()
                },
            )
        }
        (ExternalKind::Signal, Some(cause)) => {
            Attributes::SignalExternalWorkflowExecutionFailedEventAttributes(
                SignalExternalWorkflowExecutionFailedEventAttributes {
                    cause,
                    namespace: x.namespace,
                    namespace_id: x.namespace_id,
                    workflow_execution: ex,
                    initiated_event_id,
                    ..Default::default()
                },
            )
        }
        (ExternalKind::Cancel, None) => {
            Attributes::ExternalWorkflowExecutionCancelRequestedEventAttributes(
                ExternalWorkflowExecutionCancelRequestedEventAttributes {
                    initiated_event_id,
                    namespace: x.namespace,
                    namespace_id: x.namespace_id,
                    workflow_execution: ex,
                },
            )
        }
        (ExternalKind::Cancel, Some(cause)) => {
            Attributes::RequestCancelExternalWorkflowExecutionFailedEventAttributes(
                RequestCancelExternalWorkflowExecutionFailedEventAttributes {
                    cause,
                    namespace: x.namespace,
                    namespace_id: x.namespace_id,
                    workflow_execution: ex,
                    initiated_event_id,
                    ..Default::default()
                },
            )
        }
    };
    let now = b.now_ms();
    arrive(b, fx, vec![(now, event)]);
    Ok(Reply::Done)
}

/// The children the parent close policy acts on, as the run closes.
pub(crate) fn close_policy_children(b: &Batch<'_>) -> Vec<ChildRef> {
    b.state_ref()
        .children
        .iter()
        .filter(|c| c.parent_close_policy != ParentClosePolicy::Abandon as i32)
        .map(|c| ChildRef {
            namespace: c.namespace.clone(),
            namespace_id: c.namespace_id.clone(),
            workflow_id: c.workflow_id.clone(),
            run_id: c.run_id.clone(),
            policy: c.parent_close_policy,
        })
        .collect()
}
