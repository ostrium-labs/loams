//! `UpdateWorkflowExecution` and `PollWorkflowExecutionUpdate` (TMP-api,
//! D2080–D2082): admit an update through the engine, then wait on the run's
//! progress until the update reaches the stage the caller asked for.
//!
//! A wait that reaches its deadline answers the stage reached so far
//! (`ADMITTED` or `ACCEPTED`), not an error: the SDKs then call again or
//! poll, as they do against Temporal's server.

use std::time::Duration;

use loams_temporal_history::Attributes;
use loams_temporal_proto::temporal::api::common::v1::WorkflowExecution;
use loams_temporal_proto::temporal::api::enums::v1::UpdateWorkflowExecutionLifecycleStage as Stage;
use loams_temporal_proto::temporal::api::update::v1::{Outcome, UpdateRef, WaitPolicy};
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    PollWorkflowExecutionUpdateRequest, PollWorkflowExecutionUpdateResponse,
    UpdateWorkflowExecutionRequest, UpdateWorkflowExecutionResponse,
};
use loams_temporal_statemachine::update::{UpdateStatus, status};
use loams_temporal_statemachine::{COMPLETED, Input};
use loams_temporal_store::RunKey;
use tonic::Status;

use crate::auth::Principal;
use crate::service::Frontend;

/// The stage `UpdateWorkflowExecution` waits for: the caller's, or
/// `ACCEPTED` when it names none (what the SDKs ask for by default).
pub(crate) fn update_wait_stage(p: Option<&WaitPolicy>) -> Stage {
    match p.map(WaitPolicy::lifecycle_stage) {
        None | Some(Stage::Unspecified) => Stage::Accepted,
        Some(s) => s,
    }
}

/// The stage `PollWorkflowExecutionUpdate` waits for: the caller's, or
/// none (answer at once) when it names none.
pub(crate) fn poll_wait_stage(p: Option<&WaitPolicy>) -> Stage {
    match p.map(WaitPolicy::lifecycle_stage) {
        None | Some(Stage::Unspecified) => Stage::Admitted,
        Some(s) => s,
    }
}

fn update_not_found(update_id: &str) -> Status {
    Status::not_found(format!("update {update_id} not found"))
}

impl Frontend {
    /// `UpdateWorkflowExecution`.
    pub(crate) async fn update_workflow(
        &self,
        p: &Principal,
        wait: Duration,
        r: UpdateWorkflowExecutionRequest,
    ) -> Result<UpdateWorkflowExecutionResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let key = self.run_key(&ns, r.workflow_execution.as_ref()).await?;
        let request = r
            .request
            .ok_or_else(|| Status::invalid_argument("Update request is not set on request."))?;
        let update_id = request
            .meta
            .as_ref()
            .map(|m| m.update_id.clone())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Status::invalid_argument("Update id is not set on request."))?;
        let run = self.engine.load(&key).await?;
        if !r.first_execution_run_id.is_empty()
            && run.state.first_execution_run_id != r.first_execution_run_id
        {
            return Err(Status::not_found(format!(
                "workflow execution not found for first execution run ID \"{}\"",
                r.first_execution_run_id
            )));
        }
        // A retried update ID answers what the first call started, even on
        // a run that has closed since.
        if status(&run.state, &update_id).is_none() {
            self.engine
                .execute(
                    &key,
                    &Self::ctx(&ns),
                    Input::UpdateAdmitted {
                        request: Box::new(request),
                    },
                )
                .await?;
        }
        let want = update_wait_stage(r.wait_policy.as_ref());
        let (stage, outcome) = self.wait_update(&key, &update_id, want, wait).await?;
        Ok(UpdateWorkflowExecutionResponse {
            update_ref: Some(UpdateRef {
                workflow_execution: Some(WorkflowExecution {
                    workflow_id: key.workflow_id.clone(),
                    run_id: key.run_id.clone(),
                }),
                update_id,
            }),
            outcome,
            stage: stage as i32,
            ..Default::default()
        })
    }

    /// `PollWorkflowExecutionUpdate`.
    pub(crate) async fn poll_update(
        &self,
        p: &Principal,
        wait: Duration,
        r: PollWorkflowExecutionUpdateRequest,
    ) -> Result<PollWorkflowExecutionUpdateResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let update_ref = r
            .update_ref
            .ok_or_else(|| Status::invalid_argument("UpdateRef is not set on request."))?;
        if update_ref.update_id.is_empty() {
            return Err(Status::invalid_argument("Update id is not set on request."));
        }
        let key = self
            .run_key(&ns, update_ref.workflow_execution.as_ref())
            .await?;
        let want = poll_wait_stage(r.wait_policy.as_ref());
        let (stage, outcome) = self
            .wait_update(&key, &update_ref.update_id, want, wait)
            .await?;
        Ok(PollWorkflowExecutionUpdateResponse {
            outcome,
            stage: stage as i32,
            update_ref: Some(UpdateRef {
                workflow_execution: Some(WorkflowExecution {
                    workflow_id: key.workflow_id.clone(),
                    run_id: key.run_id.clone(),
                }),
                update_id: update_ref.update_id,
            }),
        })
    }

    /// Waits until the update reaches `want` or `wait` passes; answers the
    /// stage reached and, once completed, the outcome.
    async fn wait_update(
        &self,
        key: &RunKey,
        update_id: &str,
        want: Stage,
        wait: Duration,
    ) -> Result<(Stage, Option<Outcome>), Status> {
        let mut rx = self.engine.watch(key).await?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Mark the progress seen before reading, so a commit after the
            // read wakes the wait below.
            let _seen = *rx.borrow_and_update();
            let state = self.engine.load(key).await?.state;
            let reached =
                match status(&state, update_id).ok_or_else(|| update_not_found(update_id))? {
                    UpdateStatus::Admitted => (Stage::Admitted, None),
                    UpdateStatus::Accepted => (Stage::Accepted, None),
                    UpdateStatus::Completed {
                        outcome,
                        completed_event_id,
                    } => {
                        let outcome = match outcome {
                            Some(o) => Some(o),
                            None => self.completed_outcome(key, completed_event_id).await?,
                        };
                        (Stage::Completed, outcome)
                    }
                    UpdateStatus::Aborted => return Err(Status::not_found(COMPLETED)),
                };
            if reached.0 as i32 >= want as i32 {
                return Ok(reached);
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => {}
                // The deadline passed, or nobody will change the run again.
                _ => return Ok(reached),
            }
        }
    }

    /// The outcome in a `WorkflowExecutionUpdateCompleted` event.
    async fn completed_outcome(
        &self,
        key: &RunKey,
        event_id: i64,
    ) -> Result<Option<Outcome>, Status> {
        let events = self.engine.history(key, event_id, 1).await?;
        match events.into_iter().next().and_then(|e| e.attributes) {
            Some(Attributes::WorkflowExecutionUpdateCompletedEventAttributes(a)) => Ok(a.outcome),
            _ => Err(Status::internal(format!(
                "event {event_id} is not the update's completed event"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_stages_default_as_the_sdks_expect() {
        assert_eq!(update_wait_stage(None), Stage::Accepted);
        assert_eq!(
            update_wait_stage(Some(&WaitPolicy {
                lifecycle_stage: Stage::Completed as i32
            })),
            Stage::Completed
        );
        assert_eq!(poll_wait_stage(None), Stage::Admitted);
        assert_eq!(
            poll_wait_stage(Some(&WaitPolicy {
                lifecycle_stage: Stage::Unspecified as i32
            })),
            Stage::Admitted
        );
    }
}
