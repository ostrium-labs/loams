//! DocHost — per-chat `SessionDoc` handles: snapshot persistence (debounced) and the
//! HOST-ONLY durable command executor. The engine is local-only (D781): the edge room
//! sync, relay delivery and remote nudges this module carried in the fork are gone.
//!
//! Pragmatic port of loams-desktop's `session-docs.ts` + the `main.ts` executor (spec:
//! feature-inventory §3.3, ARCHITECTURE §2 "command plane"):
//! - the doc IS the outbox: commands and user entries commit locally;
//! - on every doc change the handle re-emits the joined
//!   transcript to watchers, drains pending commands, and schedules a snapshot save;
//! - command drain: evaluate via `evaluate_command` (with the DocsStore processed
//!   ledger), mark processed BEFORE execute, execute through the sessions engine, then
//!   write the outcome status back into the doc as the sole outcome writer.
//!
//! Chat ownership is gated on the workspace doc (`chats[chat_id].deviceId`), with
//! claim-on-first-command for unknown chats.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use loams_agentd_doc::{
    COMMAND_DEFAULT_TTL_MS, CommandBasedOn, CommandDisposition, DocError, EvaluationContext,
    MessagePart, MessageRole, MessageStatus, QueueDeliveryGate, QueuedMessage, SessionCommandEntry,
    SessionCommandPayload, SessionCommandStatus, SessionDoc, SessionMessageEntry, SubagentStatus,
    evaluate_command, join_continuation_entries,
};
use loams_agentd_proto::{
    ConversationSourceContext, HarnessId, UserInputAnswer, UserInputQuestion,
};
use loams_agentd_store::DocsStore;

use crate::chat_persistence::CHAT2_DOC_EPOCH;
use crate::project_actions::{
    ProjectActionSetupHandoff, ProjectActionsStore, launch_project_setup_action,
};
use crate::sessions::{SessionsEngine, SteerOutcome};
use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, Terminals, new_id, now_ms};

/// How often archived chats' tool outputs are checked against the retention.
const TOOL_OUTPUT_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Debounce window for local snapshot saves after a doc change.
const SNAPSHOT_DEBOUNCE_MS: u64 = 1_000;

/// An edit client renews every 20s. Sixty seconds tolerates two missed
/// heartbeats without turning a vanished client into an invisible permanent
/// lock. Expiry fails closed into ReviewRequired rather than releasing.
pub const QUEUE_EDIT_LEASE_MS: i64 = 60_000;

/// Warm-doc LRU: how many unwatched, run-less docs stay fully open. Everything
/// beyond this (and beyond [`loams_agentd_doc::DOC_LRU_BYTE_BUDGET`]) is evicted
/// oldest-access-first — reopening from the SQLite snapshot measured within
/// ~11ms of a warm doc, so the cap trades no perceptible open latency.
const WARM_DOC_CAP: usize = 12;

/// Resident-memory estimate per compressed snapshot byte. Loro snapshots are
/// columnar+compressed; the in-memory doc plus mirror runs well above the blob
/// size. A rough multiplier is enough here — the budget is a safety ceiling,
/// the count cap does the day-to-day work.
const RESIDENT_BYTES_PER_SNAPSHOT_BYTE: usize = 6;

/// Floor per open doc (buffers, tasks) regardless of content size.
const DOC_RESIDENT_FLOOR_BYTES: usize = 512 * 1024;

/// A command whose attachment bytes are still in transit waits at most this
/// long before the drain rejects it loudly (and the transfer task gives up on
/// the same clock) — a chat must never wedge behind bytes that aren't coming.
const ATTACHMENT_WAIT_MAX: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const ATTACHMENT_WAIT_MAX_MS: i64 = ATTACHMENT_WAIT_MAX.as_millis() as i64;
/// Re-check cadence while a chat's queue is deferred on in-transit bytes
/// (the happy path is event-driven — UploadCommit kicks the drain — this
/// timer only covers the give-up transition and missed kicks).
const ATTACHMENT_WAIT_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct DocHostConfig {
    pub device_id: String,
    /// Harness for doc-command runs on chats without a workspace `config` row.
    pub default_harness: HarnessId,
    /// Directory of full tool outputs and frozen subagent transcripts
    /// ([`crate::tool_outputs`], plan DD1 ruling T0-16).
    pub tool_outputs: std::path::PathBuf,
}

struct DocHostInner {
    store: Arc<DocsStore>,
    config: DocHostConfig,
    /// Set-once (first wins), cleared by `shutdown_workers`: sessions and
    /// doc-host reference each other through Arcs, so a retired runtime's
    /// graph only drops once this back-edge is severed.
    sessions: Mutex<Option<SessionsEngine>>,
    workspace: OnceLock<WorkspaceHost>,
    /// Worktree materialization for Run commands (see `set_repos`).
    repos: OnceLock<crate::repos::Repos>,
    project_action_runtime: OnceLock<(ProjectActionsStore, Terminals)>,
    /// Cancels every worker spawned through `spawn_worker` — the loops'
    /// own exit conditions (weak handle death, closed channels) don't cover
    /// runtime replacement, where tasks must stop even while something
    /// still pins the graph.
    shutdown: CancellationToken,
    /// Tracks every spawned worker so `shutdown_workers` can await them.
    tasks: TaskTracker,
    handles: Mutex<HashMap<String, Arc<ChatDocHandle>>>,
    document_loads: AtomicU64,
    /// Serialize cold opens without blocking access to already-live handles.
    opening: Mutex<()>,
    /// Attachment-wait re-drain timers armed (one per chat): a command
    /// deferred on in-transit attachment bytes re-checks on a cadence, and
    /// each deferral must not stack another timer.
    drain_waiting: Mutex<HashSet<String>>,
    /// Uploads store (engine assembly) — resolves `pending://` attachment refs.
    uploads: OnceLock<crate::uploads::Uploads>,
    /// Command ids currently BETWEEN mark-processed and their resolution in a
    /// drain. Distinguishes "executing right now" from "consumed by the
    /// ledger but dead" (a crash between mark and resolve): the drain
    /// terminalizes the latter as Rejected instead of leaving a forever-
    /// Pending entry no retry could ever reach (2026-08-19 swallowed-send).
    executing: Mutex<HashSet<String>>,
    /// Full tool outputs and frozen subagent transcripts (plan DD1 T0-16).
    tool_outputs: crate::tool_outputs::ToolOutputs,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct DocHost {
    inner: Arc<DocHostInner>,
}

/// How a taken queue row reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueSend {
    /// Nothing is running: start a turn with it.
    NextTurn,
    /// Something is running and can take input mid-turn: steer it in.
    Steer,
    /// The user said now: stop what is running first.
    Interrupt,
}

const ATTACHMENT_ONLY_PROMPT: &str = "See the attached image(s).";
const ATTACHMENT_PROMPT_HEADER: &str = "Attached images (local files — open them to view):";

/// Queue rows keep the user's editable text separate from attachment paths.
/// Rebuild the transcript/harness transport only when the row is dispatched.
///
/// Older clients stored the already-expanded prompt in `text`; recognize the
/// exact trailer implied by `attachments` so those rows are not expanded a
/// second time after an upgrade.
fn queued_message_prompt(text: &str, attachments: &[String]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let refs = attachments
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let trailer = format!("\n\n{ATTACHMENT_PROMPT_HEADER}\n{refs}");
    let body = text.strip_suffix(&trailer).unwrap_or(text);
    let body = if body.trim().is_empty() {
        ATTACHMENT_ONLY_PROMPT
    } else {
        body
    };
    format!("{body}{trailer}")
}

