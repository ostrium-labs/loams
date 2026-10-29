//! Run groups (design §53 §5.5, §6.5 G12–G16, D904; plan SD1 Task 12,
//! Ruling 5): one prompt run in parallel by several harnesses or models,
//! each member a chat in its own worktree, compared and resolved together.
//!
//! [`RunGroups`] owns the group records. They live in the store's
//! `run_groups` table, so a group survives the window closing and a daemon
//! restart. A scheduler task sends each member's run when the concurrency
//! limits allow ([`RunGroupLimits`]: per group and across the daemon) and
//! follows each member's turn through the sessions engine's turn hook
//! ([`GroupTurns`], beside the checkpoints of SD1 Task 10). It publishes the
//! groups, with each member's diff stats from the checkout diff watch, to
//! `WatchRunGroups`.
//!
//! The RPCs that create, compare, check and resolve groups are
//! `EngineRpc`'s (`rpc/groups_rpc.rs`), so a resolve runs Task 9's git
//! writes through their own dispatch arms and guards.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use loams_agentd_proto::chat::SessionCommandPayload;
use loams_agentd_proto::groups::{
    ApprovalMode, CHECK_TAIL_LINES, CheckState, MemberCheck, RunGroup, RunGroupMember,
    RunGroupResolution, RunGroupState, RunMemberStatus, RunVariant,
};
use loams_agentd_proto::notify::NotificationKind;
use loams_agentd_proto::{CheckoutDiff, RunRequest, SandboxLevel, SessionStatus, TerminalEvent};
use loams_agentd_store::{DocsStore, StoredRunGroup};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, watch};

use crate::checkpoints::Checkpoints;
use crate::diff_sync::CheckoutDiffSync;
use crate::doc_host::DocHost;
use crate::notify::Notifications;
use crate::sessions::{SessionsEngine, TurnCheckpoints};
use crate::terminals::Terminals;
use crate::{EngineError, now_ms};

/// The most members one group may have.
pub const MAX_VARIANTS: usize = 16;

/// The longest line of a check's output a group keeps, in characters.
const CHECK_LINE_CHARS: usize = 1_000;

/// How many members' runs may be in flight at once (`config.toml`
/// `[groups]`; §53 §5.5). A member over a limit waits `queued`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunGroupLimits {
    /// In one group (`max_running_per_group`, default 4).
    pub max_running_per_group: usize,
    /// Across every group of the daemon (`max_running`, default 8).
    pub max_running: usize,
}

impl Default for RunGroupLimits {
    fn default() -> Self {
        Self {
            max_running_per_group: 4,
            max_running: 8,
        }
    }
}

/// Where a member's run is, as the group records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    /// Waiting for a slot.
    Queued,
    /// Its run command is on the chat's queue; the turn has not started.
    Sent,
    /// Its turn started.
    Running,
    Done,
    Errored,
    /// Never sent: the group was resolved with `force` first.
    Cancelled,
}

impl Phase {
    /// Holding (or about to hold) a slot.
    fn in_flight(self) -> bool {
        matches!(self, Self::Sent | Self::Running)
    }

    /// Not finished: the group is not ready.
    pub(crate) fn is_live(self) -> bool {
        matches!(self, Self::Queued | Self::Sent | Self::Running)
    }
}

/// One member, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberRecord {
    pub chat_id: String,
    pub variant: RunVariant,
    /// Its worktree, and the branch made for it.
    pub cwd: String,
    pub branch: String,
    #[serde(default)]
    pub checkout_id: Option<String>,
    /// The id of the run's user message.
    pub message_id: String,
    /// The id of the run command sent to the member's chat, from `Sent` on.
    #[serde(default)]
    pub command_id: Option<String>,
    pub phase: Phase,
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub check: Option<MemberCheck>,
    #[serde(default)]
    pub detached: bool,
    #[serde(default)]
    pub archived: bool,
}

