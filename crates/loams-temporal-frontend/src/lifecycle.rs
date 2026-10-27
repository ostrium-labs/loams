//! Starting, signal-with-start, reset and query consistency (TMP-api group
//! B, D2083–D2085).
//!
//! - Starts follow the request's workflow ID reuse and conflict policies.
//! - `SignalWithStartWorkflowExecution` signals the running run, or starts
//!   one with the signal between its started event and its first workflow
//!   task (as Temporal writes it).
//! - `ResetWorkflowExecution` builds the new run in the statemachine
//!   ([`loams_temporal_statemachine::reset`]), terminates the current run if
//!   it is running, and creates the new run as the current one.
//! - A query waits for an outstanding workflow task to finish before it goes
//!   to a worker, so its answer includes every event the run had when the
//!   query arrived (Temporal's consistent query).

use std::time::Duration;

use loams_temporal_history::WorkflowExecutionStatus;
use loams_temporal_proto::temporal::api::enums::v1::{
    QueryRejectCondition, ResetReapplyExcludeType, ResetReapplyType, WorkflowIdConflictPolicy,
    WorkflowIdReusePolicy,
};
use loams_temporal_proto::temporal::api::query::v1::QueryRejected;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    ResetWorkflowExecutionRequest, ResetWorkflowExecutionResponse,
    SignalWithStartWorkflowExecutionRequest, SignalWithStartWorkflowExecutionResponse,
    StartWorkflowExecutionRequest, StartWorkflowExecutionResponse,
};
use loams_temporal_statemachine::reset::{ResetRequest, reset};
use loams_temporal_statemachine::{HISTORY_SERVICE, Input, StartSignal};
use loams_temporal_store::{CurrentRun, NamespaceRecord, RunKey, STATUS_RUNNING, StartOutcome};
use tonic::Status;

use crate::auth::Principal;
use crate::engine::{rejection_status, store_status};
use crate::service::Frontend;

/// How many times a start or signal-with-start retries after losing a race
/// with another start or a close.
const RACE_RETRIES: usize = 3;

/// What the ID policies say about a start when the workflow ID has a
/// current run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyDecision {
    /// Start a new run (the store still refuses a running one).
    Start,
    /// Answer the running run (`USE_EXISTING`).
    UseExisting(String),
    /// Terminate the running run, then start.
    TerminateThenStart(String),
    /// Refuse: a closed run the reuse policy does not allow to follow.
    Refuse(String),
}

/// The decision for a start with these policies over `current`.
pub(crate) fn id_policy(
    workflow_id: &str,
    request_id: &str,
    reuse: i32,
    conflict: i32,
    current: Option<&CurrentRun>,
) -> PolicyDecision {
    let Some(cur) = current else {
        return PolicyDecision::Start;
    };
    // A retried start (same request ID) answers what the first one did.
    if !request_id.is_empty() && cur.request_id == request_id {
        return PolicyDecision::Start;
    }
    if cur.status == STATUS_RUNNING {
        let conflict = WorkflowIdConflictPolicy::try_from(conflict).unwrap_or_default();
        return match conflict {
            WorkflowIdConflictPolicy::UseExisting => {
                PolicyDecision::UseExisting(cur.run_id.clone())
            }
            WorkflowIdConflictPolicy::TerminateExisting => {
                PolicyDecision::TerminateThenStart(cur.run_id.clone())
            }
            // The deprecated reuse policy that terminates a running run.
            _ if reuse == 4 => PolicyDecision::TerminateThenStart(cur.run_id.clone()),
            _ => PolicyDecision::Start,
        };
    }
    match WorkflowIdReusePolicy::try_from(reuse).unwrap_or_default() {
        WorkflowIdReusePolicy::AllowDuplicateFailedOnly
            if cur.status == WorkflowExecutionStatus::Completed as i32 =>
        {
            PolicyDecision::Refuse(format!(
                "Workflow execution already finished successfully. WorkflowId: {workflow_id}, RunId: {}. Workflow Id reuse policy: allow duplicate workflow Id if last run failed.",
                cur.run_id
            ))
        }
        WorkflowIdReusePolicy::RejectDuplicate => PolicyDecision::Refuse(format!(
            "Workflow execution already finished. WorkflowId: {workflow_id}, RunId: {}. Workflow Id reuse policy: reject duplicate workflow Id.",
            cur.run_id
        )),
        _ => PolicyDecision::Start,
    }
}