fn queue_text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BeginQueueEditOutcome {
    Acquired {
        lease_id: String,
        text: String,
        attachments: Vec<String>,
        base_text_hash: String,
        expires_at_ms: i64,
    },
    Locked {
        owner_device_id: String,
        expires_at_ms: i64,
    },
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RenewQueueEditOutcome {
    Renewed { expires_at_ms: i64 },
    Lost,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishQueueEditAction {
    Commit,
    Cancel,
    Discard,
    ReleaseUnchanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FinishQueueEditOutcome {
    Committed,
    Cancelled,
    Discarded,
    Released,
    Conflict { current_text: String },
    Lost,
    Missing,
}

/// Content and its historical presentation cutoff travel atomically, even
/// when the watch coalesces several backfill and live commits.
#[derive(Clone, Default)]
pub struct TranscriptSnapshot {
    pub entries: Arc<Vec<SessionMessageEntry>>,
    pub replay_baseline: Arc<loams_agentd_doc::TranscriptBaseline>,
}

/// One open chat doc: the `SessionDoc` and its change plumbing.
pub struct ChatDocHandle {
    chat_id: String,
    device_id: String,
    doc: Arc<SessionDoc>,
    messages_tx: watch::Sender<TranscriptSnapshot>,
    /// Serialize historical imports with publication so an async doc-change
    /// task cannot publish recovered content before its presentation cutoff.
    transcript_import: Mutex<()>,
    transcript_history: Arc<Mutex<crate::transcript_history::TranscriptHistory>>,
    /// Pending-message queue watch (WatchQueue). Cheap to rebuild — a handful
    /// of short rows — so unlike the transcript mirror it publishes on every
    /// change without a dirty flag.
    queue_tx: watch::Sender<Vec<QueuedMessage>>,
    /// Serializes everything that TAKES from the queue. Both the doc-change
    /// task and the turn-end status watcher call `drain_queue`, and nothing
    /// keeps those two apart: without this they interleave across the
    /// `dispatch` await, each taking a different head and each sending, so a
    /// queue meant to release one message releases all of them.
    ///
    /// It also covers the gap a send-now's interrupt opens — between stopping
    /// the turn and starting its own the chat reads Idle, and an idle chat with
    /// a queue is exactly what the flush drains.
    drain_lock: tokio::sync::Mutex<()>,
    /// Serialize prompt commands while still allowing interrupt/input controls.
    command_drain_lock: tokio::sync::Mutex<()>,
    /// Queue rows held as explicit steers for a turn-boundary agent. They
    /// lead ordinary queued rows, in the order they were steered.
    steered_rows: Mutex<Vec<String>>,
    /// An explicit user interrupt freezes automatic queue delivery. The next
    /// explicit prompt or queue send resumes it; incidental doc/status changes
    /// must not turn Cancel into "send the next row".
    queue_paused: AtomicBool,
    /// True when the doc changed while nobody watched: the mirror rebuild is
    /// deferred to the next `watch_messages` attach instead of paid per commit.
    mirror_dirty: AtomicBool,
    /// Epoch ms of the last open/watch touch — the LRU eviction key.
    last_access: AtomicI64,
    /// Last known snapshot blob size — the eviction budget estimate's input.
    snapshot_bytes: AtomicUsize,
    /// The doc lineage this handle was BUILT for (1 = the legacy fat doc,
    /// 2 = the thin lineage every chat born in this engine has). The
    /// staleness check in `open_local` compares it against the registry row.
    room_gen: u32,
    /// Set when a newer lineage replaced this handle's doc on disk: a retired
    /// handle never persists again unless no thin lineage exists on disk at
    /// all (`save_snapshot` double-checks, so a doc can't lose its only copy).
    retired: AtomicBool,
    writers: Arc<AtomicUsize>,
    pub(crate) persistence: Option<Arc<crate::chat_persistence::ChatPersistence>>,
    /// Doc subscription (drop = unsubscribe) — bumps the change watch on every commit.
    _sub: loro::Subscription,
}

/// A running agent explicitly owns a writer lease until its final cleanup.
/// Reference counting remains a conservative compatibility guard for read APIs.
pub struct DocWriter {
    doc: Arc<SessionDoc>,
    writers: Arc<AtomicUsize>,
}
impl std::ops::Deref for DocWriter {
    type Target = SessionDoc;
    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}
impl Drop for DocWriter {
    fn drop(&mut self) {
        self.writers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ChatDocHandle {
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub fn doc(&self) -> &SessionDoc {
        &self.doc
    }

    pub fn writer(&self) -> DocWriter {
        self.writers.fetch_add(1, Ordering::AcqRel);
        DocWriter {
            doc: self.doc.clone(),
            writers: self.writers.clone(),
        }
    }

    pub fn doc_arc(&self) -> Arc<SessionDoc> {
        self.doc.clone()
    }

    /// Joined transcript watch — re-sent on every doc change (WatchDocMessages).
    ///
    /// Attach-time refresh: the mirror is only maintained while watched, so a
    /// doc that changed unwatched materializes here, once, instead of on every
    /// commit it sat through in the background.
    pub fn watch_messages(&self) -> watch::Receiver<TranscriptSnapshot> {
        self.touch();
        // Subscribe BEFORE the dirty check: a commit racing this attach then
        // sees a live receiver and publishes, instead of re-marking dirty
        // after our refresh and leaving the new watcher a cleared mirror.
        let _import = lock(&self.transcript_import);
        let rx = {
            if self.messages_tx.receiver_count() == 0 {
                // A new viewing session must not inherit the former viewer's
                // live-part protection, even if no commit happened while away.
                *lock(&self.transcript_history) = Default::default();
            }
            self.messages_tx.subscribe()
        };
        if self.mirror_dirty.load(Ordering::Acquire) {
            self.publish_messages_locked();
        }
        rx
    }

    /// Queue watch — the composer's held messages, re-sent on every doc change.
    pub fn watch_queue(&self) -> watch::Receiver<Vec<QueuedMessage>> {
        self.touch();
        let rx = self.queue_tx.subscribe();
        self.publish_queue();
        rx
    }

    /// Where a newly steered row goes: after the rows already steered, ahead
    /// of every ordinary row. Records `id` as steered.
    fn steer_slot(&self, id: &str) -> Result<usize, DocError> {
        let mut steered = lock(&self.steered_rows);
        let queue = self.doc.read_queue()?;
        steered.retain(|row| queue.iter().any(|q| &q.id == row));
        let slot = queue
            .iter()
            .take_while(|row| steered.contains(&row.id))
            .count();
        steered.push(id.to_string());
        Ok(slot)
    }

    fn publish_queue(&self) {
        match self.doc.read_queue() {
            Ok(items) => {
                self.queue_tx.send_if_modified(|slot| {
                    if *slot == items {
                        false
                    } else {
                        *slot = items;
                        true
                    }
                });
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "queue read failed");
            }
        }
    }

    fn touch(&self) {
        self.last_access.store(now_ms(), Ordering::Relaxed);
    }

    /// Write a complete user message entry, idempotent by id (the client-minted message
    /// id — a re-executed command or optimistic echo never duplicates the entry).
    pub fn write_user_message(
        &self,
        message_id: &str,
        text: &str,
        created_at: i64,
    ) -> Result<(), DocError> {
        if self.doc.read_entries()?.iter().any(|e| e.id == message_id) {
            return Ok(());
        }
        self.doc.push_message(&SessionMessageEntry {
            id: message_id.to_string(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_string(),
            }],
            created_at,
            device_id: self.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })
    }

    /// Recovery sweep: settle this device's running subagent chips (including
    /// chips in completed parent turns), then stamp abandoned `streaming`
    /// entries `aborted`, appending
    /// `note` as a visible error part so the transcript says WHY the turn
    /// ended (loams-desktop folded "Run interrupted by backend restart" the same
    /// way). Returns the stamped entries' `(id, created_at)` — recovery uses
    /// them for the resume-freshness check.
    pub fn mark_abandoned_streams(&self, note: &str) -> Result<Vec<(String, i64)>, DocError> {
        let mut stamped = Vec::new();
        let mut chips_changed = false;
        for entry in self.doc.read_entries()? {
            if entry.role != MessageRole::Assistant || entry.device_id != self.device_id {
                continue;
            }
            for part in &entry.parts {
                if let MessagePart::Tool {
                    id,
                    subagent_status: Some(SubagentStatus::Running),
                    ..
                } = part
                {
                    chips_changed |=
                        self.doc
                            .update_subagent_chip(id, None, Some("failed"), None)?;
                }
            }
            if entry.status == Some(MessageStatus::Streaming)
                && self
                    .doc
                    .set_message_status(&entry.id, MessageStatus::Aborted)?
            {
                let part_id = format!("{}-recovery", entry.id);
                if let Err(err) = self.doc.append_error_part(&entry.id, &part_id, note) {
                    tracing::warn!(chat = %self.chat_id, error = %err, "recovery note append failed");
                }
                stamped.push((entry.id.clone(), entry.created_at));
            }
        }
        if chips_changed || !stamped.is_empty() {
            self.publish_messages();
        }
        Ok(stamped)
    }

    fn publish_messages(&self) {
        let _import = lock(&self.transcript_import);
        self.publish_messages_locked();
    }

    // Caller holds transcript_import, shared with attach and mirror clearing.
    fn publish_messages_locked(&self) {
        self.mirror_dirty.store(false, Ordering::Release);
        match self.doc.read_entries() {
            Ok(entries) => {
                let replay_baseline =
                    lock(&self.transcript_history).snapshot(self.doc.doc(), &entries);
                let joined = join_continuation_entries(entries);
                // send_replace: update the watch even with no subscribers yet, so a
                // late subscriber's first borrow sees the current transcript.
                self.messages_tx.send_replace(TranscriptSnapshot {
                    entries: Arc::new(joined),
                    replay_baseline: replay_baseline.clone(),
                });
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "transcript read failed");
            }
        }
    }

    /// Test seam: hold the transcript publication lock while `import` runs.
    #[cfg(test)]
    pub(crate) fn import_transcript<T>(&self, import: impl FnOnce() -> T) -> T {
        let _guard = lock(&self.transcript_import);
        import()
    }

    /// Per-commit publish path: unwatched docs just mark the mirror dirty —
    /// rebuilding a full transcript nobody reads was a per-tick cost on every
    /// open doc (and kept a second transcript copy hot).
    fn publish_messages_if_watched(&self) {
        // Serialize the receiver check AND clear with attach. Otherwise an
        // unwatched worker can clear the mirror after a new watcher rebuilt it.
        let _import = lock(&self.transcript_import);
        if self.messages_tx.receiver_count() == 0 {
            self.mirror_dirty.store(true, Ordering::Release);
            // Shrink the stale mirror: watch_messages rebuilds on attach.
            self.messages_tx.send_replace(TranscriptSnapshot::default());
            *lock(&self.transcript_history) = Default::default();
        } else {
            self.publish_messages_locked();
        }
    }

    /// Rough resident cost for the LRU budget.
    fn resident_estimate(&self) -> usize {
        let bytes = self
            .snapshot_bytes
            .load(Ordering::Relaxed)
            .max(self.persistence.as_ref().map_or(0, |p| p.snapshot_bytes()));
        (bytes * RESIDENT_BYTES_PER_SNAPSHOT_BYTE).max(DOC_RESIDENT_FLOOR_BYTES)
    }
}

impl DocHost {
    pub fn new(store: Arc<DocsStore>, config: DocHostConfig) -> Self {
        let tool_outputs = crate::tool_outputs::ToolOutputs::new(config.tool_outputs.clone());
        let host = Self {
            inner: Arc::new(DocHostInner {
                store,
                config,
                sessions: Mutex::new(None),
                workspace: OnceLock::new(),
                repos: OnceLock::new(),
                project_action_runtime: OnceLock::new(),
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                handles: Mutex::new(HashMap::new()),
                document_loads: AtomicU64::new(0),
                opening: Mutex::new(()),
                drain_waiting: Mutex::new(HashSet::new()),
                uploads: OnceLock::new(),
                executing: Mutex::new(HashSet::new()),
                tool_outputs,
            }),
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            host.spawn_eviction_tick();
            host.spawn_tool_output_sweep();
        }
        host
    }

    /// The archived-chat retention of the tool-output store, hourly.
    fn spawn_tool_output_sweep(&self) {
        let weak = Arc::downgrade(&self.inner);
        self.spawn_worker(async move {
            loop {
                tokio::time::sleep(TOOL_OUTPUT_SWEEP_INTERVAL).await;
                let Some(inner) = weak.upgrade() else { return };
                let host = Self { inner };
                let _ = tokio::task::spawn_blocking(move || host.sweep_tool_outputs()).await;
            }
        });
    }

    /// The warm-doc LRU also runs on a tick: a doc pinned by a viewer or a
    /// run when it was opened becomes evictable only once those let go.
    fn spawn_eviction_tick(&self) {
        let weak = Arc::downgrade(&self.inner);
        self.spawn_worker(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let Some(inner) = weak.upgrade() else { return };
                let host = Self { inner };
                if host.over_budget() {
                    // The pass saves snapshots: keep it off the async workers.
                    let _ = tokio::task::spawn_blocking(move || host.evict_over_budget()).await;
                }
            }
        });
    }

    /// Every background task rides the tracker, raced against the shutdown
    /// token: the loops' own exits stay authoritative in normal operation;
    /// the token is the retirement override.
    fn spawn_worker(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = fut => {}
            }
        });
    }

    /// `spawn_worker` for sites that pre-resolve a runtime handle (callers
    /// reachable from bare sync contexts, where `tasks.spawn` would panic).
    fn spawn_worker_on(
        &self,
        runtime: &tokio::runtime::Handle,
        fut: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn_on(
            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = fut => {}
                }
            },
            runtime,
        );
    }

    /// The sessions engine, once wired. `None` before assembly or after
    /// `shutdown_workers` — callers treat both as "executor unavailable".
    fn sessions(&self) -> Option<SessionsEngine> {
        lock(&self.inner.sessions).clone()
    }

    /// Wire the sessions engine (engine assembly; see `SessionsEngine::set_doc_host`).
    pub fn set_sessions(&self, sessions: SessionsEngine) {
        let statuses = sessions.watch_sessions();
        {
            // First set wins (the OnceLock contract this slot replaced).
            let mut slot = lock(&self.inner.sessions);
            if slot.is_none() {
                *slot = Some(sessions);
            }
        }
        // Commands may already be pending in warm-opened docs.
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
        self.spawn_queue_flush_watcher(statuses);
    }

    /// The turn-end hook for held messages. Doc changes drive `drain_queue` for
    /// everything else, but a turn ENDING is not a doc change the queue can
    /// see — so watch session status instead and re-drain every warm chat.
    /// `drain_queue` is a cheap no-op for empty queues and busy agents, which
    /// is why this can afford to be indiscriminate.
    fn spawn_queue_flush_watcher(
        &self,
        mut statuses: watch::Receiver<Vec<loams_agentd_proto::Session>>,
    ) {
        let host = self.clone();
        self.spawn_worker(async move {
            while statuses.changed().await.is_ok() {
                let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
                for handle in handles {
                    host.drain_queue(&handle).await;
                }
            }
        });
    }

    /// Retire this host's workers (runtime replacement, e.g. sign-out): cancel
    /// and await every spawned task, drop every open chat handle (ending the
    /// weak-keyed room/join loops and watcher streams), and sever the sessions
    /// back-edge so the replaced engine graph can actually drop. Idempotent.
    pub async fn shutdown_workers(&self) {
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
        // Snapshot open docs BEFORE releasing their handles: the handles map
        // holds the only strong doc refs, and an unflushed doc dies with it.
        let host = self.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || host.flush_all()).await {
            tracing::error!(%error, "shutdown snapshot flush failed");
        }
        // Take the map under the lock, drop the handles outside it.
        let handles = std::mem::take(&mut *lock(&self.inner.handles));
        drop(handles);
        lock(&self.inner.sessions).take();
    }

    /// Freeze every open queue before settling live runs during shutdown.
    /// Interrupting a run publishes Idle, which normally wakes the turn-end
    /// queue drainer; without this barrier quitting the host could promote a
    /// queued row in the narrow window before workers are retired.
    pub fn pause_all_queues(&self) {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            handle.queue_paused.store(true, Ordering::Release);
        }
    }

    /// Test-only retirement sentinel: reports true once the doc-host graph
    /// has actually been freed.
    #[doc(hidden)]
    pub fn retirement_probe(&self) -> Box<dyn Fn() -> bool + Send + Sync> {
        let weak = Arc::downgrade(&self.inner);
        Box::new(move || weak.upgrade().is_none())
    }

    /// Wire the repos engine (engine assembly) — worktree materialization for
    /// Run commands carrying a [`loams_agentd_proto::WorktreeSpec`].
    pub fn set_repos(&self, repos: crate::repos::Repos) {
        let _ = self.inner.repos.set(repos);
    }

    pub fn set_project_action_runtime(
        &self,
        project_actions: ProjectActionsStore,
        terminals: Terminals,
    ) {
        let _ = self
            .inner
            .project_action_runtime
            .set((project_actions, terminals));
    }

    /// Wire the uploads store (engine assembly) — `pending://` ref resolution
    /// and the transfer-read jail.
    pub fn set_uploads(&self, uploads: crate::uploads::Uploads) {
        let _ = self.inner.uploads.set(uploads);
    }

    /// Re-evaluate every open chat's commands and queue NOW. Called after an
    /// upload commit lands bytes on this device: a Run or queued row deferred
    /// on those bytes (`pending://` refs not yet on disk) becomes executable
    /// the moment its transfer completes — event-driven, not timer luck.
    pub fn kick_drains(&self) {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
    }

    /// Wire the workspace host (engine assembly) — the source of chat-ownership rows.
    pub fn set_workspace(&self, workspace: WorkspaceHost) {
        let _ = self.inner.workspace.set(workspace);
    }

    /// The workspace host, once wired (tests may assemble a DocHost without one).
    pub fn workspace(&self) -> Option<&WorkspaceHost> {
        self.inner.workspace.get()
    }

    pub fn device_id(&self) -> &str {
        &self.inner.config.device_id
    }

    /// Open (or return) the chat's doc handle: load the local snapshot (or init fresh)
    /// and start the change-driven task.
    pub fn open(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        self.open_local(chat_id)
    }

    /// Materialize one authoritative local document. Durable publication is
    /// installed before exposing any writer.
    pub fn open_local(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        // The registry names the doc lineage: a row that EXISTS without
        // `roomGen` is a legacy fat-doc chat (gen 1); a MISSING row is a chat
        // being born right now (its CreateChat mint stamps roomGen 2 and may
        // race this open), so it starts on the thin lineage.
        let chat_row = self
            .workspace()
            .and_then(|w| w.chat(chat_id).ok().flatten());
        let registry_gen = match chat_row.as_ref() {
            Some(row) => row.room_gen.unwrap_or(1),
            None => 2,
        };
        {
            let mut handles = lock(&self.inner.handles);
            if let Some(handle) = handles.get(chat_id) {
                let stale = registry_gen >= 2 && handle.room_gen < 2;
                if (stale || handle.retired.load(Ordering::Relaxed)) && !self.pinned(handle) {
                    // The lineage changed under a cached fat handle: drop it so
                    // this open converges onto the thin lineage. Retire only at
                    // the drop — a pinned handle keeps serving and persisting.
                    handle.retired.store(true, Ordering::Relaxed);
                    handles.remove(chat_id);
                } else {
                    handle.touch();
                    return Ok(handle.clone());
                }
            }
        }
        let opening = lock(&self.inner.opening);
        if let Some(handle) = lock(&self.inner.handles).get(chat_id) {
            handle.touch();
            return Ok(handle.clone());
        }
        // The LOCAL epoch is the second lineage signal: a stored thin doc
        // (epoch 2) wins over a registry row that still says gen 1, and the
        // row is corrected best-effort.
        self.inner.document_loads.fetch_add(1, Ordering::Relaxed);
        let stored = self.inner.store.load_snapshot_with_cursor(chat_id)?;
        let stored_epoch = stored.as_ref().map(|(_, _, e)| *e).unwrap_or(0);
        let room_gen = if stored_epoch >= CHAT2_DOC_EPOCH {
            if registry_gen < 2
                && let Some(ws) = self.workspace()
            {
                let _ = ws.set_chat_room_gen(chat_id, 2);
                tracing::info!(chat = %chat_id,
                    "completed interrupted lineage flip (local epoch 2, registry said gen 1)");
            }
            2
        } else {
            registry_gen
        };
        // A gen-2 registry row over an older stored doc: keep serving the
        // older doc as it is (its snapshot saves stay on the legacy path).
        let deferred_adoption = room_gen >= 2 && stored_epoch < CHAT2_DOC_EPOCH && stored.is_some();
        let mut snapshot_len = 0usize;
        let mut lineage_cursor = 0u64;
        let doc = match stored {
            Some((bytes, cursor, epoch)) => {
                snapshot_len = bytes.len();
                if room_gen >= 2 && epoch >= CHAT2_DOC_EPOCH {
                    lineage_cursor = cursor;
                }
                let raw = loro::LoroDoc::new();
                raw.import(&bytes)
                    .map_err(|e| EngineError::Other(format!("snapshot import failed: {e}")))?;
                SessionDoc::from_doc(raw)
            }
            None if room_gen >= 2 => {
                // Born on the thin lineage: stamp epoch 2 NOW. Plain snapshot
                // saves preserve an existing row's epoch but default a NEW row
                // to 0, and the next open would then read a pre-lineage doc.
                let doc = SessionDoc::init(chat_id)?;
                if let Ok(snapshot) = doc.export_snapshot() {
                    let _ = self.inner.store.save_snapshot_with_cursor(
                        chat_id,
                        &snapshot,
                        0,
                        CHAT2_DOC_EPOCH,
                    );
                }
                doc
            }
            None => SessionDoc::init(chat_id)?,
        };
        // Recover committed operations an older build queued in the store's
        // outbox before a crash. Imported updates do not echo as local writes.
        if room_gen >= 2 {
            for (_, bytes) in self.inner.store.pending_chat_updates(chat_id)? {
                doc.doc()
                    .import(&bytes)
                    .map_err(|e| EngineError::Other(e.to_string()))?;
            }
        }
        let doc = Arc::new(doc);
        let persistence = (room_gen >= 2 && !deferred_adoption).then(|| {
            crate::chat_persistence::ChatPersistence::new(
                &doc,
                self.inner.store.clone(),
                chat_id.to_string(),
                lineage_cursor,
            )
        });
        let changed_persistence = persistence.clone();

        let (changed_tx, changed_rx) = watch::channel(0u64);
        let (messages_tx, _) = watch::channel(TranscriptSnapshot::default());
        let transcript_history = Arc::new(Mutex::new(
            crate::transcript_history::TranscriptHistory::default(),
        ));
        let history = transcript_history.clone();
        let watched = messages_tx.clone();
        let weak_doc = Arc::downgrade(&doc);
        let sub = doc.doc().subscribe_root(Arc::new(move |diff| {
            // This callback runs before the change worker can publish. The
            // import origin belongs to the event, so concurrent local commits
            // cannot inherit a replay's presentation classification.
            if watched.receiver_count() > 0 {
                if let Some(doc) = weak_doc.upgrade() {
                    lock(&history).observe(doc.doc(), &diff);
                }
            } else {
                *lock(&history) = Default::default();
            }
            if let Some(persistence) = &changed_persistence {
                persistence.dirty(false);
            }
            changed_tx.send_modify(|v| *v = v.wrapping_add(1));
        }));
        // The mirror starts dirty and empty; watch_messages materializes it
        // once on attach instead of maintaining an unwatched transcript.
        let initial_queue = doc.read_queue().unwrap_or_default();
        // A queue already present when a handle is materialized came from a
        // persisted snapshot, not from a prompt the user submitted to this
        // live engine. Keep that recovered work frozen until an explicit
        // prompt / Send now / Steer action thaws it. Rows appended after the
        // handle exists retain the normal automatic drain.
        let recovered_queue_pending = !initial_queue.is_empty();
        let (queue_tx, _) = watch::channel(initial_queue);

        let handle = Arc::new(ChatDocHandle {
            chat_id: chat_id.to_string(),
            device_id: self.inner.config.device_id.clone(),
            doc: doc.clone(),
            messages_tx,
            transcript_import: Mutex::default(),
            transcript_history,
            queue_tx,
            drain_lock: tokio::sync::Mutex::new(()),
            command_drain_lock: tokio::sync::Mutex::new(()),
            steered_rows: Mutex::new(Vec::new()),
            queue_paused: AtomicBool::new(recovered_queue_pending),
            mirror_dirty: AtomicBool::new(true),
            last_access: AtomicI64::new(now_ms()),
            snapshot_bytes: AtomicUsize::new(snapshot_len),
            room_gen,
            retired: AtomicBool::new(false),
            writers: Arc::new(AtomicUsize::new(0)),
            persistence,
            _sub: sub,
        });
        // Snapshot recovery may restore several independently edited rows.
        // Each row needs its own checked expiry wake; otherwise a non-head
        // edit could remain displayed as live indefinitely.
        self.arm_existing_queue_edit_expiries(&handle);
        // Publish only after the change subscription (`_sub`) is installed,
        // so no commit between publication and subscription goes unseen.
        lock(&self.inner.handles).insert(chat_id.to_string(), handle.clone());
        drop(opening);
        self.spawn_worker(chat_task(self.clone(), Arc::downgrade(&handle), changed_rx));
        // Inline, so an open never leaves the warm set over its cap.
        self.evict_over_budget();
        Ok(handle)
    }

    /// LRU eviction: while the warm set exceeds [`WARM_DOC_CAP`] or the
    /// resident estimate exceeds `DOC_LRU_BYTE_BUDGET`, close the
    /// least-recently-touched unpinned docs. Pinned (never evicted):
    /// - watched docs (`messages_tx` has receivers — a UI transcript);
    /// - docs with a live writer (`Arc<SessionDoc>` held outside the handle —
    ///   a run streaming into it);
    /// - host-side docs with pending commands (the executor owes them work).
    ///
    /// Eviction flushes a final snapshot, so reopen loses nothing.
    fn evict_over_budget(&self) {
        // Within budget there is nothing to do: do not queue behind an open.
        if !self.over_budget() {
            return;
        }
        // Do not let a cold reopen race the retiring handle's final flush.
        let _opening = lock(&self.inner.opening);
        let mut by_age: Vec<(i64, String)> = {
            let handles = lock(&self.inner.handles);
            handles
                .values()
                .map(|h| (h.last_access.load(Ordering::Relaxed), h.chat_id.clone()))
                .collect()
        };
        by_age.sort_unstable();
        for (_, chat_id) in by_age {
            let (count, estimate) = {
                let handles = lock(&self.inner.handles);
                (
                    handles.len(),
                    handles
                        .values()
                        .map(|h| h.resident_estimate())
                        .sum::<usize>(),
                )
            };
            if count <= WARM_DOC_CAP && estimate <= loams_agentd_doc::DOC_LRU_BYTE_BUDGET {
                return;
            }
            let evicted = {
                let mut handles = lock(&self.inner.handles);
                match handles.get(&chat_id) {
                    Some(handle) if !self.pinned(handle) && Arc::strong_count(handle) == 1 => {
                        handles.remove(&chat_id)
                    }
                    _ => None,
                }
            };
            if let Some(handle) = evicted {
                // Final flush outside the map lock; ≤1s of changes could be
                // pending in the snapshot debounce.
                self.save_snapshot(&handle);
                tracing::debug!(chat = %handle.chat_id, "doc evicted (LRU)");
            }
        }
    }

    /// The warm set exceeds [`WARM_DOC_CAP`] or the resident estimate
    /// exceeds `DOC_LRU_BYTE_BUDGET`. Takes only the handle map lock.
    fn over_budget(&self) -> bool {
        let handles = lock(&self.inner.handles);
        handles.len() > WARM_DOC_CAP
            || handles
                .values()
                .map(|h| h.resident_estimate())
                .sum::<usize>()
                > loams_agentd_doc::DOC_LRU_BYTE_BUDGET
    }

    /// Run an eviction pass on the blocking pool when over budget (inline
    /// without a runtime): its final snapshot saves are blocking I/O.
    fn schedule_eviction(&self) {
        if !self.over_budget() {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let host = self.clone();
                self.spawn_worker_on(&runtime, async move {
                    let _ = tokio::task::spawn_blocking(move || host.evict_over_budget()).await;
                });
            }
            Err(_) => self.evict_over_budget(),
        }
    }

    fn pinned(&self, handle: &Arc<ChatDocHandle>) -> bool {
        if handle.messages_tx.receiver_count() > 0 || handle.queue_tx.receiver_count() > 0 {
            return true;
        }
        // The handle itself holds one doc ref; more means a live writer.
        if Arc::strong_count(&handle.doc) > 1 {
            return true;
        }
        if self.is_host(&handle.chat_id) {
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            match handle.doc.read_commands() {
                Ok(commands) => commands
                    .iter()
                    .any(|c| c.status == SessionCommandStatus::Pending && !is_processed(&c.id)),
                // Unreadable ledger: keep the doc, never evict blind.
                Err(_) => true,
            }
        } else {
            false
        }
    }

    /// Drop a chat's doc unconditionally and delete its local snapshot — the
    /// chat is gone (DeleteChat / DeleteSpace cascade). Watchers see the
    /// stream end; a racing writer keeps its orphaned doc until the run ends.
    pub fn purge_chat(&self, chat_id: &str) {
        let removed = lock(&self.inner.handles).remove(chat_id);
        drop(removed);
        if let Err(err) = self.inner.store.delete_snapshot(chat_id) {
            tracing::warn!(chat = %chat_id, error = %err, "snapshot delete failed");
        }
        self.inner.tool_outputs.purge_chat(chat_id);
    }

    /// Composer path: append an immutable pending command entry (rule 1). Durable by
    /// construction — the change subscription kicks the drain, so the host executes
    /// it immediately.
    pub fn queue_command(
        &self,
        chat_id: &str,
        payload: SessionCommandPayload,
    ) -> Result<String, EngineError> {
        self.refuse_unsupported(chat_id, &payload)?;
        let handle = self.open(chat_id)?;
        let id = new_id();
        let now = now_ms();
        let based_on = handle.doc.read_entries()?.last().map(|m| CommandBasedOn {
            turn_id: Some(m.id.clone()),
            frontier: None,
        });
        let is_message = matches!(
            payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        );
        let entry = SessionCommandEntry {
            id: id.clone(),
            payload,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now,
            based_on,
            expires_at: Some(now + COMMAND_DEFAULT_TTL_MS),
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        handle.doc.queue_command(&entry)?;
        // Sending a message revives an archived chat: the user is acting in it
        // again, so the LWW row flips back to active on every device. Best-
        // effort — the command itself is durable regardless.
        if is_message {
            self.unarchive_on_send(chat_id);
        }
        Ok(id)
    }

    /// A chat on [`HarnessId::Unsupported`] (an agent this daemon no longer
    /// ships, plan DD1 Task 3) is read-only: a message to it, or a Run that
    /// picks that harness, is refused. Interrupts and input answers pass.
    fn refuse_unsupported(
        &self,
        chat_id: &str,
        payload: &SessionCommandPayload,
    ) -> Result<(), EngineError> {
        let picked = match payload {
            SessionCommandPayload::Run { request, .. } => request.harness,
            SessionCommandPayload::Steer { .. } => None,
            _ => return Ok(()),
        };
        if picked == Some(HarnessId::Unsupported)
            || self.harness_for(chat_id) == HarnessId::Unsupported
        {
            return Err(EngineError::HarnessUnsupported);
        }
        Ok(())
    }

    /// A send revives an archived chat on every device (best-effort).
    fn unarchive_on_send(&self, chat_id: &str) {
        let Some(workspace) = self.workspace() else {
            return;
        };
        match workspace.chat(chat_id) {
            Ok(Some(chat)) if chat.archived => {
                if let Err(err) = workspace.set_chat_archived(chat_id, false) {
                    tracing::warn!(chat = %chat_id, error = %err, "unarchive on send failed");
                }
            }
            _ => {}
        }
    }

    /// Hold a message for later: append it to the doc's queue. Any device may
    /// write here (unlike `messages`), and the change subscription kicks
    /// [`Self::drain_queue`], so a queue that lands while the agent is already
    /// idle goes straight out instead of waiting for a turn that never comes.
    pub fn queue_message(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
    ) -> Result<String, EngineError> {
        self.queue_message_with_behavior(chat_id, text, attachments, false)
    }

    /// Append a message while preserving the submitter's active-turn policy
    /// on the synchronized row. This matters when another device hosts the
    /// chat: the host, not the submitting UI, decides when to drain it.
    pub fn queue_message_with_behavior(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
    ) -> Result<String, EngineError> {
        if self.harness_for(chat_id) == HarnessId::Unsupported {
            return Err(EngineError::HarnessUnsupported);
        }
        let handle = self.open(chat_id)?;
        let id = new_id();
        handle.doc.push_queued(&QueuedMessage {
            id: id.clone(),
            text: text.to_string(),
            attachments,
            hold_for_turn_end,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now_ms(),
            edited_at: None,
            delivery_gate: None,
        })?;
        handle.publish_queue();
        // Same reasoning as a command: the user is acting in this chat again.
        self.unarchive_on_send(chat_id);
        Ok(id)
    }

    /// Retype a queued message. Empty text deletes the row — emptying the box
    /// is how you say "drop it". `false` when the row is already gone.
    pub fn update_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.set_queued_text(id, text, now_ms())?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Reorder a queued message (drag, or the up/down buttons).
    pub fn move_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        to_index: usize,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.move_queued(id, to_index)?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Cancel one queued message at the chat host. Removal shares the same
    /// lock as automatic and explicit delivery, so the acknowledgement is the
    /// linearization point: `true` guarantees this host did not take the row.
    pub async fn remove_queued_message(
        &self,
        chat_id: &str,
        id: &str,
    ) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let removed = handle.doc.remove_queued(id)?;
        if removed {
            handle.publish_queue();
        }
        Ok(removed)
    }

    /// Acquire the host-side right to edit one queued row. This operation and
    /// every queue take share `drain_lock`, making the ACK the linearization
    /// point: after Acquired the row cannot race into the agent.
    pub async fn begin_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        owner_device_id: &str,
        owner_instance_id: &str,
    ) -> Result<BeginQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(BeginQueueEditOutcome::Missing);
        };
        if let Some(QueueDeliveryGate::Editing {
            owner_device_id,
            expires_at_ms,
            ..
        }) = &item.delivery_gate
            && *expires_at_ms > now
        {
            return Ok(BeginQueueEditOutcome::Locked {
                owner_device_id: owner_device_id.clone(),
                expires_at_ms: *expires_at_ms,
            });
        }

        let lease_id = new_id();
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let gate = QueueDeliveryGate::Editing {
            lease_id: lease_id.clone(),
            owner_device_id: owner_device_id.to_string(),
            owner_instance_id: owner_instance_id.to_string(),
            acquired_at_ms: now,
            expires_at_ms,
            base_text_hash: queue_text_hash(&item.text),
        };
        let base_text_hash = queue_text_hash(&item.text);
        if !handle.doc.set_queued_delivery_gate(id, Some(&gate))? {
            return Ok(BeginQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        // A crash immediately after the client sees Acquired must not reopen
        // the row as sendable from a pre-lease snapshot.
        self.save_snapshot(&handle);
        self.arm_queue_edit_expiry(&handle, id, &lease_id, expires_at_ms);
        Ok(BeginQueueEditOutcome::Acquired {
            lease_id,
            text: item.text,
            attachments: item.attachments,
            base_text_hash,
            expires_at_ms,
        })
    }

    /// Extend an edit lease. An already-expired generation is never revived;
    /// it remains blocked and will be surfaced as ReviewRequired.
    pub async fn renew_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
    ) -> Result<RenewQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(RenewQueueEditOutcome::Missing);
        };
        let QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        } = item
            .delivery_gate
            .unwrap_or(QueueDeliveryGate::ReviewRequired {
                previous_lease_id: String::new(),
                owner_device_id: String::new(),
                since_ms: now,
                base_text_hash: String::new(),
            })
        else {
            return Ok(RenewQueueEditOutcome::Lost);
        };
        if current != lease_id || expires_at_ms <= now {
            if current == lease_id && expires_at_ms <= now {
                let review = QueueDeliveryGate::ReviewRequired {
                    previous_lease_id: current,
                    owner_device_id,
                    since_ms: now,
                    base_text_hash,
                };
                let _ = handle.doc.set_queued_delivery_gate(id, Some(&review));
                handle.publish_queue();
                self.save_snapshot(&handle);
            }
            return Ok(RenewQueueEditOutcome::Lost);
        }
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let renewed = QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        };
        if !handle.doc.set_queued_delivery_gate(id, Some(&renewed))? {
            return Ok(RenewQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        self.arm_queue_edit_expiry(&handle, id, lease_id, expires_at_ms);
        Ok(RenewQueueEditOutcome::Renewed { expires_at_ms })
    }

    /// Resolve an edit lease. A late finish may still resolve the matching
    /// ReviewRequired generation, but can never affect a newer lease.
    pub async fn finish_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        self.finish_queued_message_edit_with_attachments(
            chat_id,
            id,
            lease_id,
            action,
            text,
            expected_text_hash,
            None,
        )
        .await
    }

    pub async fn finish_queued_message_edit_with_attachments(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
        attachments: Option<&[String]>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        if action == FinishQueueEditAction::Commit
            && (text.is_none() || expected_text_hash.is_none())
        {
            return Err(EngineError::Other(
                "commit requires text and expectedTextHash".into(),
            ));
        }
        let handle = self.open(chat_id)?;
        let outcome = {
            let _drain = handle.drain_lock.lock().await;
            let Some(item) = handle
                .doc
                .read_queue()?
                .into_iter()
                .find(|item| item.id == id)
            else {
                return Ok(FinishQueueEditOutcome::Missing);
            };
            let (current_lease, base_text_hash) = match &item.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    base_text_hash,
                    ..
                }) => (lease_id, base_text_hash),
                Some(QueueDeliveryGate::ReviewRequired {
                    previous_lease_id,
                    base_text_hash,
                    ..
                }) => (previous_lease_id, base_text_hash),
                None => return Ok(FinishQueueEditOutcome::Lost),
            };
            if current_lease != lease_id {
                return Ok(FinishQueueEditOutcome::Lost);
            }
            if action == FinishQueueEditAction::Commit
                && (expected_text_hash != Some(base_text_hash.as_str())
                    || queue_text_hash(&item.text) != *base_text_hash)
            {
                return Ok(FinishQueueEditOutcome::Conflict {
                    current_text: item.text,
                });
            }

            let replacement = match action {
                FinishQueueEditAction::Commit => Some(text.unwrap_or_default()),
                FinishQueueEditAction::Cancel | FinishQueueEditAction::ReleaseUnchanged => None,
                FinishQueueEditAction::Discard => Some(""),
            };
            if !handle.doc.finish_queued_edit_with_attachments(
                id,
                replacement,
                if action == FinishQueueEditAction::Commit {
                    attachments
                } else {
                    None
                },
                now_ms(),
            )? {
                return Ok(FinishQueueEditOutcome::Missing);
            }
            handle.publish_queue();
            self.save_snapshot(&handle);
            match action {
                FinishQueueEditAction::Commit => FinishQueueEditOutcome::Committed,
                FinishQueueEditAction::Cancel => FinishQueueEditOutcome::Cancelled,
                FinishQueueEditAction::Discard => FinishQueueEditOutcome::Discarded,
                FinishQueueEditAction::ReleaseUnchanged => FinishQueueEditOutcome::Released,
            }
        };
        // Turn-end may already have happened while the edit was open.
        self.drain_queue(&handle).await;
        Ok(outcome)
    }

    fn arm_existing_queue_edit_expiries(&self, handle: &Arc<ChatDocHandle>) {
        let Ok(queue) = handle.doc.read_queue() else {
            return;
        };
        for item in queue {
            if let Some(QueueDeliveryGate::Editing {
                lease_id,
                expires_at_ms,
                ..
            }) = item.delivery_gate
            {
                self.arm_queue_edit_expiry(handle, &item.id, &lease_id, expires_at_ms);
            }
        }
    }

    fn arm_queue_edit_expiry(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        expires_at_ms: i64,
    ) {
        let delay_ms = expires_at_ms.saturating_sub(now_ms()).max(0) as u64;
        let host = self.clone();
        let handle = handle.clone();
        let id = id.to_string();
        let lease_id = lease_id.to_string();
        self.spawn_worker(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            host.expire_queued_message_edit(&handle, &id, &lease_id, expires_at_ms)
                .await;
        });
    }

    /// Expire exactly the lease generation that scheduled this wake. A stale
    /// timer from before a renewal observes a different deadline and no-ops;
    /// timers for other rows are completely independent.
    async fn expire_queued_message_edit(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        scheduled_expires_at_ms: i64,
    ) {
        let changed = {
            let _drain = handle.drain_lock.lock().await;
            let Ok(queue) = handle.doc.read_queue() else {
                return;
            };
            let Some(item) = queue.into_iter().find(|item| item.id == id) else {
                return;
            };
            let Some(QueueDeliveryGate::Editing {
                lease_id: current_lease_id,
                owner_device_id,
                expires_at_ms,
                base_text_hash,
                ..
            }) = item.delivery_gate
            else {
                return;
            };
            if current_lease_id != lease_id
                || expires_at_ms != scheduled_expires_at_ms
                || expires_at_ms > now_ms()
            {
                return;
            }
            let review = QueueDeliveryGate::ReviewRequired {
                previous_lease_id: current_lease_id,
                owner_device_id,
                since_ms: now_ms(),
                base_text_hash,
            };
            let Ok(changed) = handle.doc.set_queued_delivery_gate(id, Some(&review)) else {
                return;
            };
            if changed {
                handle.publish_queue();
                self.save_snapshot(handle);
            }
            changed
        };
        if changed {
            // If this was the head, the drain now observes ReviewRequired. If
            // it was not, publishing still updates every client's row state.
            self.drain_queue(handle).await;
        }
    }

    /// "Send this one now": take it out of the queue and put it in front of the
    /// agent, interrupting whatever is running. Deliberately blunt — it is the
    /// explicit override. The empty-composer Enter gesture reaches this path
    /// only when the selected provider cannot steer the row. `false` when
    /// another device already took it.
    pub async fn send_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        // Sending one now sends ONE: the lock keeps the flush out of the idle
        // window the interrupt opens, and out of the take itself.
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        if let Err(err) = self
            .dispatch_queued(&handle, &item, QueueSend::Interrupt)
            .await
        {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            // Put it back rather than swallowing what the user typed — at the
            // head, because the user just said this one was the urgent one.
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Promote one held row without ever interrupting a turn. A live,
    /// steerable turn receives it as steering; if that turn has already ended,
    /// it starts normally as the next turn. Turn-boundary providers retain it
    /// in their mailbox until ready. Attachment-bearing rows stay untouched.
    pub async fn steer_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        if !candidate.attachments.is_empty() {
            return Err(EngineError::Other(
                "messages with attachments cannot be steered mid-turn".into(),
            ));
        }
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        if self
            .sessions()
            .is_some_and(|sessions| sessions.defers_to_turn_end(chat_id, None))
        {
            // Send next: lead the ordinary rows; the drain delivers it the
            // moment the current turn ends.
            let Some(item) = handle.doc.take_queued(id)? else {
                return Ok(false);
            };
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
            handle.queue_paused.store(false, Ordering::Release);
            handle.publish_queue();
            return Ok(true);
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        // Always attempt the non-interrupting path first. If no turn exists,
        // `dispatch_queued` falls through to NextTurn; if a new turn appeared
        // after our capability check, the prompt steers that turn instead.
        if let Err(err) = self.dispatch_queued(&handle, &item, QueueSend::Steer).await {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Host-only: hand queued messages to the agent when there is somewhere to
    /// put them.
    ///
    /// - Idle: send the head as the next turn (and loop — the agent is free).
    /// - Turn in flight: hold. The turn-end watcher comes back for it.
    ///
    /// One at a time by design: each send changes the status this reads.
    pub async fn drain_queue(&self, handle: &Arc<ChatDocHandle>) {
        let Some(sessions) = self.sessions() else {
            return; // executor not wired yet; the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        // One drain at a time per chat. Waiters are cheap: whoever takes the
        // lock next re-reads the queue and the status, so a drain that became
        // unnecessary while it waited simply finds nothing to do.
        let _drain = handle.drain_lock.lock().await;
        if handle.queue_paused.load(Ordering::Acquire) {
            return;
        }
        loop {
            let Ok(Some(head)) = handle.doc.read_queue().map(|q| q.into_iter().next()) else {
                return;
            };
            match &head.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    owner_device_id,
                    expires_at_ms,
                    base_text_hash,
                    ..
                }) if *expires_at_ms <= now_ms() => {
                    let review = QueueDeliveryGate::ReviewRequired {
                        previous_lease_id: lease_id.clone(),
                        owner_device_id: owner_device_id.clone(),
                        since_ms: now_ms(),
                        base_text_hash: base_text_hash.clone(),
                    };
                    let _ = handle.doc.set_queued_delivery_gate(&head.id, Some(&review));
                    handle.publish_queue();
                    self.save_snapshot(handle);
                    return;
                }
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    expires_at_ms,
                    ..
                }) => {
                    self.arm_queue_edit_expiry(handle, &head.id, lease_id, *expires_at_ms);
                    return;
                }
                Some(QueueDeliveryGate::ReviewRequired { .. }) => return,
                None => {}
            }
            // In flight, not just Working: an agent parked on a question owns
            // the turn too, and the composer queues on the same reading. Taking
            // `AwaitingInput` for idle would send the follow-up as a fresh turn
            // and abandon the question.
            if sessions.turn_in_flight(&handle.chat_id) {
                return; // All queued messages wait, including rows from older clients.
            }
            // A row from another device names its images by `pending://` ref
            // while the bytes chase it over the peer link. Hold it (in order)
            // until they land — UploadCommit re-drains — rather than handing
            // the agent refs it cannot open.
            if !self.missing_row_attachments(&head).is_empty() {
                tracing::info!(chat = %handle.chat_id, row = %head.id,
                    "queued row held: attachment bytes in transit");
                self.arm_attachment_wait(handle);
                return;
            }
            let send = QueueSend::NextTurn;
            // Take it only once we know it is going out — a row that stays in
            // the queue on a failed send is recoverable; a vanished one is not.
            let Ok(Some(item)) = handle.doc.take_queued(&head.id) else {
                return;
            };
            lock(&handle.steered_rows).retain(|row| row != &item.id);
            handle.publish_queue();
            if let Err(err) = self.dispatch_queued(handle, &item, send).await {
                tracing::warn!(chat = %handle.chat_id, error = %err, "queued send failed");
                handle.queue_paused.store(true, Ordering::Release);
                let _ = handle.doc.insert_queued(0, &item);
                handle.publish_queue();
                return;
            }
        }
    }

    /// Stop the active turn without treating the resulting Idle transition as
    /// permission to release the next queued message. The same lock used by
    /// drains closes the race between clicking Cancel and the status watcher.
    async fn interrupt_and_pause_queue(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
    ) -> Result<bool, EngineError> {
        let _drain = handle.drain_lock.lock().await;
        if !sessions.turn_in_flight(&handle.chat_id) {
            return Ok(false);
        }
        handle.queue_paused.store(true, Ordering::Release);
        match sessions.interrupt(&handle.chat_id).await {
            Ok(true) => Ok(true),
            Ok(false) => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok(false)
            }
            Err(err) => {
                handle.queue_paused.store(false, Ordering::Release);
                Err(err)
            }
        }
    }

    /// Send one taken queue row.
    async fn dispatch_queued(
        &self,
        handle: &Arc<ChatDocHandle>,
        item: &QueuedMessage,
        send: QueueSend,
    ) -> Result<(), EngineError> {
        let Some(sessions) = self.sessions() else {
            return Err(EngineError::Other("sessions engine not wired".into()));
        };
        let chat_id = &handle.chat_id;
        // Keep the queue row's identity when it becomes a real user message.
        // The submitting viewport learns this id from QueueMessage and can
        // therefore wait without disturbing the active turn's runway, then
        // anchor the prompt only once this exact row reaches the transcript.
        let message_id = item.id.clone();
        if !self.missing_row_attachments(item).is_empty() {
            return Err(EngineError::Other(
                "this message's images are still uploading".into(),
            ));
        }
        let mut attachments = item.attachments.clone();
        let mut prompt = queued_message_prompt(&item.text, &attachments);
        self.resolve_attachment_refs(&mut prompt, &mut attachments);
        if send == QueueSend::Steer {
            match sessions
                .steer(chat_id, &prompt, Some(message_id.clone()))
                .await?
            {
                SteerOutcome::Accepted => return Ok(()),
                SteerOutcome::DeferredByUpdate => {
                    return Err(EngineError::Other(
                        "agent update pending; the message remains queued".into(),
                    ));
                }
                // The run died under us between the status read and the send;
                // fall through and start a fresh turn with it.
                SteerOutcome::NotSteerable => {}
            }
        }
        // Same reading of "busy" as the drain: a turn parked on a question is
        // still a turn, and it has to be stopped before this one starts.
        if send == QueueSend::Interrupt && sessions.turn_in_flight(chat_id) {
            sessions.interrupt(chat_id).await?;
        }
        let previous = sessions.last_request(chat_id);
        let request = self
            .request_from_chat_row(chat_id, &prompt)
            .map(|mut current| {
                if let Some(previous) = &previous {
                    current.auto_approve = previous.auto_approve;
                    current.worktree = previous.worktree.clone();
                }
                current
            })
            .or(previous);
        let Some(mut request) = request else {
            return Err(EngineError::Other(
                "no live run and no prior run config".into(),
            ));
        };
        request.prompt = prompt;
        request.resume = None; // dispatch re-derives the harness session
        request.attachments = attachments;
        let harness = self.harness_for_request(chat_id, &request);
        self.dispatch_with_source_context(&sessions, chat_id, harness, request, Some(message_id))
            .await?;
        Ok(())
    }

    /// Keep a tool result's full output/diff (or a frozen subagent
    /// transcript) in the local tool-output store, keyed `{chatId}/{partId}`
    /// (`.diff` for the diff JSON) — the refs `apply_sidecar_refs` stamps.
    /// Fire-and-forget: the doc already carries the summary, so a failed
    /// write degrades to "full output unavailable" and never blocks the run.
    pub fn store_tool_output(&self, chat_id: &str, payload: loams_agentd_doc::SidecarPayload) {
        let outputs = self.inner.tool_outputs.clone();
        let chat_id = chat_id.to_string();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => self.spawn_worker_on(&runtime, async move {
                let _ =
                    tokio::task::spawn_blocking(move || outputs.write(&chat_id, &payload)).await;
            }),
            Err(_) => outputs.write(&chat_id, &payload),
        }
    }

    /// Read one page of a stored tool output by its doc-resident ref
    /// (`{chatId}/{partId}` or `….diff`), `len` bytes from `offset` (at most
    /// [`crate::tool_outputs::TOOL_OUTPUT_MAX_FRAME`]) — the UI's lazy "Show
    /// full output" path.
    pub async fn fetch_tool_blob(
        &self,
        blob_ref: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<crate::tool_outputs::ToolOutputChunk, EngineError> {
        let outputs = self.inner.tool_outputs.clone();
        let blob_ref = blob_ref.to_string();
        tokio::task::spawn_blocking(move || outputs.read(&blob_ref, offset, len))
            .await
            .map_err(|e| EngineError::Other(format!("tool output read failed: {e}")))?
    }

    /// Bound the tool-output store (`EngineConfig::tool_outputs`).
    pub fn set_tool_output_limits(&self, limits: crate::tool_outputs::ToolOutputLimits) {
        self.inner.tool_outputs.set_limits(limits);
    }

    /// Delete archived chats' tool outputs older than the retention. Runs
    /// hourly on the blocking pool; a no-op before the workspace is wired.
    pub fn sweep_tool_outputs(&self) {
        let Some(workspace) = self.workspace() else {
            return;
        };
        let archived: Vec<String> = workspace
            .read_chats()
            .unwrap_or_default()
            .into_iter()
            .filter(|chat| chat.archived)
            .map(|chat| chat.id)
            .collect();
        if !archived.is_empty() {
            self.inner
                .tool_outputs
                .sweep_archived(&archived, std::time::SystemTime::now());
        }
    }

    /// §2.2 writer discipline: we host a chat iff its workspace row's `deviceId` is
    /// ours; a chat with no row is claimable (claim-on-first-command). Without a
    /// wired workspace host (bare-DocHost tests) every open chat is ours — M2's
    /// behavior, now the degenerate case.
    fn is_host(&self, chat_id: &str) -> bool {
        self.workspace().is_none_or(|ws| ws.is_host(chat_id))
    }

    /// Chat-config harness when the workspace row carries one, else the default.
    pub(crate) fn harness_for(&self, chat_id: &str) -> HarnessId {
        self.workspace()
            .and_then(|ws| ws.chat_config(chat_id))
            .map(|config| config.harness)
            .unwrap_or(self.inner.config.default_harness)
    }

    /// The harness a request dispatches on: the request's own pick when it
    /// carries one (rides the command plane, immune to registry-row races),
    /// else [`Self::harness_for`].
    pub(crate) fn harness_for_request(
        &self,
        chat_id: &str,
        request: &loams_agentd_proto::RunRequest,
    ) -> HarnessId {
        request.harness.unwrap_or_else(|| self.harness_for(chat_id))
    }

    /// User-driven retry (the failed-send affordance, `RetryDelivery`):
    /// re-send every dead Run/Steer attempt under a fresh id, then run a
    /// drain pass. Exactly-once is per command id, so a retry mints a new
    /// attempt with the same payload and message id (the executor's
    /// user-entry pre-write dedupes by message id). A dead attempt is one
    /// whose user message never landed and that can never execute again:
    /// Rejected (execute failed, or the dead-command sweep terminalized it),
    /// Expired (an explicit retry is the consent to re-send), or consumed by
    /// the ledger with no outcome and not executing (a crash between mark
    /// and resolve). One re-issue per message: the latest attempt speaks for
    /// it. The fork's redial and host nudge went with the edge (plan DD1
    /// ruling T2-12).
    pub fn retry_delivery(&self, chat_id: &str) -> Result<(), EngineError> {
        let handle = self.open(chat_id)?;
        let commands = handle.doc.read_commands()?;
        let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
        let messages = handle.doc.read_entries().unwrap_or_default();
        let message_landed = |mid: &str| messages.iter().any(|m| m.id == mid);
        let mut latest_dead: HashMap<String, &SessionCommandEntry> = HashMap::new();
        for c in &commands {
            let Some(mid) = retry_message_id(&c.payload) else {
                continue;
            };
            if message_landed(mid) {
                continue;
            }
            let dead = match c.status {
                SessionCommandStatus::Rejected | SessionCommandStatus::Expired => true,
                SessionCommandStatus::Pending => {
                    is_processed(&c.id) && !lock(&self.inner.executing).contains(&c.id)
                }
                _ => false,
            };
            if !dead {
                continue;
            }
            // A live pending attempt for the same message makes a re-issue
            // a duplicate.
            let live_attempt = commands.iter().any(|o| {
                o.id != c.id
                    && o.status == SessionCommandStatus::Pending
                    && !is_processed(&o.id)
                    && same_send(&o.payload, &c.payload)
            });
            if live_attempt {
                continue;
            }
            match latest_dead.entry(mid.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if c.issued_at > slot.get().issued_at {
                        slot.insert(c);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(c);
                }
            }
        }
        for old in latest_dead.values() {
            if old.status == SessionCommandStatus::Pending {
                // Terminalize the consumed-but-dead original so the doc tells
                // the truth and the next retry does not see it again.
                self.resolve_command(
                    &handle,
                    &old.id,
                    SessionCommandStatus::Rejected,
                    Some("interrupted before completion — superseded by retry"),
                );
            }
            let now = now_ms();
            let reissue = SessionCommandEntry {
                id: new_id(),
                payload: old.payload.clone(),
                issued_by: self.inner.config.device_id.clone(),
                issued_at: now,
                based_on: messages.last().map(|m| CommandBasedOn {
                    turn_id: Some(m.id.clone()),
                    frontier: None,
                }),
                expires_at: Some(now + COMMAND_DEFAULT_TTL_MS),
                status: SessionCommandStatus::Pending,
                resolution: None,
            };
            tracing::info!(chat = %chat_id, old = %old.id, new = %reissue.id,
                "retry re-issues a dead send attempt");
            handle.doc.queue_command(&reissue)?;
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            let host = self.clone();
            let handle = handle.clone();
            self.spawn_worker(async move { host.drain_commands(&handle).await });
        }
        Ok(())
    }

    /// Drain pending commands (host-only): evaluate → mark processed BEFORE execute →
    /// execute → write the outcome as the sole outcome writer.
    pub async fn drain_commands(&self, handle: &Arc<ChatDocHandle>) {
        self.drain_command_kind(handle, false).await;
    }

    async fn drain_command_kind(&self, handle: &Arc<ChatDocHandle>, controls_only: bool) {
        let Some(sessions) = self.sessions() else {
            return; // executor not wired yet (or retired); the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        // Do not let another drain overtake a prompt waiting for mailbox
        // capacity (or preflight). Controls bypass this lock so a stalled
        // provider can still be interrupted or have its question answered.
        let mut prompt_guard = if controls_only {
            None
        } else {
            handle.command_drain_lock.try_lock().ok()
        };
        // Entries this pass decided to leave alone (processed dedupe hits).
        let mut skipped: HashSet<String> = HashSet::new();
        loop {
            let commands = match handle.doc.read_commands() {
                Ok(commands) => commands,
                Err(err) => {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "command read failed");
                    return;
                }
            };
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            // Dead-command sweep: Pending in the doc, consumed by the ledger,
            // and NOT mid-execution in this process — the crash window
            // between mark-processed and the outcome write. Left alone it is
            // a send no drain or retry can ever reach ("Sending…" forever,
            // 2026-08-19); terminalize it so the truth lands in the doc and
            // a user retry can mint a fresh attempt.
            for c in &commands {
                if c.status == SessionCommandStatus::Pending
                    && !skipped.contains(&c.id)
                    && is_processed(&c.id)
                    && !lock(&self.inner.executing).contains(&c.id)
                {
                    tracing::warn!(chat = %handle.chat_id, command = %c.id,
                        "command consumed but never resolved (crash mid-execute?); rejecting");
                    self.resolve_command(
                        handle,
                        &c.id,
                        SessionCommandStatus::Rejected,
                        Some("interrupted before completion — retry to send again"),
                    );
                    skipped.insert(c.id.clone());
                }
            }
            let Some(entry) = commands
                .iter()
                .find(|c| {
                    c.status == SessionCommandStatus::Pending
                        && !skipped.contains(&c.id)
                        && !is_processed(&c.id)
                        && (prompt_guard.is_some()
                            || !matches!(
                                c.payload,
                                SessionCommandPayload::Run { .. }
                                    | SessionCommandPayload::Steer { .. }
                            ))
                })
                .cloned()
            else {
                if prompt_guard.is_none() && !controls_only {
                    // Wait after handling controls, then re-read: simply
                    // returning here could miss a newly appended prompt.
                    prompt_guard = Some(handle.command_drain_lock.lock().await);
                    continue;
                }
                return;
            };
            let messages = handle.doc.read_entries().unwrap_or_default();
            let current_turn_id = messages.last().map(|m| m.id.clone());
            let turn_is_past = |turn_id: &str| messages.iter().any(|m| m.id == turn_id);
            let disposition = evaluate_command(
                &entry,
                &EvaluationContext {
                    is_processed: &is_processed,
                    now_ms: now_ms(),
                    entries: &commands,
                    current_turn_id: current_turn_id.as_deref(),
                    turn_is_past: &turn_is_past,
                },
            );
            // Queued-attachment gate (BEFORE the processed mark — a deferred
            // command must stay eligible): a Run/Steer naming `pending://`
            // refs whose bytes haven't landed on this device yet waits for
            // the transfer instead of running without its images. The wait is
            // bounded; past it the command fails loudly.
            if matches!(disposition, CommandDisposition::Execute) {
                let missing = self.missing_attachments(&entry);
                if !missing.is_empty() {
                    if now_ms().saturating_sub(entry.issued_at) < ATTACHMENT_WAIT_MAX_MS {
                        tracing::info!(chat = %handle.chat_id, command = %entry.id,
                            missing = missing.len(), "command deferred: attachment bytes in transit");
                        self.arm_attachment_wait(handle);
                        return; // preserve order; UploadCommit / the wait timer re-kick
                    }
                    if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                        tracing::error!(chat = %handle.chat_id, error = %err,
                            "processed-ledger write failed; halting drain");
                        return;
                    }
                    tracing::warn!(chat = %handle.chat_id, command = %entry.id,
                        "command rejected: attachments never arrived");
                    self.resolve_command(
                        handle,
                        &entry.id,
                        SessionCommandStatus::Rejected,
                        Some("attachments never arrived"),
                    );
                    continue;
                }
            }
            // In-flight claim: guards the dead-command sweep (an id in
            // `executing` is alive, not crashed) and serializes racing
            // drains on the same entry.
            if !lock(&self.inner.executing).insert(entry.id.clone()) {
                skipped.insert(entry.id.clone());
                continue;
            }
            // Mark BEFORE executing: a crash mid-execution must never double-run a
            // command whose side effect may already have happened.
            if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                tracing::error!(chat = %handle.chat_id, error = %err, "processed-ledger write failed; halting drain");
                lock(&self.inner.executing).remove(&entry.id);
                return;
            }
            match disposition {
                CommandDisposition::Skip => {
                    skipped.insert(entry.id.clone());
                }
                CommandDisposition::Expired => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Expired, None);
                }
                CommandDisposition::Superseded => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Superseded, None);
                }
                CommandDisposition::Execute => {
                    let (status, resolution) = match self.execute(&sessions, handle, &entry).await {
                        Ok(outcome) => outcome,
                        Err(err) => (SessionCommandStatus::Rejected, Some(err.to_string())),
                    };
                    self.resolve_command(handle, &entry.id, status, resolution.as_deref());
                }
            }
            lock(&self.inner.executing).remove(&entry.id);
        }
    }

    /// The command's `pending://` attachment refs whose bytes are NOT on this
    /// device's disk yet. Empty when the command names none, when everything
    /// has landed, or when no uploads store is wired (tests) — absence of the
    /// subsystem must never wedge a queue.
    fn missing_attachments(&self, entry: &SessionCommandEntry) -> Vec<String> {
        let refs: Vec<String> = match &entry.payload {
            SessionCommandPayload::Run { request, .. } => request
                .attachments
                .iter()
                .filter(|p| crate::uploads::is_pending_ref(p))
                .cloned()
                .collect(),
            SessionCommandPayload::Steer { prompt, .. } => crate::uploads::pending_refs_in(prompt),
            _ => Vec::new(),
        };
        if refs.is_empty() {
            return refs;
        }
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// Arm (once per chat) the deferred-command re-check loop: while a
    /// pending unprocessed command still waits on attachment bytes, re-drain
    /// on a cadence so the bounded wait actually expires even if every
    /// event-driven kick was missed.
    fn arm_attachment_wait(&self, handle: &Arc<ChatDocHandle>) {
        let chat = handle.chat_id.clone();
        if !lock(&self.inner.drain_waiting).insert(chat.clone()) {
            return;
        }
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        self.spawn_worker(async move {
            loop {
                tokio::time::sleep(ATTACHMENT_WAIT_RECHECK).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
            }
            lock(&host.inner.drain_waiting).remove(&chat);
        });
    }

    /// True while some pending, unprocessed command still waits on bytes.
    fn awaiting_attachments(&self, handle: &Arc<ChatDocHandle>) -> bool {
        let commands = handle.doc.read_commands().unwrap_or_default();
        let command_waiting = commands.iter().any(|c| {
            c.status == SessionCommandStatus::Pending
                && !self.inner.store.is_processed(&c.id).unwrap_or(false)
                && !self.missing_attachments(c).is_empty()
        });
        command_waiting
            || handle
                .doc
                .read_queue()
                .ok()
                .and_then(|q| q.into_iter().next())
                .is_some_and(|head| !self.missing_row_attachments(&head).is_empty())
    }

    /// Rewrite a request's landed `pending://` refs to this device's absolute
    /// paths — in the attachments list AND the prompt text — so the harness
    /// (and the persisted user entry) see ordinary local files, exactly like
    /// the legacy pre-upload flow produced.
    fn resolve_request_attachments(&self, request: &mut loams_agentd_proto::RunRequest) {
        self.resolve_attachment_refs(&mut request.prompt, &mut request.attachments);
    }

    /// Rewrite landed `pending://` refs in `attachments` (and wherever the
    /// prompt names them) to this device's absolute paths.
    fn resolve_attachment_refs(&self, prompt: &mut String, attachments: &mut [String]) {
        let Some(uploads) = self.inner.uploads.get() else {
            return;
        };
        for path in attachments.iter_mut() {
            if let Some(abs) = uploads.resolve_pending(path) {
                *prompt = prompt.replace(path.as_str(), &abs);
                *path = abs;
            }
        }
    }

    /// A queue row's `pending://` refs whose bytes are not on this device
    /// yet ([`Self::missing_attachments`] for rows). Covers refs in the
    /// row's text too: older clients inlined the attachment trailer.
    fn missing_row_attachments(&self, item: &QueuedMessage) -> Vec<String> {
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        let mut refs: Vec<String> = item
            .attachments
            .iter()
            .filter(|p| crate::uploads::is_pending_ref(p))
            .cloned()
            .collect();
        for r in crate::uploads::pending_refs_in(&item.text) {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// [`Self::resolve_request_attachments`] for a bare prompt (Steer).
    fn resolve_prompt_attachments(&self, prompt: &str) -> String {
        let Some(uploads) = self.inner.uploads.get() else {
            return prompt.to_string();
        };
        let mut out = prompt.to_string();
        for r in crate::uploads::pending_refs_in(prompt) {
            if let Some(abs) = uploads.resolve_pending(&r) {
                out = out.replace(&r, &abs);
            }
        }
        out
    }

    /// Host-only outcome write (ledger rule 2).
    fn resolve_command(
        &self,
        handle: &ChatDocHandle,
        command_id: &str,
        status: SessionCommandStatus,
        resolution: Option<&str>,
    ) {
        if let Err(err) = handle
            .doc
            .set_command_status(command_id, status, resolution)
        {
            tracing::warn!(
                chat = %handle.chat_id,
                command = %command_id,
                error = %err,
                "command outcome write failed"
            );
        }
    }

    async fn execute(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        // A command an older build left in the doc of a now-unsupported chat.
        self.refuse_unsupported(chat_id, &entry.payload)?;
        match &entry.payload {
            SessionCommandPayload::Run {
                request,
                message_id,
            } => {
                let mut request = request.clone();
                // Queued-attachment refs (`pending://`) resolve to this
                // host's absolute paths before anything persists or
                // dispatches — the drain already gated on the bytes being
                // present, so every ref resolves here.
                self.resolve_request_attachments(&mut request);
                // Worktree directive (WorktreeSpec): materialize on THIS host at
                // drain time — the durable command plane replaces the sender's
                // old blocking CreateWorktree relay RPC, whose lost reply wedged
                // the composer on "Sending…" while the run proceeded anyway.
                // `take()` resolves the request before dispatch, so the journal
                // and steer→new-turn fallbacks reuse the created path instead of
                // minting another checkout.
                let worktree_spec = request.worktree.take();
                let fresh_worktree = match &worktree_spec {
                    Some(spec) => {
                        let (cwd, fresh) = self.materialize_worktree(chat_id, spec).await?;
                        request.cwd = cwd;
                        fresh
                    }
                    None => None,
                };
                // Claim-on-first-command: a run for a chat with no workspace row
                // creates the row under our device id (we are about to host it).
                if let Some(ws) = self.workspace() {
                    ws.claim_chat(chat_id, Some(&request.cwd))?;
                    // A pre-existing row (the client's createChat raced ahead)
                    // still carries the repo folder — repoint it at the fresh
                    // worktree, and stamp the actual `loams-desktop/<name>` branch so
                    // the footer and the title-rename flow see it.
                    if let Some(wt) = &fresh_worktree {
                        if let Err(err) = ws.set_chat_cwd(chat_id, &wt.path) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree cwd stamp failed");
                        }
                        if let Err(err) = ws.set_chat_branch(chat_id, &wt.branch) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree branch stamp failed");
                        }
                    }
                }
                if let Some(spec) = worktree_spec.as_ref()
                    && spec.space_id.is_some()
                {
                    self.complete_worktree_setup_handoff(
                        &entry.id,
                        chat_id,
                        spec,
                        fresh_worktree.as_ref(),
                    );
                }
                let harness = self.harness_for_request(chat_id, &request);
                // A row with no config renders no harness glyph (and every
                // later dispatch falls back to the engine default), so stamp
                // what this run actually executes with. Claimed rows and
                // catalog-not-loaded createChats both land here; the racing
                // real createChat carries the same picked values.
                if let Some(ws) = self.workspace()
                    && ws.chat_config(chat_id).is_none()
                {
                    let config = loams_agentd_proto::ChatConfig {
                        harness,
                        model: request.model.clone(),
                        reasoning: request.reasoning,
                        model_options: request.model_options.clone(),
                        sandbox: request.sandbox,
                    };
                    if let Err(err) = ws.set_chat_config(chat_id, &config) {
                        tracing::warn!(chat = %chat_id, error = %err, "run-config backfill failed");
                    }
                }
                if sessions.defers_to_turn_end(chat_id, Some((harness, &request))) {
                    self.hold_until_turn_end(
                        handle,
                        message_id,
                        &request.prompt,
                        entry.issued_at,
                        false,
                    )?;
                    return Ok((
                        SessionCommandStatus::Applied,
                        Some("held until the turn ends".into()),
                    ));
                }
                // Timestamp canonicalization: the user message lands in
                // history at the moment the user SENT it (the entry's
                // issued_at, clamped against clock skew) — not whenever this
                // host got around to draining a queued command. Idempotent by
                // id, so the dispatch path's own execution-time write dedupes
                // to a no-op.
                if let Err(err) = handle.write_user_message(
                    message_id,
                    &request.prompt,
                    entry.issued_at.min(now_ms()),
                ) {
                    tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
                }
                self.dispatch_with_source_context(
                    sessions,
                    chat_id,
                    harness,
                    request,
                    Some(message_id.clone()),
                )
                .await?;
                // A fresh user-authored turn is the deliberate action that
                // thaws a queue frozen by Cancel. Clear only after dispatch
                // succeeds so a failed send cannot silently unfreeze it.
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::Steer { prompt, message_id } => {
                self.deliver_prompt(
                    sessions,
                    handle,
                    prompt,
                    message_id.clone(),
                    entry.issued_at,
                )
                .await
            }
            SessionCommandPayload::Interrupt {} => {
                self.interrupt_and_pause_queue(sessions, handle).await?;
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::RespondInput {
                request_id,
                answers,
            } => {
                if sessions.respond_input(chat_id, request_id, answers.clone())? {
                    return Ok((SessionCommandStatus::Applied, None));
                }
                // No live resolver. Only a request id the doc shows as an
                // OPEN question on a SETTLED entry gets the orphan fallback:
                // a mismatched or already-resolved id is a stale/buggy answer
                // and must still reject, and a still-streaming entry's
                // question belongs to the live run (a just-consumed resolver
                // racing a second answer must not spawn a duplicate turn).
                let questions = handle.doc.read_entries().ok().and_then(|entries| {
                    entries
                        .iter()
                        .rev()
                        .filter(|e| e.status != Some(MessageStatus::Streaming))
                        .find_map(|e| {
                            e.parts.iter().find_map(|p| match p {
                                MessagePart::Input {
                                    request_id: rid,
                                    questions,
                                    resolved: false,
                                    ..
                                } if rid == request_id => Some(questions.clone()),
                                _ => None,
                            })
                        })
                });
                let Some(questions) = questions else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request".into()),
                    ));
                };
                // The run died under the question (engine restart, crash).
                // The question is still open in the doc and the command is
                // durable, so honor it anyway — stamp the part resolved and
                // deliver the answers as the next (resumed) turn, the same
                // fallback a dead-run steer takes. The question UI stays up
                // until the user answers (user requirement); this is what
                // makes that answer still WORK.
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, ""));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request and no prior run config".into()),
                    ));
                };
                request.prompt = respond_input_prompt(&questions, answers);
                request.resume = None; // dispatch re-derives the harness session
                request.attachments = Vec::new();
                if let Err(err) = handle.doc.resolve_input(request_id) {
                    tracing::warn!(chat = %chat_id, request = %request_id, error = %err,
                        "orphaned input resolve failed");
                }
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, None)
                    .await?;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("answered as new turn".into()),
                ))
            }
        }
    }

    /// Park a prompt for a turn-boundary agent in the visible queue instead
    /// of its mailbox, keeping the message id so the transcript entry written
    /// at delivery is the same message. Steers lead ordinary rows.
    fn hold_until_turn_end(
        &self,
        handle: &Arc<ChatDocHandle>,
        message_id: &str,
        prompt: &str,
        issued_at: i64,
        steer: bool,
    ) -> Result<(), EngineError> {
        let item = QueuedMessage {
            id: message_id.to_string(),
            text: prompt.to_string(),
            attachments: Vec::new(),
            hold_for_turn_end: false,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: issued_at.min(now_ms()),
            edited_at: None,
            delivery_gate: None,
        };
        if handle.doc.read_queue()?.iter().any(|row| row.id == item.id) {
            return Ok(()); // a redelivered command: already held
        }
        if steer {
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
        } else {
            handle.doc.push_queued(&item)?;
        }
        // Sending is the deliberate action that thaws a queue frozen by Cancel.
        handle.queue_paused.store(false, Ordering::Release);
        handle.publish_queue();
        Ok(())
    }

    /// Put a typed prompt in front of a live agent: steer it in, or — with no
    /// live steerable run — deliver the durable command as the next turn.
    /// After an engine restart `last_request` is empty too, so rebuild the run
    /// config from the chat's workspace row (loams-desktop derived dispatch config from
    /// the chat row the same way — sessions.ts:601-620); dispatch's engine-owned
    /// resume then reattaches the prior harness conversation.
    ///
    /// Turn-boundary drivers retain explicit steers until their next boundary.
    async fn deliver_prompt(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        prompt: &str,
        message_id: Option<String>,
        issued_at: i64,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        // Explicit steering uses the run mailbox when the agent reads it
        // mid-turn. A turn-boundary agent would read it only after the turn,
        // so it waits in the queue, ahead of ordinary rows, and reaches the
        // transcript when it is actually delivered. Never interrupt to hurry
        // steering.
        let prompt = self.resolve_prompt_attachments(prompt);
        // A live turn without a mailbox can't take it either: the fresh
        // dispatch below would interrupt it.
        let unsteerable_turn =
            sessions.turn_in_flight(chat_id) && !sessions.live_run_steerable(chat_id);
        if !prompt.trim().is_empty()
            && (unsteerable_turn || sessions.defers_to_turn_end(chat_id, None))
        {
            let id = message_id.unwrap_or_else(new_id);
            self.hold_until_turn_end(handle, &id, &prompt, issued_at, true)?;
            return Ok((
                SessionCommandStatus::Applied,
                Some("held until the turn ends".into()),
            ));
        }
        // The transcript shows the send while mailbox backpressure holds it.
        // A pending agent update instead holds it in the queue below, where a
        // transcript copy would duplicate it until the update finishes.
        if !sessions.live_run_update_pending(chat_id)
            && let Some(message_id) = message_id.as_deref()
            && let Err(err) =
                handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))
        {
            tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
        }
        match sessions
            .steer_at(chat_id, &prompt, message_id.clone(), issued_at)
            .await?
        {
            SteerOutcome::Accepted => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SteerOutcome::NotSteerable => {
                if let Some(message_id) = message_id.as_deref() {
                    handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))?;
                }
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, &prompt));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no live run and no prior run config".into()),
                    ));
                };
                request.prompt = prompt;
                request.resume = None; // dispatch re-derives the harness session
                // A reused config must not re-inline the PREVIOUS turn's
                // images; this prompt's own refs (if any) ride its text.
                request.attachments = Vec::new();
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, message_id)
                    .await?;
                handle.queue_paused.store(false, Ordering::Release);
                Ok((
                    SessionCommandStatus::Applied,
                    Some("queued as new turn".into()),
                ))
            }
            SteerOutcome::DeferredByUpdate => {
                let id = message_id.unwrap_or_else(new_id);
                self.hold_until_turn_end(handle, &id, &prompt, issued_at, true)?;
                // The completed turn's status publication normally re-drains
                // this queue. Also cover completion racing the enqueue itself.
                self.drain_queue(handle).await;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("held until the agent update finishes".into()),
                ))
            }
        }
    }

    async fn capture_source_context(&self, cwd: &str) -> Option<ConversationSourceContext> {
        let repos = self.inner.repos.get()?;
        let path = Path::new(cwd);
        let identity = repos.checkout_identity(path).await.ok()?;
        let branch = repos.current_branch(path).await.ok()?;
        let head_sha = repos.head_sha(path).await.ok().flatten();
        Some(ConversationSourceContext {
            checkout_id: identity.id,
            repo_root: identity.root.to_string_lossy().into_owned(),
            cwd: cwd.to_string(),
            branch,
            head_sha,
            observed_at: chrono::Utc::now(),
        })
    }

    /// Every fresh harness dispatch crosses this boundary, including
    /// steer/input fallbacks and crash recovery. Capture immediately before
    /// dispatch so the conversation records the checkout the harness will
    /// actually observe, rather than whichever checkout state was current on
    /// an earlier turn.
    pub(crate) async fn dispatch_with_source_context(
        &self,
        sessions: &SessionsEngine,
        chat_id: &str,
        harness: HarnessId,
        request: loams_agentd_proto::RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        if harness == HarnessId::Unsupported {
            return Err(EngineError::HarnessUnsupported);
        }
        if let Some(workspace) = self.workspace()
            && let Some(context) = self.capture_source_context(&request.cwd).await
            && let Err(err) = workspace.set_chat_source_context(chat_id, &context)
        {
            tracing::warn!(chat = %chat_id, error = %err, "conversation source stamp failed");
        }
        sessions
            .dispatch(chat_id, harness, request, message_id)
            .await
    }

    /// Create (or reuse) the isolated worktree a Run's [`loams_agentd_proto::WorktreeSpec`]
    /// asks for, returning the resolved cwd plus the fresh worktree when one was
    /// actually created. Reuse guard: a chat whose row already points inside a
    /// linked worktree of the same repo keeps it — a duplicate Run (client retry
    /// after a lost ack, ledger reset) must not mint a second checkout.
    async fn materialize_worktree(
        &self,
        chat_id: &str,
        spec: &loams_agentd_proto::WorktreeSpec,
    ) -> Result<(String, Option<loams_agentd_proto::Worktree>), EngineError> {
        if let Some(ws) = self.workspace()
            && let Ok(Some(chat)) = ws.chat(chat_id)
            && let Some(cwd) = chat.cwd
            && cwd != spec.repo_path
            && crate::workspace_host::linked_worktree_root(std::path::Path::new(&cwd)).as_deref()
                == Some(spec.repo_path.as_str())
        {
            tracing::info!(chat = %chat_id, cwd = %cwd, "worktree spec: reusing the chat's existing worktree");
            return Ok((cwd, None));
        }
        let repos = self
            .inner
            .repos
            .get()
            .ok_or_else(|| EngineError::Other("repos engine not wired".into()))?;
        let worktree = repos
            .create_worktree(std::path::Path::new(&spec.repo_path), &spec.base)
            .await?;
        tracing::info!(
            chat = %chat_id,
            path = %worktree.path,
            branch = %worktree.branch,
            "worktree materialized for run"
        );
        Ok((worktree.path.clone(), Some(worktree)))
    }

    fn complete_worktree_setup_handoff(
        &self,
        command_id: &str,
        chat_id: &str,
        spec: &loams_agentd_proto::WorktreeSpec,
        fresh_worktree: Option<&loams_agentd_proto::Worktree>,
    ) {
        let Some((project_actions, terminals)) = self.inner.project_action_runtime.get() else {
            return;
        };
        let outcome = match (spec.space_id.as_deref(), fresh_worktree) {
            (Some(space_id), Some(worktree)) => self
                .resolve_and_launch_worktree_setup(
                    project_actions,
                    terminals,
                    space_id,
                    spec,
                    worktree,
                )
                .unwrap_or_else(|err| ProjectActionSetupHandoff {
                    setup_action: None,
                    setup_error: Some(err.to_string()),
                }),
            _ => ProjectActionSetupHandoff {
                setup_action: None,
                setup_error: None,
            },
        };
        project_actions.complete_setup_handoff(command_id, chat_id, outcome);
    }

    fn resolve_and_launch_worktree_setup(
        &self,
        project_actions: &ProjectActionsStore,
        terminals: &Terminals,
        space_id: &str,
        spec: &loams_agentd_proto::WorktreeSpec,
        worktree: &loams_agentd_proto::Worktree,
    ) -> Result<ProjectActionSetupHandoff, EngineError> {
        let workspace = self
            .workspace()
            .ok_or_else(|| EngineError::Other("workspace host not wired".into()))?;
        let space = workspace
            .space(space_id)?
            .ok_or_else(|| EngineError::Other("Project not found".into()))?;
        if space.device_id != self.inner.config.device_id {
            return Err(EngineError::Other(
                "Project belongs to another device".into(),
            ));
        }
        let project_root = std::fs::canonicalize(&space.path)?;
        let requested_root = std::fs::canonicalize(&spec.repo_path)?;
        if project_root != requested_root {
            return Err(EngineError::Other(
                "Project path does not match worktree repository".into(),
            ));
        }
        // The store keys configuration by the original Space path, which may
        // be a symlink. Keep canonical paths for validation and execution only.
        let setup_action = project_actions
            .setup_action(space_id, std::path::Path::new(&space.path))?
            .map(|action| {
                launch_project_setup_action(
                    terminals,
                    &action,
                    &project_root,
                    std::path::Path::new(&worktree.path),
                    120,
                    32,
                )
            })
            .transpose()?;
        Ok(ProjectActionSetupHandoff {
            setup_action,
            setup_error: None,
        })
    }

    /// A steer-turned-run with no in-process `last_request` (engine restarted
    /// since the last turn): rebuild the run config from the chat's workspace
    /// row — cwd from the row, model/reasoning/options/sandbox from its config
    /// (composer defaults otherwise). `None` without a workspace host or row.
    // (Also the RespondInput dead-run fallback's config source.)
    pub(crate) fn request_from_chat_row(
        &self,
        chat_id: &str,
        prompt: &str,
    ) -> Option<loams_agentd_proto::RunRequest> {
        let workspace = self.workspace()?;
        let chat = match workspace.chat(chat_id) {
            Ok(chat) => chat?,
            Err(err) => {
                tracing::warn!(chat = %chat_id, error = %err, "workspace chat read failed");
                return None;
            }
        };
        let config = chat.config;
        Some(loams_agentd_proto::RunRequest {
            mcp: None,
            prompt: prompt.to_string(),
            harness: config.as_ref().map(|c| c.harness),
            model: config.as_ref().and_then(|c| c.model.clone()),
            reasoning: config.as_ref().and_then(|c| c.reasoning),
            model_options: config
                .as_ref()
                .map(|c| c.model_options.clone())
                .unwrap_or_default(),
            cwd: chat.cwd.unwrap_or_default(),
            sandbox: config
                .as_ref()
                .map(|c| c.sandbox)
                .unwrap_or(loams_agentd_proto::SandboxLevel::WorkspaceWrite),
            auto_approve: false,
            attachments: Vec::new(),
            resume: None,
            worktree: None,
        })
    }

    fn save_snapshot(&self, handle: &ChatDocHandle) {
        if handle.retired.load(Ordering::Relaxed) {
            // A thin lineage replaced this doc on disk; persisting this
            // handle's fat doc would clobber it. But retired with NO thin
            // lineage on disk means this doc is its transcript's only copy —
            // skipping the save turned an app quit into total loss
            // (2026-08-11), so persist it.
            let thin_on_disk = matches!(
                self.inner.store.load_snapshot_with_cursor(&handle.chat_id),
                Ok(Some((_, _, epoch))) if epoch >= CHAT2_DOC_EPOCH
            );
            if thin_on_disk {
                return;
            }
        }
        if let Some(persistence) = &handle.persistence {
            persistence.flush_sync();
            return;
        }
        match handle.doc.export_snapshot() {
            Ok(bytes) => {
                handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                if let Err(err) = self.inner.store.save_snapshot(&handle.chat_id, &bytes) {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot save failed");
                }
            }
            Err(err) => {
                tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot export failed");
            }
        }
    }

    /// A fork must be durable before publishing its discoverable registry row.
    pub(crate) fn persist_fork(&self, handle: &ChatDocHandle) -> Result<(), EngineError> {
        let bytes = handle.doc.export_snapshot()?;
        self.inner.store.save_snapshot(&handle.chat_id, &bytes)?;
        handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Persist every open doc now (shutdown path; bypasses the debounce).
    pub fn flush_all(&self) {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            self.save_snapshot(&handle);
        }
    }
}

