//! Temporal Schedules (D2070–D2073): the `WorkflowService` schedule methods
//! and the driver that fires them.
//!
//! A schedule is a row in the schedule store ([`ScheduleStore`]) changed by
//! compare-and-set. Every change (an API call, a timer, a sweep, a scheduled
//! workflow closing) runs one *pass*:
//!
//! 1. Load the row; apply the change (create, update, patch).
//! 2. Re-read the status of the workflows it started and still thinks are
//!    running; a failure pauses it under `pause_on_failure`.
//! 3. Buffer the spec times now due (the catch-up window counts the ones too
//!    late), then plan the buffer with the overlap policies and act: start,
//!    request cancellation, terminate.
//! 4. Work out the next wake-up and commit. On a version conflict the pass
//!    runs again from the stored row.
//! 5. Arm a Resonate timer for the wake-up with the deterministic `sch`
//!    promise ID `<ns>.<schedule_id>.<generation>:sch.<wake_ms>` and a
//!    listener on the callback group (D2070).
//!
//! Starts carry a deterministic request ID (the `start` promise ID of the
//! nominal time and the schedule's generation), so a pass that runs twice,
//! on two replicas or after a conflict, starts each workflow once. A sweep
//! every [`SWEEP_EVERY`] runs the passes whose wake-up is overdue: the
//! backstop for a timer that could not be armed or a delivery lost in a
//! subscription gap.

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use loams_temporal_proto::temporal::api::common::v1::{Payload, WorkflowExecution};
use loams_temporal_proto::temporal::api::schedule::v1::{
    Schedule, ScheduleActionResult, ScheduleInfo, ScheduleListEntry, ScheduleListInfo,
    SchedulePatch, ScheduleState, schedule_action,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::*;
use loams_temporal_resonate::ids::PromiseId;
use loams_temporal_resonate::{CreatePromise, SettleAs};
use loams_temporal_schedule::spec::{ms_ts, ts_ms};
use loams_temporal_schedule::state::{
    self, Overlap, collect_due, is_failure, jitter_seed, scheduled_workflow_id,
};
use loams_temporal_schedule::{
    CompiledSpec, Policies, ScheduleRecord, ScheduleStoreError, SpecError,
};
use loams_temporal_statemachine::Input;
use loams_temporal_store::{NamespaceRecord, RunKey, STATUS_RUNNING};
use tonic::Status;

use crate::auth::Principal;
use crate::effects::{CALLBACK_GROUP, EffectContext};
use crate::service::Frontend;

/// How often overdue schedules are swept.
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// How late a wake-up must be before the sweep takes it (the timer usually
/// gets there first).
pub const SWEEP_GRACE_MS: i64 = 5_000;
/// While starts wait for a running workflow, the schedule looks again this
/// often even if no close was seen (closes are seen in-process; this covers
/// another replica's).
pub const RECHECK_MS: i64 = 60_000;
/// Compare-and-set attempts per pass.
const MAX_PASS_RETRIES: usize = 8;
/// DescribeSchedule's future action times.
const FUTURE_TIMES: usize = 10;
/// ListSchedules' future action times per entry.
const LIST_FUTURE_TIMES: usize = 5;
/// ListScheduleMatchingTimes' most times.
const MAX_MATCHING_TIMES: usize = 1000;
/// The longest schedule ID.
const MAX_SCHEDULE_ID_LEN: usize = 200;
/// ListSchedules' default and largest page.
const DEFAULT_LIST_PAGE: usize = 100;
const MAX_LIST_PAGE: usize = 1000;

/// The identity the scheduler starts, cancels and terminates with.
const SCHEDULER_IDENTITY: &str = "loams-temporal-scheduler";

/// The system search attribute naming the schedule that started a workflow.
pub const SA_SCHEDULED_BY_ID: &str = "TemporalScheduledById";
/// The system search attribute holding a scheduled workflow's start time.
pub const SA_SCHEDULED_START_TIME: &str = "TemporalScheduledStartTime";

fn store_status(e: ScheduleStoreError) -> Status {
    match e {
        ScheduleStoreError::NotFound => Status::not_found("schedule not found"),
        ScheduleStoreError::Exists => crate::errors::schedule_already_exists(),
        ScheduleStoreError::InvalidArgument(m) => Status::invalid_argument(m),
        ScheduleStoreError::Unavailable(m) => Status::unavailable(m),
        ScheduleStoreError::Conflict { .. } => Status::unavailable("concurrent update, retry"),
        other => Status::internal(other.to_string()),
    }
}

fn spec_status(e: SpecError) -> Status {
    Status::invalid_argument(e.to_string())
}

fn not_found(schedule_id: &str) -> Status {
    Status::not_found(format!("schedule {schedule_id:?} not found"))
}

/// The conflict token of a version.
pub fn conflict_token(version: u64) -> Bytes {
    Bytes::copy_from_slice(&version.to_be_bytes())
}

/// A JSON payload as the SDKs write search attributes.
pub fn json_payload(value: &serde_json::Value, sa_type: &str) -> Payload {
    let mut p = Payload {
        data: Bytes::from(value.to_string()),
        ..Default::default()
    };
    p.metadata
        .insert("encoding".into(), Bytes::from_static(b"json/plain"));
    p.metadata
        .insert("type".into(), Bytes::from(sa_type.to_string()));
    p
}

/// What a pass changes before it runs.
type Mutation<'a> = &'a (dyn Fn(&mut ScheduleRecord, u64, i64) -> Result<(), Status> + Send + Sync);

fn no_change(_: &mut ScheduleRecord, _: u64, _: i64) -> Result<(), Status> {
    Ok(())
}

/// Validates a schedule and returns it with its spec in canonical form.
fn canonical_schedule(mut s: Schedule) -> Result<Schedule, Status> {
    let spec = s.spec.take().unwrap_or_default();
    let spec = loams_temporal_schedule::canonicalize(&spec).map_err(spec_status)?;
    CompiledSpec::compile(&spec).map_err(spec_status)?;
    s.spec = Some(spec);
    let Some(schedule_action::Action::StartWorkflow(info)) =
        s.action.as_ref().and_then(|a| a.action.as_ref())
    else {
        return Err(Status::invalid_argument(
            "the schedule's action must be start_workflow",
        ));
    };
    if info.workflow_id.is_empty() {
        return Err(Status::invalid_argument(
            "the schedule's start_workflow needs a workflow_id",
        ));
    }
    if info
        .workflow_type
        .as_ref()
        .is_none_or(|t| t.name.is_empty())
    {
        return Err(Status::invalid_argument(
            "the schedule's start_workflow needs a workflow_type",
        ));
    }
    if info.task_queue.as_ref().is_none_or(|t| t.name.is_empty()) {
        return Err(Status::invalid_argument(
            "the schedule's start_workflow needs a task_queue",
        ));
    }
    if s.state.is_none() {
        s.state = Some(ScheduleState::default());
    }
    Ok(s)
}

/// Pauses or unpauses, setting the notes; unpausing starts looking for
/// times from now (times passed while paused are skipped).
fn set_paused(rec: &mut ScheduleRecord, paused: bool, notes: &str, now_ms: i64) {
    let state = rec.schedule.state.get_or_insert_with(Default::default);
    if state.paused && !paused {
        rec.runtime.cursor_ms = rec.runtime.cursor_ms.max(now_ms);
    }
    state.paused = paused;
    state.notes = notes.to_string();
}

/// Applies a patch: trigger, backfills, pause or unpause.
fn apply_patch(
    rec: &mut ScheduleRecord,
    seed: &str,
    patch: &SchedulePatch,
    now_ms: i64,
) -> Result<(), Status> {
    if !patch.pause.is_empty() && !patch.unpause.is_empty() {
        return Err(Status::invalid_argument(
            "a patch cannot both pause and unpause",
        ));
    }
    let policies = Policies::from_proto(rec.schedule.policies.as_ref());
    if let Some(t) = &patch.trigger_immediately {
        rec.runtime.push(state::trigger(
            now_ms,
            t.scheduled_time.as_ref().map(ts_ms),
            Overlap::from_proto(t.overlap_policy, policies.overlap),
        ));
    }
    if !patch.backfill_request.is_empty() {
        let spec = CompiledSpec::compile(rec.schedule.spec.as_ref().unwrap_or(&Default::default()))
            .map_err(spec_status)?;
        for b in &patch.backfill_request {
            let (Some(from), Some(to)) = (&b.start_time, &b.end_time) else {
                return Err(Status::invalid_argument(
                    "a backfill needs a start_time and an end_time",
                ));
            };
            let (from, to) = (ts_ms(from), ts_ms(to));
            if to < from {
                return Err(Status::invalid_argument(
                    "a backfill's end_time is before its start_time",
                ));
            }
            let overlap = Overlap::from_proto(b.overlap_policy, policies.overlap);
            for s in state::backfill(&spec, seed, from, to, overlap) {
                rec.runtime.push(s);
            }
        }
    }
    if !patch.pause.is_empty() {
        set_paused(rec, true, &patch.pause, now_ms);
    }
    if !patch.unpause.is_empty() {
        set_paused(rec, false, &patch.unpause, now_ms);
    }
    Ok(())
}

fn action_result(a: &state::RecentAction) -> ScheduleActionResult {
    ScheduleActionResult {
        schedule_time: Some(ms_ts(a.schedule_ms)),
        actual_time: Some(ms_ts(a.actual_ms)),
        start_workflow_result: Some(WorkflowExecution {
            workflow_id: a.workflow_id.clone(),
            run_id: a.run_id.clone(),
        }),
        start_workflow_status: a.status,
    }
}

impl Frontend {
    fn schedule_ctx_ns(ns: &NamespaceRecord) -> String {
        EffectContext {
            namespace_id: ns.namespace_id.clone(),
            loams_namespace_id: ns.loams_namespace_id.clone(),
        }
        .ns()
    }

    /// The timer of one wake-up.
    fn wake_promise(ns: &NamespaceRecord, rec: &ScheduleRecord, wake_ms: i64) -> CreatePromise {
        let id = PromiseId::Sch {
            ns: Self::schedule_ctx_ns(ns),
            schedule_id: rec.schedule_id.clone(),
            generation: rec.generation.clone(),
            wake_unix_ms: wake_ms,
        };
        let (encoded, tags) = id.encode();
        CreatePromise {
            id: encoded,
            timeout_at_ms: wake_ms,
            tags,
            timer: true,
            listener_group: Some(CALLBACK_GROUP.to_string()),
            ..Default::default()
        }
    }

    /// Runs one pass over a schedule (module docs). `generation`, when
    /// given, must match (a timer of a deleted and re-created schedule does
    /// nothing). `from_timer` re-arms even an unchanged wake-up. Answers the
    /// committed record and version, or `None` if there is no such schedule.
    pub(crate) async fn schedule_pass(
        &self,
        ns: &NamespaceRecord,
        schedule_id: &str,
        generation: Option<&str>,
        from_timer: bool,
        change: Mutation<'_>,
    ) -> Result<Option<(ScheduleRecord, u64)>, Status> {
        for _ in 0..MAX_PASS_RETRIES {
            let Some((mut rec, version)) = self
                .schedules
                .get(&ns.namespace_id, schedule_id)
                .await
                .map_err(store_status)?
            else {
                return Ok(None);
            };
            if generation.is_some_and(|g| g != rec.generation) {
                return Ok(None);
            }
            let old_wake = rec.runtime.next_wake_ms;
            let now = self.engine.now_ms();
            change(&mut rec, version, now)?;
            self.drive(ns, &mut rec, now).await;
            match self.schedules.update(&rec, version).await {
                Ok(v) => {
                    if let Some(w) = rec.runtime.next_wake_ms
                        && (Some(w) != old_wake || from_timer)
                    {
                        let p = Self::wake_promise(ns, &rec, w);
                        if let Err(e) = self.durability.promise_create(&p).await {
                            // The sweep picks the schedule up when it is due.
                            tracing::warn!(schedule = %schedule_id, error = %e, "arming a schedule timer failed");
                        }
                    }
                    return Ok(Some((rec, v)));
                }
                Err(ScheduleStoreError::Conflict { .. }) => continue,
                Err(e) => return Err(store_status(e)),
            }
        }
        Err(Status::unavailable(
            "the schedule is busy; retry the request",
        ))
    }

    /// Steps 2 and 3 of a pass: refresh running workflows, buffer due
    /// times, act on the buffer; then the next wake-up.
    async fn drive(&self, ns: &NamespaceRecord, rec: &mut ScheduleRecord, now: i64) {
        let seed = jitter_seed(&ns.namespace_id, &rec.schedule_id);
        let policies = Policies::from_proto(rec.schedule.policies.as_ref());
        let spec = match CompiledSpec::compile(
            rec.schedule.spec.as_ref().unwrap_or(&Default::default()),
        ) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(schedule = %rec.schedule_id, error = %e, "a stored schedule spec no longer compiles");
                None
            }
        };

        // Workflows that closed since the last pass.
        for r in rec.runtime.running.clone() {
            let key = RunKey::new(&ns.namespace_id, &r.workflow_id, &r.run_id);
            let status = match self.store().load(&key).await {
                Ok(Some(l)) if l.status == STATUS_RUNNING => continue,
                Ok(Some(l)) => l.status,
                // Gone (retention): closed, status unknown.
                Ok(None) => 0,
                Err(e) => {
                    tracing::warn!(schedule = %rec.schedule_id, error = %e, "reading a scheduled run failed");
                    continue;
                }
            };
            rec.runtime.closed(&r.workflow_id, &r.run_id, status);
            self.forget_scheduled_run(&key);
            if policies.pause_on_failure && is_failure(status) {
                set_paused(
                    rec,
                    true,
                    &format!(
                        "paused: workflow {} (run {}) failed or timed out",
                        r.workflow_id, r.run_id
                    ),
                    now,
                );
            }
        }

        let (paused, limited, remaining) =
            rec.schedule.state.as_ref().map_or((false, false, 0), |s| {
                (s.paused, s.limited_actions, s.remaining_actions)
            });
        let due = match &spec {
            Some(spec) => collect_due(
                &mut rec.runtime,
                spec,
                &seed,
                now,
                &policies,
                paused,
                limited.then_some(remaining),
            ),
            None => state::Due {
                next_wake_ms: None,
                buffered: 0,
            },
        };

        let mut remaining = remaining;
        // At most three rounds: plan, terminate, plan again.
        for _ in 0..3 {
            let plan = state::plan(&rec.runtime.buffer, !rec.runtime.running.is_empty());
            rec.runtime.buffer = plan.buffer;
            rec.runtime.overlap_skipped += plan.overlap_skipped;
            for s in &plan.starts {
                if !s.manual && limited {
                    if remaining <= 0 {
                        continue;
                    }
                    remaining -= 1;
                }
                match self.start_scheduled(ns, rec, s, &policies).await {
                    Ok((workflow_id, run_id)) => {
                        rec.runtime.started(s, now, &workflow_id, &run_id);
                        self.remember_scheduled_run(
                            RunKey::new(&ns.namespace_id, &workflow_id, &run_id),
                            &rec.schedule_id,
                        );
                    }
                    Err(e) => {
                        tracing::warn!(schedule = %rec.schedule_id, error = %e, "a scheduled start failed");
                    }
                }
            }
            if plan.cancel_running {
                for r in rec
                    .runtime
                    .running
                    .iter_mut()
                    .filter(|r| !r.cancel_requested)
                {
                    let key = RunKey::new(&ns.namespace_id, &r.workflow_id, &r.run_id);
                    let input = Input::CancelRequested {
                        reason: "canceled by the schedule's CANCEL_OTHER overlap policy".into(),
                        identity: SCHEDULER_IDENTITY.into(),
                    };
                    match self.engine.execute(&key, &Self::ctx(ns), input).await {
                        Ok(_) => r.cancel_requested = true,
                        Err(e) if e.code() == tonic::Code::NotFound => r.cancel_requested = true,
                        Err(e) => {
                            tracing::warn!(schedule = %rec.schedule_id, error = %e, "cancel for CANCEL_OTHER failed");
                        }
                    }
                }
            }
            if !plan.terminate_running {
                break;
            }
            for r in rec.runtime.running.clone() {
                let key = RunKey::new(&ns.namespace_id, &r.workflow_id, &r.run_id);
                let input = Input::Terminate {
                    reason: "terminated by the schedule's TERMINATE_OTHER overlap policy".into(),
                    details: None,
                    identity: SCHEDULER_IDENTITY.into(),
                };
                match self.engine.execute(&key, &Self::ctx(ns), input).await {
                    Ok(_) => {
                        rec.runtime
                            .closed(&r.workflow_id, &r.run_id, state::status::TERMINATED);
                        self.forget_scheduled_run(&key);
                    }
                    // Already closed: the next round sees it gone.
                    Err(e) if e.code() == tonic::Code::NotFound => {
                        let status = match self.store().load(&key).await {
                            Ok(Some(l)) => l.status,
                            _ => 0,
                        };
                        rec.runtime.closed(&r.workflow_id, &r.run_id, status);
                        self.forget_scheduled_run(&key);
                    }
                    Err(e) => {
                        tracing::warn!(schedule = %rec.schedule_id, error = %e, "terminate for TERMINATE_OTHER failed");
                    }
                }
            }
        }
        if let Some(state) = rec.schedule.state.as_mut()
            && state.limited_actions
        {
            state.remaining_actions = remaining.max(0);
        }

        let mut wake = due.next_wake_ms;
        let waits_on_running = !rec.runtime.running.is_empty()
            && (!rec.runtime.buffer.is_empty() || policies.pause_on_failure);
        if waits_on_running {
            let recheck = now + RECHECK_MS;
            wake = Some(wake.map_or(recheck, |w| w.min(recheck)));
        }
        rec.runtime.next_wake_ms = wake;
    }

    /// Starts one scheduled workflow; answers its workflow and run IDs.
    async fn start_scheduled(
        &self,
        ns: &NamespaceRecord,
        rec: &ScheduleRecord,
        s: &state::BufferedStart,
        policies: &Policies,
    ) -> Result<(String, String), Status> {
        let Some(schedule_action::Action::StartWorkflow(info)) =
            rec.schedule.action.as_ref().and_then(|a| a.action.as_ref())
        else {
            return Err(Status::failed_precondition(
                "the schedule has no start_workflow action",
            ));
        };
        let workflow_id = scheduled_workflow_id(
            &info.workflow_id,
            s.nominal_ms,
            policies.keep_original_workflow_id,
        );
        // Deterministic (D2071): the same start, retried, is the same request.
        let start_id = PromiseId::Start {
            ns: Self::schedule_ctx_ns(ns),
            schedule_id: rec.schedule_id.clone(),
            nominal_time_unix_ms: s.nominal_ms,
        }
        .encode()
        .0;
        let request_id = format!("{start_id}@{}", rec.generation);
        let mut search = info.search_attributes.clone().unwrap_or_default();
        search.indexed_fields.insert(
            SA_SCHEDULED_BY_ID.into(),
            json_payload(
                &serde_json::Value::String(rec.schedule_id.clone()),
                "Keyword",
            ),
        );
        search.indexed_fields.insert(
            SA_SCHEDULED_START_TIME.into(),
            json_payload(
                &serde_json::Value::String(state::rfc3339(s.actual_ms)),
                "Datetime",
            ),
        );
        let request = StartWorkflowExecutionRequest {
            namespace: ns.name.clone(),
            workflow_id,
            workflow_type: info.workflow_type.clone(),
            task_queue: info.task_queue.clone(),
            input: info.input.clone(),
            workflow_execution_timeout: info.workflow_execution_timeout,
            workflow_run_timeout: info.workflow_run_timeout,
            workflow_task_timeout: info.workflow_task_timeout,
            identity: SCHEDULER_IDENTITY.into(),
            request_id,
            workflow_id_reuse_policy: info.workflow_id_reuse_policy,
            retry_policy: info.retry_policy.clone(),
            memo: info.memo.clone(),
            search_attributes: Some(search),
            header: info.header.clone(),
            user_metadata: info.user_metadata.clone(),
            priority: info.priority.clone(),
            ..Default::default()
        };
        let workflow_id = request.workflow_id.clone();
        let (run_id, _) = self
            .engine
            .start(&Self::ctx(ns), uuid::Uuid::new_v4().to_string(), request)
            .await?;
        Ok((workflow_id, run_id))
    }

    fn remember_scheduled_run(&self, key: RunKey, schedule_id: &str) {
        self.scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, schedule_id.to_string());
    }

    fn forget_scheduled_run(&self, key: &RunKey) {
        self.scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }

    /// A run closed: if a schedule started it, run that schedule's pass so
    /// a buffered start goes at once.
    pub async fn on_run_closed(self: &Arc<Self>, key: RunKey) {
        let schedule_id = self
            .scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let Some(schedule_id) = schedule_id else {
            return;
        };
        let ns = match self.namespace_by_id(&key.namespace_id).await {
            Ok(ns) => ns,
            Err(e) => {
                tracing::warn!(error = %e, "a scheduled run's namespace");
                return;
            }
        };
        if let Err(e) = self
            .schedule_pass(&ns, &schedule_id, None, false, &no_change)
            .await
        {
            tracing::warn!(schedule = %schedule_id, error = %e, "schedule pass after a close failed");
        }
    }

    /// A schedule's timer fired (`sch` promise).
    pub(crate) async fn on_schedule_wake(
        &self,
        ns_segment: &str,
        schedule_id: &str,
        generation: &str,
    ) -> Result<(), String> {
        let (_, ns_id) = ns_segment
            .rsplit_once('.')
            .ok_or("a namespace segment without a Temporal namespace ID")?;
        let ns = self
            .namespace_by_id(ns_id)
            .await
            .map_err(|e| e.to_string())?;
        self.schedule_pass(&ns, schedule_id, Some(generation), true, &no_change)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Runs the passes whose wake-up is overdue (the sweep).
    pub async fn sweep_schedules(&self) {
        let before = self.engine.now_ms() - SWEEP_GRACE_MS;
        let due = match self.schedules.due(before, 100).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "schedule sweep failed");
                return;
            }
        };
        for (ns_id, schedule_id) in due {
            let ns = match self.namespace_by_id(&ns_id).await {
                Ok(ns) => ns,
                Err(e) => {
                    tracing::warn!(schedule = %schedule_id, error = %e, "schedule sweep: namespace");
                    continue;
                }
            };
            if let Err(e) = self
                .schedule_pass(&ns, &schedule_id, None, true, &no_change)
                .await
            {
                tracing::warn!(schedule = %schedule_id, error = %e, "schedule sweep pass failed");
            }
        }
    }

    /// The workflows of `rec` still running now (read-only).
    async fn still_running(
        &self,
        ns: &NamespaceRecord,
        rec: &ScheduleRecord,
    ) -> Vec<WorkflowExecution> {
        let mut out = Vec::new();
        for r in &rec.runtime.running {
            let key = RunKey::new(&ns.namespace_id, &r.workflow_id, &r.run_id);
            if let Ok(Some(l)) = self.store().load(&key).await
                && l.status == STATUS_RUNNING
            {
                out.push(WorkflowExecution {
                    workflow_id: r.workflow_id.clone(),
                    run_id: r.run_id.clone(),
                });
            }
        }
        out
    }

    fn future_times(
        ns: &NamespaceRecord,
        rec: &ScheduleRecord,
        now: i64,
        n: usize,
    ) -> Vec<prost_types::Timestamp> {
        if rec.paused() {
            return Vec::new();
        }
        let Ok(spec) =
            CompiledSpec::compile(rec.schedule.spec.as_ref().unwrap_or(&Default::default()))
        else {
            return Vec::new();
        };
        let seed = jitter_seed(&ns.namespace_id, &rec.schedule_id);
        let n = rec
            .schedule
            .state
            .as_ref()
            .filter(|s| s.limited_actions)
            .map_or(n, |s| (s.remaining_actions.max(0) as usize).min(n));
        spec.upcoming(&seed, rec.runtime.cursor_ms.max(now), n)
            .into_iter()
            .map(|t| ms_ts(t.actual_ms))
            .collect()
    }

    // ---- WorkflowService methods ----

    pub(crate) async fn create_schedule(
        &self,
        p: &Principal,
        r: CreateScheduleRequest,
    ) -> Result<CreateScheduleResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        if r.schedule_id.is_empty() {
            return Err(Status::invalid_argument("schedule_id is not set"));
        }
        if r.schedule_id.len() > MAX_SCHEDULE_ID_LEN {
            return Err(Status::invalid_argument(format!(
                "schedule_id is longer than {MAX_SCHEDULE_ID_LEN} bytes"
            )));
        }
        let schedule = canonical_schedule(r.schedule.unwrap_or_default())?;
        let generation = uuid::Uuid::new_v4().simple().to_string();
        let now = self.engine.now_ms();
        let mut rec = ScheduleRecord {
            namespace_id: ns.namespace_id.clone(),
            schedule_id: r.schedule_id.clone(),
            generation,
            request_id: r.request_id.clone(),
            schedule,
            memo: r.memo,
            search_attributes: r.search_attributes,
            runtime: state::Runtime::new(now),
        };
        // The promise IDs this schedule will use must fit (D2070).
        PromiseId::Sch {
            ns: Self::schedule_ctx_ns(&ns),
            schedule_id: rec.schedule_id.clone(),
            generation: rec.generation.clone(),
            wake_unix_ms: i64::MAX,
        }
        .encode_checked()
        .map_err(|e| Status::invalid_argument(e.to_string()))?;
        if let Some(patch) = &r.initial_patch {
            let seed = jitter_seed(&ns.namespace_id, &rec.schedule_id);
            apply_patch(&mut rec, &seed, patch, now)?;
        }
        match self.schedules.create(&rec).await {
            Ok(_) => {}
            Err(ScheduleStoreError::Exists) => {
                // A retried create with the same request ID is answered.
                if let Some((existing, version)) = self
                    .schedules
                    .get(&ns.namespace_id, &r.schedule_id)
                    .await
                    .map_err(store_status)?
                    && !r.request_id.is_empty()
                    && existing.request_id == r.request_id
                {
                    return Ok(CreateScheduleResponse {
                        conflict_token: conflict_token(version),
                    });
                }
                return Err(crate::errors::schedule_already_exists());
            }
            Err(e) => return Err(store_status(e)),
        }
        let (_, version) = self
            .schedule_pass(&ns, &r.schedule_id, None, false, &no_change)
            .await?
            .ok_or_else(|| not_found(&r.schedule_id))?;
        Ok(CreateScheduleResponse {
            conflict_token: conflict_token(version),
        })
    }

    pub(crate) async fn describe_schedule(
        &self,
        p: &Principal,
        r: DescribeScheduleRequest,
    ) -> Result<DescribeScheduleResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let (rec, version) = self
            .schedules
            .get(&ns.namespace_id, &r.schedule_id)
            .await
            .map_err(store_status)?
            .ok_or_else(|| not_found(&r.schedule_id))?;
        let now = self.engine.now_ms();
        let info = ScheduleInfo {
            action_count: rec.runtime.action_count,
            missed_catchup_window: rec.runtime.missed_catchup_window,
            overlap_skipped: rec.runtime.overlap_skipped,
            buffer_dropped: rec.runtime.buffer_dropped,
            buffer_size: rec.runtime.buffer.len() as i64,
            running_workflows: self.still_running(&ns, &rec).await,
            recent_actions: rec.runtime.recent.iter().map(action_result).collect(),
            future_action_times: Self::future_times(&ns, &rec, now, FUTURE_TIMES),
            create_time: Some(ms_ts(rec.runtime.create_time_ms)),
            update_time: Some(ms_ts(rec.runtime.update_time_ms)),
            ..Default::default()
        };
        Ok(DescribeScheduleResponse {
            schedule: Some(rec.schedule.clone()),
            info: Some(info),
            memo: rec.memo.clone(),
            search_attributes: rec.search_attributes.clone(),
            conflict_token: conflict_token(version),
        })
    }

    pub(crate) async fn update_schedule(
        &self,
        p: &Principal,
        r: UpdateScheduleRequest,
    ) -> Result<UpdateScheduleResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let schedule = canonical_schedule(r.schedule.unwrap_or_default())?;
        let token = r.conflict_token.clone();
        let memo = r.memo.clone();
        let search = r.search_attributes.clone();
        let change = move |rec: &mut ScheduleRecord, version: u64, now: i64| {
            if !token.is_empty() && token != conflict_token(version) {
                return Err(Status::failed_precondition(
                    "mismatched conflict token: the schedule changed since it was read",
                ));
            }
            let was_paused = rec.paused();
            rec.schedule = schedule.clone();
            if was_paused && !rec.paused() {
                rec.runtime.cursor_ms = rec.runtime.cursor_ms.max(now);
            }
            // The new spec applies from now: its past times are not caught up.
            rec.runtime.cursor_ms = rec.runtime.cursor_ms.max(now);
            if memo.is_some() {
                rec.memo = memo.clone();
            }
            if search.is_some() {
                rec.search_attributes = search.clone();
            }
            rec.runtime.update_time_ms = now;
            Ok(())
        };
        self.schedule_pass(&ns, &r.schedule_id, None, false, &change)
            .await?
            .ok_or_else(|| not_found(&r.schedule_id))?;
        Ok(UpdateScheduleResponse {})
    }

    pub(crate) async fn patch_schedule(
        &self,
        p: &Principal,
        r: PatchScheduleRequest,
    ) -> Result<PatchScheduleResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let patch = r.patch.unwrap_or_default();
        let seed = jitter_seed(&ns.namespace_id, &r.schedule_id);
        let change = move |rec: &mut ScheduleRecord, _: u64, now: i64| {
            apply_patch(rec, &seed, &patch, now)?;
            rec.runtime.update_time_ms = now;
            Ok(())
        };
        self.schedule_pass(&ns, &r.schedule_id, None, false, &change)
            .await?
            .ok_or_else(|| not_found(&r.schedule_id))?;
        Ok(PatchScheduleResponse {})
    }

    pub(crate) async fn delete_schedule(
        &self,
        p: &Principal,
        r: DeleteScheduleRequest,
    ) -> Result<DeleteScheduleResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let existing = self
            .schedules
            .get(&ns.namespace_id, &r.schedule_id)
            .await
            .map_err(store_status)?;
        if !self
            .schedules
            .delete(&ns.namespace_id, &r.schedule_id)
            .await
            .map_err(store_status)?
        {
            return Err(not_found(&r.schedule_id));
        }
        // Its pending timer would find nothing; cancel it to keep Resonate
        // tidy (best effort).
        if let Some((rec, _)) = existing
            && let Some(w) = rec.runtime.next_wake_ms
        {
            let id = Self::wake_promise(&ns, &rec, w).id;
            if let Err(e) = self
                .durability
                .promise_settle(&id, SettleAs::RejectedCanceled)
                .await
            {
                tracing::debug!(promise = %id, error = %e, "canceling a deleted schedule's timer");
            }
        }
        Ok(DeleteScheduleResponse {})
    }

    pub(crate) async fn list_schedules(
        &self,
        p: &Principal,
        r: ListSchedulesRequest,
    ) -> Result<ListSchedulesResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let filter = self.schedule_filter(&ns, &r.query).await?;
        let page = match r.maximum_page_size {
            n if n > 0 => (n as usize).min(MAX_LIST_PAGE),
            _ => DEFAULT_LIST_PAGE,
        };
        let mut after = (!r.next_page_token.is_empty())
            .then(|| String::from_utf8(r.next_page_token.to_vec()))
            .transpose()
            .map_err(|_| Status::invalid_argument("invalid next_page_token"))?;
        let now = self.engine.now_ms();
        let mut schedules = Vec::new();
        let mut last: Option<String> = None;
        let mut exhausted = false;
        // With a filter, read on until the page is full or the list ends.
        while schedules.len() < page {
            let batch = self
                .schedules
                .list(&ns.namespace_id, after.as_deref(), page)
                .await
                .map_err(store_status)?;
            if batch.len() < page {
                exhausted = true;
            }
            for rec in batch {
                last = Some(rec.schedule_id.clone());
                if schedules.len() >= page {
                    exhausted = false;
                    break;
                }
                if !filter.matches(&rec) {
                    continue;
                }
                schedules.push(self.list_entry(&ns, &rec, now));
            }
            if exhausted {
                break;
            }
            after = last.clone();
        }
        Ok(ListSchedulesResponse {
            schedules,
            next_page_token: match (exhausted, last) {
                (false, Some(l)) => Bytes::from(l.into_bytes()),
                _ => Bytes::new(),
            },
        })
    }

    fn list_entry(
        &self,
        ns: &NamespaceRecord,
        rec: &ScheduleRecord,
        now: i64,
    ) -> ScheduleListEntry {
        let workflow_type = match rec.schedule.action.as_ref().and_then(|a| a.action.as_ref()) {
            Some(schedule_action::Action::StartWorkflow(info)) => info.workflow_type.clone(),
            None => None,
        };
        let state = rec.schedule.state.clone().unwrap_or_default();
        ScheduleListEntry {
            schedule_id: rec.schedule_id.clone(),
            memo: rec.memo.clone(),
            search_attributes: rec.search_attributes.clone(),
            info: Some(ScheduleListInfo {
                spec: rec.schedule.spec.clone(),
                workflow_type,
                notes: state.notes,
                paused: state.paused,
                recent_actions: rec.runtime.recent.iter().map(action_result).collect(),
                future_action_times: Self::future_times(ns, rec, now, LIST_FUTURE_TIMES),
                ..Default::default()
            }),
        }
    }

    pub(crate) async fn list_schedule_matching_times(
        &self,
        p: &Principal,
        r: ListScheduleMatchingTimesRequest,
    ) -> Result<ListScheduleMatchingTimesResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let (Some(from), Some(to)) = (&r.start_time, &r.end_time) else {
            return Err(Status::invalid_argument(
                "start_time and end_time are required",
            ));
        };
        let (from, to) = (ts_ms(from), ts_ms(to));
        if to < from {
            return Err(Status::invalid_argument("end_time is before start_time"));
        }
        let (rec, _) = self
            .schedules
            .get(&ns.namespace_id, &r.schedule_id)
            .await
            .map_err(store_status)?
            .ok_or_else(|| not_found(&r.schedule_id))?;
        let spec = CompiledSpec::compile(rec.schedule.spec.as_ref().unwrap_or(&Default::default()))
            .map_err(spec_status)?;
        let seed = jitter_seed(&ns.namespace_id, &rec.schedule_id);
        Ok(ListScheduleMatchingTimesResponse {
            start_time: spec
                .times_between(&seed, from, to, MAX_MATCHING_TIMES)
                .into_iter()
                .map(|t| ms_ts(t.actual_ms))
                .collect(),
        })
    }
}