/// One group, as stored (the `run_groups` table's `record`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupRecord {
    pub id: String,
    #[serde(default)]
    pub space_id: Option<String>,
    pub repo_path: String,
    pub prompt: String,
    pub base: String,
    pub base_sha: String,
    pub created_at_ms: i64,
    pub approval_mode: ApprovalMode,
    #[serde(default)]
    pub check_action_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
    pub members: Vec<MemberRecord>,
    #[serde(default)]
    pub resolution: Option<RunGroupResolution>,
}

impl GroupRecord {
    pub(crate) fn member(&self, chat_id: &str) -> Option<&MemberRecord> {
        self.members.iter().find(|member| member.chat_id == chat_id)
    }

    fn member_mut(&mut self, chat_id: &str) -> Option<&mut MemberRecord> {
        self.members
            .iter_mut()
            .find(|member| member.chat_id == chat_id)
    }

    /// The members still queued or running, detached ones aside.
    pub(crate) fn live_members(&self) -> Vec<&MemberRecord> {
        self.members
            .iter()
            .filter(|member| !member.detached && member.phase.is_live())
            .collect()
    }

    fn stored(&self) -> Result<StoredRunGroup, EngineError> {
        Ok(StoredRunGroup {
            group_id: self.id.clone(),
            space_id: self.space_id.clone(),
            created_at: self.created_at_ms,
            record: serde_json::to_string(self)
                .map_err(|e| EngineError::Other(format!("run group serialize: {e}")))?,
        })
    }

    /// The run every member is sent: the group's prompt in the member's
    /// worktree, with its variant's harness and model.
    fn run_request(&self, member: &MemberRecord) -> RunRequest {
        RunRequest {
            mcp_servers: Vec::new(),
            mcp: None,
            prompt: self.prompt.clone(),
            harness: Some(member.variant.harness),
            model: member.variant.model.clone(),
            reasoning: member.variant.reasoning,
            model_options: member.variant.model_options.clone(),
            cwd: member.cwd.clone(),
            sandbox: SandboxLevel::WorkspaceWrite,
            // One approval mode for the whole group (§53 §8.1).
            auto_approve: self.approval_mode == ApprovalMode::FullAccess,
            attachments: self.attachments.clone(),
            worktree: None,
            resume: None,
        }
    }
}

/// A turn of a member's chat, noted by the turn hook and applied by the
/// scheduler (the hook must not block).
#[derive(Debug, Clone)]
enum TurnNote {
    Start { chat_id: String, at: i64 },
    End { chat_id: String, at: i64 },
}

/// What the scheduler needs of the engine, wired at assembly.
struct Wiring {
    doc_host: DocHost,
    sessions: SessionsEngine,
    diff_sync: CheckoutDiffSync,
    terminals: Terminals,
}

