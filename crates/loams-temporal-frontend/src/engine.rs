//! The engine: load a run, apply one input, commit events, state and outbox
//! rows in one compare-and-set (design 74 §5.5 rule 1, D1502).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use loams_temporal_history::{HistoryEvent, MutableState, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::{Effect, Input, Rejection, Reply, Transition, apply};
use loams_temporal_store::{
    Commit, EventRow, Loaded, RunKey, StartExecution, StartOutcome, Store, StoreError,
    VisibilityRow,
};
use prost::Message as _;
use tokio::sync::watch;
use tonic::Status;

use crate::effects::{EffectContext, outbox_rows};

/// How often a commit is retried after a version conflict before the
/// request is answered `UNAVAILABLE`.
pub const MAX_CAS_RETRIES: usize = 8;

/// What a run looks like to the watchers of its history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunProgress {
    /// The next event ID (so the last event is one less).
    pub next_event_id: i64,
    /// Whether the run has closed.
    pub closed: bool,
}

/// The engine shared by every request.
pub struct Engine {
    store: Arc<dyn Store>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    watchers: Mutex<HashMap<RunKey, watch::Sender<RunProgress>>>,
    followups: Mutex<Option<tokio::sync::mpsc::UnboundedSender<FollowUp>>>,
}

/// A committed transition whose effects the frontend carries out itself
/// (children, continue-as-new, external signals and cancels, the parent
/// close policy, telling a parent its child closed; D2087).
#[derive(Debug, Clone)]
pub struct FollowUp {
    /// The run's namespace.
    pub ctx: EffectContext,
    /// The run.
    pub key: RunKey,
    /// What was committed.
    pub transition: Transition,
}