/// Which schedules a ListSchedules query keeps.
pub(crate) struct ScheduleFilter {
    keep: Option<Box<dyn Fn(&ScheduleRecord) -> bool + Send + Sync>>,
}

impl ScheduleFilter {
    /// Every schedule.
    pub(crate) fn all() -> Self {
        ScheduleFilter { keep: None }
    }

    /// The schedules `keep` accepts.
    pub(crate) fn new(keep: impl Fn(&ScheduleRecord) -> bool + Send + Sync + 'static) -> Self {
        ScheduleFilter {
            keep: Some(Box::new(keep)),
        }
    }

    fn matches(&self, rec: &ScheduleRecord) -> bool {
        self.keep.as_ref().is_none_or(|k| k(rec))
    }
}

impl Frontend {
    /// The ListSchedules filter: the visibility grammar over each
    /// schedule's attributes (`crate::visibility::schedule_record`).
    async fn schedule_filter(
        &self,
        ns: &NamespaceRecord,
        query: &str,
    ) -> Result<ScheduleFilter, Status> {
        self.schedule_query(ns, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::common::v1::WorkflowType;
    use loams_temporal_proto::temporal::api::schedule::v1::{
        BackfillRequest, ScheduleAction, ScheduleSpec, TriggerImmediatelyRequest,
    };
    use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
    use loams_temporal_proto::temporal::api::workflow::v1::NewWorkflowExecutionInfo;

    fn schedule(cron: &str) -> Schedule {
        Schedule {
            spec: Some(ScheduleSpec {
                cron_string: vec![cron.into()],
                ..Default::default()
            }),
            action: Some(ScheduleAction {
                action: Some(schedule_action::Action::StartWorkflow(
                    NewWorkflowExecutionInfo {
                        workflow_id: "wf".into(),
                        workflow_type: Some(WorkflowType { name: "T".into() }),
                        task_queue: Some(TaskQueue {
                            name: "q".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )),
            }),
            ..Default::default()
        }
    }

    fn record(s: Schedule) -> ScheduleRecord {
        ScheduleRecord {
            namespace_id: "ns".into(),
            schedule_id: "s".into(),
            generation: "g".into(),
            request_id: String::new(),
            schedule: s,
            memo: None,
            search_attributes: None,
            runtime: state::Runtime::new(0),
        }
    }

    #[test]
    fn canonical_schedules_are_checked() {
        let s = canonical_schedule(schedule("@hourly")).unwrap();
        assert!(s.spec.as_ref().unwrap().cron_string.is_empty());
        assert_eq!(s.spec.as_ref().unwrap().structured_calendar.len(), 1);
        assert!(s.state.is_some());
        assert!(canonical_schedule(schedule("not a cron")).is_err());
        let mut no_action = schedule("@hourly");
        no_action.action = None;
        assert!(canonical_schedule(no_action).is_err());
        let mut no_queue = schedule("@hourly");
        if let Some(schedule_action::Action::StartWorkflow(i)) =
            no_queue.action.as_mut().and_then(|a| a.action.as_mut())
        {
            i.task_queue = None;
        }
        assert!(canonical_schedule(no_queue).is_err());
    }

    #[test]
    fn patches_trigger_backfill_and_pause() {
        let mut rec = record(canonical_schedule(schedule("@every 1m")).unwrap());
        let patch = SchedulePatch {
            trigger_immediately: Some(TriggerImmediatelyRequest {
                overlap_policy: Overlap::AllowAll.to_proto(),
                scheduled_time: None,
            }),
            backfill_request: vec![BackfillRequest {
                start_time: Some(ms_ts(60_000)),
                end_time: Some(ms_ts(180_000)),
                overlap_policy: Overlap::BufferAll.to_proto(),
            }],
            pause: "maintenance".into(),
            unpause: String::new(),
        };
        apply_patch(&mut rec, "seed", &patch, 500_000).unwrap();
        assert_eq!(rec.runtime.buffer.len(), 4);
        assert_eq!(rec.runtime.buffer[0].overlap, Overlap::AllowAll);
        assert!(rec.runtime.buffer.iter().all(|b| b.manual));
        assert!(rec.paused());
        assert_eq!(rec.schedule.state.as_ref().unwrap().notes, "maintenance");
        let unpause = SchedulePatch {
            unpause: "back".into(),
            ..Default::default()
        };
        apply_patch(&mut rec, "seed", &unpause, 900_000).unwrap();
        assert!(!rec.paused());
        assert_eq!(rec.runtime.cursor_ms, 900_000);
        let both = SchedulePatch {
            pause: "a".into(),
            unpause: "b".into(),
            ..Default::default()
        };
        assert!(apply_patch(&mut rec, "seed", &both, 0).is_err());
        let bad_backfill = SchedulePatch {
            backfill_request: vec![BackfillRequest {
                start_time: Some(ms_ts(10)),
                end_time: Some(ms_ts(5)),
                overlap_policy: 0,
            }],
            ..Default::default()
        };
        assert!(apply_patch(&mut rec, "seed", &bad_backfill, 0).is_err());
    }

    #[test]
    fn payloads_and_tokens() {
        let p = json_payload(&serde_json::json!("sched-1"), "Keyword");
        assert_eq!(p.data.as_ref(), b"\"sched-1\"");
        assert_eq!(
            p.metadata.get("encoding").map(|b| b.as_ref()),
            Some(&b"json/plain"[..])
        );
        assert_eq!(
            p.metadata.get("type").map(|b| b.as_ref()),
            Some(&b"Keyword"[..])
        );
        assert_eq!(conflict_token(7).as_ref(), &7u64.to_be_bytes());
        assert!(ScheduleFilter::all().matches(&record(schedule("@hourly"))));
        assert!(!ScheduleFilter::new(|_| false).matches(&record(schedule("@hourly"))));
    }

    #[test]
    fn wake_promises_are_timers_with_a_listener() {
        let ns = NamespaceRecord {
            namespace_id: "tns".into(),
            loams_namespace_id: "lns".into(),
            name: "default".into(),
            retention_secs: 0,
            state: 1,
            config: serde_json::json!({}),
        };
        let rec = record(schedule("@hourly"));
        let p = Frontend::wake_promise(&ns, &rec, 3_600_000);
        assert_eq!(p.id, "lns.tns.s.g:sch.3600000");
        assert!(p.timer);
        assert_eq!(p.timeout_at_ms, 3_600_000);
        assert_eq!(p.listener_group.as_deref(), Some(CALLBACK_GROUP));
        assert!(p.validate().is_ok());
    }
}