struct Inner {
    store: Arc<DocsStore>,
    limits: Mutex<RunGroupLimits>,
    /// Every group, oldest first.
    groups: Mutex<Vec<GroupRecord>>,
    /// The chats of every group's members, for the turn hook.
    members: Mutex<HashSet<String>>,
    notes: Mutex<Vec<TurnNote>>,
    /// Members whose run the daemon had started when it last stopped (see
    /// [`started_before_boot`]): done unless their run was resumed.
    resumed: Mutex<HashSet<String>>,
    view: watch::Sender<Vec<RunGroup>>,
    kick: Notify,
    /// Dropped at shutdown: it holds the engine, and the engine's turn hook
    /// holds these groups.
    wiring: Mutex<Option<Arc<Wiring>>>,
    /// Where `group_ready` and `check_failed` go (SD1 Task 14).
    notifications: Mutex<Option<Notifications>>,
    /// Each group's state as last published, so a group that becomes ready
    /// is told once (a group already ready at boot is not).
    states: Mutex<HashMap<String, RunGroupState>>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The run groups of this daemon (see the module docs).
#[derive(Clone)]
pub struct RunGroups {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RunGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunGroups").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RunGroups {
    /// Load every stored group. A record that no longer parses is skipped
    /// with a warning, never deleted.
    pub fn open(store: Arc<DocsStore>) -> Result<Self, EngineError> {
        let mut groups = Vec::new();
        let mut resumed = HashSet::new();
        for stored in store.run_groups()? {
            match serde_json::from_str::<GroupRecord>(&stored.record) {
                Ok(mut group) => {
                    // A check's terminal does not outlive the daemon.
                    for member in &mut group.members {
                        if let Some(check) = member
                            .check
                            .as_mut()
                            .filter(|check| check.state == CheckState::Running)
                        {
                            check.state = CheckState::Failed;
                            check.log_tail.push("(the daemon stopped)".into());
                        }
                    }
                    if group.resolution.is_none() {
                        resumed.extend(
                            group
                                .members
                                .iter()
                                .filter(|member| started_before_boot(&store, member))
                                .map(|member| member.chat_id.clone()),
                        );
                    }
                    groups.push(group);
                }
                Err(error) => {
                    tracing::warn!(group = %stored.group_id, %error, "run group record skipped");
                }
            }
        }
        let members = groups
            .iter()
            .flat_map(|group| group.members.iter().map(|member| member.chat_id.clone()))
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                limits: Mutex::new(RunGroupLimits::default()),
                groups: Mutex::new(groups),
                members: Mutex::new(members),
                notes: Mutex::new(Vec::new()),
                resumed: Mutex::new(resumed),
                view: watch::channel(Vec::new()).0,
                kick: Notify::new(),
                wiring: Mutex::new(None),
                notifications: Mutex::new(None),
                states: Mutex::new(HashMap::new()),
                stop: watch::channel(false).0,
                task: Mutex::new(None),
            }),
        })
    }

    /// Post `group_ready` and `check_failed` to `notifications` (SD1 Task 14).
    pub fn set_notifications(&self, notifications: Notifications) {
        *lock(&self.inner.notifications) = Some(notifications);
    }

    fn notify(&self, kind: NotificationKind, group_id: &str, chat_id: Option<&str>, title: String) {
        if let Some(notifications) = lock(&self.inner.notifications).clone() {
            notifications.push(
                kind,
                chat_id.map(str::to_owned),
                Some(group_id.to_owned()),
                title,
            );
        }
    }

    /// The group's prompt's first line, for a notification's title.
    fn group_title(&self, group_id: &str) -> String {
        lock(&self.inner.groups)
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| group.prompt.lines().find(|line| !line.trim().is_empty()))
            .map(|line| line.trim().to_owned())
            .unwrap_or_else(|| "Run group".to_owned())
    }

    /// Wire the engine and start the scheduler: queued members are sent as
    /// slots allow, and the groups are published.
    pub fn start(
        &self,
        doc_host: DocHost,
        sessions: SessionsEngine,
        diff_sync: CheckoutDiffSync,
        terminals: Terminals,
    ) {
        {
            let mut wiring = lock(&self.inner.wiring);
            if wiring.is_some() {
                return;
            }
            *wiring = Some(Arc::new(Wiring {
                doc_host,
                sessions,
                diff_sync,
                terminals,
            }));
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("run groups: no runtime; the scheduler is off");
            return;
        };
        let this = self.clone();
        let task = runtime.spawn(async move { this.schedule().await });
        *lock(&self.inner.task) = Some(task);
    }

    /// Stop the scheduler (engine shutdown). Records stay stored.
    pub async fn shutdown(&self) {
        self.inner.stop.send_replace(true);
        self.inner.kick.notify_one();
        let task = lock(&self.inner.task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        // Break the engine -> turn hook -> groups -> engine cycle.
        lock(&self.inner.wiring).take();
    }

    fn wiring(&self) -> Option<Arc<Wiring>> {
        lock(&self.inner.wiring).clone()
    }

    pub fn limits(&self) -> RunGroupLimits {
        *lock(&self.inner.limits)
    }

    /// Change the limits (from `config.toml`); a value of 0 is read as 1.
    pub fn set_limits(&self, limits: RunGroupLimits) {
        *lock(&self.inner.limits) = RunGroupLimits {
            max_running_per_group: limits.max_running_per_group.max(1),
            max_running: limits.max_running.max(1),
        };
        self.inner.kick.notify_one();
    }

    /// Every group, newest first, again on each change.
    pub fn watch(&self) -> watch::Receiver<Vec<RunGroup>> {
        self.inner.view.subscribe()
    }

    /// The groups as `WatchRunGroups` sends them now.
    pub fn snapshot(&self) -> Vec<RunGroup> {
        self.view()
    }

    /// Store a new group and wake the scheduler.
    pub(crate) fn create(&self, group: GroupRecord) -> Result<(), EngineError> {
        self.inner.store.save_run_group(&group.stored()?)?;
        lock(&self.inner.members).extend(group.members.iter().map(|m| m.chat_id.clone()));
        lock(&self.inner.groups).push(group);
        self.publish();
        self.inner.kick.notify_one();
        Ok(())
    }

    pub(crate) fn group(&self, group_id: &str) -> Option<GroupRecord> {
        lock(&self.inner.groups)
            .iter()
            .find(|group| group.id == group_id)
            .cloned()
    }

    /// Change one group, store it and republish. `None` when there is no
    /// such group.
    pub(crate) fn update<R>(
        &self,
        group_id: &str,
        change: impl FnOnce(&mut GroupRecord) -> R,
    ) -> Result<Option<R>, EngineError> {
        let (out, stored) = {
            let mut groups = lock(&self.inner.groups);
            let Some(group) = groups.iter_mut().find(|group| group.id == group_id) else {
                return Ok(None);
            };
            let out = change(group);
            (out, group.stored()?)
        };
        self.inner.store.save_run_group(&stored)?;
        self.publish();
        self.inner.kick.notify_one();
        Ok(Some(out))
    }

    /// A member's check started in `terminal_id`: record it as running and
    /// follow the terminal to its exit.
    pub(crate) fn start_check(
        &self,
        group_id: &str,
        chat_id: &str,
        terminal_id: &str,
    ) -> Result<(), EngineError> {
        let receiver = self
            .wiring()
            .ok_or_else(|| EngineError::Other("run groups are not started".into()))?
            .terminals
            .subscribe(terminal_id, None)?;
        self.update(group_id, |group| {
            if let Some(member) = group.member_mut(chat_id) {
                member.check = Some(MemberCheck {
                    state: CheckState::Running,
                    exit_code: None,
                    terminal_id: Some(terminal_id.to_string()),
                    log_tail: Vec::new(),
                });
            }
        })?;
        let this = self.clone();
        let (group_id, chat_id, terminal_id) = (
            group_id.to_string(),
            chat_id.to_string(),
            terminal_id.to_string(),
        );
        tokio::spawn(async move {
            let (exit_code, tail) = follow_check(receiver).await;
            let finished = this.update(&group_id, |group| {
                if let Some(member) = group.member_mut(&chat_id) {
                    // A newer check of this member replaced this one.
                    if member
                        .check
                        .as_ref()
                        .is_some_and(|check| check.terminal_id.as_deref() != Some(&terminal_id))
                    {
                        return;
                    }
                    member.check = Some(MemberCheck {
                        state: if exit_code == Some(0) {
                            CheckState::Passed
                        } else {
                            CheckState::Failed
                        },
                        exit_code,
                        terminal_id: Some(terminal_id.clone()),
                        log_tail: tail,
                    });
                }
            });
            if let Err(error) = finished {
                tracing::warn!(group = %group_id, %error, "run group check not recorded");
            } else if exit_code != Some(0) {
                let title = this.group_title(&group_id);
                this.notify(
                    NotificationKind::CheckFailed,
                    &group_id,
                    Some(&chat_id),
                    title,
                );
            }
        });
        Ok(())
    }

    /// The turn hook: a member's turn started.
    fn note_turn_start(&self, chat_id: &str) {
        if lock(&self.inner.members).contains(chat_id) {
            lock(&self.inner.notes).push(TurnNote::Start {
                chat_id: chat_id.to_string(),
                at: now_ms(),
            });
            self.inner.kick.notify_one();
        }
    }

    /// The turn hook: a member's turn ended (Idle or Errored).
    fn note_turn_end(&self, chat_id: &str) {
        if lock(&self.inner.members).contains(chat_id) {
            lock(&self.inner.notes).push(TurnNote::End {
                chat_id: chat_id.to_string(),
                at: now_ms(),
            });
            self.inner.kick.notify_one();
        }
    }

    async fn schedule(&self) {
        let Some(wiring) = self.wiring() else {
            return;
        };
        let mut sessions = wiring.sessions.watch_sessions();
        let mut diffs = wiring.diff_sync.watch_diffs();
        let mut stop = self.inner.stop.subscribe();
        loop {
            // A stop sent before this task first ran is not a change to
            // `stop`'s receiver, so it is read here as well.
            if *stop.borrow_and_update() {
                break;
            }
            self.reconcile(&wiring);
            self.publish();
            tokio::select! {
                _ = self.inner.kick.notified() => {}
                changed = sessions.changed() => if changed.is_err() { break },
                changed = diffs.changed() => if changed.is_err() { break },
                _ = stop.changed() => break,
            }
        }
    }

    /// Apply the turn notes, settle members whose run ended without one, and
    /// send queued members while the limits allow.
    fn reconcile(&self, wiring: &Wiring) {
        let notes = std::mem::take(&mut *lock(&self.inner.notes));
        let limits = self.limits();
        let mut changed: HashSet<String> = HashSet::new();
        let stored: Vec<StoredRunGroup> = {
            let mut groups = lock(&self.inner.groups);
            let mut resumed = lock(&self.inner.resumed);
            for note in notes {
                let (chat_id, at, start) = match &note {
                    TurnNote::Start { chat_id, at } => (chat_id, *at, true),
                    TurnNote::End { chat_id, at } => (chat_id, *at, false),
                };
                for group in groups.iter_mut() {
                    let group_id = group.id.clone();
                    let Some(member) = group.member_mut(chat_id) else {
                        continue;
                    };
                    resumed.remove(chat_id);
                    if start && member.phase == Phase::Sent {
                        member.phase = Phase::Running;
                        member.started_at_ms = Some(at);
                        changed.insert(group_id);
                    } else if !start && member.phase.in_flight() {
                        let errored = wiring
                            .sessions
                            .session_status(chat_id)
                            .is_some_and(|s| s.status == SessionStatus::Errored);
                        member.phase = if errored { Phase::Errored } else { Phase::Done };
                        member.started_at_ms.get_or_insert(at);
                        member.finished_at_ms = Some(at);
                        changed.insert(group_id);
                    }
                }
            }
            for group in groups.iter_mut().filter(|g| g.resolution.is_none()) {
                for member in group.members.iter_mut().filter(|m| m.phase.in_flight()) {
                    let status = wiring.sessions.session_status(&member.chat_id);
                    let active = wiring.sessions.turn_in_flight(&member.chat_id);
                    // A send that failed before its turn started.
                    let failed = status
                        .as_ref()
                        .is_some_and(|s| s.status == SessionStatus::Errored);
                    // A run the last daemon started and this one did not
                    // resume.
                    let abandoned = !active && resumed.contains(&member.chat_id);
                    if (failed && !active) || abandoned {
                        resumed.remove(&member.chat_id);
                        member.phase = if failed { Phase::Errored } else { Phase::Done };
                        member.finished_at_ms = Some(now_ms());
                        changed.insert(group.id.clone());
                    }
                }
            }
            let mut running: usize = groups
                .iter()
                .filter(|g| g.resolution.is_none())
                .flat_map(|g| g.members.iter())
                .filter(|m| m.phase.in_flight())
                .count();
            for group in groups.iter_mut().filter(|g| g.resolution.is_none()) {
                let mut in_group = group.members.iter().filter(|m| m.phase.in_flight()).count();
                for index in 0..group.members.len() {
                    if running >= limits.max_running || in_group >= limits.max_running_per_group {
                        break;
                    }
                    if group.members[index].phase != Phase::Queued {
                        continue;
                    }
                    let request = group.run_request(&group.members[index]);
                    let member = &mut group.members[index];
                    let sent = wiring.doc_host.queue_command(
                        &member.chat_id,
                        SessionCommandPayload::Run {
                            request,
                            message_id: member.message_id.clone(),
                        },
                    );
                    match sent {
                        Ok(command_id) => {
                            member.command_id = Some(command_id);
                            member.phase = Phase::Sent;
                            running += 1;
                            in_group += 1;
                        }
                        Err(error) => {
                            tracing::warn!(chat = %member.chat_id, %error, "run group member not sent");
                            member.phase = Phase::Errored;
                            member.finished_at_ms = Some(now_ms());
                        }
                    }
                    changed.insert(group.id.clone());
                }
            }
            groups
                .iter()
                .filter(|group| changed.contains(&group.id))
                .filter_map(|group| group.stored().ok())
                .collect()
        };
        for group in stored {
            if let Err(error) = self.inner.store.save_run_group(&group) {
                tracing::warn!(group = %group.group_id, %error, "run group not stored");
            }
        }
    }

    fn publish(&self) {
        let view = self.view();
        let ready: Vec<String> = {
            let mut states = lock(&self.inner.states);
            let ready = view
                .iter()
                .filter(|group| {
                    group.state == RunGroupState::Ready
                        && states.get(&group.id) == Some(&RunGroupState::Running)
                })
                .map(|group| group.id.clone())
                .collect();
            *states = view
                .iter()
                .map(|group| (group.id.clone(), group.state))
                .collect();
            ready
        };
        for group_id in &ready {
            let title = self.group_title(group_id);
            self.notify(NotificationKind::GroupReady, group_id, None, title);
        }
        self.inner.view.send_if_modified(|current| {
            if *current == view {
                false
            } else {
                *current = view;
                true
            }
        });
    }

    /// Every group as the wire sends it, newest first.
    fn view(&self) -> Vec<RunGroup> {
        let wiring = self.wiring();
        let wiring = wiring.as_deref();
        let diffs: Vec<CheckoutDiff> = wiring
            .map(|w| w.diff_sync.watch_diffs().borrow().clone())
            .unwrap_or_default();
        let now = now_ms();
        let groups = lock(&self.inner.groups);
        groups
            .iter()
            .rev()
            .map(|group| {
                let members: Vec<RunGroupMember> = group
                    .members
                    .iter()
                    .map(|member| member_view(member, &diffs, wiring, now))
                    .collect();
                let state = if group.resolution.is_some() {
                    RunGroupState::Resolved
                } else if group.live_members().is_empty() {
                    RunGroupState::Ready
                } else {
                    RunGroupState::Running
                };
                RunGroup {
                    id: group.id.clone(),
                    space_id: group.space_id.clone(),
                    repo_path: group.repo_path.clone(),
                    prompt: group.prompt.clone(),
                    base: group.base.clone(),
                    base_sha: group.base_sha.clone(),
                    created_at_ms: group.created_at_ms,
                    state,
                    approval_mode: group.approval_mode,
                    check_action_id: group.check_action_id.clone(),
                    members,
                    resolution: group.resolution.clone(),
                }
            })
            .collect()
    }
}

