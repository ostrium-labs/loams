//! Resonate's listener deliveries (a timer fired, a timeout passed) come
//! back in through the engine (design 74 §5.5 rule 3). A stale one changes
//! nothing (§13.2 invariant 4).

use std::sync::Arc;

use loams_temporal_resonate::ids::{PromiseId, TAG_WF, WorkflowRef};
use loams_temporal_resonate::{Delivery, PromiseStatus};
use loams_temporal_statemachine::{Input, TimeoutKind};
use loams_temporal_store::RunKey;

use crate::effects::{EffectContext, sm_timeout};
use crate::service::Frontend;

impl Frontend {
    /// Handles one delivery to the callback group.
    pub async fn on_delivery(self: &Arc<Self>, d: Delivery) {
        match d {
            Delivery::Settled { promise_id, status } => {
                if let Err(e) = self.on_settled(&promise_id, status).await {
                    tracing::warn!(promise = %promise_id, error = %e, "callback not applied");
                }
            }
            Delivery::Gap { group } => {
                // Unblock messages sent while disconnected are lost; the
                // timers and timeouts they carried are re-read in DX3's
                // reconciliation. Until then, a lost firing waits for the
                // next input of its run.
                tracing::warn!(%group, "callback subscription gap");
            }
            Delivery::TaskReady { .. } => {}
        }
    }

    async fn on_settled(
        self: &Arc<Self>,
        promise_id: &str,
        status: PromiseStatus,
    ) -> Result<(), String> {
        let id = PromiseId::parse(promise_id).map_err(|e| e.to_string())?;
        // Our own cancellations (a cancelled timer) are not firings.
        if status == PromiseStatus::RejectedCanceled {
            return Ok(());
        }
        if let PromiseId::Sch {
            ns,
            schedule_id,
            generation,
            ..
        } = &id
        {
            return self.on_schedule_wake(ns, schedule_id, generation).await;
        }
        let (Some(wf), Some(run)) = (id.wf().cloned(), id.run().map(str::to_string)) else {
            return Ok(());
        };
        let workflow_id = match wf {
            WorkflowRef::Id(s) => s,
            WorkflowRef::Hash(_) => {
                let view = self
                    .durability
                    .promise_get(promise_id)
                    .await
                    .map_err(|e| e.to_string())?;
                view.tags
                    .get(TAG_WF)
                    .cloned()
                    .ok_or("a hashed workflow ID without its tag")?
            }
        };
        let (loams_ns, ns_id) = id
            .ns()
            .rsplit_once('.')
            .ok_or("a namespace segment without a Temporal namespace ID")?;
        let input = match id {
            PromiseId::Tmr {
                started_event_id, ..
            } => Input::TimerFired { started_event_id },
            PromiseId::To {
                event_id,
                kind,
                attempt,
                ..
            } => match sm_timeout(kind) {
                TimeoutKind::WorkflowTask => Input::WorkflowTaskTimedOut {
                    scheduled_event_id: event_id,
                    attempt,
                },
                k => Input::ActivityTimedOut {
                    scheduled_event_id: event_id,
                    attempt,
                    kind: k,
                },
            },
            _ => return Ok(()),
        };
        let ctx = EffectContext {
            namespace_id: ns_id.to_string(),
            loams_namespace_id: loams_ns.to_string(),
        };
        let key = RunKey::new(ns_id, workflow_id, run);
        match self.engine.execute(&key, &ctx, input).await {
            Ok(_) => Ok(()),
            // The run closed or moved on: nothing to do.
            Err(s) if s.code() == tonic::Code::NotFound => Ok(()),
            Err(s) => Err(s.to_string()),
        }
    }
}