/// Whether a reset writes the base run's later signals again.
pub(crate) fn reapplies_signals(r: &ResetWorkflowExecutionRequest) -> bool {
    #[allow(deprecated)]
    let none = r.reset_reapply_type == ResetReapplyType::None as i32;
    !none
        && !r
            .reset_reapply_exclude_types
            .contains(&(ResetReapplyExcludeType::Signal as i32))
}

/// The status a query's reject condition refuses, if it does.
pub(crate) fn query_rejected(condition: i32, status: i32) -> Option<QueryRejected> {
    let running = status == WorkflowExecutionStatus::Running as i32;
    let rejected = match QueryRejectCondition::try_from(condition).unwrap_or_default() {
        QueryRejectCondition::NotOpen => !running,
        QueryRejectCondition::NotCompletedCleanly => {
            !running && status != WorkflowExecutionStatus::Completed as i32
        }
        _ => false,
    };
    rejected.then_some(QueryRejected { status })
}

fn start_part(r: &SignalWithStartWorkflowExecutionRequest) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: r.namespace.clone(),
        workflow_id: r.workflow_id.clone(),
        workflow_type: r.workflow_type.clone(),
        task_queue: r.task_queue.clone(),
        input: r.input.clone(),
        workflow_execution_timeout: r.workflow_execution_timeout,
        workflow_run_timeout: r.workflow_run_timeout,
        workflow_task_timeout: r.workflow_task_timeout,
        identity: r.identity.clone(),
        request_id: r.request_id.clone(),
        workflow_id_reuse_policy: r.workflow_id_reuse_policy,
        workflow_id_conflict_policy: r.workflow_id_conflict_policy,
        retry_policy: r.retry_policy.clone(),
        cron_schedule: r.cron_schedule.clone(),
        memo: r.memo.clone(),
        search_attributes: r.search_attributes.clone(),
        header: r.header.clone(),
        workflow_start_delay: r.workflow_start_delay,
        user_metadata: r.user_metadata.clone(),
        links: r.links.clone(),
        priority: r.priority.clone(),
        ..Default::default()
    }
}

impl Frontend {
    async fn current(
        &self,
        ns: &NamespaceRecord,
        workflow_id: &str,
    ) -> Result<Option<CurrentRun>, Status> {
        self.store()
            .current_run(&ns.namespace_id, workflow_id)
            .await
            .map_err(store_status)
    }

    /// Terminates a running run for a policy or a reset; a run that closed
    /// meanwhile is fine.
    async fn terminate_for(
        &self,
        ns: &NamespaceRecord,
        workflow_id: &str,
        run_id: &str,
        reason: String,
        identity: String,
    ) -> Result<(), Status> {
        let key = RunKey::new(&ns.namespace_id, workflow_id, run_id);
        match self
            .engine
            .execute(
                &key,
                &Self::ctx(ns),
                Input::Terminate {
                    reason,
                    details: None,
                    identity,
                },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(s) if s.code() == tonic::Code::NotFound => Ok(()),
            Err(s) => Err(s),
        }
    }