/// The resumed-turn prompt for answers to a question whose run died: each
/// answer paired with its question text so the reattached conversation reads
/// naturally. Pure.
pub fn respond_input_prompt(
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
) -> String {
    let mut lines = vec!["Answering your earlier question:".to_string()];
    for answer in answers {
        let picked = answer.labels.join(", ");
        let question = questions
            .iter()
            .find(|q| q.id == answer.question_id)
            .map(|q| q.question.trim())
            .filter(|q| !q.is_empty());
        match question {
            Some(question) => lines.push(format!("{question} — {picked}")),
            None => lines.push(picked),
        }
    }
    lines.join("\n")
}

/// The message a Run or Steer command delivers, the key [`DocHost::retry_delivery`]
/// groups attempts by.
fn retry_message_id(payload: &SessionCommandPayload) -> Option<&str> {
    match payload {
        SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
        SessionCommandPayload::Steer { message_id, .. } => message_id.as_deref(),
        _ => None,
    }
}

/// Two attempts of the same send: same kind, same message id.
fn same_send(a: &SessionCommandPayload, b: &SessionCommandPayload) -> bool {
    match (a, b) {
        (
            SessionCommandPayload::Run { message_id: a, .. },
            SessionCommandPayload::Run { message_id: b, .. },
        ) => a == b,
        (
            SessionCommandPayload::Steer { message_id: a, .. },
            SessionCommandPayload::Steer { message_id: b, .. },
        ) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod local_doc_tests {
    use super::{DocHost, DocHostConfig};
    use std::sync::Arc;

    fn host() -> (tempfile::TempDir, DocHost) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(loams_agentd_store::DocsStore::open(dir.path()).expect("store opens"));
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "dev-test".into(),
                default_harness: loams_agentd_proto::HarnessId::Mock,
                tool_outputs: dir.path().join("tool-outputs"),
            },
        );
        (dir, host)
    }

    #[tokio::test]
    async fn whale_snapshot_opens_and_reopens_without_network() {
        let (_dir, host) = host();
        let source = loams_agentd_doc::SessionDoc::init("persisted-whale").unwrap();
        for i in 0..2000 {
            source
                .push_message(&loams_agentd_doc::SessionMessageEntry {
                    id: format!("row-{i}"),
                    role: loams_agentd_doc::MessageRole::User,
                    parts: vec![loams_agentd_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    }],
                    created_at: i,
                    device_id: "remote".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap();
        }
        host.inner
            .store
            .save_snapshot_with_cursor("persisted-whale", &source.export_snapshot().unwrap(), 0, 2)
            .unwrap();
        drop(source);
        let start = std::time::Instant::now();
        let handle = host.open("persisted-whale").unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 2000);
        eprintln!("offline whale cold open: {:?}", start.elapsed());
        drop(rx);
        // An unwatched commit clears the mirror; attach still serves local data.
        handle.publish_messages_if_watched();
        let start = std::time::Instant::now();
        assert_eq!(handle.watch_messages().borrow().entries.len(), 2000);
        eprintln!("offline whale rebuilt mirror: {:?}", start.elapsed());
    }

    #[tokio::test]
    async fn transcript_attach_and_unwatched_clear_share_a_critical_section() {
        let (_dir, host) = host();
        let handle = host.open("cached").unwrap();
        handle
            .write_user_message("row", "locally persisted transcript", 0)
            .unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 1);
        drop(rx);

        // Freeze attach's critical section. An unwatched publisher must not
        // pass its receiver check and clear the mirror while attach owns it.
        let guard = super::lock(&handle.transcript_import);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_handle = handle.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_handle.publish_messages_if_watched();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        let result = done_rx.recv_timeout(std::time::Duration::from_millis(100));
        // Simulate the subscription attaching before the worker can inspect it.
        let rx = handle.messages_tx.subscribe();
        drop(guard);
        worker.join().unwrap();
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "unwatched clear escaped attach's critical section"
        );
        assert_eq!(rx.borrow().entries.len(), 1, "no empty reset after attach");
        drop(rx);
        handle.publish_messages_if_watched();
        assert!(handle.messages_tx.borrow().entries.is_empty());
        assert_eq!(
            handle.watch_messages().borrow().entries.len(),
            1,
            "offline reopen rebuilds from local content"
        );
    }
}