fn member_view(
    member: &MemberRecord,
    diffs: &[CheckoutDiff],
    wiring: Option<&Wiring>,
    now: i64,
) -> RunGroupMember {
    let awaiting = wiring
        .and_then(|w| w.sessions.session_status(&member.chat_id))
        .is_some_and(|s| s.status == SessionStatus::AwaitingInput);
    let status = match member.phase {
        Phase::Queued => RunMemberStatus::Queued,
        Phase::Sent => RunMemberStatus::Running,
        Phase::Running if awaiting => RunMemberStatus::AwaitingInput,
        Phase::Running => RunMemberStatus::Running,
        Phase::Done => RunMemberStatus::Done,
        Phase::Errored => RunMemberStatus::Errored,
        Phase::Cancelled => RunMemberStatus::Cancelled,
    };
    let diff = diffs.iter().find(|diff| {
        member.checkout_id.as_deref() == Some(diff.checkout_id.as_str())
            || Path::new(&diff.cwd) == Path::new(&member.cwd)
    });
    let elapsed_ms = member
        .started_at_ms
        .map(|started| (member.finished_at_ms.unwrap_or(now) - started).max(0))
        .unwrap_or(0);
    RunGroupMember {
        chat_id: member.chat_id.clone(),
        harness: member.variant.harness,
        model: member.variant.model.clone(),
        status,
        files_changed: diff
            .map(|d| u32::try_from(d.files.len()).unwrap_or(u32::MAX))
            .unwrap_or(0),
        additions: diff.map(|d| d.additions).unwrap_or(0),
        deletions: diff.map(|d| d.deletions).unwrap_or(0),
        elapsed_ms,
        started_at_ms: member.started_at_ms,
        tokens: member.tokens,
        check: member.check.clone(),
        cwd: member.cwd.clone(),
        branch: member.branch.clone(),
        detached: member.detached,
        archived: member.archived,
    }
}

