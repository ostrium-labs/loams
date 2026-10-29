//! Effects become outbox rows: deterministic Resonate promises (design 74
//! §5.4, as amended by X1.R13) that the relay creates or settles.

use loams_temporal_history::{MutableState, WorkflowExecutionStatus};
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

/// The outbox `kind` of the rows the frontend itself carries out (children,
/// continue-as-new, external requests, the parent close policy, telling a
/// parent; D2089). The relay hands them to the follow-up executor instead
/// of Resonate.
pub const FOLLOW_UP_KIND: &str = "fu";

/// A child the parent close policy acts on, as an outbox row carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildTarget {
    /// The child's namespace name (empty: the parent's).
    pub namespace: String,
    /// The child's namespace ID, if known.
    pub namespace_id: String,
    /// The child's workflow ID.
    pub workflow_id: String,
    /// The child's run ID.
    pub run_id: String,
    /// `temporal.api.enums.v1.ParentClosePolicy`.
    pub policy: i32,
}

/// What a follow-up row asks for. Everything else is read back from the
/// run's history and state, so a row carried out again after a restart
/// does the same thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FollowUpIntent {
    /// Start the child its `StartChildWorkflowExecutionInitiated` names.
    StartChild {
        /// The initiated event.
        initiated_event_id: i64,
    },
    /// Start the run a continue-as-new named.
    ContinueAsNew {
        /// Its run ID.
        new_run_id: String,
    },
    /// Signal the workflow the initiated event names.
    SignalExternal {
        /// The initiated event.
        initiated_event_id: i64,
    },
    /// Ask the workflow the initiated event names to cancel.
    CancelExternal {
        /// The initiated event.
        initiated_event_id: i64,
    },
    /// Terminate or cancel children (the parent close policy).
    ParentClose {
        /// The children.
        children: Vec<ChildTarget>,
    },
    /// Tell the parent this child's run closed.
    NotifyParent,
}

/// A follow-up outbox row: the run and the intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpRow {
    /// The gateway's namespace ID of the run.
    pub namespace_id: String,
    /// Its Loams namespace.
    pub loams_namespace_id: String,
    /// The workflow ID.
    pub workflow_id: String,
    /// The run ID.
    pub run_id: String,
    /// What to do.
    pub intent: FollowUpIntent,
}

impl FollowUpRow {
    /// A short, deterministic name of the row (its `resonate_id` column;
    /// never sent to Resonate).
    pub fn name(&self) -> String {
        let what = match &self.intent {
            FollowUpIntent::StartChild { initiated_event_id } => {
                format!("child.{initiated_event_id}")
            }
            FollowUpIntent::ContinueAsNew { new_run_id } => format!("can.{new_run_id}"),
            FollowUpIntent::SignalExternal { initiated_event_id } => {
                format!("sig.{initiated_event_id}")
            }
            FollowUpIntent::CancelExternal { initiated_event_id } => {
                format!("cancel.{initiated_event_id}")
            }
            FollowUpIntent::ParentClose { .. } => "close-policy".to_string(),
            FollowUpIntent::NotifyParent => "notify-parent".to_string(),
        };
        format!("fu:{}.{}:{what}", self.namespace_id, self.run_id)
    }
}

fn follow_up(ctx: &EffectContext, state: &MutableState, intent: FollowUpIntent) -> OutboxInsert {
    let row = FollowUpRow {
        namespace_id: ctx.namespace_id.clone(),
        loams_namespace_id: ctx.loams_namespace_id.clone(),
        workflow_id: state.workflow_id.clone(),
        run_id: state.run_id.clone(),
        intent,
    };
    OutboxInsert {
        kind: FOLLOW_UP_KIND.to_string(),
        resonate_id: row.name(),
        request: serde_json::to_value(&row).expect("follow-up rows serialize"),
    }
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
            // A child's close is its parent's news (a continued run's is not:
            // its last run tells). Other outstanding promises of a closed
            // run are settled lazily.
            Effect::RunClosed { status } => {
                if state.parent.is_some() && *status != WorkflowExecutionStatus::ContinuedAsNew {
                    out.push(follow_up(ctx, state, FollowUpIntent::NotifyParent));
                }
            }
            // Carried out by the frontend's follow-up executor
            // (`crate::followups`), in the same commit as the events.
            Effect::StartChild { initiated_event_id } => out.push(follow_up(
                ctx,
                state,
                FollowUpIntent::StartChild {
                    initiated_event_id: *initiated_event_id,
                },
            )),
            Effect::ContinuedAsNew { new_run_id } => out.push(follow_up(
                ctx,
                state,
                FollowUpIntent::ContinueAsNew {
                    new_run_id: new_run_id.clone(),
                },
            )),
            Effect::SignalExternal { initiated_event_id } => out.push(follow_up(
                ctx,
                state,
                FollowUpIntent::SignalExternal {
                    initiated_event_id: *initiated_event_id,
                },
            )),
            Effect::CancelExternal { initiated_event_id } => out.push(follow_up(
                ctx,
                state,
                FollowUpIntent::CancelExternal {
                    initiated_event_id: *initiated_event_id,
                },
            )),
            Effect::ParentClosePolicy { children } => out.push(follow_up(
                ctx,
                state,
                FollowUpIntent::ParentClose {
                    children: children
                        .iter()
                        .map(|c| ChildTarget {
                            namespace: c.namespace.clone(),
                            namespace_id: c.namespace_id.clone(),
                            workflow_id: c.workflow_id.clone(),
                            run_id: c.run_id.clone(),
                            policy: c.policy,
                        })
                        .collect(),
                },
            )),
        }
    }
    out
}
