//! Effects become outbox rows: deterministic Resonate promises (design 74
//! §5.4, as amended by X1.R13) that the relay creates or settles.

use loams_temporal_history::MutableState;
use loams_temporal_matching::{QueueKey, TaskKind};
use loams_temporal_resonate::ids::{PromiseId, TimeoutKind as IdTimeout, WorkflowRef, namespace};
use loams_temporal_resonate::{CreatePromise, OutboxRequest, SettleAs};
use loams_temporal_statemachine::{Effect, TimeoutKind};
use loams_temporal_store::OutboxInsert;
use serde::{Deserialize, Serialize};

/// A task promise's deadline when nothing else bounds it: far enough that
/// only the gateway's own timeouts end an attempt (one year).
pub const TASK_PROMISE_TTL_MS: i64 = 365 * 24 * 3600 * 1000;

/// The poll group every timer and timeout listener reports to.
pub const CALLBACK_GROUP: &str = "ltc-callbacks";

/// Which namespace a run belongs to, for its promise IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectContext {
    /// The gateway's (Temporal) namespace ID.
    pub namespace_id: String,
    /// The Loams namespace that owns it.
    pub loams_namespace_id: String,
}

impl EffectContext {
    /// The promise-ID namespace segment (`<loams-ns-id>.<temporal-ns-id>`).
    pub fn ns(&self) -> String {
        namespace(&self.loams_namespace_id, &self.namespace_id)
    }
}

/// What a task promise's parameter carries: enough to find the run without
/// parsing the ID (which may hold a hashed workflow ID).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    /// The gateway's namespace ID.
    pub namespace_id: String,
    /// The workflow ID, raw.
    pub workflow_id: String,
    /// The run ID.
    pub run_id: String,
    /// `"wft"` or `"act"`.
    pub kind: String,
    /// The task's scheduled event.
    pub scheduled_event_id: i64,
    /// The attempt.
    pub attempt: i32,
}

fn id_timeout(k: TimeoutKind) -> IdTimeout {
    match k {
        TimeoutKind::WorkflowTask => IdTimeout::WorkflowTask,
        TimeoutKind::ScheduleToStart => IdTimeout::ScheduleToStart,
        TimeoutKind::StartToClose => IdTimeout::StartToClose,
        TimeoutKind::ScheduleToClose => IdTimeout::ScheduleToClose,
        TimeoutKind::Heartbeat => IdTimeout::Heartbeat,
    }
}

/// The statemachine's timeout kind for a promise-ID timeout kind.
pub fn sm_timeout(k: IdTimeout) -> TimeoutKind {
    match k {
        IdTimeout::WorkflowTask => TimeoutKind::WorkflowTask,
        IdTimeout::ScheduleToStart => TimeoutKind::ScheduleToStart,
        IdTimeout::StartToClose => TimeoutKind::StartToClose,
        IdTimeout::ScheduleToClose => TimeoutKind::ScheduleToClose,
        IdTimeout::Heartbeat => TimeoutKind::Heartbeat,
    }
}

fn row(id: PromiseId, request: OutboxRequest) -> OutboxInsert {
    OutboxInsert {
        kind: id.kind().to_string(),
        resonate_id: id.encode().0,
        request: serde_json::to_value(&request).expect("outbox requests serialize"),
    }
}