#[cfg(test)]
mod source_context_tests {
    use super::{DocHost, DocHostConfig};
    use std::process::Command;
    use std::sync::Arc;

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn capture_source_context_reads_the_dispatch_checkout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "feature/captured"]);
        git(&repo, &["config", "user.name", "Loams Desktop Test"]);
        git(
            &repo,
            &["config", "user.email", "loams-desktop@example.com"],
        );
        std::fs::write(repo.join("README.md"), "capture\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "capture"]);

        let store = Arc::new(
            loams_agentd_store::DocsStore::open(dir.path().join("docs")).expect("store opens"),
        );
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "device-a".into(),
                default_harness: loams_agentd_proto::HarnessId::Mock,
                tool_outputs: dir.path().join("tool-outputs"),
            },
        );
        host.set_repos(crate::repos::Repos::new(
            &dir.path().join("data"),
            "device-a",
        ));

        let context = host
            .capture_source_context(repo.to_str().unwrap())
            .await
            .expect("source context");
        assert_eq!(context.branch, "feature/captured");
        assert_eq!(context.cwd, repo.to_string_lossy());
        assert_eq!(
            context.repo_root,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert!(context.head_sha.is_some());
        assert!(!context.checkout_id.is_empty());
    }
}

#[cfg(test)]
mod queued_message_prompt_tests {
    use super::{ATTACHMENT_ONLY_PROMPT, ATTACHMENT_PROMPT_HEADER, queued_message_prompt};