    /// `StartWorkflowExecution` with the ID reuse and conflict policies.
    pub(crate) async fn start_workflow(
        &self,
        p: &Principal,
        mut r: StartWorkflowExecutionRequest,
    ) -> Result<StartWorkflowExecutionResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        if r.request_id.is_empty() {
            r.request_id = uuid::Uuid::new_v4().to_string();
        }
        let current = self.current(&ns, &r.workflow_id).await?;
        match id_policy(
            &r.workflow_id,
            &r.request_id,
            r.workflow_id_reuse_policy,
            r.workflow_id_conflict_policy,
            current.as_ref(),
        ) {
            PolicyDecision::Start => {}
            PolicyDecision::UseExisting(run_id) => {
                return Ok(StartWorkflowExecutionResponse {
                    run_id,
                    started: false,
                    status: WorkflowExecutionStatus::Running as i32,
                    ..Default::default()
                });
            }
            PolicyDecision::TerminateThenStart(run_id) => {
                self.terminate_for(
                    &ns,
                    &r.workflow_id,
                    &run_id,
                    "terminated by a new start (workflow ID conflict policy)".into(),
                    r.identity.clone(),
                )
                .await?;
            }
            PolicyDecision::Refuse(message) => {
                let cur = current.expect("a refusal names a current run");
                return Err(crate::errors::already_started_message(
                    message,
                    &cur.run_id,
                    &cur.request_id,
                ));
            }
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        let (run_id, started) = self.engine.start(&Self::ctx(&ns), run_id, r).await?;
        Ok(StartWorkflowExecutionResponse {
            run_id,
            started,
            status: WorkflowExecutionStatus::Running as i32,
            ..Default::default()
        })
    }