/// Whether the last daemon had started this member's run when it stopped:
/// its turn was recorded as running, or its run command had been taken. A
/// command enters the processed-command ledger before it executes and never
/// runs again, so a `Sent` member whose command is there gets no turn from it
/// in this daemon. Its turn's start can be missing from the record: the turn
/// hook leaves that to the scheduler, which stops before the runs do
/// (`EngineCore::shutdown`).
fn started_before_boot(store: &DocsStore, member: &MemberRecord) -> bool {
    match member.phase {
        Phase::Running => true,
        Phase::Sent => member
            .command_id
            .as_deref()
            .is_some_and(|id| store.is_processed(id).unwrap_or(false)),
        _ => false,
    }
}

/// Read a check's terminal to its end: the exit code (`None` when the
/// terminal closed without one) and the output's last lines.
async fn follow_check(
    mut events: tokio::sync::mpsc::UnboundedReceiver<TerminalEvent>,
) -> (Option<i32>, Vec<String>) {
    let mut tail = Tail::default();
    while let Some(event) = events.recv().await {
        match event {
            TerminalEvent::Data { data, .. } => {
                let bytes = BASE64.decode(data.as_bytes()).unwrap_or_default();
                tail.push(&String::from_utf8_lossy(&bytes));
            }
            TerminalEvent::Exit { exit_code, .. } => return (Some(exit_code), tail.finish()),
        }
    }
    (None, tail.finish())
}