/// The outbox rows for `effects` of a run whose state is now `state`.
pub fn outbox_rows(
    ctx: &EffectContext,
    state: &MutableState,
    effects: &[Effect],
    now_ms: i64,
) -> Vec<OutboxInsert> {
    let ns = ctx.ns();
    let wf = || WorkflowRef::Id(state.workflow_id.clone());
    let run = state.run_id.clone();
    let task = |kind: &str, scheduled_event_id: i64, attempt: i32| {
        serde_json::to_vec(&TaskRef {
            namespace_id: ctx.namespace_id.clone(),
            workflow_id: state.workflow_id.clone(),
            run_id: run.clone(),
            kind: kind.to_string(),
            scheduled_event_id,
            attempt,
        })
        .expect("task refs serialize")
    };
    let create = |id: &PromiseId| {
        let (encoded, tags) = id.encode();
        CreatePromise {
            id: encoded,
            tags,
            ..Default::default()
        }
    };
    let mut out = Vec::new();
    for e in effects {
        match e {
            Effect::ScheduleWorkflowTask {
                scheduled_event_id,
                attempt,
                task_queue,
            } => {
                let id = PromiseId::Wft {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    scheduled_event_id: *scheduled_event_id,
                };
                let mut c = create(&id);
                c.timeout_at_ms = now_ms + TASK_PROMISE_TTL_MS;
                c.param = Some(task("wft", *scheduled_event_id, *attempt));
                c.target_group = Some(
                    QueueKey {
                        namespace_id: ctx.namespace_id.clone(),
                        task_queue: task_queue.clone(),
                        kind: TaskKind::Workflow,
                    }
                    .group(),
                );
                out.push(row(id, OutboxRequest::CreatePromise(c)));
            }
            Effect::WorkflowTaskDone {
                scheduled_event_id, ..
            } => {
                let id = PromiseId::Wft {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    scheduled_event_id: *scheduled_event_id,
                };
                let encoded = id.encode().0;
                out.push(row(
                    id,
                    OutboxRequest::Settle {
                        id: encoded,
                        state: SettleAs::Resolved,
                    },
                ));
            }
            Effect::ScheduleActivity {
                scheduled_event_id,
                attempt,
                task_queue,
                not_before_ms,
            } => {
                let id = PromiseId::Act {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    scheduled_event_id: *scheduled_event_id,
                    attempt: *attempt,
                };
                let mut c = create(&id);
                c.timeout_at_ms = now_ms.max(*not_before_ms) + TASK_PROMISE_TTL_MS;
                c.param = Some(task("act", *scheduled_event_id, *attempt));
                c.target_group = Some(
                    QueueKey {
                        namespace_id: ctx.namespace_id.clone(),
                        task_queue: task_queue.clone(),
                        kind: TaskKind::Activity,
                    }
                    .group(),
                );
                if *not_before_ms > now_ms {
                    c.delay_until_ms = Some(*not_before_ms);
                }
                out.push(row(id, OutboxRequest::CreatePromise(c)));
            }
            Effect::ActivityDone {
                scheduled_event_id,
                attempt,
            } => {
                let id = PromiseId::Act {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    scheduled_event_id: *scheduled_event_id,
                    attempt: *attempt,
                };
                let encoded = id.encode().0;
                out.push(row(
                    id,
                    OutboxRequest::Settle {
                        id: encoded,
                        state: SettleAs::Resolved,
                    },
                ));
            }
            Effect::ArmTimeout {
                kind,
                event_id,
                attempt,
                at_ms,
            } => {
                let id = PromiseId::To {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    event_id: *event_id,
                    kind: id_timeout(*kind),
                    attempt: *attempt,
                };
                let mut c = create(&id);
                c.timeout_at_ms = *at_ms;
                c.awaitable = true;
                c.listener_group = Some(CALLBACK_GROUP.to_string());
                out.push(row(id, OutboxRequest::CreatePromise(c)));
            }
            Effect::StartTimer {
                started_event_id,
                fire_at_ms,
            } => {
                let id = PromiseId::Tmr {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    started_event_id: *started_event_id,
                };
                let mut c = create(&id);
                c.timeout_at_ms = *fire_at_ms;
                c.timer = true;
                c.listener_group = Some(CALLBACK_GROUP.to_string());
                out.push(row(id, OutboxRequest::CreatePromise(c)));
            }
            Effect::CancelTimer { started_event_id } => {
                let id = PromiseId::Tmr {
                    ns: ns.clone(),
                    wf: wf(),
                    run: run.clone(),
                    started_event_id: *started_event_id,
                };
                let encoded = id.encode().0;
                out.push(row(
                    id,
                    OutboxRequest::Settle {
                        id: encoded,
                        state: SettleAs::RejectedCanceled,
                    },
                ));
            }
            // Outstanding promises of a closed run are settled lazily: a
            // task handed out for it, or a timer firing, finds the run
            // closed and settles or drops it (§13.2 invariant 4).
            Effect::RunClosed { .. } => {}
            // Carried out by the frontend after the commit
            // (`crate::followups`), not through Resonate.
            Effect::StartChild { .. }
            | Effect::ContinuedAsNew { .. }
            | Effect::SignalExternal { .. }
            | Effect::CancelExternal { .. }
            | Effect::ParentClosePolicy { .. } => {}
        }
    }
    out
}