/// Whether a committed transition needs the frontend after the commit.
pub fn needs_follow_up(t: &Transition) -> bool {
    t.effects.iter().any(|e| match e {
        Effect::StartChild { .. }
        | Effect::ContinuedAsNew { .. }
        | Effect::SignalExternal { .. }
        | Effect::CancelExternal { .. }
        | Effect::ParentClosePolicy { .. } => true,
        Effect::RunClosed { .. } => t.state.parent.is_some(),
        _ => false,
    })
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

/// A run's state as loaded.
#[derive(Debug, Clone)]
pub struct LoadedRun {
    /// The decoded mutable state.
    pub state: MutableState,
    /// The stored version.
    pub version: u64,
}

pub(crate) fn store_status(e: StoreError) -> Status {
    match e {
        StoreError::NotFound => Status::not_found("workflow execution not found"),
        StoreError::Unavailable(m) => Status::unavailable(m),
        StoreError::Conflict { .. } => Status::unavailable("concurrent update, retry"),
        StoreError::InvalidArgument(m) => Status::invalid_argument(m),
        other => Status::internal(other.to_string()),
    }
}

pub(crate) fn rejection_status(r: Rejection) -> Status {
    match r {
        Rejection::NotFound(m) => Status::not_found(m),
        Rejection::InvalidArgument(m) => Status::invalid_argument(m),
        Rejection::Unimplemented(m) => Status::unimplemented(m),
        Rejection::ResourceExhausted(m) => Status::resource_exhausted(m),
    }
}

fn rows(events: &[HistoryEvent]) -> Vec<EventRow> {
    let first = events.first().map_or(0, |e| e.event_id);
    events
        .iter()
        .map(|e| EventRow {
            event_id: e.event_id,
            batch_first_event_id: first,
            event_type: e.event_type,
            blob: e.encode_to_vec(),
        })
        .collect()
}

fn visibility(state: &MutableState) -> VisibilityRow {
    let ms = |t: &Option<prost_types::Timestamp>| t.as_ref().map(loams_temporal_history::time::ms);
    VisibilityRow {
        workflow_type: state.workflow_type.clone(),
        status: state.status,
        start_time_ms: ms(&state.start_time).unwrap_or(0),
        close_time_ms: ms(&state.close_time),
        execution_time_ms: ms(&state.start_time).unwrap_or(0),
        task_queue: state.task_queue.clone(),
        memo: None,
        search_attrs: serde_json::Value::Object(Default::default()),
    }
}

impl Engine {
    /// An engine on `store`, reading time from `clock` (Unix ms).
    pub fn new(store: Arc<dyn Store>, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Engine {
            store,
            clock,
            watchers: Mutex::new(HashMap::new()),
            followups: Mutex::new(None),
        }
    }

    /// Sends every committed transition that needs a follow-up to `tx`.
    pub fn set_followups(&self, tx: tokio::sync::mpsc::UnboundedSender<FollowUp>) {
        *self
            .followups
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    fn follow_up(&self, ctx: &EffectContext, key: &RunKey, t: &Transition) {
        if !needs_follow_up(t) {
            return;
        }
        if let Some(tx) = self
            .followups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = tx.send(FollowUp {
                ctx: ctx.clone(),
                key: key.clone(),
                transition: t.clone(),
            });
        }
    }

    /// The store.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// The clock's time now.
    pub fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    /// Loads and decodes a run; `NOT_FOUND` if there is none.
    pub async fn load(&self, key: &RunKey) -> Result<LoadedRun, Status> {
        let Loaded {
            state_blob,
            version,
            ..
        } = self
            .store
            .load(key)
            .await
            .map_err(store_status)?
            .ok_or_else(|| Status::not_found("workflow execution not found"))?;
        let state = MutableState::decode(state_blob.as_slice())
            .map_err(|e| Status::internal(format!("corrupt mutable state: {e}")))?;
        Ok(LoadedRun { state, version })
    }

    /// Applies `input` to the run and commits the result, retrying on a
    /// version conflict. A stale input (a timeout for something that moved
    /// on) commits nothing and answers [`Reply::Stale`].
    pub async fn execute(
        &self,
        key: &RunKey,
        ctx: &EffectContext,
        input: Input,
    ) -> Result<Transition, Status> {
        for _ in 0..MAX_CAS_RETRIES {
            let run = self.load(key).await?;
            let t = apply(&run.state, input.clone(), self.now_ms()).map_err(rejection_status)?;
            if matches!(t.reply, Reply::Stale) {
                return Ok(t);
            }
            let closed = t
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RunClosed { .. }));
            let commit = Commit {
                events: rows(&t.events),
                state_blob: t.state.encode_to_vec(),
                status: t.state.status,
                next_event_id: t.state.next_event_id,
                outbox: outbox_rows(ctx, &t.state, &t.effects, self.now_ms()),
                visibility: closed.then(|| visibility(&t.state)),
            };
            match self.store.commit(key, run.version, commit).await {
                Ok(_) => {
                    self.notify(key, &t.state);
                    self.follow_up(ctx, key, &t);
                    return Ok(t);
                }
                Err(StoreError::Conflict { .. }) => continue,
                Err(e) => return Err(store_status(e)),
            }
        }
        Err(Status::unavailable(
            "the workflow execution is busy; retry the request",
        ))
    }

    /// Starts a run. Answers the run ID and whether this call started it
    /// (`false` for a retried start with the same request ID).
    pub async fn start(
        &self,
        ctx: &EffectContext,
        run_id: String,
        request: StartWorkflowExecutionRequest,
    ) -> Result<(String, bool), Status> {
        let workflow_id = request.workflow_id.clone();
        let request_id = request.request_id.clone();
        let input = Input::Start {
            namespace_id: ctx.namespace_id.clone(),
            run_id: run_id.clone(),
            request: Box::new(request),
        };
        self.start_with(ctx, &workflow_id, run_id, request_id, input)
            .await
    }

    /// Starts a run with a start-shaped `input` (`Start` or
    /// `SignalWithStart`); answers as [`Engine::start`] does.
    pub async fn start_with(
        &self,
        ctx: &EffectContext,
        workflow_id: &str,
        run_id: String,
        request_id: String,
        input: Input,
    ) -> Result<(String, bool), Status> {
        let key = RunKey::new(&ctx.namespace_id, workflow_id, &run_id);
        let t = apply(&MutableState::default(), input, self.now_ms()).map_err(rejection_status)?;
        match self
            .create_run(ctx, &key, &request_id, &t.state, &t.events, &t.effects)
            .await?
        {
            StartOutcome::Started { .. } => Ok((run_id, true)),
            StartOutcome::AlreadyStarted {
                run_id: existing,
                request_id: existing_request,
                ..
            } => {
                if !request_id.is_empty() && existing_request == request_id {
                    Ok((existing, false))
                } else {
                    Err(crate::errors::already_started(
                        &key.workflow_id,
                        &existing,
                        &existing_request,
                    ))
                }
            }
        }
    }

    /// Creates a run from a first commit that is already built (a start, or
    /// a reset's new run): the store's current-run check, the commit and the
    /// current-run upsert in one transaction.
    pub async fn create_run(
        &self,
        ctx: &EffectContext,
        key: &RunKey,
        request_id: &str,
        state: &MutableState,
        events: &[HistoryEvent],
        effects: &[Effect],
    ) -> Result<StartOutcome, Status> {
        let start = StartExecution {
            key: key.clone(),
            request_id: request_id.to_string(),
            commit: Commit {
                events: rows(events),
                state_blob: state.encode_to_vec(),
                status: state.status,
                next_event_id: state.next_event_id,
                outbox: outbox_rows(ctx, state, effects, self.now_ms()),
                visibility: Some(visibility(state)),
            },
        };
        let outcome = self.store.start(start).await.map_err(store_status)?;
        if matches!(outcome, StartOutcome::Started { .. }) {
            self.notify(key, state);
        }
        Ok(outcome)
    }

    fn notify(&self, key: &RunKey, state: &MutableState) {
        let progress = RunProgress {
            next_event_id: state.next_event_id,
            closed: state.status != WorkflowExecutionStatus::Running as i32,
        };
        let mut w = self.watchers.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tx) = w.get(key) {
            tx.send_replace(progress);
            if tx.receiver_count() == 0 {
                w.remove(key);
            }
        }
    }

    /// Watches a run's progress (for long polls on its history).
    pub async fn watch(&self, key: &RunKey) -> Result<watch::Receiver<RunProgress>, Status> {
        let rx = {
            let mut w = self.watchers.lock().unwrap_or_else(PoisonError::into_inner);
            w.entry(key.clone())
                .or_insert_with(|| watch::channel(RunProgress::default()).0)
                .subscribe()
        };
        // Seed with what is stored now (a commit may have raced the
        // subscription; `send_if_modified` keeps the newer value).
        let run = self.load(key).await?;
        let progress = RunProgress {
            next_event_id: run.state.next_event_id,
            closed: run.state.status != WorkflowExecutionStatus::Running as i32,
        };
        if let Some(tx) = self
            .watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            tx.send_if_modified(|p| {
                if progress.next_event_id > p.next_event_id {
                    *p = progress;
                    true
                } else {
                    false
                }
            });
        }
        Ok(rx)
    }

    /// The run's events from `from_event_id`, decoded.
    pub async fn history(
        &self,
        key: &RunKey,
        from_event_id: i64,
        max_events: usize,
    ) -> Result<Vec<HistoryEvent>, Status> {
        let rows = self
            .store
            .history(key, from_event_id, max_events)
            .await
            .map_err(store_status)?;
        rows.into_iter()
            .map(|r| {
                HistoryEvent::decode(r.blob.as_slice())
                    .map_err(|e| Status::internal(format!("corrupt event {}: {e}", r.event_id)))
            })
            .collect()
    }
}