/// The last [`CHECK_TAIL_LINES`] lines of a stream of text.
#[derive(Default)]
struct Tail {
    lines: std::collections::VecDeque<String>,
    partial: String,
}

impl Tail {
    fn push(&mut self, text: &str) {
        for piece in text.split_inclusive('\n') {
            if let Some(line) = piece.strip_suffix('\n') {
                self.partial.push_str(line);
                let line = std::mem::take(&mut self.partial);
                self.line(line);
            } else {
                self.partial.push_str(piece);
            }
        }
    }

    fn line(&mut self, line: String) {
        let line = line.trim_end_matches('\r');
        let line: String = line.chars().take(CHECK_LINE_CHARS).collect();
        self.lines.push_back(line);
        while self.lines.len() > CHECK_TAIL_LINES {
            self.lines.pop_front();
        }
    }

    fn finish(mut self) -> Vec<String> {
        if !self.partial.is_empty() {
            let partial = std::mem::take(&mut self.partial);
            self.line(partial);
        }
        self.lines.into()
    }
}

/// The sessions engine's turn hook: Task 10's checkpoints, then the run
/// groups' note of the turn.
pub(crate) struct GroupTurns {
    pub checkpoints: Checkpoints,
    pub groups: RunGroups,
}

impl TurnCheckpoints for GroupTurns {
    fn turn_start<'a>(
        &'a self,
        chat_id: &'a str,
        cwd: &'a str,
        message_id: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            TurnCheckpoints::turn_start(&self.checkpoints, chat_id, cwd, message_id).await;
            self.groups.note_turn_start(chat_id);
        })
    }

    fn turn_end(&self, chat_id: &str) {
        TurnCheckpoints::turn_end(&self.checkpoints, chat_id);
        self.groups.note_turn_end(chat_id);
    }
}