    /// `SignalWithStartWorkflowExecution`.
    pub(crate) async fn signal_with_start(
        &self,
        p: &Principal,
        mut r: SignalWithStartWorkflowExecutionRequest,
    ) -> Result<SignalWithStartWorkflowExecutionResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        if r.workflow_id.is_empty() {
            return Err(Status::invalid_argument(
                "WorkflowId is not set on request.",
            ));
        }
        if r.signal_name.is_empty() {
            return Err(Status::invalid_argument(
                "SignalName is not set on request.",
            ));
        }
        if r.request_id.is_empty() {
            r.request_id = uuid::Uuid::new_v4().to_string();
        }
        let signal = StartSignal {
            name: r.signal_name.clone(),
            input: r.signal_input.clone(),
            identity: r.identity.clone(),
            header: r.header.clone(),
            request_id: r.request_id.clone(),
        };
        let terminate_existing =
            r.workflow_id_conflict_policy == WorkflowIdConflictPolicy::TerminateExisting as i32;
        for _ in 0..RACE_RETRIES {
            let current = self.current(&ns, &r.workflow_id).await?;
            if let Some(cur) = current.as_ref().filter(|c| c.status == STATUS_RUNNING) {
                if terminate_existing {
                    self.terminate_for(
                        &ns,
                        &r.workflow_id,
                        &cur.run_id,
                        "terminated by signal-with-start (workflow ID conflict policy)".into(),
                        r.identity.clone(),
                    )
                    .await?;
                } else {
                    let key = RunKey::new(&ns.namespace_id, &r.workflow_id, &cur.run_id);
                    match self
                        .engine
                        .execute(
                            &key,
                            &Self::ctx(&ns),
                            Input::Signal {
                                name: signal.name.clone(),
                                input: signal.input.clone(),
                                identity: signal.identity.clone(),
                                header: signal.header.clone(),
                                request_id: signal.request_id.clone(),
                            },
                        )
                        .await
                    {
                        Ok(_) => {
                            return Ok(SignalWithStartWorkflowExecutionResponse {
                                run_id: cur.run_id.clone(),
                                started: false,
                                ..Default::default()
                            });
                        }
                        // It closed meanwhile: start a new one.
                        Err(s) if s.code() == tonic::Code::NotFound => {}
                        Err(s) => return Err(s),
                    }
                }
            } else if let PolicyDecision::Refuse(message) = id_policy(
                &r.workflow_id,
                "",
                r.workflow_id_reuse_policy,
                r.workflow_id_conflict_policy,
                current.as_ref(),
            ) {
                let cur = current.expect("a refusal names a current run");
                return Err(crate::errors::already_started_message(
                    message,
                    &cur.run_id,
                    &cur.request_id,
                ));
            }
            let run_id = uuid::Uuid::new_v4().to_string();
            let input = Input::SignalWithStart {
                namespace_id: ns.namespace_id.clone(),
                run_id: run_id.clone(),
                request: Box::new(start_part(&r)),
                signal: signal.clone(),
            };
            match self
                .engine
                .start_with(
                    &Self::ctx(&ns),
                    &r.workflow_id,
                    run_id,
                    r.request_id.clone(),
                    input,
                )
                .await
            {
                Ok((run_id, started)) => {
                    return Ok(SignalWithStartWorkflowExecutionResponse {
                        first_execution_run_id: if started {
                            run_id.clone()
                        } else {
                            String::new()
                        },
                        run_id,
                        started,
                        ..Default::default()
                    });
                }
                // Another start won the race: signal its run.
                Err(s) if s.code() == tonic::Code::AlreadyExists => {}
                Err(s) => return Err(s),
            }
        }
        Err(Status::unavailable(
            "the workflow kept starting and closing; retry signal-with-start",
        ))
    }

    /// `ResetWorkflowExecution`.
    pub(crate) async fn reset_workflow(
        &self,
        p: &Principal,
        r: ResetWorkflowExecutionRequest,
    ) -> Result<ResetWorkflowExecutionResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let base = self.run_key(&ns, r.workflow_execution.as_ref()).await?;
        if r.workflow_task_finish_event_id <= 0 {
            return Err(Status::invalid_argument(
                "WorkflowTaskFinishEventId is not set on request.",
            ));
        }
        let events = self.engine.history(&base, 1, usize::MAX).await?;
        let new_run_id = uuid::Uuid::new_v4().to_string();
        let request_id = if r.request_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            r.request_id.clone()
        };
        let run = reset(
            &events,
            &ResetRequest {
                namespace_id: ns.namespace_id.clone(),
                base_run_id: base.run_id.clone(),
                new_run_id: new_run_id.clone(),
                workflow_task_finish_event_id: r.workflow_task_finish_event_id,
                reason: r.reason.clone(),
                reapply_signals: reapplies_signals(&r),
            },
            self.engine.now_ms(),
        )
        .map_err(rejection_status)?;
        if let Some(cur) = self
            .current(&ns, &base.workflow_id)
            .await?
            .filter(|c| c.status == STATUS_RUNNING)
        {
            if cur.request_id == request_id {
                // A retried reset whose new run is already current.
                return Ok(ResetWorkflowExecutionResponse { run_id: cur.run_id });
            }
            let identity = if r.identity.is_empty() {
                HISTORY_SERVICE.to_string()
            } else {
                r.identity.clone()
            };
            self.terminate_for(
                &ns,
                &base.workflow_id,
                &cur.run_id,
                r.reason.clone(),
                identity,
            )
            .await?;
        }
        let key = RunKey::new(&ns.namespace_id, &base.workflow_id, &new_run_id);
        match self
            .engine
            .create_run(
                &Self::ctx(&ns),
                &key,
                &request_id,
                &run.state,
                &run.events,
                &run.effects,
            )
            .await?
        {
            StartOutcome::Started { .. } => {
                Ok(ResetWorkflowExecutionResponse { run_id: new_run_id })
            }
            StartOutcome::AlreadyStarted {
                run_id,
                request_id: existing,
                ..
            } => {
                if existing == request_id {
                    Ok(ResetWorkflowExecutionResponse { run_id })
                } else {
                    Err(crate::errors::already_started(
                        &base.workflow_id,
                        &run_id,
                        &existing,
                    ))
                }
            }
        }
    }

    /// Waits (up to `wait`) until the run has no outstanding workflow task,
    /// so a query sees every event the run had when it arrived.
    pub(crate) async fn settle_for_query(
        &self,
        key: &RunKey,
        wait: Duration,
    ) -> Result<(), Status> {
        let mut rx = self.engine.watch(key).await?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let _seen = *rx.borrow_and_update();
            let s = self.engine.load(key).await?.state;
            if s.workflow_task.is_none() || !loams_temporal_history::is_open(&s) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                // Answer from what there is rather than not at all.
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur(status: i32, request_id: &str) -> CurrentRun {
        CurrentRun {
            run_id: "r0".into(),
            status,
            request_id: request_id.into(),
        }
    }

    #[test]
    fn reuse_policies_follow_temporal() {
        let completed = cur(WorkflowExecutionStatus::Completed as i32, "a");
        let failed = cur(WorkflowExecutionStatus::Failed as i32, "a");
        let failed_only = WorkflowIdReusePolicy::AllowDuplicateFailedOnly as i32;
        assert!(matches!(
            id_policy("w", "b", failed_only, 0, Some(&completed)),
            PolicyDecision::Refuse(_)
        ));
        assert_eq!(
            id_policy("w", "b", failed_only, 0, Some(&failed)),
            PolicyDecision::Start
        );
        assert!(matches!(
            id_policy(
                "w",
                "b",
                WorkflowIdReusePolicy::RejectDuplicate as i32,
                0,
                Some(&failed)
            ),
            PolicyDecision::Refuse(_)
        ));
        assert_eq!(
            id_policy("w", "b", 0, 0, Some(&completed)),
            PolicyDecision::Start
        );
        assert_eq!(id_policy("w", "b", 0, 0, None), PolicyDecision::Start);
        // A retry of the same request is never refused.
        assert_eq!(
            id_policy("w", "a", failed_only, 0, Some(&completed)),
            PolicyDecision::Start
        );
    }

    #[test]
    fn conflict_policies_follow_temporal() {
        let running = cur(STATUS_RUNNING, "a");
        assert_eq!(
            id_policy(
                "w",
                "b",
                0,
                WorkflowIdConflictPolicy::UseExisting as i32,
                Some(&running)
            ),
            PolicyDecision::UseExisting("r0".into())
        );
        assert_eq!(
            id_policy(
                "w",
                "b",
                0,
                WorkflowIdConflictPolicy::TerminateExisting as i32,
                Some(&running)
            ),
            PolicyDecision::TerminateThenStart("r0".into())
        );
        assert_eq!(
            id_policy("w", "b", 4, 0, Some(&running)),
            PolicyDecision::TerminateThenStart("r0".into())
        );
        assert_eq!(
            id_policy("w", "b", 0, 0, Some(&running)),
            PolicyDecision::Start
        );
    }

    #[test]
    fn query_reject_conditions() {
        let running = WorkflowExecutionStatus::Running as i32;
        let completed = WorkflowExecutionStatus::Completed as i32;
        let failed = WorkflowExecutionStatus::Failed as i32;
        let not_open = QueryRejectCondition::NotOpen as i32;
        let not_clean = QueryRejectCondition::NotCompletedCleanly as i32;
        assert_eq!(query_rejected(not_open, running), None);
        assert_eq!(
            query_rejected(not_open, completed),
            Some(QueryRejected { status: completed })
        );
        assert_eq!(query_rejected(not_clean, completed), None);
        assert_eq!(
            query_rejected(not_clean, failed),
            Some(QueryRejected { status: failed })
        );
        assert_eq!(query_rejected(0, failed), None);
    }

    #[test]
    fn signals_are_reapplied_unless_excluded() {
        let mut r = ResetWorkflowExecutionRequest::default();
        assert!(reapplies_signals(&r));
        r.reset_reapply_exclude_types = vec![ResetReapplyExcludeType::Signal as i32];
        assert!(!reapplies_signals(&r));
        let mut r = ResetWorkflowExecutionRequest::default();
        #[allow(deprecated)]
        {
            r.reset_reapply_type = ResetReapplyType::None as i32;
        }
        assert!(!reapplies_signals(&r));
    }
}
