//! What a committed transition asks the frontend to do (TMP-api group D,
//! D2087): start a child or the next run of a continue-as-new, signal or
//! cancel another workflow, apply the parent close policy, and tell a
//! parent its child closed. Each answer re-enters through the engine as an
//! input.
//!
//! Preview limit, stated plainly: these run after the commit, in this
//! process. If the gateway stops between the commit and the follow-up, the
//! follow-up is lost until a reconciler re-derives it from mutable state
//! (pending children without a started event, a continued run without its
//! next run). Every step uses a deterministic request ID, so re-running one
//! is safe.

use std::sync::Arc;

use loams_temporal_history::time::dur_ms;
use loams_temporal_history::{Attributes, HistoryEvent, ParentInfo, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::enums::v1::{
    CancelExternalWorkflowExecutionFailedCause, ParentClosePolicy, RetryState,
    SignalExternalWorkflowExecutionFailedCause, StartChildWorkflowExecutionFailedCause,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::children::{ChildOutcome, ChildRef, ExternalKind};
use loams_temporal_statemachine::{Effect, HISTORY_SERVICE, Input, StartChain};
use loams_temporal_store::{NamespaceRecord, RunKey, STATUS_RUNNING};
use tonic::Status;

use crate::engine::{FollowUp, store_status};
use crate::lifecycle::{PolicyDecision, id_policy};
use crate::service::Frontend;

fn event(f: &FollowUp, id: i64) -> Option<&HistoryEvent> {
    f.transition.events.iter().find(|e| e.event_id == id)
}

/// How a closed run's last event reads to its parent; `None` for a run that
/// continued as new (its parent hears from the last run).
pub(crate) fn child_outcome(close: &Attributes) -> Option<ChildOutcome> {
    Some(match close {
        Attributes::WorkflowExecutionCompletedEventAttributes(a) => {
            ChildOutcome::Completed(a.result.clone())
        }
        Attributes::WorkflowExecutionFailedEventAttributes(a) => ChildOutcome::Failed(
            a.failure.clone(),
            RetryState::try_from(a.retry_state).unwrap_or_default(),
        ),
        Attributes::WorkflowExecutionCanceledEventAttributes(a) => {
            ChildOutcome::Canceled(a.details.clone())
        }
        Attributes::WorkflowExecutionTimedOutEventAttributes(a) => {
            ChildOutcome::TimedOut(RetryState::try_from(a.retry_state).unwrap_or_default())
        }
        Attributes::WorkflowExecutionTerminatedEventAttributes(_) => ChildOutcome::Terminated,
        _ => return None,
    })
}

impl Frontend {
    /// Carries out one follow-up; failures are logged (the caller's commit
    /// already happened).
    pub async fn on_follow_up(self: &Arc<Self>, f: FollowUp) {
        for e in &f.transition.effects {
            let r = match e {
                Effect::StartChild { initiated_event_id } => {
                    self.start_child(&f, *initiated_event_id).await
                }
                Effect::ContinuedAsNew { new_run_id } => self.continue_run(&f, new_run_id).await,
                Effect::SignalExternal { initiated_event_id } => {
                    self.external(&f, ExternalKind::Signal, *initiated_event_id)
                        .await
                }
                Effect::CancelExternal { initiated_event_id } => {
                    self.external(&f, ExternalKind::Cancel, *initiated_event_id)
                        .await
                }
                Effect::ParentClosePolicy { children } => self.parent_close(&f, children).await,
                Effect::RunClosed { status } => self.tell_parent(&f, *status).await,
                _ => Ok(()),
            };
            if let Err(s) = r {
                tracing::warn!(
                    workflow = %f.key.workflow_id,
                    run = %f.key.run_id,
                    effect = ?e,
                    error = %s,
                    "follow-up not carried out"
                );
            }
        }
    }

    /// A namespace of the same Loams namespace, by ID or by name.
    async fn ns_for(
        &self,
        loams_namespace_id: &str,
        name: &str,
        id: &str,
    ) -> Result<NamespaceRecord, Status> {
        let ns = if id.is_empty() {
            self.store()
                .namespace_by_name(loams_namespace_id, name)
                .await
                .map_err(store_status)?
                .ok_or_else(|| crate::errors::namespace_not_found(name))?
        } else {
            self.namespace_by_id(id).await?
        };
        if ns.loams_namespace_id != loams_namespace_id {
            return Err(crate::errors::namespace_not_found(name));
        }
        Ok(ns)
    }

    /// Applies `input` to the run of the follow-up; a closed run is fine.
    async fn answer(&self, f: &FollowUp, input: Input) -> Result<(), Status> {
        match self.engine.execute(&f.key, &f.ctx, input).await {
            Ok(_) => Ok(()),
            Err(s) if s.code() == tonic::Code::NotFound => Ok(()),
            Err(s) => Err(s),
        }
    }

    async fn start_child(&self, f: &FollowUp, initiated_event_id: i64) -> Result<(), Status> {
        let Some(Attributes::StartChildWorkflowExecutionInitiatedEventAttributes(a)) =
            event(f, initiated_event_id).and_then(|e| e.attributes.clone())
        else {
            return Ok(());
        };
        let failed = |cause: StartChildWorkflowExecutionFailedCause| Input::ChildStartFailed {
            initiated_event_id,
            cause: cause as i32,
        };
        let target = match self
            .ns_for(&f.ctx.loams_namespace_id, &a.namespace, &a.namespace_id)
            .await
        {
            Ok(ns) => ns,
            Err(_) => {
                return self
                    .answer(
                        f,
                        failed(StartChildWorkflowExecutionFailedCause::NamespaceNotFound),
                    )
                    .await;
            }
        };
        let parent_ns = self.namespace_by_id(&f.ctx.namespace_id).await?;
        let request_id = format!("child:{}:{initiated_event_id}", f.key.run_id);
        let current = self
            .store()
            .current_run(&target.namespace_id, &a.workflow_id)
            .await
            .map_err(store_status)?;
        if let Some(cur) = &current
            && cur.request_id == request_id
        {
            // A repeated follow-up: the child already started.
            return self
                .answer(
                    f,
                    Input::ChildStarted {
                        initiated_event_id,
                        run_id: cur.run_id.clone(),
                        header: a.header.clone(),
                    },
                )
                .await;
        }
        let refused = matches!(
            id_policy(
                &a.workflow_id,
                &request_id,
                a.workflow_id_reuse_policy,
                0,
                current.as_ref()
            ),
            PolicyDecision::Refuse(_)
        ) || current.as_ref().is_some_and(|c| c.status == STATUS_RUNNING);
        if refused {
            return self
                .answer(
                    f,
                    failed(StartChildWorkflowExecutionFailedCause::WorkflowAlreadyExists),
                )
                .await;
        }
        let request = StartWorkflowExecutionRequest {
            namespace: target.name.clone(),
            workflow_id: a.workflow_id.clone(),
            workflow_type: a.workflow_type.clone(),
            task_queue: a.task_queue.clone(),
            input: a.input.clone(),
            workflow_execution_timeout: a.workflow_execution_timeout,
            workflow_run_timeout: a.workflow_run_timeout,
            workflow_task_timeout: a.workflow_task_timeout,
            request_id: request_id.clone(),
            workflow_id_reuse_policy: a.workflow_id_reuse_policy,
            retry_policy: a.retry_policy.clone(),
            memo: a.memo.clone(),
            search_attributes: a.search_attributes.clone(),
            header: a.header.clone(),
            priority: a.priority.clone(),
            ..Default::default()
        };
        let chain = StartChain {
            parent: Some(ParentInfo {
                namespace: parent_ns.name.clone(),
                namespace_id: parent_ns.namespace_id.clone(),
                workflow_id: f.key.workflow_id.clone(),
                run_id: f.key.run_id.clone(),
                initiated_event_id,
            }),
            ..Default::default()
        };
        let run_id = uuid::Uuid::new_v4().to_string();
        let input = Input::StartChained {
            namespace_id: target.namespace_id.clone(),
            run_id: run_id.clone(),
            request: Box::new(request),
            chain: Box::new(chain),
        };
        match self
            .engine
            .start_with(
                &Self::ctx(&target),
                &a.workflow_id,
                run_id,
                request_id,
                input,
            )
            .await
        {
            Ok((run_id, _)) => {
                self.answer(
                    f,
                    Input::ChildStarted {
                        initiated_event_id,
                        run_id,
                        header: a.header.clone(),
                    },
                )
                .await
            }
            Err(s) if s.code() == tonic::Code::AlreadyExists => {
                self.answer(
                    f,
                    failed(StartChildWorkflowExecutionFailedCause::WorkflowAlreadyExists),
                )
                .await
            }
            Err(s) => Err(s),
        }
    }

    async fn continue_run(&self, f: &FollowUp, new_run_id: &str) -> Result<(), Status> {
        let Some(a) = f
            .transition
            .events
            .iter()
            .find_map(|e| match &e.attributes {
                Some(Attributes::WorkflowExecutionContinuedAsNewEventAttributes(a)) => {
                    Some(a.clone())
                }
                _ => None,
            })
        else {
            return Ok(());
        };
        let state = &f.transition.state;
        let ns = self.namespace_by_id(&f.ctx.namespace_id).await?;
        let history = self.engine.history(&f.key, 1, usize::MAX).await?;
        let request = StartWorkflowExecutionRequest {
            namespace: ns.name.clone(),
            workflow_id: f.key.workflow_id.clone(),
            workflow_type: a.workflow_type.clone(),
            task_queue: a.task_queue.clone(),
            input: a.input.clone(),
            workflow_execution_timeout: state.workflow_execution_timeout,
            workflow_run_timeout: a.workflow_run_timeout,
            workflow_task_timeout: a.workflow_task_timeout,
            request_id: format!("can:{}", f.key.run_id),
            memo: a.memo.clone(),
            search_attributes: a.search_attributes.clone(),
            header: a.header.clone(),
            ..Default::default()
        };
        let chain = StartChain {
            parent: state.parent.clone(),
            continued_execution_run_id: f.key.run_id.clone(),
            initiator: a.initiator,
            first_execution_run_id: state.first_execution_run_id.clone(),
            backoff_ms: dur_ms(a.backoff_start_interval.as_ref()),
            last_completion_result: a.last_completion_result.clone(),
            continued_failure: a.failure.clone(),
            prev_auto_reset_points: Some(
                loams_temporal_proto::temporal::api::workflow::v1::ResetPoints {
                    points: loams_temporal_statemachine::reset::reset_points(
                        &f.key.run_id,
                        &history,
                    ),
                },
            ),
        };
        let request_id = request.request_id.clone();
        let input = Input::StartChained {
            namespace_id: f.ctx.namespace_id.clone(),
            run_id: new_run_id.to_string(),
            request: Box::new(request),
            chain: Box::new(chain),
        };
        self.engine
            .start_with(
                &f.ctx,
                &f.key.workflow_id,
                new_run_id.to_string(),
                request_id,
                input,
            )
            .await
            .map(|_| ())
    }

    async fn external(
        &self,
        f: &FollowUp,
        kind: ExternalKind,
        initiated_event_id: i64,
    ) -> Result<(), Status> {
        let (namespace, namespace_id, execution, target_input) =
            match event(f, initiated_event_id).and_then(|e| e.attributes.clone()) {
                Some(Attributes::SignalExternalWorkflowExecutionInitiatedEventAttributes(a)) => (
                    a.namespace,
                    a.namespace_id,
                    a.workflow_execution.unwrap_or_default(),
                    Input::Signal {
                        name: a.signal_name,
                        input: a.input,
                        identity: String::new(),
                        header: a.header,
                        request_id: format!("sig:{}:{initiated_event_id}", f.key.run_id),
                    },
                ),
                Some(
                    Attributes::RequestCancelExternalWorkflowExecutionInitiatedEventAttributes(a),
                ) => (
                    a.namespace,
                    a.namespace_id,
                    a.workflow_execution.unwrap_or_default(),
                    Input::CancelRequested {
                        reason: a.reason,
                        identity: String::new(),
                    },
                ),
                _ => return Ok(()),
            };
        let (not_found, ns_not_found) = match kind {
            ExternalKind::Signal => (
                SignalExternalWorkflowExecutionFailedCause::ExternalWorkflowExecutionNotFound
                    as i32,
                SignalExternalWorkflowExecutionFailedCause::NamespaceNotFound as i32,
            ),
            ExternalKind::Cancel => (
                CancelExternalWorkflowExecutionFailedCause::ExternalWorkflowExecutionNotFound
                    as i32,
                CancelExternalWorkflowExecutionFailedCause::NamespaceNotFound as i32,
            ),
        };
        let result = |failed_cause| Input::ExternalResult {
            kind,
            initiated_event_id,
            failed_cause,
        };
        let Ok(target) = self
            .ns_for(&f.ctx.loams_namespace_id, &namespace, &namespace_id)
            .await
        else {
            return self.answer(f, result(Some(ns_not_found))).await;
        };
        let run_id = if execution.run_id.is_empty() {
            match self
                .store()
                .current_run(&target.namespace_id, &execution.workflow_id)
                .await
                .map_err(store_status)?
            {
                Some(c) => c.run_id,
                None => return self.answer(f, result(Some(not_found))).await,
            }
        } else {
            execution.run_id.clone()
        };
        let key = RunKey::new(&target.namespace_id, &execution.workflow_id, run_id);
        match self
            .engine
            .execute(&key, &Self::ctx(&target), target_input)
            .await
        {
            Ok(_) => self.answer(f, result(None)).await,
            Err(s) if s.code() == tonic::Code::NotFound => {
                self.answer(f, result(Some(not_found))).await
            }
            Err(s) => Err(s),
        }
    }

    async fn parent_close(&self, f: &FollowUp, children: &[ChildRef]) -> Result<(), Status> {
        for c in children.iter().filter(|c| !c.run_id.is_empty()) {
            let input = if c.policy == ParentClosePolicy::RequestCancel as i32 {
                Input::CancelRequested {
                    reason: "by parent close policy".into(),
                    identity: HISTORY_SERVICE.into(),
                }
            } else {
                Input::Terminate {
                    reason: "by parent close policy".into(),
                    details: None,
                    identity: HISTORY_SERVICE.into(),
                }
            };
            let Ok(ns) = self
                .ns_for(&f.ctx.loams_namespace_id, &c.namespace, &c.namespace_id)
                .await
            else {
                continue;
            };
            let key = RunKey::new(&ns.namespace_id, &c.workflow_id, &c.run_id);
            match self.engine.execute(&key, &Self::ctx(&ns), input).await {
                Ok(_) => {}
                Err(s) if s.code() == tonic::Code::NotFound => {}
                Err(s) => return Err(s),
            }
        }
        Ok(())
    }

    async fn tell_parent(
        &self,
        f: &FollowUp,
        status: WorkflowExecutionStatus,
    ) -> Result<(), Status> {
        let state = &f.transition.state;
        let Some(parent) = state.parent.clone() else {
            return Ok(());
        };
        if status == WorkflowExecutionStatus::ContinuedAsNew {
            return Ok(());
        }
        let Some(outcome) = event(f, state.completion_event_id)
            .and_then(|e| e.attributes.as_ref())
            .and_then(child_outcome)
        else {
            return Ok(());
        };
        let ns = self.namespace_by_id(&parent.namespace_id).await?;
        let key = RunKey::new(&parent.namespace_id, &parent.workflow_id, &parent.run_id);
        match self
            .engine
            .execute(
                &key,
                &Self::ctx(&ns),
                Input::ChildClosed {
                    initiated_event_id: parent.initiated_event_id,
                    run_id: f.key.run_id.clone(),
                    outcome,
                },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(s) if s.code() == tonic::Code::NotFound => Ok(()),
            Err(s) => Err(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::history::v1::{
        WorkflowExecutionCompletedEventAttributes, WorkflowExecutionContinuedAsNewEventAttributes,
        WorkflowExecutionTerminatedEventAttributes,
    };

    #[test]
    fn close_events_map_to_child_outcomes() {
        assert_eq!(
            child_outcome(&Attributes::WorkflowExecutionCompletedEventAttributes(
                WorkflowExecutionCompletedEventAttributes::default()
            )),
            Some(ChildOutcome::Completed(None))
        );
        assert_eq!(
            child_outcome(&Attributes::WorkflowExecutionTerminatedEventAttributes(
                WorkflowExecutionTerminatedEventAttributes::default()
            )),
            Some(ChildOutcome::Terminated)
        );
        assert_eq!(
            child_outcome(&Attributes::WorkflowExecutionContinuedAsNewEventAttributes(
                WorkflowExecutionContinuedAsNewEventAttributes::default()
            )),
            None
        );
    }
}