/// `git diff --numstat` from `from` to `to` (commits or trees), run in
/// `repo`: `(path, additions, deletions)` per path, binary files as 0/0.
pub(crate) async fn numstat(
    repo: &Path,
    from: &str,
    to: &str,
    path: Option<&str>,
) -> Result<Vec<PathStat>, EngineError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["diff", "--no-ext-diff", "--no-renames", "--numstat", "-z"])
        .arg(from)
        .arg(to)
        .arg("--");
    if let Some(path) = path {
        command.arg(format!(":(literal){path}"));
    }
    command.stdin(std::process::Stdio::null());
    let output = command
        .output()
        .await
        .map_err(|e| EngineError::Other(format!("git diff failed: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Other(format!(
            "git diff: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut rows = Vec::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        let text = String::from_utf8_lossy(record);
        let mut fields = text.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        rows.push((
            path.to_string(),
            added.parse().unwrap_or(0),
            deleted.parse().unwrap_or(0),
        ));
    }
    Ok(rows)
}

/// One path's `(path, additions, deletions)`.
pub(crate) type PathStat = (String, u32, u32);

/// The per-path, per-member matrix from each member's numstat.
pub(crate) fn matrix(
    per_member: Vec<(String, Vec<PathStat>)>,
) -> Vec<loams_agentd_proto::groups::ComparisonRow> {
    use loams_agentd_proto::groups::{ComparisonRow, MemberDelta};
    let mut rows: std::collections::BTreeMap<String, Vec<MemberDelta>> = Default::default();
    for (chat_id, stats) in per_member {
        for (path, additions, deletions) in stats {
            rows.entry(path).or_default().push(MemberDelta {
                chat_id: chat_id.clone(),
                additions,
                deletions,
            });
        }
    }
    rows.into_iter()
        .map(|(path, per_member)| ComparisonRow { path, per_member })
        .collect()
}

/// Whether a checkout has any change, untracked files included.
pub(crate) async fn is_dirty(root: &Path) -> Result<bool, EngineError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| EngineError::Other(format!("git status failed: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Other(format!(
            "git status: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(!output.stdout.is_empty())
}

/// `git rev-parse --verify --quiet <rev>^{commit}` in `repo`.
pub(crate) async fn resolve_commit(repo: &Path, rev: &str) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !sha.is_empty()).then_some(sha)
}

