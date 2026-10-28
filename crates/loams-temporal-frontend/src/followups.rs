//! What a committed transition asks the frontend to do (TMP-api group D,
//! D2087): start a child or the next run of a continue-as-new, signal or
//! cancel another workflow, apply the parent close policy, and tell a
//! parent its child closed. Each answer re-enters through the engine as an
//! input.
//!
//! Crash safety (D2089): every follow-up is an outbox row (kind `fu`)
//! written in the same commit as the events that ask for it. The relay hands
//! claimed rows to [`Frontend::run_claimed_follow_up`], which marks a row
//! dispatched only after carrying it out. A gateway that stops in between
//! leaves the row claimed. When the lease runs out, the next gateway claims
//! it again and redoes it. Each step reads what it needs back from the
//! store and is idempotent: deterministic request IDs, signal request IDs
//! deduplicated by the target, and answers the parent already has are
//! `Stale`.

use std::sync::Arc;

use loams_temporal_history::time::dur_ms;
use loams_temporal_history::{Attributes, HistoryEvent, ParentInfo, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::enums::v1::{
    CancelExternalWorkflowExecutionFailedCause, ParentClosePolicy, RetryState,
    SignalExternalWorkflowExecutionFailedCause, StartChildWorkflowExecutionFailedCause,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::children::{ChildOutcome, ExternalKind};
use loams_temporal_statemachine::{HISTORY_SERVICE, Input, StartChain};
use loams_temporal_store::{NamespaceRecord, RunKey, STATUS_RUNNING};
use tonic::Status;

use crate::effects::{ChildTarget, EffectContext, FollowUpIntent, FollowUpRow};
use crate::engine::store_status;
use crate::lifecycle::{PolicyDecision, id_policy};
use crate::outbox::ClaimedFollowUp;
use crate::service::Frontend;

/// The run a follow-up row belongs to, read back from the store.
#[derive(Debug, Clone)]
pub(crate) struct Site {
    ctx: EffectContext,
    key: RunKey,
    state: loams_temporal_history::MutableState,
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
    /// Carries out a claimed follow-up row and marks it dispatched; on an
    /// error the row stays claimed and is carried out again when its lease
    /// runs out.
    pub async fn run_claimed_follow_up(self: &Arc<Self>, c: ClaimedFollowUp) {
        match self.run_follow_up(&c.row).await {
            Ok(()) => {
                if let Err(e) = self.store().mark_dispatched(&[c.id]).await {
                    tracing::warn!(id = c.id, error = %e, "follow-up done but not marked");
                }
            }
            Err(s) => tracing::warn!(
                id = c.id,
                attempts = c.attempts,
                row = ?c.row,
                error = %s,
                "follow-up not carried out; it is retried after its lease"
            ),
        }
    }

    /// Carries out one follow-up row. Safe to repeat.
    pub async fn run_follow_up(&self, row: &FollowUpRow) -> Result<(), Status> {
        let ctx = EffectContext {
            namespace_id: row.namespace_id.clone(),
            loams_namespace_id: row.loams_namespace_id.clone(),
        };
        let key = RunKey::new(&row.namespace_id, &row.workflow_id, &row.run_id);
        let state = match self.engine.load(&key).await {
            Ok(r) => r.state,
            // The run is gone (retention): nothing left to do.
            Err(s) if s.code() == tonic::Code::NotFound => return Ok(()),
            Err(s) => return Err(s),
        };
        let f = Site { ctx, key, state };
        match &row.intent {
            FollowUpIntent::StartChild { initiated_event_id } => {
                self.start_child(&f, *initiated_event_id).await
            }
            FollowUpIntent::ContinueAsNew { new_run_id } => self.continue_run(&f, new_run_id).await,
            FollowUpIntent::SignalExternal { initiated_event_id } => {
                self.external(&f, ExternalKind::Signal, *initiated_event_id)
                    .await
            }
            FollowUpIntent::CancelExternal { initiated_event_id } => {
                self.external(&f, ExternalKind::Cancel, *initiated_event_id)
                    .await
            }
            FollowUpIntent::ParentClose { children } => self.parent_close(&f, children).await,
            FollowUpIntent::NotifyParent => self.tell_parent(&f).await,
        }
    }

    /// One event of the follow-up's run.
    async fn event(&self, f: &Site, id: i64) -> Result<Option<HistoryEvent>, Status> {
        if id <= 0 {
            return Ok(None);
        }
        Ok(self.engine.history(&f.key, id, 1).await?.into_iter().next())
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
    async fn answer(&self, f: &Site, input: Input) -> Result<(), Status> {
        match self.engine.execute(&f.key, &f.ctx, input).await {
            Ok(_) => Ok(()),
            Err(s) if s.code() == tonic::Code::NotFound => Ok(()),
            Err(s) => Err(s),
        }
    }

    async fn start_child(&self, f: &Site, initiated_event_id: i64) -> Result<(), Status> {
        // Already answered (started, failed, or closed): nothing to redo.
        if loams_temporal_history::child(&f.state, initiated_event_id)
            .is_none_or(|c| c.started_event_id != 0)
        {
            return Ok(());
        }
        let Some(Attributes::StartChildWorkflowExecutionInitiatedEventAttributes(a)) = self
            .event(f, initiated_event_id)
            .await?
            .and_then(|e| e.attributes)
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

    async fn continue_run(&self, f: &Site, new_run_id: &str) -> Result<(), Status> {
        let Some(Attributes::WorkflowExecutionContinuedAsNewEventAttributes(a)) = self
            .event(f, f.state.completion_event_id)
            .await?
            .and_then(|e| e.attributes)
        else {
            return Ok(());
        };
        let state = &f.state;
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
            #[allow(deprecated)]
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
        f: &Site,
        kind: ExternalKind,
        initiated_event_id: i64,
    ) -> Result<(), Status> {
        let pending = match kind {
            ExternalKind::Signal => &f.state.external_signals,
            ExternalKind::Cancel => &f.state.external_cancels,
        };
        if !pending
            .iter()
            .any(|x| x.initiated_event_id == initiated_event_id)
        {
            return Ok(());
        }
        let (namespace, namespace_id, execution, target_input) = match self
            .event(f, initiated_event_id)
            .await?
            .and_then(|e| e.attributes)
        {
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
            Some(Attributes::RequestCancelExternalWorkflowExecutionInitiatedEventAttributes(a)) => {
                (
                    a.namespace,
                    a.namespace_id,
                    a.workflow_execution.unwrap_or_default(),
                    Input::CancelRequested {
                        reason: a.reason,
                        identity: String::new(),
                    },
                )
            }
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

    async fn parent_close(&self, f: &Site, children: &[ChildTarget]) -> Result<(), Status> {
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

    async fn tell_parent(&self, f: &Site) -> Result<(), Status> {
        let state = &f.state;
        let Some(parent) = state.parent.clone() else {
            return Ok(());
        };
        if state.status == WorkflowExecutionStatus::ContinuedAsNew as i32 {
            return Ok(());
        }
        let Some(outcome) = self
            .event(f, state.completion_event_id)
            .await?
            .and_then(|e| e.attributes)
            .as_ref()
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