    #[test]
    fn dispatch_adds_the_attachment_transport_to_visible_queue_text() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("inspect this", &paths),
            format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }

    #[test]
    fn legacy_expanded_rows_are_not_expanded_twice() {
        let paths = vec!["/tmp/image.png".to_string()];
        let legacy = format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png");
        assert_eq!(queued_message_prompt(&legacy, &paths), legacy);
    }

    #[test]
    fn attachment_only_rows_get_a_non_empty_prompt_body() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("", &paths),
            format!("{ATTACHMENT_ONLY_PROMPT}\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }
}

/// Per-chat background task: reacts to doc changes
/// by re-publishing the transcript watch, draining commands, and debouncing snapshots.
/// Holds only a weak handle so a dropped host tears the task down.
async fn chat_task(host: DocHost, weak: Weak<ChatDocHandle>, mut changed_rx: watch::Receiver<u64>) {
    // Prompt delivery may wait for mailbox capacity. Keep a separate watcher
    // for interrupt/question controls so that wait cannot block recovery.
    let control_host = host.clone();
    let control_weak = weak.clone();
    let mut control_changes = changed_rx.clone();
    host.spawn_worker(async move {
        loop {
            let Some(handle) = control_weak.upgrade() else {
                break;
            };
            control_host.drain_command_kind(&handle, true).await;
            drop(handle);
            if control_changes.changed().await.is_err() {
                break;
            }
        }
    });
    // Initial pass: the snapshot may already carry pending commands. The
    // mirror stays lazy — it materializes on the first watch attach.
    {
        let Some(handle) = weak.upgrade() else { return };
        host.drain_commands(&handle).await;
        host.drain_queue(&handle).await;
    }
    let mut save_deadline: Option<tokio::time::Instant> = None;
    loop {
        let sleep_until = save_deadline.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            changed = changed_rx.changed() => {
                if changed.is_err() {
                    break; // doc handle (and its change sender) is gone
                }
                let Some(handle) = weak.upgrade() else { break };
                let publishing = handle.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    publishing.publish_messages_if_watched();
                    publishing.publish_queue();
                }).await;
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
                if save_deadline.is_none() {
                    save_deadline = Some(
                        tokio::time::Instant::now()
                            + std::time::Duration::from_millis(SNAPSHOT_DEBOUNCE_MS),
                    );
                }
            }
            _ = tokio::time::sleep_until(sleep_until), if save_deadline.is_some() => {
                save_deadline = None;
                let Some(handle) = weak.upgrade() else { break };
                // Thin-lineage docs have their own coalescing blocking-pool
                // persister. The legacy worker must not duplicate every export.
                if handle.persistence.is_none() { host.save_snapshot(&handle); }
                // Post-quiesce eviction pass: sizes just refreshed.
                host.schedule_eviction();
            }
        }
    }
}