/// A member chat's title: the prompt's start and its variant, so no title
/// model runs and the worktree branch keeps its name.
pub(crate) fn member_title(prompt: &str, variant: &RunVariant) -> String {
    let start: String = prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(48)
        .collect();
    let harness = serde_json::to_value(variant.harness)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    match &variant.model {
        Some(model) => format!("{start} · {harness} {model}"),
        None => format!("{start} · {harness}"),
    }
}

/// The commit subject `merge` and `pr` use for a winner's uncommitted work.
pub(crate) fn commit_subject(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Run group result");
    line.trim().chars().take(72).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_lines() {
        let mut tail = Tail::default();
        for n in 0..250 {
            tail.push(&format!("line {n}\r\n"));
        }
        tail.push("partial");
        let lines = tail.finish();
        assert_eq!(lines.len(), CHECK_TAIL_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("line 51"));
        assert_eq!(lines.last().map(String::as_str), Some("partial"));
    }

    #[test]
    fn titles_and_subjects() {
        let variant = RunVariant {
            harness: loams_agentd_proto::HarnessId::Mock,
            model: Some("m-1".into()),
            reasoning: None,
            model_options: Default::default(),
        };
        assert_eq!(
            member_title("fix   the\nbuild", &variant),
            "fix the build · mock m-1"
        );
        assert_eq!(commit_subject("\n  Fix the build\nmore"), "Fix the build");
    }
}