#[cfg(test)]
mod publication_eviction_tests {
    use super::*;

    fn host(dir: &std::path::Path) -> (Arc<DocsStore>, DocHost) {
        let store = Arc::new(DocsStore::open(dir).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                tool_outputs: dir.join("tool-outputs"),
            },
        );
        (store, host)
    }

    #[tokio::test]
    async fn caller_and_watch_protect_the_open_to_attach_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let (_store, host) = host(dir.path());
        let held = host.open_local("held").unwrap();
        for i in 0..30 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        let watch = held.watch_messages();
        drop(held);
        for i in 30..60 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        drop(watch);
        host.open_local("overflow").unwrap();
        assert!(!lock(&host.inner.handles).contains_key("held"));
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lru_eviction_flushes_and_reopen_restores_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let (_store, host) = host(dir.path());
        let handle = host.open("evicted").unwrap();
        handle
            .doc
            .doc()
            .get_text("body")
            .insert(0, "written before eviction")
            .unwrap();
        handle.doc.doc().commit();
        for i in 0..WARM_DOC_CAP {
            host.open(&format!("other-{i}")).unwrap();
        }
        assert!(!host.pinned(&handle));
        drop(handle);
        host.open("overflow").unwrap();
        host.evict_over_budget();
        assert!(!lock(&host.inner.handles).contains_key("evicted"));
        let reopened = host.open("evicted").unwrap();
        assert_eq!(
            reopened.doc.doc().get_text("body").to_string(),
            "written before eviction"
        );
        drop(reopened);
        host.shutdown_workers().await;
    }

    /// The 100 ms eviction tick must not queue behind a slow open: within
    /// budget it returns without taking the `opening` lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_eviction_pass_within_budget_skips_the_opening_lock() {
        let dir = tempfile::tempdir().unwrap();
        let (_store, host) = host(dir.path());
        drop(host.open("warm").unwrap());
        let opening = lock(&host.inner.opening);
        let pass = {
            let host = host.clone();
            std::thread::spawn(move || host.evict_over_budget())
        };
        let started = std::time::Instant::now();
        while !pass.is_finished() && started.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let finished = pass.is_finished();
        drop(opening);
        pass.join().unwrap();
        assert!(finished, "a pass within budget waited for the opening lock");
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn tool_outputs_round_trip_through_the_local_store() {
        let dir = tempfile::tempdir().unwrap();
        let (_store, host) = host(dir.path());
        host.store_tool_output(
            "chat-1",
            loams_agentd_doc::SidecarPayload {
                part_id: "m1#tool:call_9".into(),
                output: Some("full output".into()),
                diff: None,
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while host
                .fetch_tool_blob("chat-1/m1#tool:call_9", 0, None)
                .await
                .is_err()
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            host.fetch_tool_blob("chat-1/m1#tool:call_9", 0, None)
                .await
                .unwrap()
                .text,
            "full output"
        );
        host.purge_chat("chat-1");
        assert!(
            host.fetch_tool_blob("chat-1/m1#tool:call_9", 0, None)
                .await
                .is_err()
        );
        host.shutdown_workers().await;
    }
}
