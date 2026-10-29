//! EngineRpc — the engine-side `RpcService`: sessions + docs + the workspace-doc
//! entity surface.
//!
//! Every arm parses and answers with the types of its row in the method
//! table (`loams_agentd_proto::rpc::METHODS`, through `params_of`, `reply`,
//! `stream_reply` and `watch_stream`), the source of the TypeScript client.
//!
//! Methods (feature-inventory §2):
//! - `ListHarnesses` → `[HarnessDescriptor]`
//! - `ListModels {harness}` → `[Model]`
//! - `QueueCommand {chatId, command}` → `{commandId}` (durable doc command)
//! - `WatchDocMessages {chatId, openingTail?}` → stream of `TranscriptUpdate`:
//!   a `reset` frame, then deltas on every doc change
//! - `WatchChats` → stream of the workspace doc's chat rows
//! - `WatchSessions` → stream of `Session[]`: this engine's live statuses merged with
//!   remote devices' workspace session rows
//! - `Mutate {op, …}` → `{ok}` — workspace entity mutations (createChat, renameChat,
//!   setChatArchived, deleteChat, markChatSeen, …)
//! - `EngineInfo` → `{deviceId, workspaceScope}` — this runtime's fixed identity
//!   and data boundary
//! - `LocalDevice` → `{deviceId}` — legacy engine identity
//! - Repos (§3.5): `ListRepos`, `AddRepo {path}`, `CloneRepo {url}`,
//!   `CreateRepo {name}`, `ListBranches {repoPath}` (default branch first),
//!   `ListFolders {path?}`, `CreateWorktree {repoPath, branch}`, `DeleteWorktree
//!   {repoPath, worktreePath}`; `WatchCheckoutDiffs` → stream of `CheckoutDiff[]`
//! - Workspace files: lazy directory listing, recursive path search, bounded text
//!   reads, hash-guarded writes, and a checkout-scoped filesystem change stream.
//! - Terminals (§3.4): `OpenTerminal {chatId, cols, rows, cwd?}` → `TerminalSession`,
//!   `SubscribeTerminal {terminalId, afterSeq?}` → stream of `TerminalEvent`
//!   (replay then live tail), `WriteTerminal {terminalId, data}`, `ResizeTerminal`,
//!   `CloseTerminal`. M5 is single-user local: per-user owner checks land with
//!   real multi-account auth in M6.
//! - Agent accounts (§3.7): `ListAgentAccounts {forceUsage?}` →
//!   `AgentAccountsSnapshot`, `ActivateAgentAccount`/`ForgetAgentAccount`
//!   `{harness, accountId}` → snapshot, `StartAgentLogin {harness}` →
//!   `{loginId, url, mode}`, `CompleteAgentLogin {loginId, code}` → snapshot,
//!   `PollAgentLogin {loginId}`, `CancelAgentLogin {loginId}`.
//! - Uploads (§3.7): `UploadChunk {uploadId, data, seq?}`,
//!   `UploadCommit {uploadId, fileName}` → `{path}`,
//!   `ReadAttachmentChunk {path, offset}` → `{name, mimeType, data, nextOffset,
//!   done}` (path-jailed to the uploads dir + workspace-known chat cwds).
//!
//! The engine is local-only (D781): the fork's `targetDeviceId` relay forwarding,
//! the edge sync methods and the WorkOS sign-in surface are gone.

use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;

use loams_agentd_doc::MessagePart;
use loams_agentd_proto::params::{
    ApplyHarnessUpdateReply, CancelledReply, ChangedReply, DiscardWorkingTreeReply, Empty,
    EngineReadyReply, IdReply, LocalDeviceReply, MutateParams, MutateReply, OkReply,
    ProjectActionSetup, QueueCommandReply, QueueSnapshot, RemovedReply, SearchFilesParams,
    SentReply, SwitchRefReply, UploadCommitReply,
};
use loams_agentd_proto::rpc::{Method, WireAs, calls};
use loams_agentd_proto::transcript::TranscriptUpdate;
use loams_agentd_proto::{
    CreateWorktreeOutcome, EngineInfo, HarnessId, Space, ToolCall, WorkspaceScope,
};
use loams_agentd_rpc::{RpcError, RpcReply, RpcService, methods, params_of, reply, stream_reply};

use crate::agent_accounts::AgentAccounts;
use crate::change_request_create::ChangeRequestCreator;
use crate::change_requests::CheckoutChangeRequests;
use crate::checkpoints::Checkpoints;
use crate::diff_sync::CheckoutDiffSync;
use crate::doc_host::DocHost;
use crate::git_write::GitWriteError;
use crate::project_actions::ProjectActionsStore;
use crate::registry::HarnessRegistry;
use crate::repos::{Repos, session_home_dir};
use crate::sessions::SessionsEngine;
use crate::terminals::Terminals;
use crate::uploads::Uploads;
use crate::workspace_host::WorkspaceHost;

mod factory_loop_rpc;
mod groups_rpc;

const FILE_SEARCH_RPC_TIMEOUT: Duration = Duration::from_secs(6);
const FILE_SEARCH_FEATURED_PATHS: usize = 32;

async fn update_harness_enabled(
    registry: &HarnessRegistry,
    harness: HarnessId,
    enabled: bool,
) -> Result<(), RpcError> {
    registry
        .set_enabled(harness, enabled)
        .map_err(RpcError::Failed)
}

fn tool_file_path(call: &ToolCall) -> Option<&str> {
    match call {
        ToolCall::ReadFile { path }
        | ToolCall::WriteFile { path, .. }
        | ToolCall::EditFile { path, .. } => Some(path),
        ToolCall::ApplyPatch { path } | ToolCall::Search { path, .. } => path.as_deref(),
        ToolCall::Exec { .. }
        | ToolCall::Glob { .. }
        | ToolCall::WebFetch { .. }
        | ToolCall::WebSearch { .. }
        | ToolCall::Todo { .. }
        | ToolCall::Mcp { .. }
        | ToolCall::Unknown { .. } => None,
    }
}

/// Matches the UI canvas panel key (`AppState::panel_session_key`).
const CANVAS_TERMINAL_PREFIX: &str = "space-canvas:";

/// A cwd the user (or a project-less chat) meant as "host home", not a folder.
fn meaningful_cwd(cwd: Option<String>) -> Option<String> {
    cwd.filter(|cwd| {
        let trimmed = cwd.trim();
        !trimmed.is_empty() && trimmed != "~"
    })
}

/// Space id encoded in a new-chat canvas terminal key, if any.
fn canvas_space_id(chat_id: &str) -> Option<&str> {
    chat_id
        .strip_prefix(CANVAS_TERMINAL_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Resolve the PTY cwd: a real explicit path wins, then the chat row, then
/// the project folder named by `space-canvas:{spaceId}`, then `~`.
/// The portable `~` marker is a fallback, not an override — otherwise a
/// canvas OpenTerminal that still says `~` (spaces watch not landed in the
/// UI) would ignore the selected project encoded in `chatId`.
fn resolve_open_terminal_cwd(
    explicit: Option<String>,
    chat_cwd: Option<String>,
    space_cwd: Option<String>,
) -> Result<String, &'static str> {
    let raw = meaningful_cwd(explicit)
        .or_else(|| meaningful_cwd(chat_cwd))
        .or_else(|| meaningful_cwd(space_cwd))
        .unwrap_or_else(|| "~".to_string());
    crate::repos::expand_home(&raw)
}

pub struct EngineRpc {
    sessions: SessionsEngine,
    doc_host: DocHost,
    workspace: WorkspaceHost,
    registry: std::sync::Arc<HarnessRegistry>,
    repos: Repos,
    workspace_files: crate::WorkspaceFiles,
    terminals: Terminals,
    project_actions: ProjectActionsStore,
    previews: Option<loams_agentd_preview::PreviewService>,
    change_requests: CheckoutChangeRequests,
    diff_sync: CheckoutDiffSync,
    uploads: Uploads,
    agent_accounts: AgentAccounts,
    harness_updates: Option<crate::harness_updates::HarnessUpdateCoordinator>,
    checkpoints: Option<Checkpoints>,
    /// Parallel run groups (SD1 Task 12; `rpc/groups_rpc.rs`).
    run_groups: Option<crate::groups::RunGroups>,
    /// Studio's notifications (SD1 Task 14, G19).
    notifications: Option<crate::notify::Notifications>,
    /// Per-turn usage (SD1 Task 14, G20).
    usage: Option<crate::usage::ThreadUsageLog>,
    /// Supervised approvals (SD1 Task 13, G17, G18, G22).
    approvals: Option<crate::approvals::Approvals>,
    engine_info: EngineInfo,
    /// Opens pull requests (G4) with the daemon's forge credentials.
    change_request_creator: ChangeRequestCreator,
    /// Deploys to Loams Live for the factory (FT0 Task 8) with the daemon's
    /// Live credential.
    live_deployer: crate::live_deploy::LiveDeployer,
    /// Serializes the git writes of SD1 Task 9 (G2, G3, G5, G21): one
    /// checksum check and its write at a time.
    git_writes: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Studio's review comments (SD1 Task 11, G8-G11), in the engine's store.
    review: crate::review::ReviewComments,
}

/// A git write's failure as the RPC's.
fn git_write_error(error: GitWriteError) -> RpcError {
    match error {
        GitWriteError::BadParams(text) => RpcError::BadParams(text),
        GitWriteError::Failed(text) => RpcError::Failed(text),
    }
}

/// A review-comment call's failure as the RPC's (`not_found: …` for a
/// comment the checkout doesn't have).
fn review_error(error: crate::review::ReviewError) -> RpcError {
    match error {
        crate::review::ReviewError::BadParams(text) => RpcError::BadParams(text),
        other => RpcError::Failed(other.to_string()),
    }
}

/// Where a fork of a chat's transcript ends.
#[derive(Debug, Clone, Copy)]
enum ForkAt<'a> {
    /// After the last complete assistant response (`ForkSideChat`).
    LastResponse,
    /// Just before the entry of this id: a checkpoint's turn
    /// (`RestoreCheckpoint` with `worktree+transcript`).
    BeforeTurn(&'a str),
}

impl std::fmt::Debug for EngineRpc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineRpc").finish_non_exhaustive()
    }
}

impl EngineRpc {
    #[allow(clippy::too_many_arguments)] // engine assembly seam, not a public API
    pub fn new(
        sessions: SessionsEngine,
        doc_host: DocHost,
        workspace: WorkspaceHost,
        registry: std::sync::Arc<HarnessRegistry>,
        repos: Repos,
        workspace_files: crate::WorkspaceFiles,
        terminals: Terminals,
        project_actions: ProjectActionsStore,
        change_requests: CheckoutChangeRequests,
        diff_sync: CheckoutDiffSync,
        uploads: Uploads,
        agent_accounts: AgentAccounts,
        workspace_scope: WorkspaceScope,
    ) -> Self {
        let review = crate::review::ReviewComments::new(doc_host.store());
        let engine_info = EngineInfo {
            device_id: doc_host.device_id().to_string(),
            workspace_scope,
            capabilities: loams_agentd_proto::capabilities::current(),
        };
        Self {
            sessions,
            doc_host,
            workspace,
            registry,
            repos,
            workspace_files,
            terminals,
            project_actions,
            previews: None,
            change_requests,
            diff_sync,
            uploads,
            agent_accounts,
            harness_updates: None,
            checkpoints: None,
            run_groups: None,
            notifications: None,
            usage: None,
            approvals: None,
            engine_info,
            change_request_creator: ChangeRequestCreator::from_keyring(),
            live_deployer: crate::live_deploy::LiveDeployer::from_keyring(),
            git_writes: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            review,
        }
    }

    pub fn with_notifications(mut self, notifications: crate::notify::Notifications) -> Self {
        self.notifications = Some(notifications);
        self
    }

    pub fn with_approvals(mut self, approvals: crate::approvals::Approvals) -> Self {
        self.approvals = Some(approvals);
        self
    }

    /// The approvals, or a failure when the engine has none.
    pub(crate) fn approvals(&self) -> Result<&crate::approvals::Approvals, RpcError> {
        self.approvals
            .as_ref()
            .ok_or_else(|| RpcError::Failed("approvals unavailable".into()))
    }

    pub fn with_usage(mut self, usage: crate::usage::ThreadUsageLog) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_checkpoints(mut self, checkpoints: Checkpoints) -> Self {
        self.checkpoints = Some(checkpoints);
        self
    }

    pub fn with_run_groups(mut self, run_groups: crate::groups::RunGroups) -> Self {
        self.run_groups = Some(run_groups);
        self
    }

    fn checkpoints(&self) -> Result<&Checkpoints, RpcError> {
        self.checkpoints
            .as_ref()
            .ok_or_else(|| RpcError::Failed("checkpoints unavailable".into()))
    }

    /// The git checkout a chat runs in, from the chat's own row (or, for a
    /// chat with no row yet, its last run), never from the caller. `None`
    /// when that directory is not a checkout.
    async fn chat_checkout_root(
        &self,
        chat_id: &str,
    ) -> Result<Option<std::path::PathBuf>, RpcError> {
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        let cwd = match self.workspace.chat(chat_id).map_err(failed)? {
            Some(chat) => {
                if chat.device_id != self.doc_host.device_id() {
                    return Err(RpcError::Failed(
                        "Checkpoints live on the chat's own device".into(),
                    ));
                }
                chat.cwd
            }
            None => self.sessions.last_request(chat_id).map(|r| r.cwd),
        }
        .ok_or_else(|| RpcError::Failed(format!("not_found: chat {chat_id}")))?;
        let cwd = crate::repos::expand_home(&cwd).map_err(|e| RpcError::Failed(e.to_owned()))?;
        Ok(self
            .checkpoints()?
            .checkout_root(std::path::Path::new(&cwd))
            .await)
    }

    /// Where a fork of `source_chat_id` at `at` ends (exclusive).
    fn fork_end(&self, source_chat_id: &str, at: ForkAt<'_>) -> Result<usize, RpcError> {
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        let entries = self
            .doc_host
            .open(source_chat_id)
            .map_err(failed)?
            .doc()
            .read_entries()
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        match at {
            ForkAt::LastResponse => entries
                .iter()
                .rposition(|entry| {
                    entry.role == loams_agentd_doc::MessageRole::Assistant
                        && entry.status == Some(loams_agentd_doc::MessageStatus::Complete)
                })
                .map(|boundary| boundary + 1)
                .ok_or_else(|| {
                    RpcError::Failed(
                        "Wait for a completed response before starting a side chat".into(),
                    )
                }),
            ForkAt::BeforeTurn(turn_id) => entries
                .iter()
                .position(|entry| entry.id == turn_id)
                .ok_or_else(|| {
                    RpcError::Failed(format!(
                        "{}: turn {turn_id:?} is not in the transcript",
                        loams_agentd_proto::checkpoints::CHECKPOINT_MISSING
                    ))
                }),
        }
    }

    /// The source chat of a fork, checked to live on this device.
    fn fork_source(&self, source_chat_id: &str) -> Result<loams_agentd_proto::Chat, RpcError> {
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        let source = self
            .workspace
            .chat(source_chat_id)
            .map_err(failed)?
            .ok_or_else(|| RpcError::Failed("Source chat no longer exists".into()))?;
        if source.device_id != self.doc_host.device_id() {
            return Err(RpcError::Failed(
                "Fork must be created on the source device".into(),
            ));
        }
        Ok(source)
    }

    /// Copy `source_chat_id`'s transcript up to `at` into a new chat
    /// `chat_id` hung under `parent_chat_id` (default: the source). The
    /// source is never changed.
    fn fork_chat(
        &self,
        source_chat_id: &str,
        chat_id: String,
        parent_chat_id: Option<String>,
        at: ForkAt<'_>,
    ) -> Result<loams_agentd_proto::Chat, RpcError> {
        let parent_chat_id = parent_chat_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| source_chat_id.to_owned());
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        let source = self.fork_source(source_chat_id)?;
        if let Some(existing) = self.workspace.chat(&chat_id).map_err(failed)? {
            if existing.parent_chat_id.as_deref() == Some(parent_chat_id.as_str()) {
                return Ok(existing);
            }
            return Err(RpcError::Failed("Chat id already exists".into()));
        }
        let end = self.fork_end(source_chat_id, at)?;
        let source_doc = self.doc_host.open(source_chat_id).map_err(failed)?;
        let entries = source_doc
            .doc()
            .read_entries()
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        let end = end.min(entries.len());
        let mut chat = source.clone();
        chat.id = chat_id;
        chat.parent_chat_id = Some(parent_chat_id);
        chat.title = None; // First side-chat turn receives its own generated title.
        chat.archived = false;
        chat.created_at = chrono::Utc::now();
        chat.last_message_at = None;
        chat.last_message_preview = None;
        chat.last_seen_at = None;
        chat.harness_session_id = None;
        chat.harness_session_cwd = None;
        chat.room_gen = Some(2);
        // Missing rows open on chat2. Persist history before publishing
        // the registry row so a crash cannot leave a discoverable empty fork.
        let target = self.doc_host.open(&chat.id).map_err(failed)?;
        let existing = target
            .doc()
            .read_entries()
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        for entry in entries[..end]
            .iter()
            .filter(|entry| !existing.iter().any(|e| e.id == entry.id))
        {
            let mut entry = entry.clone();
            // Historical approvals belong to the source runtime; they
            // must never block or send answers from the new composer.
            for part in &mut entry.parts {
                if let loams_agentd_doc::MessagePart::Input { resolved, .. } = part {
                    *resolved = true;
                }
            }
            target
                .doc()
                .push_message(&entry)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
        }
        // The seam: everything above came from the source. Its own
        // entry (system role, complete) so the copied history and the
        // fork's first turn never share a row.
        let marker_id = format!("fork:{}", chat.id);
        if !existing.iter().any(|e| e.id == marker_id) {
            let source_title = source
                .title
                .clone()
                .or_else(|| source.last_message_preview.clone())
                .unwrap_or_else(|| "New session".into());
            target
                .doc()
                .push_message(&loams_agentd_doc::SessionMessageEntry {
                    duration_ms: None,
                    id: marker_id.clone(),
                    role: loams_agentd_doc::MessageRole::System,
                    parts: vec![loams_agentd_doc::MessagePart::Fork {
                        id: marker_id,
                        source_chat_id: source.id.clone(),
                        source_title,
                    }],
                    created_at: chrono::Utc::now().timestamp_millis(),
                    device_id: self.doc_host.device_id().to_owned(),
                    status: Some(loams_agentd_doc::MessageStatus::Complete),
                    continuation_of: None,
                })
                .map_err(|e| RpcError::Failed(e.to_string()))?;
        }
        self.doc_host.persist_fork(&target).map_err(failed)?;
        // A fork keeps its source's approval mode whatever parent it hangs
        // under: a supervised thread's fork is never quietly full access
        // (SD1 Task 30). Recorded before the row is published, so no run of
        // the fork sees another mode.
        if let Some(approvals) = &self.approvals {
            let (mode, set) = approvals.resolved_mode(source_chat_id);
            if set {
                approvals.set_mode(&chat.id, mode).map_err(failed)?;
            }
        }
        self.workspace.import_chat_row(&chat).map_err(failed)?;
        Ok(chat)
    }

    /// `RestoreCheckpoint` (SD1 Task 10, G7). The restore is confirmed by
    /// the checkout's diff checksum the caller saw (a stale one is
    /// `stale_diff`), runs only in the chat's own checkout, never while the
    /// chat's turn is running, and a transcript restore forks the thread:
    /// nothing is deleted from its history.
    async fn restore_checkpoint(
        &self,
        p: loams_agentd_proto::checkpoints::RestoreCheckpointParams,
    ) -> Result<RpcReply, RpcError> {
        use loams_agentd_proto::SessionStatus;
        use loams_agentd_proto::checkpoints::{CHECKPOINT_MISSING, CheckpointScope};
        let checkpoints = self.checkpoints()?;
        if p.expected_checksum.trim().is_empty() {
            return Err(RpcError::BadParams(
                "expectedChecksum is required: a restore discards the checkout's current state"
                    .into(),
            ));
        }
        if self
            .sessions
            .session_status(&p.chat_id)
            .is_some_and(|session| {
                matches!(
                    session.status,
                    SessionStatus::Working | SessionStatus::AwaitingInput
                )
            })
        {
            return Err(RpcError::Failed(
                "The thread is running; interrupt it before restoring a checkpoint".into(),
            ));
        }
        let root = self.chat_checkout_root(&p.chat_id).await?.ok_or_else(|| {
            RpcError::Failed(format!(
                "{CHECKPOINT_MISSING}: chat {:?} has no git checkout",
                p.chat_id
            ))
        })?;
        let fork = p.scope == CheckpointScope::WorktreeAndTranscript;
        if fork {
            // Check the fork can be made before the checkout changes.
            self.fork_source(&p.chat_id)?;
            self.fork_end(&p.chat_id, ForkAt::BeforeTurn(&p.turn_id))?;
        }
        let restored_to = checkpoints
            .restore_worktree(&p.chat_id, &root, &p.turn_id, &p.expected_checksum)
            .await
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        self.diff_sync.sync_all();
        let forked_chat_id = if fork {
            let chat = self.fork_chat(
                &p.chat_id,
                crate::new_id(),
                None,
                ForkAt::BeforeTurn(&p.turn_id),
            )?;
            Some(chat.id)
        } else {
            None
        };
        reply::<calls::RestoreCheckpoint, _>(
            &loams_agentd_proto::checkpoints::RestoreCheckpointReply {
                restored_to,
                forked_chat_id,
            },
        )
    }

    /// Open change requests with `creator` (tests: a fake Forgejo or `gh`).
    pub fn with_change_request_creator(mut self, creator: ChangeRequestCreator) -> Self {
        self.change_request_creator = creator;
        self
    }

    /// Deploy to Live with `deployer` (tests: a fake Live).
    pub fn with_live_deployer(mut self, deployer: crate::live_deploy::LiveDeployer) -> Self {
        self.live_deployer = deployer;
        self
    }

    pub fn with_previews(mut self, previews: loams_agentd_preview::PreviewService) -> Self {
        self.previews = Some(previews);
        self
    }

    pub fn with_harness_updates(
        mut self,
        coordinator: crate::harness_updates::HarnessUpdateCoordinator,
    ) -> Self {
        self.harness_updates = Some(coordinator);
        self
    }

    fn harness_updates(
        &self,
    ) -> Result<&crate::harness_updates::HarnessUpdateCoordinator, RpcError> {
        self.harness_updates
            .as_ref()
            .ok_or_else(|| RpcError::Failed("agent updates unavailable".into()))
    }

    /// The chats (and their cwds) a `deleteChat` or `deleteSpace` removes,
    /// read before the mutation so their checkpoints can follow them.
    fn chats_deleted_by(&self, params: &MutateParams) -> Vec<(String, Option<String>)> {
        if self.checkpoints.is_none() {
            return Vec::new();
        }
        match params {
            MutateParams::DeleteChat { chat_id } => {
                let cwd = self
                    .workspace
                    .chat(chat_id)
                    .ok()
                    .flatten()
                    .and_then(|c| c.cwd);
                vec![(chat_id.clone(), cwd)]
            }
            MutateParams::DeleteSpace { space_id } => self
                .workspace
                .read_chats()
                .unwrap_or_default()
                .into_iter()
                .filter(|chat| chat.space_id.as_deref() == Some(space_id.as_str()))
                .map(|chat| (chat.id, chat.cwd))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Delete the checkpoint refs of deleted chats (best-effort: the
    /// deletion itself already succeeded).
    async fn delete_checkpoints(&self, chats: Vec<(String, Option<String>)>) {
        let Some(checkpoints) = &self.checkpoints else {
            return;
        };
        for (chat_id, cwd) in chats {
            let cwd = cwd.and_then(|cwd| crate::repos::expand_home(&cwd).ok());
            if let Err(error) = checkpoints
                .delete_chat(&chat_id, cwd.as_deref().map(std::path::Path::new))
                .await
            {
                tracing::warn!(chat = %chat_id, %error, "checkpoint refs not deleted");
            }
        }
    }

    fn local_project_action_space(&self, space_id: &str) -> Result<Space, RpcError> {
        let space = self
            .workspace
            .space(space_id)
            .map_err(|err| RpcError::Failed(err.to_string()))?
            .ok_or_else(|| RpcError::Failed("Project space not found".into()))?;
        if space.device_id != self.doc_host.device_id() {
            return Err(RpcError::Failed(
                "Project space belongs to another device".into(),
            ));
        }
        Ok(space)
    }

    /// Resolve a mention-search root from synced workspace rows. A client may
    /// name an existing linked worktree for a new chat, but it is verified
    /// against the space repository before any filesystem walk begins.
    async fn file_search_root(
        &self,
        p: &SearchFilesParams,
    ) -> Result<std::path::PathBuf, RpcError> {
        let target = loams_agentd_proto::WorkspaceTarget {
            chat_id: p.chat_id.clone(),
            space_id: p.space_id.clone(),
            checkout_path: p.path.clone(),
        };
        self.workspace_files
            .resolve_target(&target)
            .await
            .map(|workspace| workspace.root)
            .map_err(Into::into)
    }

    /// Catalogs also serve projectless sessions, which have no workspace space.
    /// Keep workspace validation for project targets and use only a known local
    /// chat's persisted cwd (or home) for a projectless conversation.
    async fn catalog_root(&self, p: &SearchFilesParams) -> Result<std::path::PathBuf, RpcError> {
        if p.space_id.is_none() && p.path.is_none() {
            let Some(chat_id) = &p.chat_id else {
                return session_home_dir().map_err(|error| RpcError::Failed(error.to_string()));
            };
            let chat = self
                .workspace
                .chat(chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("chat not found".into()))?;
            if chat.device_id != self.doc_host.device_id() {
                return Err(RpcError::BadParams("chat belongs to another device".into()));
            }
            if chat.space_id.is_none() {
                let cwd = chat.cwd.as_deref().unwrap_or("~");
                return crate::repos::expand_home(cwd)
                    .map(std::path::PathBuf::from)
                    .map_err(|error| RpcError::Failed(error.to_string()));
            }
        }
        self.file_search_root(p).await
    }

    /// Accept only a checkout already named by a local chat or contained in a
    /// local space. Remote clients must not turn this RPC into an arbitrary path probe.
    async fn change_request_root(&self, cwd: &str) -> Result<std::path::PathBuf, RpcError> {
        let requested = std::path::PathBuf::from(cwd);
        let local_device = self.doc_host.device_id();
        let mut chats_rx = self.workspace.watch_chats();
        let mut spaces_rx = self.workspace.watch_spaces();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        loop {
            let chats = chats_rx.borrow_and_update().clone();
            if chats.iter().any(|chat| {
                chat.device_id == local_device
                    && chat.cwd.as_deref().map(std::path::Path::new) == Some(requested.as_path())
            }) {
                return Ok(requested);
            }

            let spaces = spaces_rx.borrow_and_update().clone();
            for space in spaces
                .iter()
                .filter(|space| space.device_id == local_device)
            {
                if let Some(checkout) = self
                    .repos
                    .workspace_checkout(std::path::Path::new(&space.path), &requested)
                    .await
                {
                    return Ok(checkout);
                }
            }

            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::select! {
                _ = chats_rx.changed() => {}
                _ = spaces_rx.changed() => {}
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        Err(RpcError::BadParams(
            "cwd is not a known checkout on this device".into(),
        ))
    }

    /// The identity of a checkout this device knows (a local chat's cwd, a
    /// local project's or a registered repository's root, or one of their
    /// worktrees): the jail of the git writes that name a `cwd`.
    async fn known_checkout(&self, cwd: &str) -> Result<crate::repos::CheckoutIdentity, RpcError> {
        let mut root = None;
        for repo in self.repos.list().await {
            if let Some(checkout) = self
                .repos
                .workspace_checkout(std::path::Path::new(&repo.path), std::path::Path::new(cwd))
                .await
            {
                root = Some(checkout);
                break;
            }
        }
        let root = match root {
            Some(root) => root,
            None => self.change_request_root(cwd).await?,
        };
        self.repos
            .checkout_identity(&root)
            .await
            .map_err(|e| RpcError::Failed(e.to_string()))
    }

    /// A repository this device knows: a known checkout (as
    /// [`Self::known_checkout`]) or a registered repository (`AddRepo`).
    async fn known_repo(&self, repo_path: &str) -> Result<std::path::PathBuf, RpcError> {
        let unknown =
            || RpcError::BadParams("repoPath is not a known repository on this device".into());
        let canonical = std::fs::canonicalize(repo_path).map_err(|_| unknown())?;
        for repo in self.repos.list().await {
            if std::fs::canonicalize(&repo.path).is_ok_and(|path| path == canonical) {
                return Ok(canonical);
            }
        }
        self.change_request_root(repo_path)
            .await
            .map_err(|_| unknown())
    }

    /// Refuse a write to a checkout while any local chat on it has a live
    /// run: a git write never interrupts an agent as a side effect.
    async fn ensure_no_live_run(
        &self,
        identity: &crate::repos::CheckoutIdentity,
    ) -> Result<(), RpcError> {
        let chats = self.workspace.watch_chats().borrow().clone();
        for candidate in chats {
            if candidate.device_id != self.doc_host.device_id() {
                continue;
            }
            let same_checkout = if candidate.checkout_id.as_deref() == Some(identity.id.as_str()) {
                true
            } else if let Some(candidate_cwd) = candidate.cwd.as_deref() {
                self.repos
                    .checkout_identity(std::path::Path::new(candidate_cwd))
                    .await
                    .is_ok_and(|candidate_identity| candidate_identity.id == identity.id)
            } else {
                false
            };
            if same_checkout
                && self
                    .sessions
                    .session_status(&candidate.id)
                    .is_some_and(|session| {
                        matches!(
                            session.status,
                            loams_agentd_proto::SessionStatus::Working
                                | loams_agentd_proto::SessionStatus::AwaitingInput
                        )
                    })
            {
                return Err(RpcError::Failed(
                    "an agent is active in this working tree".into(),
                ));
            }
        }
        Ok(())
    }

    /// The root of the checkout `checkout_id` names, if it is one this
    /// device knows: a checkout the diff sync tracks (one with a chat), or a
    /// worktree of a registered repository or a local project. The jail of
    /// the review comments (SD1 Task 11).
    async fn review_checkout_root(
        &self,
        checkout_id: &str,
    ) -> Result<std::path::PathBuf, RpcError> {
        if let Some(root) = self.diff_sync.checkout_root(checkout_id) {
            return Ok(root);
        }
        let mut repos: Vec<std::path::PathBuf> = self
            .repos
            .list()
            .await
            .into_iter()
            .map(|repo| std::path::PathBuf::from(repo.path))
            .collect();
        let device = self.doc_host.device_id().to_owned();
        let spaces: Vec<std::path::PathBuf> = self
            .workspace
            .watch_spaces()
            .borrow()
            .iter()
            .filter(|space| space.device_id == device)
            .map(|space| std::path::PathBuf::from(&space.path))
            .collect();
        repos.extend(spaces);
        for repo in repos {
            let Ok(stanzas) = crate::git_write::worktrees(&repo).await else {
                continue;
            };
            for stanza in stanzas {
                if let Ok(identity) = self.repos.checkout_identity(&stanza.path).await
                    && identity.id == checkout_id
                {
                    return Ok(identity.root);
                }
            }
        }
        Err(RpcError::BadParams(
            "checkoutId is not a known checkout on this device".into(),
        ))
    }

    /// This device's chats.
    fn local_chats(&self) -> Vec<loams_agentd_proto::Chat> {
        let device = self.doc_host.device_id();
        self.workspace
            .watch_chats()
            .borrow()
            .iter()
            .filter(|chat| chat.device_id == device)
            .cloned()
            .collect()
    }

    /// Most-recent-first paths the current chat actually touched, followed by
    /// files still changed in its checkout. The search worker validates and
    /// normalizes them against the resolved root before using them as ranking
    /// hints, so stale or out-of-workspace tool paths simply disappear.
    fn featured_file_paths(&self, chat_id: &str) -> Vec<String> {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(handle) = self.doc_host.open(chat_id)
            && let Ok(entries) = handle.doc().read_entries()
        {
            for entry in entries.into_iter().rev() {
                for part in entry.parts.into_iter().rev() {
                    if let MessagePart::Tool { call, .. } = part
                        && let Some(path) = tool_file_path(&call)
                        && !path.trim().is_empty()
                        && seen.insert(path.to_string())
                    {
                        paths.push(path.to_string());
                        if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                            break;
                        }
                    }
                }
                if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                    break;
                }
            }
        }

        if let Ok(Some(chat)) = self.workspace.chat(chat_id) {
            let diffs = self.diff_sync.watch_diffs().borrow().clone();
            let diff = chat
                .checkout_id
                .as_deref()
                .and_then(|id| diffs.iter().find(|diff| diff.checkout_id == id))
                .or_else(|| {
                    chat.cwd
                        .as_deref()
                        .and_then(|cwd| diffs.iter().find(|diff| diff.cwd == cwd))
                });
            if let Some(diff) = diff {
                for file in &diff.files {
                    if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                        break;
                    }
                    if seen.insert(file.path.clone()) {
                        paths.push(file.path.clone());
                    }
                }
            }
        }
        paths
    }

    fn mutate(&self, params: MutateParams) -> Result<(), RpcError> {
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        // A client may not pick a harness this daemon does not ship.
        let picked = match &params {
            MutateParams::CreateChat { config, .. } => config.as_ref(),
            MutateParams::SetChatConfig { config, .. } => Some(config),
            _ => None,
        };
        if picked.is_some_and(|config| config.harness == HarnessId::Unsupported) {
            return Err(RpcError::BadParams("unknown harness".into()));
        }
        match params {
            MutateParams::CreateChat {
                chat_id,
                space_id,
                device_id,
                config,
                branch,
                cwd,
                parent_chat_id,
            } => {
                self.workspace
                    .create_chat_with_parent(
                        &chat_id,
                        space_id.as_deref(),
                        device_id.as_deref(),
                        config,
                        cwd,
                        parent_chat_id,
                    )
                    .map_err(failed)?;
                if let Some(branch) = branch.as_deref().filter(|b| !b.is_empty()) {
                    self.workspace
                        .set_chat_branch(&chat_id, branch)
                        .map_err(failed)?;
                }
                Ok(())
            }
            MutateParams::CreateSpace {
                space_id,
                device_id,
                path,
                name,
                git_detected,
            } => self
                .workspace
                .create_space(&space_id, &device_id, &path, name, git_detected)
                .map_err(failed),
            MutateParams::RenameSpace { space_id, name } => self
                .workspace
                .rename_space(&space_id, name.as_deref())
                .map_err(failed)
                .map(drop),
            MutateParams::DeleteSpace { space_id } => {
                let deleted = self.workspace.delete_space(&space_id).map_err(failed)?;
                // Best-effort teardown of live runs we host for the deleted chats
                // (the doc rows are already tombstoned; a straggler run would only
                // write into an orphaned session doc).
                let sessions = self.sessions.clone();
                let doc_host = self.doc_host.clone();
                let chat_ids = deleted.chat_ids;
                tokio::spawn(async move {
                    for chat_id in chat_ids {
                        if let Err(err) = sessions.interrupt(&chat_id).await {
                            tracing::debug!(chat = %chat_id, error = %err, "deleteSpace interrupt skipped");
                        }
                        doc_host.purge_chat(&chat_id);
                    }
                });
                Ok(())
            }
            MutateParams::RenameChat { chat_id, title } => self
                .workspace
                .rename_chat(&chat_id, &title)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatBranch { chat_id, branch } => self
                .workspace
                .set_chat_branch(&chat_id, &branch)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatCwd { chat_id, cwd } => self
                .workspace
                .set_chat_cwd(&chat_id, &cwd)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatActivity {
                chat_id,
                last_message_at,
                created_at,
            } => self
                .workspace
                .set_chat_activity(&chat_id, last_message_at, created_at)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatHost { chat_id, device_id } => self
                .workspace
                .set_chat_host(&chat_id, &device_id)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatArchived { chat_id, archived } => self
                .workspace
                .set_chat_archived(&chat_id, archived)
                .map_err(failed)
                .map(drop),
            MutateParams::ChangeSidebarPin { change } => {
                self.workspace.change_sidebar_pin(&change).map_err(failed)
            }
            MutateParams::SetChatConfig { chat_id, config } => self
                .workspace
                .set_chat_config(&chat_id, &config)
                .map_err(failed)
                .map(drop),
            MutateParams::DeleteChat { chat_id } => {
                self.workspace.delete_chat(&chat_id).map_err(failed)?;
                self.doc_host.purge_chat(&chat_id);
                if let Some(usage) = &self.usage {
                    usage.forget(&chat_id);
                }
                if let Some(approvals) = &self.approvals {
                    approvals.forget_chat(&chat_id);
                }
                Ok(())
            }
            MutateParams::MarkChatSeen { chat_id, at } => {
                let at = at
                    .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                    .unwrap_or_else(chrono::Utc::now);
                self.workspace
                    .mark_chat_seen(&chat_id, at)
                    .map_err(failed)
                    .map(drop)
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Installations(
    std::sync::Mutex<std::collections::HashMap<HarnessId, loams_agentd_harness::CancellationToken>>,
);

struct Installing<'a> {
    installs: &'a Installations,
    harness: HarnessId,
    cancel: loams_agentd_harness::CancellationToken,
}
impl Drop for Installing<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.installs
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.harness);
    }
}
impl Installations {
    fn begin(&self, harness: HarnessId) -> Result<Installing<'_>, RpcError> {
        let mut installs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if installs.contains_key(&harness) {
            return Err(RpcError::Failed("already installing".into()));
        }
        let cancel = loams_agentd_harness::CancellationToken::new();
        installs.insert(harness, cancel.clone());
        Ok(Installing {
            installs: self,
            harness,
            cancel,
        })
    }
    fn cancel(&self, harness: HarnessId) {
        if let Some(cancel) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&harness)
        {
            cancel.cancel();
        }
    }
}

async fn run_requested_install(
    harness: HarnessId,
    cancel: loams_agentd_harness::CancellationToken,
) -> Result<(), loams_agentd_harness::HarnessError> {
    #[cfg(test)]
    if let Ok(script) =
        std::env::var(format!("LOAMS_DESKTOP_INSTALLER_COMMAND_{harness:?}").to_uppercase())
    {
        return loams_agentd_harness::install::install_with_command(harness, &script, cancel).await;
    }
    loams_agentd_harness::install::install_harness(harness, cancel).await
}

async fn install_harness_with<F, Fut>(
    registry: &HarnessRegistry,
    harness: HarnessId,
    install: F,
) -> Result<Vec<crate::registry::HarnessDescriptor>, RpcError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), loams_agentd_harness::HarnessError>>,
{
    if !loams_agentd_harness::install::can_install(harness) {
        return Err(RpcError::Failed(
            "No supported installer or required tools available on this device".into(),
        ));
    }
    install()
        .await
        .map_err(|error| RpcError::Failed(error.to_string()))?;
    Ok(registry.descriptors())
}

/// A watch receiver as method `M`'s stream: current value first, then every
/// change. Each value is serialized straight from the watch (no copy).
fn watch_stream<M, T>(rx: watch::Receiver<T>) -> RpcReply
where
    M: Method,
    T: serde::Serialize + WireAs<M::Result> + Send + Sync + 'static,
{
    RpcReply::Stream(
        futures::stream::unfold((rx, false), |(mut rx, emitted)| async move {
            if emitted {
                rx.changed().await.ok()?;
            }
            let value = {
                let borrowed = rx.borrow_and_update();
                serde_json::to_value(&*borrowed).ok()?
            };
            Some((value, (rx, true)))
        })
        .boxed(),
    )
}

/// The transcript watch as delta frames (`loams_agentd_doc::transcript_delta`): a
/// full `reset` first, then only changed entries per commit — the whole-Vec
/// serialization here was the per-tick cost that scaled with transcript size.
fn doc_messages_stream(
    rx: watch::Receiver<crate::doc_host::TranscriptSnapshot>,
    doc: std::sync::Arc<loams_agentd_doc::SessionDoc>,
) -> BoxStream<'static, TranscriptUpdate> {
    use loams_agentd_doc::transcript_delta::{TranscriptFrame, diff_transcript};
    futures::stream::unfold(
        (
            rx,
            None::<crate::doc_host::TranscriptSnapshot>,
            doc,
            None,
            loams_agentd_doc::TranscriptBaseline::default(),
        ),
        |(mut rx, mut prev, doc, mut previous_usage, mut opening_baseline)| async move {
            loop {
                if prev.is_some() {
                    rx.changed().await.ok()?;
                }
                // Watchers retain the immutable published snapshot. Each
                // connection used to deep-copy the entire transcript here.
                let current = rx.borrow_and_update().clone();
                let frame = match prev.as_ref() {
                    None => TranscriptFrame::reset(&current.entries),
                    Some(prev) => diff_transcript(&prev.entries, &current.entries),
                };
                let replay_baseline = match prev.as_ref() {
                    None => {
                        opening_baseline =
                            loams_agentd_doc::TranscriptBaseline::capture(&current.entries);
                        Some(opening_baseline.clone())
                    }
                    Some(prev)
                        if !std::sync::Arc::ptr_eq(
                            &prev.replay_baseline,
                            &current.replay_baseline,
                        ) =>
                    {
                        // The tracker only observes changes after attach; its
                        // baseline omits unchanged cached parts. Preserve this
                        // subscription's opening cutoff without capturing live
                        // appends or sharing another viewer's later cutoff.
                        // Ordinary live updates never rebuild this metadata.
                        let mut baseline = (*current.replay_baseline).clone();
                        for (entry, parts) in &opening_baseline.entries {
                            let merged = baseline.entries.entry(entry.clone()).or_default();
                            for (part, &len) in parts {
                                let cutoff = merged.entry(part.clone()).or_default();
                                *cutoff = (*cutoff).max(len);
                            }
                        }
                        Some(baseline)
                    }
                    _ => None,
                };
                prev = Some(current);
                // No-op commits (a second watcher attaching, command-only
                // changes) produce empty deltas — skip the frame entirely.
                let usage = doc.context_usage();
                if frame.is_empty_delta() && usage == previous_usage && replay_baseline.is_none() {
                    continue;
                }
                previous_usage = usage;
                let update = TranscriptUpdate {
                    frame,
                    context_usage: usage,
                    replay_baseline,
                    history_pending: false,
                };
                return Some((update, (rx, prev, doc, previous_usage, opening_baseline)));
            }
        },
    )
    .boxed()
}

/// First paint reads only the local tail; full history is deferred until the
/// next stream poll. No network dependency or persisted truncation.
async fn opening_doc_messages_stream(
    host: crate::doc_host::DocHost,
    chat_id: String,
) -> Result<BoxStream<'static, TranscriptUpdate>, RpcError> {
    let (handle, preview) = tokio::task::spawn_blocking(move || {
        let handle = host.open(&chat_id)?;
        let entries = handle.doc().read_opening_tail(128)?;
        let preview = TranscriptUpdate {
            frame: loams_agentd_doc::TranscriptFrame::reset(&entries),
            context_usage: handle.doc().context_usage(),
            replay_baseline: Some(loams_agentd_doc::TranscriptBaseline::capture(&entries)),
            history_pending: true,
        };
        Ok::<_, crate::EngineError>((handle, preview))
    })
    .await
    .map_err(|e| RpcError::Failed(e.to_string()))?
    .map_err(|e| RpcError::Failed(e.to_string()))?;
    // Do not build the full mirror before yielding the preview.
    // The next poll attaches normally and begins with a complete
    // authoritative reset; subsequent frames use normal deltas.
    let full = futures::stream::once(async move {
        match tokio::task::spawn_blocking(move || (handle.watch_messages(), handle.doc_arc())).await
        {
            Ok((rx, doc)) => doc_messages_stream(rx, doc),
            Err(error) => {
                tracing::warn!(%error, "transcript opening failed");
                futures::stream::empty().boxed()
            }
        }
    })
    .flatten();
    Ok(futures::stream::once(async move { preview })
        .chain(full)
        .boxed())
}

#[async_trait]
impl RpcService for EngineRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::ENGINE_INFO => reply::<calls::EngineInfo, _>(&self.engine_info),
            methods::ENGINE_READY => {
                reply::<calls::EngineReady, _>(&EngineReadyReply { ready: true })
            }
            methods::LIST_HARNESSES => {
                reply::<calls::ListHarnesses, _>(&self.registry.descriptors())
            }
            methods::INSTALL_HARNESS => {
                let p = params_of::<calls::InstallHarness>(params)?;
                let installing = self.registry.installs.begin(p.harness)?;
                let descriptors = install_harness_with(&self.registry, p.harness, || {
                    run_requested_install(p.harness, installing.cancel.clone())
                })
                .await?;
                reply::<calls::InstallHarness, _>(&descriptors)
            }
            methods::CANCEL_INSTALL => {
                let p = params_of::<calls::CancelInstall>(params)?;
                self.registry.installs.cancel(p.harness);
                reply::<calls::CancelInstall, _>(&Empty {})
            }
            methods::GET_TITLE_SETTINGS => {
                reply::<calls::GetTitleSettings, _>(&self.registry.title_settings())
            }
            methods::SET_TITLE_SETTINGS => {
                let p = params_of::<calls::SetTitleSettings>(params)?;
                self.registry
                    .set_title_settings(p)
                    .map_err(RpcError::Failed)?;
                reply::<calls::SetTitleSettings, _>(&self.registry.title_settings())
            }
            methods::SET_HARNESS_ENABLED => {
                let p = params_of::<calls::SetHarnessEnabled>(params)?;
                update_harness_enabled(&self.registry, p.harness, p.enabled).await?;
                if let Some(coordinator) = &self.harness_updates {
                    coordinator.refresh_enabled();
                }
                // Fresh catalog in the reply: the page repaints from it in one
                // round trip, and a refused/raced toggle self-corrects.
                reply::<calls::SetHarnessEnabled, _>(&self.registry.descriptors())
            }
            methods::LIST_MODELS => {
                let p = params_of::<calls::ListModels>(params)?;
                let lease = std::sync::Arc::new(self.registry.execution_lease(p.harness).await);
                let harness = self
                    .registry
                    .resolve(p.harness)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let models = crate::model_catalogs::list_with_lease(
                    self.repos.data_dir(),
                    harness,
                    p.force,
                    Some(lease),
                )
                .await
                .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListModels, _>(&models)
            }
            methods::LIST_SKILLS => {
                let p = params_of::<calls::ListSkills>(params)?;
                let root = self
                    .catalog_root(&SearchFilesParams {
                        query: String::new(),
                        chat_id: p.chat_id,
                        space_id: p.space_id,
                        path: p.path,
                    })
                    .await?;
                let skills = self
                    .registry
                    .discover_skills(p.harness, &root)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListSkills, _>(&skills)
            }
            methods::LIST_COMMANDS => {
                let p = params_of::<calls::ListCommands>(params)?;
                let root = self
                    .catalog_root(&SearchFilesParams {
                        query: String::new(),
                        chat_id: p.chat_id,
                        space_id: p.space_id,
                        path: p.path,
                    })
                    .await?;
                let commands = self
                    .registry
                    .discover_commands(p.harness, &root)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListCommands, _>(&commands)
            }
            methods::QUEUE_COMMAND => {
                let p = params_of::<calls::QueueCommand>(params)?;
                let command_id = self
                    .doc_host
                    .queue_command(&p.chat_id, p.command)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::QueueCommand, _>(&QueueCommandReply { command_id })
            }
            methods::TAKE_PROJECT_ACTION_SETUP => {
                let p = params_of::<calls::TakeProjectActionSetup>(params)?;
                let outcome = self
                    .project_actions
                    .take_setup_handoff(&p.command_id, &p.chat_id);
                match outcome {
                    Some(outcome) => {
                        reply::<calls::TakeProjectActionSetup, _>(&ProjectActionSetup {
                            ready: true,
                            setup_action: outcome.setup_action,
                            setup_error: outcome.setup_error,
                        })
                    }
                    None => reply::<calls::TakeProjectActionSetup, _>(&ProjectActionSetup {
                        ready: false,
                        setup_action: None,
                        setup_error: None,
                    }),
                }
            }
            methods::RETRY_DELIVERY => {
                let p = params_of::<calls::RetryDelivery>(params)?;
                self.doc_host
                    .retry_delivery(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::RetryDelivery, _>(&Empty {})
            }
            methods::FORK_SIDE_CHAT => {
                let p = params_of::<calls::ForkSideChat>(params)?;
                let chat = self.fork_chat(
                    &p.source_chat_id,
                    p.chat_id,
                    p.parent_chat_id,
                    ForkAt::LastResponse,
                )?;
                reply::<calls::ForkSideChat, _>(&chat)
            }
            methods::LIST_CHECKPOINTS => {
                let p = params_of::<calls::ListCheckpoints>(params)?;
                let list = match self.chat_checkout_root(&p.chat_id).await? {
                    Some(root) => self
                        .checkpoints()?
                        .list(&p.chat_id, &root)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?,
                    None => Default::default(),
                };
                reply::<calls::ListCheckpoints, _>(&list)
            }
            methods::RESTORE_CHECKPOINT => {
                let p = params_of::<calls::RestoreCheckpoint>(params)?;
                self.restore_checkpoint(p).await
            }
            methods::GET_THREAD_USAGE => {
                let p = params_of::<calls::GetThreadUsage>(params)?;
                let usage = match &self.usage {
                    Some(usage) => usage
                        .thread(&p.chat_id)
                        .map_err(|e| RpcError::Failed(e.to_string()))?,
                    None => Default::default(),
                };
                reply::<calls::GetThreadUsage, _>(&usage)
            }
            methods::WATCH_NOTIFICATIONS => {
                params_of::<calls::WatchNotifications>(params)?;
                let notifications = self
                    .notifications
                    .as_ref()
                    .ok_or_else(|| RpcError::Failed("notifications unavailable".into()))?;
                Ok(stream_reply::<calls::WatchNotifications, _, _>(
                    notifications.watch(),
                ))
            }
            methods::WATCH_APPROVALS => {
                let p = params_of::<calls::WatchApprovals>(params)?;
                Ok(stream_reply::<calls::WatchApprovals, _, _>(
                    self.approvals()?.watch(p.chat_id),
                ))
            }
            methods::RESOLVE_APPROVAL => {
                let p = params_of::<calls::ResolveApproval>(params)?;
                if p.call_id.is_empty() {
                    return Err(RpcError::BadParams("callId is required".into()));
                }
                self.approvals()?
                    .resolve(p.chat_id.as_deref(), &p.call_id, p.decision)
                    .map_err(|error| match error {
                        crate::approvals::ResolveError::Ambiguous(_) => {
                            RpcError::BadParams(error.to_string())
                        }
                        other => RpcError::Failed(other.to_string()),
                    })?;
                reply::<calls::ResolveApproval, _>(&OkReply::OK)
            }
            methods::SET_THREAD_APPROVAL_MODE => {
                let p = params_of::<calls::SetThreadApprovalMode>(params)?;
                if p.chat_id.is_empty() {
                    return Err(RpcError::BadParams("chatId is required".into()));
                }
                self.approvals()?
                    .set_mode(&p.chat_id, p.mode)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::SetThreadApprovalMode, _>(&OkReply::OK)
            }
            methods::WATCH_DOC_MESSAGES => {
                let p = params_of::<calls::WatchDocMessages>(params)?;
                // Opt-in: older viewports retain the full-reset contract.
                if p.opening_tail {
                    return Ok(stream_reply::<calls::WatchDocMessages, _, _>(
                        opening_doc_messages_stream(self.doc_host.clone(), p.chat_id).await?,
                    ));
                }
                let handle = self
                    .doc_host
                    .open(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                Ok(stream_reply::<calls::WatchDocMessages, _, _>(
                    doc_messages_stream(handle.watch_messages(), handle.doc_arc()),
                ))
            }
            methods::WATCH_QUEUE => {
                let p = params_of::<calls::WatchQueue>(params)?;
                let handle = self
                    .doc_host
                    .open(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let rx = handle.watch_queue();
                Ok(stream_reply::<calls::WatchQueue, _, _>(
                    futures::stream::unfold((rx, true), |(mut rx, first)| async move {
                        if !first {
                            rx.changed().await.ok()?;
                        }
                        let items = rx.borrow_and_update().clone();
                        Some((QueueSnapshot { items }, (rx, false)))
                    }),
                ))
            }
            methods::QUEUE_MESSAGE => {
                let p = params_of::<calls::QueueMessage>(params)?;
                let id = self
                    .doc_host
                    .queue_message_with_behavior(
                        &p.chat_id,
                        &p.text,
                        p.attachments,
                        p.hold_for_turn_end,
                    )
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::QueueMessage, _>(&IdReply { id })
            }
            methods::UPDATE_QUEUED_MESSAGE => {
                let p = params_of::<calls::UpdateQueuedMessage>(params)?;
                let changed = self
                    .doc_host
                    .update_queued_message(&p.chat_id, &p.id, &p.text)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::UpdateQueuedMessage, _>(&ChangedReply { changed })
            }
            methods::BEGIN_QUEUED_MESSAGE_EDIT => {
                let p = params_of::<calls::BeginQueuedMessageEdit>(params)?;
                let outcome = self
                    .doc_host
                    .begin_queued_message_edit(
                        &p.chat_id,
                        &p.id,
                        &p.editor_device_id,
                        &p.editor_instance_id,
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::BeginQueuedMessageEdit, _>(&outcome)
            }
            methods::RENEW_QUEUED_MESSAGE_EDIT => {
                let p = params_of::<calls::RenewQueuedMessageEdit>(params)?;
                let outcome = self
                    .doc_host
                    .renew_queued_message_edit(&p.chat_id, &p.id, &p.lease_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::RenewQueuedMessageEdit, _>(&outcome)
            }
            methods::FINISH_QUEUED_MESSAGE_EDIT => {
                let p = params_of::<calls::FinishQueuedMessageEdit>(params)?;
                let outcome = self
                    .doc_host
                    .finish_queued_message_edit_with_attachments(
                        &p.chat_id,
                        &p.id,
                        &p.lease_id,
                        p.action,
                        p.text.as_deref(),
                        p.expected_text_hash.as_deref(),
                        p.attachments.as_deref(),
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::FinishQueuedMessageEdit, _>(&outcome)
            }
            methods::MOVE_QUEUED_MESSAGE => {
                let p = params_of::<calls::MoveQueuedMessage>(params)?;
                let changed = self
                    .doc_host
                    .move_queued_message(&p.chat_id, &p.id, p.to_index)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::MoveQueuedMessage, _>(&ChangedReply { changed })
            }
            methods::REMOVE_QUEUED_MESSAGE => {
                let p = params_of::<calls::RemoveQueuedMessage>(params)?;
                let removed = self
                    .doc_host
                    .remove_queued_message(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::RemoveQueuedMessage, _>(&RemovedReply { removed })
            }
            methods::SEND_QUEUED_MESSAGE_NOW => {
                let p = params_of::<calls::SendQueuedMessageNow>(params)?;
                let sent = self
                    .doc_host
                    .send_queued_now(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::SendQueuedMessageNow, _>(&SentReply { sent })
            }
            methods::STEER_QUEUED_MESSAGE_NOW => {
                let p = params_of::<calls::SteerQueuedMessageNow>(params)?;
                let sent = self
                    .doc_host
                    .steer_queued_now(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::SteerQueuedMessageNow, _>(&SentReply { sent })
            }
            methods::WATCH_PREVIEWS => {
                let p = params_of::<calls::WatchPreviews>(params)?;
                if self
                    .workspace
                    .chat(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?
                    .is_none()
                {
                    return Err(RpcError::Failed("Project session not found".into()));
                }
                let previews = self
                    .previews
                    .as_ref()
                    .ok_or_else(|| RpcError::Failed("Preview discovery unavailable".into()))?;
                let changes = previews.catalog().subscribe();
                let chats = self.workspace.watch_chats();
                let workspace = self.workspace.clone();
                // Only services discovered on this device under the chat's checkout.
                let stream = futures::stream::unfold(
                    (changes, chats, true, workspace, p.chat_id),
                    |(mut changes, mut chats, first, workspace, chat_id)| async move {
                        if !first {
                            tokio::select! {
                                result = changes.changed() => { if result.is_err() { return None; } }
                                result = chats.changed() => { if result.is_err() { return None; } }
                            }
                        }
                        let mut snapshot = changes.borrow_and_update().clone();
                        chats.borrow_and_update();
                        let chat = workspace.chat(&chat_id).ok().flatten();
                        let device = chat
                            .as_ref()
                            .map(|c| c.device_id.clone())
                            .unwrap_or_default();
                        let cwd = chat.and_then(|c| c.cwd);
                        let cwd = cwd.map(|cwd| {
                            std::path::PathBuf::from(&cwd)
                                .canonicalize()
                                .unwrap_or_else(|_| cwd.into())
                        });
                        snapshot.project_name = cwd
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned());
                        snapshot.services.retain(|service| {
                            service.device_id == device
                                && cwd.as_ref().is_some_and(|cwd| {
                                    cwd == std::path::Path::new(&service.project_cwd)
                                })
                        });
                        Some((snapshot, (changes, chats, false, workspace, chat_id)))
                    },
                );
                Ok(stream_reply::<calls::WatchPreviews, _, _>(stream))
            }
            methods::WATCH_CHATS => Ok(watch_stream::<calls::WatchChats, _>(
                self.workspace.watch_chats(),
            )),
            methods::WATCH_SIDEBAR_PREFERENCES => {
                Ok(watch_stream::<calls::WatchSidebarPreferences, _>(
                    self.workspace.watch_sidebar_preferences(),
                ))
            }
            methods::WATCH_SPACES => Ok(watch_stream::<calls::WatchSpaces, _>(
                self.workspace.watch_spaces(),
            )),
            methods::WATCH_SESSIONS => {
                // Local live statuses merged with remote devices' workspace rows.
                let merged = self
                    .workspace
                    .merged_sessions_watch(self.sessions.watch_sessions());
                Ok(watch_stream::<calls::WatchSessions, _>(merged))
            }
            methods::LOCAL_DEVICE => reply::<calls::LocalDevice, _>(&LocalDeviceReply {
                device_id: self.doc_host.device_id().to_owned(),
            }),
            methods::WATCH_HARNESS_UPDATES => Ok(watch_stream::<calls::WatchHarnessUpdates, _>(
                self.harness_updates()?.watch(),
            )),
            methods::CHECK_HARNESS_UPDATES => {
                let p = params_of::<calls::CheckHarnessUpdates>(params)?;
                let coordinator = self.harness_updates()?.clone();
                // Provider checks are engine-owned once accepted. If a
                // Settings view closes or a relay drops, the request future
                // may disappear; detaching the work prevents a permanent
                // `Checking` status and still publishes the result by watch.
                let statuses = tokio::spawn(async move {
                    if let Some(harness) = p.harness {
                        coordinator.check_one(harness).await?;
                    } else {
                        coordinator.check_all().await;
                    }
                    Ok::<_, String>(coordinator.snapshot())
                })
                .await
                .map_err(|error| {
                    RpcError::Failed(format!("agent update check task failed: {error}"))
                })?
                .map_err(RpcError::Failed)?;
                reply::<calls::CheckHarnessUpdates, _>(&statuses)
            }
            methods::APPLY_HARNESS_UPDATE => {
                let p = params_of::<calls::ApplyHarnessUpdate>(params)?;
                let version = self
                    .harness_updates()?
                    .apply(p.harness)
                    .await
                    .map_err(RpcError::Failed)?;
                reply::<calls::ApplyHarnessUpdate, _>(&ApplyHarnessUpdateReply {
                    ok: true,
                    version,
                })
            }
            methods::CANCEL_HARNESS_UPDATE => {
                let p = params_of::<calls::CancelHarnessUpdate>(params)?;
                reply::<calls::CancelHarnessUpdate, _>(&CancelledReply {
                    cancelled: self.harness_updates()?.cancel(p.harness),
                })
            }
            methods::DISMISS_HARNESS_UPDATE => {
                let p = params_of::<calls::DismissHarnessUpdate>(params)?;
                reply::<calls::DismissHarnessUpdate, _>(
                    &self.harness_updates()?.dismiss(p.harness, p.version),
                )
            }
            methods::MUTATE => {
                let p = params_of::<calls::Mutate>(params)?;
                let sidebar_pins = matches!(&p, MutateParams::ChangeSidebarPin { .. });
                let deleted_chats = self.chats_deleted_by(&p);
                self.mutate(p)?;
                self.delete_checkpoints(deleted_chats).await;
                if sidebar_pins {
                    return reply::<calls::Mutate, _>(&MutateReply {
                        ok: true,
                        sidebar_preferences: Some(self.workspace.sidebar_preferences_snapshot()),
                    });
                }
                reply::<calls::Mutate, _>(&MutateReply {
                    ok: true,
                    sidebar_preferences: None,
                })
            }
            methods::WATCH_CHECKOUT_DIFFS => Ok(watch_stream::<calls::WatchCheckoutDiffs, _>(
                self.diff_sync.watch_diffs(),
            )),
            methods::WATCH_WORKSPACE_GIT_STATUS => {
                let request = params_of::<calls::WatchWorkspaceGitStatus>(params)?;
                let workspace = self.workspace_files.resolve_target(&request.target).await?;
                let rx = self.diff_sync.watch_git_statuses();
                // Only this authorized checkout crosses the connection. None means
                // unavailable, including plain folders and initial/restarting engines.
                let stream = futures::stream::unfold(
                    (rx, workspace.checkout_id, None, false),
                    |(mut rx, checkout_id, mut previous, mut emitted)| async move {
                        loop {
                            if emitted {
                                rx.changed().await.ok()?;
                            }
                            let next = rx
                                .borrow_and_update()
                                .iter()
                                .find(|s| s.checkout_id == checkout_id)
                                .cloned();
                            if !emitted || previous != next {
                                emitted = true;
                                previous = next.clone();
                                let frame =
                                    loams_agentd_proto::WorkspaceGitStatusFrame { status: next };
                                return Some((frame, (rx, checkout_id, previous, emitted)));
                            }
                        }
                    },
                );
                Ok(stream_reply::<calls::WatchWorkspaceGitStatus, _, _>(stream))
            }
            methods::WATCH_CHECKOUT_CHANGE_REQUEST => {
                let p = params_of::<calls::WatchCheckoutChangeRequest>(params)?;
                let cwd = self.change_request_root(&p.cwd).await?;
                let stream = self
                    .change_requests
                    .watch_for_branch(&cwd, p.branch.as_deref())
                    .await
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                Ok(stream_reply::<calls::WatchCheckoutChangeRequest, _, _>(
                    stream,
                ))
            }
            // One-shot scoped capture for the Changes pane: `branch` diffs the
            // working tree against merge-base(baseRef, HEAD); `turn` diffs the
            // turn-start tree snapshot against the current tree; anything else
            // is the plain working-tree capture.
            methods::GET_CHECKOUT_DIFF => {
                // Keep the scoped-diff future off the dispatcher's stack. The
                // per-commit path adds another nested git-capture future.
                Box::pin(async move {
                    let p = params_of::<calls::GetCheckoutDiff>(params)?;
                    let identity = self
                        .repos
                        .checkout_identity(std::path::Path::new(&p.cwd))
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    let root = identity.root.as_path();
                    let snapshot = match p.mode.as_str() {
                        "branch" => {
                            let base_ref = p
                                .base_ref
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("baseRef required".into()))?;
                            let base = crate::diff_sync::merge_base(root, base_ref)
                                .await
                                .map_err(|e| RpcError::Failed(e.to_string()))?;
                            crate::diff_sync::capture_diff_against(&self.repos, root, Some(&base))
                                .await
                        }
                        // One commit's own changes (History → per-commit tab):
                        // parent (or the empty tree) vs the commit itself.
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            crate::diff_sync::capture_commit_diff(&self.repos, root, sha).await
                        }
                        "turn" => {
                            let chat_id = p
                                .chat_id
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("chatId required".into()))?;
                            let snapshot = self
                                .diff_sync
                                .turn_snapshot(chat_id)
                                .filter(|s| s.root == identity.root)
                                .ok_or_else(|| RpcError::Failed("no turn recorded".into()))?;
                            crate::diff_sync::capture_turn_diff(&self.repos, root, &snapshot.tree)
                                .await
                        }
                        _ => crate::diff_sync::capture_diff(&self.repos, root).await,
                    }
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                    reply::<calls::GetCheckoutDiff, _>(&loams_agentd_proto::CheckoutDiff {
                        checkout_id: identity.id,
                        device_id: self.doc_host.device_id().to_string(),
                        cwd: identity.root.to_string_lossy().to_string(),
                        patch: snapshot.patch,
                        files: snapshot.files,
                        additions: snapshot.additions,
                        deletions: snapshot.deletions,
                        truncated: snapshot.truncated,
                        checksum: snapshot.checksum,
                        updated_at: chrono::Utc::now(),
                    })
                })
                .await
            }
            methods::DISCARD_WORKING_TREE => {
                // This destructive branch performs several nested filesystem
                // futures. Box it so unrelated RPC calls do not inherit that
                // state in the already-large dispatcher stack frame.
                Box::pin(async move {
                    let p = params_of::<calls::DiscardWorkingTree>(params)?;
                    let chat = self
                        .workspace
                        .chat(&p.chat_id)
                        .map_err(|e| RpcError::Failed(e.to_string()))?
                        .ok_or_else(|| RpcError::Failed("chat not found".into()))?;
                    if chat.device_id != self.doc_host.device_id() {
                        return Err(RpcError::Failed("chat is not hosted by this device".into()));
                    }
                    let cwd = chat
                        .cwd
                        .as_deref()
                        .ok_or_else(|| RpcError::Failed("chat has no checkout".into()))?;
                    let identity = self
                        .repos
                        .checkout_identity(std::path::Path::new(cwd))
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    if identity.id != p.checkout_id {
                        return Err(RpcError::Failed(
                            "chat checkout changed since the confirmation was opened".into(),
                        ));
                    }

                    // Refuse the mutation when any local chat on this exact
                    // checkout has a live run. We never interrupt an agent as a
                    // side effect of discarding files.
                    self.ensure_no_live_run(&identity).await?;

                    let snapshot = self
                        .diff_sync
                        .discard_working_tree(&identity.id, &p.expected_checksum)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    reply::<calls::DiscardWorkingTree, _>(&DiscardWorkingTreeReply {
                        ok: true,
                        checksum: snapshot.checksum,
                    })
                })
                .await
            }
            // SD1 Task 9 (§53 §6.5 G1–G5, G21). Each git write names a
            // checkout or repository this device knows, refuses while an
            // agent runs there, and runs under `git_writes`.
            methods::DISCARD_PATHS => {
                Box::pin(async move {
                    let p = params_of::<calls::DiscardPaths>(params)?;
                    for path in &p.paths {
                        crate::git_write::checkout_relative(path).map_err(git_write_error)?;
                    }
                    let root = self
                        .diff_sync
                        .checkout_root(&p.checkout_id)
                        .ok_or_else(|| {
                            RpcError::Failed("checkout is no longer available".into())
                        })?;
                    let identity = self
                        .repos
                        .checkout_identity(&root)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    self.ensure_no_live_run(&identity).await?;
                    let _write = self.git_writes.lock().await;
                    self.diff_sync
                        .discard_paths(&p.checkout_id, p.paths, &p.expected_checksum)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    reply::<calls::DiscardPaths, _>(&Empty {})
                })
                .await
            }
            methods::LIST_WORKTREES => {
                Box::pin(async move {
                    let p = params_of::<calls::ListWorktrees>(params)?;
                    let repo = self.known_repo(&p.repo_path).await?;
                    let chats = self.local_chats();
                    let worktrees =
                        crate::worktree_list::list_worktrees(&self.repos, &repo, &chats)
                            .await
                            .map_err(git_write_error)?;
                    reply::<calls::ListWorktrees, _>(&worktrees)
                })
                .await
            }
            methods::COMMIT_WORKTREE => {
                Box::pin(async move {
                    let p = params_of::<calls::CommitWorktree>(params)?;
                    let identity = self.known_checkout(&p.cwd).await?;
                    if identity.id != p.checkout_id {
                        return Err(RpcError::Failed(format!(
                            "{}: the checkout changed since the commit was confirmed",
                            loams_agentd_proto::git_write::STALE_DIFF
                        )));
                    }
                    self.ensure_no_live_run(&identity).await?;
                    let _write = self.git_writes.lock().await;
                    let committed = crate::git_write::commit(&self.repos, &identity.root, &p)
                        .await
                        .map_err(git_write_error)?;
                    self.diff_sync.sync_all();
                    reply::<calls::CommitWorktree, _>(&committed)
                })
                .await
            }
            methods::PUSH_BRANCH => {
                Box::pin(async move {
                    let p = params_of::<calls::PushBranch>(params)?;
                    let identity = self.known_checkout(&p.cwd).await?;
                    let _write = self.git_writes.lock().await;
                    let pushed = crate::git_write::push(&identity.root, &p)
                        .await
                        .map_err(git_write_error)?;
                    // Remote-tracking refs moved; publish fresh snapshots.
                    self.diff_sync.sync_all();
                    reply::<calls::PushBranch, _>(&pushed)
                })
                .await
            }
            methods::CREATE_CHANGE_REQUEST => {
                Box::pin(async move {
                    let p = params_of::<calls::CreateChangeRequest>(params)?;
                    let identity = self.known_checkout(&p.cwd).await?;
                    let created = self
                        .change_request_creator
                        .create(&identity.root, &p)
                        .await
                        .map_err(git_write_error)?;
                    reply::<calls::CreateChangeRequest, _>(&created)
                })
                .await
            }
            methods::MERGE_BRANCH => {
                Box::pin(async move {
                    let p = params_of::<calls::MergeBranch>(params)?;
                    let repo = self.known_repo(&p.repo_path).await?;
                    // Neither the target's checkout nor a source worktree
                    // about to be removed may have an agent running in it.
                    for stanza in crate::git_write::worktrees(&repo)
                        .await
                        .map_err(git_write_error)?
                    {
                        let involved = stanza.branch.as_deref() == Some(p.target.as_str())
                            || (p.remove_worktree
                                && stanza.branch.as_deref() == Some(p.source.as_str()));
                        if involved
                            && let Ok(identity) = self.repos.checkout_identity(&stanza.path).await
                        {
                            self.ensure_no_live_run(&identity).await?;
                        }
                    }
                    let _write = self.git_writes.lock().await;
                    let merged = crate::git_write::merge(&self.repos, &repo, &p)
                        .await
                        .map_err(git_write_error)?;
                    self.diff_sync.sync_all();
                    reply::<calls::MergeBranch, _>(&merged)
                })
                .await
            }
            // SD1 Task 11 (§53 §6.5 G8–G11): review comments, keyed by a
            // checkout this device knows.
            methods::WATCH_REVIEW_COMMENTS => {
                Box::pin(async move {
                    let p = params_of::<calls::WatchReviewComments>(params)?;
                    let root = self.review_checkout_root(&p.checkout_id).await?;
                    let source = crate::review::DiffSource {
                        diffs: self.diff_sync.watch_diffs(),
                        repos: self.repos.clone(),
                        root,
                    };
                    Ok(stream_reply::<calls::WatchReviewComments, _, _>(
                        crate::review::watch(self.review.clone(), p.checkout_id, source),
                    ))
                })
                .await
            }
            methods::UPSERT_REVIEW_COMMENT => {
                Box::pin(async move {
                    let comment = params_of::<calls::UpsertReviewComment>(params)?;
                    self.review_checkout_root(&comment.checkout_id).await?;
                    let id = self.review.upsert(comment).await.map_err(review_error)?;
                    reply::<calls::UpsertReviewComment, _>(&IdReply { id })
                })
                .await
            }
            methods::DELETE_REVIEW_COMMENT => {
                Box::pin(async move {
                    let p = params_of::<calls::DeleteReviewComment>(params)?;
                    self.review
                        .delete(&p.checkout_id, &p.id)
                        .await
                        .map_err(review_error)?;
                    reply::<calls::DeleteReviewComment, _>(&IdReply { id: p.id })
                })
                .await
            }
            methods::RESOLVE_REVIEW_COMMENT => {
                Box::pin(async move {
                    let p = params_of::<calls::ResolveReviewComment>(params)?;
                    self.review
                        .resolve(&p.checkout_id, &p.id, p.resolved)
                        .await
                        .map_err(review_error)?;
                    reply::<calls::ResolveReviewComment, _>(&IdReply { id: p.id })
                })
                .await
            }
            methods::SEND_REVIEW_TO_THREAD => {
                Box::pin(async move {
                    let p = params_of::<calls::SendReviewToThread>(params)?;
                    let not_found = || RpcError::Failed(format!("not_found: chat {}", p.chat_id));
                    let chat = self
                        .workspace
                        .chat(&p.chat_id)
                        .map_err(|e| RpcError::Failed(e.to_string()))?
                        .ok_or_else(not_found)?;
                    if chat.device_id != self.doc_host.device_id() {
                        return Err(RpcError::BadParams("chat belongs to another device".into()));
                    }
                    // The comments must be on the thread's own checkout.
                    let from_cwd = match chat.cwd.as_deref() {
                        Some(cwd) => self
                            .repos
                            .checkout_identity(std::path::Path::new(cwd))
                            .await
                            .ok()
                            .map(|identity| identity.id),
                        None => None,
                    };
                    let checkout_id = from_cwd
                        .or(chat.checkout_id.clone())
                        .ok_or_else(|| RpcError::BadParams("the thread has no checkout".into()))?;
                    let comments = self
                        .review
                        .select(&checkout_id, &p.comment_ids)
                        .await
                        .map_err(review_error)?;
                    let prompt = crate::review::prompt(&comments);
                    let request = self
                        .doc_host
                        .request_from_chat_row(&p.chat_id, &prompt)
                        .ok_or_else(not_found)?;
                    let command_id = self
                        .doc_host
                        .queue_command(
                            &p.chat_id,
                            loams_agentd_proto::SessionCommandPayload::Run {
                                request,
                                message_id: crate::new_id(),
                            },
                        )
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    reply::<calls::SendReviewToThread, _>(&QueueCommandReply { command_id })
                })
                .await
            }
            methods::POST_REVIEW_TO_CHANGE_REQUEST => {
                Box::pin(async move {
                    let p = params_of::<calls::PostReviewToChangeRequest>(params)?;
                    let root = self.review_checkout_root(&p.checkout_id).await?;
                    let mut comments = self
                        .review
                        .select(&p.checkout_id, &p.comment_ids)
                        .await
                        .map_err(review_error)?;
                    // An outdated anchor would land on the wrong line; a
                    // diff that can't be read refuses (SD1 Task 30).
                    let current = crate::diff_sync::capture_diff(&self.repos, &root)
                        .await
                        .map(|snapshot| crate::review::CurrentDiff {
                            checksum: snapshot.checksum,
                            patch: snapshot.patch,
                            truncated: snapshot.truncated,
                        })
                        .map_err(|error| error.to_string());
                    match crate::review::refuse_outdated(&mut comments, current) {
                        Ok(()) => {}
                        Err(crate::review::NotPostable::DiffUnreadable(error)) => {
                            return Err(RpcError::Failed(format!(
                                "the checkout's diff can't be read, so outdated comments \
                                 can't be ruled out: {error}"
                            )));
                        }
                        Err(crate::review::NotPostable::Outdated(outdated)) => {
                            return Err(RpcError::BadParams(format!(
                                "outdated comments can't be posted: {outdated:?}"
                            )));
                        }
                    }
                    let url = crate::review::post_review(
                        &self.change_request_creator,
                        &root,
                        &comments,
                        p.event,
                    )
                    .await
                    .map_err(git_write_error)?;
                    reply::<calls::PostReviewToChangeRequest, _>(
                        &loams_agentd_proto::PostReviewToChangeRequestReply { url },
                    )
                })
                .await
            }
            // G23: content search in a workspace target's root.
            methods::SEARCH_WORKSPACE_CONTENT => {
                Box::pin(async move {
                    let p = params_of::<calls::SearchWorkspaceContent>(params)?;
                    let query = crate::content_search::ContentQuery::new(&p)
                        .map_err(|e| RpcError::BadParams(e.to_string()))?;
                    let workspace = self
                        .workspace_files
                        .resolve_target(&loams_agentd_proto::WorkspaceTarget {
                            chat_id: p.chat_id,
                            space_id: p.space_id,
                            checkout_path: None,
                        })
                        .await
                        .map_err(RpcError::from)?;
                    Ok(stream_reply::<calls::SearchWorkspaceContent, _, _>(
                        crate::content_search::search(workspace.root, query),
                    ))
                })
                .await
            }
            // FT1 FT0 Task 4 (§72 §5.2): the factory loop's tools, role `mcp`,
            // in `rpc/factory_loop_rpc.rs`.
            methods::CREATE_RUN_WORKTREE => {
                Box::pin(async move {
                    let p = params_of::<calls::CreateRunWorktree>(params)?;
                    let worktree = self.create_run_worktree(p).await?;
                    reply::<calls::CreateRunWorktree, _>(&worktree)
                })
                .await
            }
            methods::GET_THREAD_DIFF => {
                Box::pin(async move {
                    let p = params_of::<calls::GetThreadDiff>(params)?;
                    let diff = self.thread_diff(p).await?;
                    reply::<calls::GetThreadDiff, _>(&diff)
                })
                .await
            }
            methods::POST_REVIEW_COMMENT => {
                Box::pin(async move {
                    let p = params_of::<calls::PostReviewComment>(params)?;
                    let id = self.post_review_comment(p).await?;
                    reply::<calls::PostReviewComment, _>(&IdReply { id })
                })
                .await
            }
            methods::OPEN_CHANGE_REQUEST => {
                Box::pin(async move {
                    let p = params_of::<calls::OpenChangeRequest>(params)?;
                    let opened = self.open_change_request(p).await?;
                    reply::<calls::OpenChangeRequest, _>(&opened)
                })
                .await
            }
            methods::REQUEST_APPROVAL => {
                let p = params_of::<calls::RequestApproval>(params)?;
                reply::<calls::RequestApproval, _>(&self.request_typed_approval(p)?)
            }
            methods::GET_APPROVAL => {
                let p = params_of::<calls::GetApproval>(params)?;
                reply::<calls::GetApproval, _>(&self.get_typed_approval(p)?)
            }
            methods::GET_CHECKS => {
                Box::pin(async move {
                    let p = params_of::<calls::GetChecks>(params)?;
                    let checks = self.get_checks(p).await?;
                    reply::<calls::GetChecks, _>(&checks)
                })
                .await
            }
            methods::GET_CHECK_LOG => {
                Box::pin(async move {
                    let p = params_of::<calls::GetCheckLog>(params)?;
                    let log = self.get_check_log(p).await?;
                    reply::<calls::GetCheckLog, _>(&log)
                })
                .await
            }
            methods::CLOSE_CHANGE_REQUEST => {
                Box::pin(async move {
                    let p = params_of::<calls::CloseChangeRequest>(params)?;
                    let closed = self.close_change_request(p).await?;
                    reply::<calls::CloseChangeRequest, _>(&closed)
                })
                .await
            }
            methods::DEPLOY_LIVE_APP => {
                Box::pin(async move {
                    let p = params_of::<calls::DeployLiveApp>(params)?;
                    let deployed = self.deploy_live_app(p).await?;
                    reply::<calls::DeployLiveApp, _>(&deployed)
                })
                .await
            }
            methods::MERGE_CHANGE_REQUEST => {
                Box::pin(async move {
                    let p = params_of::<calls::MergeChangeRequest>(params)?;
                    let merged = self.merge_change_request(p).await?;
                    reply::<calls::MergeChangeRequest, _>(&merged)
                })
                .await
            }
            // SD1 Task 12 (§53 §6.5 G12–G16): run groups, in `rpc/groups_rpc.rs`.
            methods::CREATE_RUN_GROUP => {
                Box::pin(async move {
                    let p = params_of::<calls::CreateRunGroup>(params)?;
                    let created = self.create_run_group(p).await?;
                    reply::<calls::CreateRunGroup, _>(&created)
                })
                .await
            }
            methods::WATCH_RUN_GROUPS => {
                let p = params_of::<calls::WatchRunGroups>(params)?;
                Ok(stream_reply::<calls::WatchRunGroups, _, _>(
                    self.watch_run_groups(p)?,
                ))
            }
            methods::GET_RUN_GROUP_COMPARISON => {
                Box::pin(async move {
                    let p = params_of::<calls::GetRunGroupComparison>(params)?;
                    let comparison = self.run_group_comparison(p).await?;
                    reply::<calls::GetRunGroupComparison, _>(&comparison)
                })
                .await
            }
            methods::RUN_GROUP_CHECK => {
                Box::pin(async move {
                    let p = params_of::<calls::RunGroupCheck>(params)?;
                    let runs = self.run_group_check(p).await?;
                    reply::<calls::RunGroupCheck, _>(&runs)
                })
                .await
            }
            methods::RESOLVE_RUN_GROUP => {
                Box::pin(async move {
                    let p = params_of::<calls::ResolveRunGroup>(params)?;
                    let resolved = self.resolve_run_group(p).await?;
                    reply::<calls::ResolveRunGroup, _>(&resolved)
                })
                .await
            }
            methods::GET_CHECKOUT_FILE_DIFF_TEXT => {
                // This branch contains several large nested async futures. Keep it
                // behind an allocation so every unrelated RPC does not carry that
                // state in `EngineRpc::handle`'s stack frame.
                Box::pin(async move {
                    let p = params_of::<calls::GetCheckoutFileDiffText>(params)?;
                    let identity =
                        Box::pin(self.repos.checkout_identity(std::path::Path::new(&p.cwd)))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                    if identity.id != p.checkout_id {
                        return Err(RpcError::Failed("checkoutId does not match cwd".into()));
                    }
                    let root = identity.root.as_path();
                    let (snapshot, base, target) = match p.mode.as_str() {
                        "branch" => {
                            let base_ref = p
                                .base_ref
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("baseRef required".into()))?;
                            let base = Box::pin(crate::diff_sync::merge_base(root, base_ref))
                                .await
                                .map_err(|error| RpcError::Failed(error.to_string()))?;
                            let snapshot = Box::pin(crate::diff_sync::capture_diff_against(
                                &self.repos,
                                root,
                                Some(&base),
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, None)
                        }
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            let base =
                                Box::pin(crate::diff_sync::commit_diff_base(root, sha)).await;
                            let snapshot = Box::pin(crate::diff_sync::capture_commit_diff(
                                &self.repos,
                                root,
                                sha,
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, Some(sha.to_string()))
                        }
                        "turn" => {
                            let chat_id = p
                                .chat_id
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("chatId required".into()))?;
                            let turn = self
                                .diff_sync
                                .turn_snapshot(chat_id)
                                .filter(|snapshot| snapshot.root == identity.root)
                                .ok_or_else(|| RpcError::Failed("no turn recorded".into()))?;
                            let snapshot = Box::pin(crate::diff_sync::capture_turn_diff(
                                &self.repos,
                                root,
                                &turn.tree,
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, turn.tree, None)
                        }
                        _ => {
                            let base = Box::pin(crate::diff_sync::working_diff_base(root))
                                .await
                                .map_err(|error| RpcError::Failed(error.to_string()))?;
                            let snapshot =
                                Box::pin(crate::diff_sync::capture_diff(&self.repos, root))
                                    .await
                                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, None)
                        }
                    };
                    let stale = || loams_agentd_proto::CheckoutFileDiffText {
                        diff_checksum: p.diff_checksum.clone(),
                        old_text: None,
                        new_text: None,
                        old_content_hash: None,
                        new_content_hash: None,
                        binary: false,
                        truncated: false,
                        stale: true,
                    };
                    if snapshot.checksum != p.diff_checksum {
                        return reply::<calls::GetCheckoutFileDiffText, _>(&stale());
                    }
                    let file = snapshot
                        .files
                        .iter()
                        .find(|file| file.path == p.path)
                        .ok_or_else(|| {
                            RpcError::Failed("path is not part of diff snapshot".into())
                        })?;
                    let pair = Box::pin(crate::diff_sync::read_diff_file_text_at(
                        root,
                        &base,
                        target.as_deref(),
                        file,
                    ))
                    .await
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                    let current = match p.mode.as_str() {
                        "branch" => {
                            Box::pin(crate::diff_sync::capture_diff_against(
                                &self.repos,
                                root,
                                Some(&base),
                            ))
                            .await
                        }
                        "turn" => {
                            Box::pin(crate::diff_sync::capture_turn_diff(
                                &self.repos,
                                root,
                                &base,
                            ))
                            .await
                        }
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            Box::pin(crate::diff_sync::capture_commit_diff(
                                &self.repos,
                                root,
                                sha,
                            ))
                            .await
                        }
                        _ => Box::pin(crate::diff_sync::capture_diff(&self.repos, root)).await,
                    }
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                    if current.checksum != p.diff_checksum {
                        return reply::<calls::GetCheckoutFileDiffText, _>(&stale());
                    }
                    reply::<calls::GetCheckoutFileDiffText, _>(
                        &loams_agentd_proto::CheckoutFileDiffText {
                            diff_checksum: p.diff_checksum,
                            old_text: pair.old_text,
                            new_text: pair.new_text,
                            old_content_hash: pair.old_content_hash,
                            new_content_hash: pair.new_content_hash,
                            binary: pair.binary,
                            truncated: pair.truncated,
                            stale: false,
                        },
                    )
                })
                .await
            }
            methods::LIST_REPOS => reply::<calls::ListRepos, _>(&self.repos.list().await),
            methods::ADD_REPO => {
                let p = params_of::<calls::AddRepo>(params)?;
                let repo = self
                    .repos
                    .add(&p.path)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::AddRepo, _>(&repo)
            }
            methods::CLONE_REPO => {
                let p = params_of::<calls::CloneRepo>(params)?;
                let repo = self
                    .repos
                    .clone_repo(&p.url)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::CloneRepo, _>(&repo)
            }
            methods::CREATE_REPO => {
                let p = params_of::<calls::CreateRepo>(params)?;
                let repo = self
                    .repos
                    .create(&p.name)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::CreateRepo, _>(&repo)
            }
            methods::LIST_BRANCHES => {
                let p = params_of::<calls::ListBranches>(params)?;
                let branches = self
                    .repos
                    .branches(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListBranches, _>(&branches)
            }
            methods::LIST_REFS => {
                let p = params_of::<calls::ListRefs>(params)?;
                let refs = self
                    .repos
                    .refs(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListRefs, _>(&refs)
            }
            methods::LIST_GIT_HISTORY => {
                let p = params_of::<calls::ListGitHistory>(params)?;
                let history = self
                    .repos
                    .history(std::path::Path::new(&p.cwd), p.cursor, p.limit)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListGitHistory, _>(&history)
            }
            methods::SEARCH_GIT_HISTORY => {
                let p = params_of::<calls::SearchGitHistory>(params)?;
                let history = self
                    .repos
                    .search_history(std::path::Path::new(&p.cwd), &p.query, p.cursor, p.limit)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::SearchGitHistory, _>(&history)
            }
            methods::RESOLVE_GIT_AVATARS => {
                let p = params_of::<calls::ResolveGitAvatars>(params)?;
                let authors: Vec<_> = p
                    .authors
                    .into_iter()
                    .take(crate::repos::GIT_HISTORY_MAX_LIMIT)
                    .filter(|author| author.sha.len() <= 64 && author.email.len() <= 512)
                    .map(|author| (author.sha, author.email))
                    .collect();
                let avatar_paths = self
                    .repos
                    .history_avatar_urls(std::path::Path::new(&p.cwd), &authors, p.cursor, p.limit)
                    .await;
                let mut avatars = std::collections::HashMap::new();
                for (email, path) in avatar_paths {
                    if let Ok(bytes) = tokio::fs::read(path).await {
                        avatars.insert(
                            email,
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                        );
                    }
                }
                reply::<calls::ResolveGitAvatars, _>(&avatars)
            }
            methods::FETCH_ALL => {
                let p = params_of::<calls::FetchAll>(params)?;
                self.repos
                    .fetch_all(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                // Remote refs are repository state too. Force the checkout
                // watchers to publish a fresh snapshot instead of waiting for
                // the repair tick (some platforms do not report packed-refs).
                self.diff_sync.sync_all();
                reply::<calls::FetchAll, _>(&OkReply::OK)
            }
            methods::SWITCH_REF => {
                let p = params_of::<calls::SwitchRef>(params)?;
                let branch = self
                    .repos
                    .switch_ref(std::path::Path::new(&p.repo_path), &p.ref_name)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::SwitchRef, _>(&SwitchRefReply { branch })
            }
            methods::LIST_FOLDERS => {
                let p = params_of::<calls::ListFolders>(params)?;
                let listing = self
                    .repos
                    .list_folders(p.path)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListFolders, _>(&listing)
            }
            methods::LIST_DRIVES => {
                let drives = self
                    .repos
                    .list_drives()
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListDrives, _>(&loams_agentd_proto::DriveListing { drives })
            }
            methods::SEARCH_FILES => {
                let p = params_of::<calls::SearchFiles>(params)?;
                if p.query.chars().count() > 256 {
                    return Err(RpcError::BadParams(
                        "SearchFiles query must not exceed 256 characters".into(),
                    ));
                }
                let matches = tokio::time::timeout(FILE_SEARCH_RPC_TIMEOUT, async {
                    let root = self.file_search_root(&p).await?;
                    let featured_paths = p
                        .chat_id
                        .as_deref()
                        .filter(|_| p.query.is_empty())
                        .map(|chat_id| self.featured_file_paths(chat_id))
                        .unwrap_or_default();
                    self.repos
                        .search_files(root, p.query, featured_paths)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))
                })
                .await
                .map_err(|_| RpcError::Failed("file search timed out".into()))??;
                reply::<calls::SearchFiles, _>(&matches)
            }
            methods::LIST_WORKSPACE_DIRECTORY => {
                let request = params_of::<calls::ListWorkspaceDirectory>(params)?;
                let page = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.list_directory(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace directory listing timed out".into()))?
                .map_err(RpcError::from)?;
                reply::<calls::ListWorkspaceDirectory, _>(&page)
            }
            methods::SEARCH_WORKSPACE_FILES => {
                let request = params_of::<calls::SearchWorkspaceFiles>(params)?;
                let matches = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.search(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file search timed out".into()))?
                .map_err(RpcError::from)?;
                reply::<calls::SearchWorkspaceFiles, _>(&matches)
            }
            methods::READ_WORKSPACE_IMAGE => {
                let request = params_of::<calls::ReadWorkspaceImage>(params)?;
                let chunk = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.read_image(request),
                )
                .await
                .map_err(|_| RpcError::Failed("Workspace image read timed out".into()))?
                .map_err(RpcError::from)?;
                reply::<calls::ReadWorkspaceImage, _>(&chunk)
            }
            methods::READ_WORKSPACE_FILE => {
                let request = params_of::<calls::ReadWorkspaceFile>(params)?;
                let file = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.read_file(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file read timed out".into()))?
                .map_err(RpcError::from)?;
                reply::<calls::ReadWorkspaceFile, _>(&file)
            }
            methods::DELETE_WORKSPACE_ENTRY => {
                let request = params_of::<calls::DeleteWorkspaceEntry>(params)?;
                let outcome = self
                    .workspace_files
                    .delete_entry(request)
                    .await
                    .map_err(RpcError::from)?;
                reply::<calls::DeleteWorkspaceEntry, _>(&outcome)
            }
            methods::MOVE_WORKSPACE_ENTRY => {
                let request = params_of::<calls::MoveWorkspaceEntry>(params)?;
                let outcome = self
                    .workspace_files
                    .move_entry(request)
                    .await
                    .map_err(RpcError::from)?;
                reply::<calls::MoveWorkspaceEntry, _>(&outcome)
            }
            methods::WRITE_WORKSPACE_FILE => {
                let request = params_of::<calls::WriteWorkspaceFile>(params)?;
                let outcome = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.write_file(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file write timed out".into()))?
                .map_err(RpcError::from)?;
                reply::<calls::WriteWorkspaceFile, _>(&outcome)
            }
            methods::WATCH_WORKSPACE_FILES => {
                let request = params_of::<calls::WatchWorkspaceFiles>(params)?;
                let subscription = self
                    .workspace_files
                    .watch_files(request)
                    .await
                    .map_err(RpcError::from)?;
                let stream = futures::stream::unfold(subscription, |mut subscription| async move {
                    let changes = subscription.recv().await?;
                    Some((changes, subscription))
                });
                Ok(stream_reply::<calls::WatchWorkspaceFiles, _, _>(stream))
            }
            methods::CREATE_WORKTREE => {
                let p = params_of::<calls::CreateWorktree>(params)?;
                let setup_space = match p.space_id.as_deref() {
                    Some(space_id) => {
                        let space = self.local_project_action_space(space_id)?;
                        let space_root = std::fs::canonicalize(&space.path)
                            .map_err(|_| RpcError::Failed("Project root is unavailable".into()))?;
                        let repo_root = std::fs::canonicalize(&p.repo_path).map_err(|_| {
                            RpcError::Failed("Worktree repository is unavailable".into())
                        })?;
                        if space_root != repo_root {
                            return Err(RpcError::Failed(
                                "Worktree repository does not match project space".into(),
                            ));
                        }
                        Some((space, space_root))
                    }
                    None => None,
                };
                let worktree = self
                    .repos
                    .create_worktree(std::path::Path::new(&p.repo_path), &p.branch)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let mut outcome = CreateWorktreeOutcome {
                    worktree,
                    setup_action: None,
                    setup_error: None,
                };
                if let Some((space, project_root)) = setup_space {
                    match self
                        .project_actions
                        .setup_action(&space.id, std::path::Path::new(&space.path))
                    {
                        Ok(Some(action)) => {
                            let worktree_root = std::fs::canonicalize(&outcome.worktree.path)
                                .unwrap_or_else(|_| outcome.worktree.path.clone().into());
                            match crate::project_actions::launch_project_setup_action(
                                &self.terminals,
                                &action,
                                &project_root,
                                &worktree_root,
                                80,
                                24,
                            ) {
                                Ok(run) => outcome.setup_action = Some(run),
                                Err(err) => {
                                    tracing::warn!(
                                        space_id = %space.id,
                                        worktree = %outcome.worktree.path,
                                        error = %err,
                                        "failed to start project setup Action"
                                    );
                                    outcome.setup_error = Some(err.to_string());
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(
                                space_id = %space.id,
                                error = %err,
                                "failed to resolve project setup Action"
                            );
                            outcome.setup_error = Some(err.to_string());
                        }
                    }
                }
                reply::<calls::CreateWorktree, _>(&outcome)
            }
            methods::DELETE_WORKTREE => {
                let p = params_of::<calls::DeleteWorktree>(params)?;
                self.repos
                    .delete_worktree(
                        std::path::Path::new(&p.repo_path),
                        std::path::Path::new(&p.worktree_path),
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::DeleteWorktree, _>(&OkReply::OK)
            }
            methods::LIST_PROJECT_ACTIONS => {
                let p = params_of::<calls::ListProjectActions>(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                // Snapshots discover repository files; keep all filesystem work
                // (including mutation persistence below) off the async worker.
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.snapshot(&space.id, std::path::Path::new(&space.path))
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                reply::<calls::ListProjectActions, _>(&snapshot)
            }
            methods::UPSERT_PROJECT_ACTION => {
                let p = params_of::<calls::UpsertProjectAction>(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.upsert(
                        &space.id,
                        std::path::Path::new(&space.path),
                        p.action_id.as_deref(),
                        p.action,
                    )
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                reply::<calls::UpsertProjectAction, _>(&snapshot)
            }
            methods::DELETE_PROJECT_ACTION => {
                let p = params_of::<calls::DeleteProjectAction>(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.delete(&space.id, std::path::Path::new(&space.path), &p.action_id)
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                reply::<calls::DeleteProjectAction, _>(&snapshot)
            }
            methods::RUN_PROJECT_ACTION => {
                let p = params_of::<calls::RunProjectAction>(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let chat = self
                    .workspace
                    .chat(&p.chat_id)
                    .map_err(|err| RpcError::Failed(err.to_string()))?
                    .ok_or_else(|| RpcError::Failed("Project chat not found".into()))?;
                if chat.device_id != self.doc_host.device_id() {
                    return Err(RpcError::Failed(
                        "Project chat belongs to another device".into(),
                    ));
                }
                if chat.space_id.as_deref() != Some(space.id.as_str()) {
                    return Err(RpcError::Failed(
                        "Project chat belongs to another space".into(),
                    ));
                }
                let cwd = chat
                    .cwd
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| RpcError::Failed("Project chat has no checkout".into()))?;
                let checkout = self
                    .repos
                    .workspace_checkout(std::path::Path::new(&space.path), &cwd)
                    .await
                    .ok_or_else(|| {
                        RpcError::Failed("Project chat checkout is unavailable".into())
                    })?;
                let project_root = std::fs::canonicalize(&space.path)
                    .map_err(|_| RpcError::Failed("Project root is unavailable".into()))?;
                let action = self
                    .project_actions
                    .action(&space.id, std::path::Path::new(&space.path), &p.action_id)
                    .map_err(|err| RpcError::Failed(err.to_string()))?
                    .ok_or_else(|| RpcError::Failed("Project action not found".into()))?;
                let run = crate::project_actions::launch_project_action(
                    &self.terminals,
                    &action,
                    &project_root,
                    &checkout,
                    p.cols,
                    p.rows,
                )
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                reply::<calls::RunProjectAction, _>(&run)
            }
            methods::OPEN_TERMINAL => {
                let p = params_of::<calls::OpenTerminal>(params)?;
                // Prefer an explicit real path (new-chat canvas has no row
                // yet). `space-canvas:{spaceId}` names the selected project
                // so a missing/tilde cwd still lands in that folder.
                let chat_cwd = self
                    .workspace
                    .chat(&p.chat_id)
                    .ok()
                    .flatten()
                    .and_then(|chat| chat.cwd);
                let space_cwd = canvas_space_id(&p.chat_id).and_then(|space_id| {
                    self.workspace
                        .space(space_id)
                        .ok()
                        .flatten()
                        .map(|space| space.path)
                });
                let cwd = resolve_open_terminal_cwd(p.cwd, chat_cwd, space_cwd)
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                let session = self
                    .terminals
                    .open(&cwd, p.cols, p.rows)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::OpenTerminal, _>(&session)
            }
            methods::SUBSCRIBE_TERMINAL => {
                let p = params_of::<calls::SubscribeTerminal>(params)?;
                let rx = self
                    .terminals
                    .subscribe(&p.terminal_id, p.after_seq)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let stream = futures::stream::unfold(rx, |mut rx| async move {
                    let event = rx.recv().await?;
                    Some((event, rx))
                });
                Ok(stream_reply::<calls::SubscribeTerminal, _, _>(stream))
            }
            methods::WRITE_TERMINAL => {
                let p = params_of::<calls::WriteTerminal>(params)?;
                self.terminals
                    .write(&p.terminal_id, &p.data)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::WriteTerminal, _>(&OkReply::OK)
            }
            methods::RESIZE_TERMINAL => {
                let p = params_of::<calls::ResizeTerminal>(params)?;
                self.terminals
                    .resize(&p.terminal_id, p.cols, p.rows)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ResizeTerminal, _>(&OkReply::OK)
            }
            methods::CLOSE_TERMINAL => {
                let p = params_of::<calls::CloseTerminal>(params)?;
                self.terminals
                    .close(&p.terminal_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::CloseTerminal, _>(&OkReply::OK)
            }
            methods::LIST_AGENT_ACCOUNTS => {
                let p = params_of::<calls::ListAgentAccounts>(params)?;
                let snapshot = self
                    .agent_accounts
                    .list(p.force_usage.unwrap_or(false))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ListAgentAccounts, _>(&snapshot)
            }
            methods::ACTIVATE_AGENT_ACCOUNT => {
                let p = params_of::<calls::ActivateAgentAccount>(params)?;
                let snapshot = self
                    .agent_accounts
                    .activate(p.harness, &p.account_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ActivateAgentAccount, _>(&snapshot)
            }
            methods::FORGET_AGENT_ACCOUNT => {
                let p = params_of::<calls::ForgetAgentAccount>(params)?;
                let snapshot = self
                    .agent_accounts
                    .forget(p.harness, &p.account_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ForgetAgentAccount, _>(&snapshot)
            }
            methods::START_AGENT_LOGIN => {
                let p = params_of::<calls::StartAgentLogin>(params)?;
                let start = self
                    .agent_accounts
                    .start_login_with(p.harness, p.provider.as_deref())
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::StartAgentLogin, _>(&start)
            }
            methods::COMPLETE_AGENT_LOGIN => {
                let p = params_of::<calls::CompleteAgentLogin>(params)?;
                let snapshot = self
                    .agent_accounts
                    .complete_login(&p.login_id, &p.code)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::CompleteAgentLogin, _>(&snapshot)
            }
            methods::POLL_AGENT_LOGIN => {
                let p = params_of::<calls::PollAgentLogin>(params)?;
                let poll = self
                    .agent_accounts
                    .poll_login(&p.login_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::PollAgentLogin, _>(&poll)
            }
            methods::CANCEL_AGENT_LOGIN => {
                let p = params_of::<calls::CancelAgentLogin>(params)?;
                self.agent_accounts.cancel_login(&p.login_id);
                reply::<calls::CancelAgentLogin, _>(&OkReply::OK)
            }
            methods::UPLOAD_CHUNK => {
                let p = params_of::<calls::UploadChunk>(params)?;
                self.uploads
                    .append(&p.upload_id, &p.data, p.seq)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::UploadChunk, _>(&OkReply::OK)
            }
            methods::UPLOAD_COMMIT => {
                let p = params_of::<calls::UploadCommit>(params)?;
                let path = self
                    .uploads
                    .commit(&p.upload_id, &p.file_name)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                // Bytes just landed on this device: any command deferred on
                // them (queued-attachment refs) is executable NOW.
                self.doc_host.kick_drains();
                reply::<calls::UploadCommit, _>(&UploadCommitReply { path })
            }
            methods::READ_ATTACHMENT_CHUNK => {
                let p = params_of::<calls::ReadAttachmentChunk>(params)?;
                // Path jail: the uploads dir plus every workspace-known chat cwd.
                let roots: Vec<std::path::PathBuf> = self
                    .workspace
                    .read_chats()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|chat| chat.cwd)
                    .map(std::path::PathBuf::from)
                    .collect();
                let chunk = self
                    .uploads
                    .read_chunk(&p.path, p.offset, &roots)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::ReadAttachmentChunk, _>(&chunk)
            }
            methods::FETCH_TOOL_BLOB => {
                let p = params_of::<calls::FetchToolBlob>(params)?;
                let chunk = self
                    .doc_host
                    .fetch_tool_blob(&p.blob_ref, p.offset, p.len)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                reply::<calls::FetchToolBlob, _>(&chunk)
            }
            other => Err(RpcError::UnknownMethod(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each subprocess has private HOME/PATH/overrides, avoiding process-global test races.
    #[cfg(unix)]
    async fn installer_rpc_fixture(mode: &str) {
        use std::{os::unix::fs::PermissionsExt, sync::Arc};
        if std::env::var_os("LOAMS_DESKTOP_INSTALL_FIXTURE_CHILD").is_none() {
            let root = tempfile::tempdir().unwrap();
            let bin = root.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            let script = match mode {
                "success" => {
                    "test -z \"$LOAMS_DESKTOP_INSTALL_FIXTURE_CHILD\" && test -z \"$CLAUDECODE\" && printf '#!/bin/sh\\necho 99.0.0\\n' > \"$CODEX_EXECUTABLE\" && /bin/chmod +x \"$CODEX_EXECUTABLE\""
                }
                "failure" => "echo 'fixture failure api_key=private' >&2; exit 7",
                "missing" => "exit 0",
                "cancel" => "echo ready > \"$READY_FILE\"; sleep 60",
                "npm" => "npm install -g @openai/codex",
                _ => unreachable!(),
            };
            let test = format!("rpc::tests::installer_rpc_{mode}");
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &test, "--nocapture", "--include-ignored"])
                .env("LOAMS_DESKTOP_INSTALL_FIXTURE_CHILD", root.path())
                .env("LOAMS_DESKTOP_INSTALLER_COMMAND_CODEX", script)
                .env("LOAMS_DESKTOP_NO_LOGIN_SHELL", "1")
                .env("HOME", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("CODEX_EXECUTABLE", bin.join("codex"))
                .env("CLAUDECODE", "nested-test")
                .env("READY_FILE", root.path().join("ready"))
                .env("npm_config_prefix", root.path())
                .env("npm_config_cache", root.path().join("npm-cache"))
                .env(
                    "PATH",
                    std::env::join_paths(
                        std::iter::once(bin)
                            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                    )
                    .unwrap(),
                )
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            println!("{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        let root = std::path::PathBuf::from(
            std::env::var_os("LOAMS_DESKTOP_INSTALL_FIXTURE_CHILD").unwrap(),
        );
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(loams_agentd_harness::CodexHarness::new()));
        let core =
            crate::EngineCore::assemble(&root.join("engine"), registry.clone(), HarnessId::Codex)
                .unwrap();
        let rpc = core.rpc_service();
        let params = serde_json::json!({"harness": "codex"});
        assert!(!registry.descriptors()[0].installed);
        assert_eq!(registry.descriptors()[0].enabled, Some(false));
        let result = if mode == "cancel" {
            let rpc = rpc.clone();
            let task = tokio::spawn(async move {
                rpc.handle(
                    methods::INSTALL_HARNESS,
                    serde_json::json!({"harness": "codex"}),
                )
                .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !root.join("ready").exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            // A second service for the same device must share the in-flight guard.
            let other = core.rpc_service();
            assert!(
                matches!(other.handle(methods::INSTALL_HARNESS, params.clone()).await, Err(RpcError::Failed(e)) if e == "already installing")
            );
            other.handle(methods::CANCEL_INSTALL, params).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
        } else {
            rpc.handle(methods::INSTALL_HARNESS, params).await
        };
        match mode {
            "success" | "npm" => {
                let RpcReply::Value(value) = result.unwrap() else {
                    panic!("expected descriptors");
                };
                let list: Vec<crate::registry::HarnessDescriptor> =
                    serde_json::from_value(value).unwrap();
                assert!(list[0].installed);
                assert_eq!(list[0].enabled, Some(true));
                assert!(list[0].can_install);
                let path = root.join("bin/codex");
                assert!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o111 != 0);
                println!(
                    "InstallHarness ({mode}): installed=false/enabled=false -> installed=true/enabled=true; {}",
                    path.display()
                );
            }
            "failure" => assert!(
                matches!(result, Err(RpcError::Failed(e)) if e.contains("fixture failure") && e.contains("[REDACTED]") && !e.contains("private"))
            ),
            "missing" => assert!(
                matches!(result, Err(RpcError::Failed(e)) if e.contains("installer finished but `codex` was not found on PATH"))
            ),
            "cancel" => {
                assert!(matches!(result, Err(RpcError::Failed(e)) if e.contains("cancelled")));
                assert!(registry.installs.begin(HarnessId::Codex).is_ok());
            }
            _ => unreachable!(),
        }
        core.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_success() {
        installer_rpc_fixture("success").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_failure() {
        installer_rpc_fixture("failure").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_missing() {
        installer_rpc_fixture("missing").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_cancel() {
        installer_rpc_fixture("cancel").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "downloads the official npm package into an isolated temporary prefix"]
    async fn installer_rpc_npm() {
        installer_rpc_fixture("npm").await;
    }

    #[tokio::test]
    async fn explicit_install_rpc_verifies_archive_and_refreshes_descriptors() {
        use loams_agentd_harness::archive_install::{
            ArchivePin, ensure_installed, installed_entry,
        };
        use sha2::{Digest, Sha512};
        use std::io::Write;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if std::env::var_os("LOAMS_DESKTOP_INSTALL_RPC_TEST").is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rpc::tests::explicit_install_rpc_verifies_archive_and_refreshes_descriptors",
                    "--nocapture",
                ])
                .env("LOAMS_DESKTOP_INSTALL_RPC_TEST", "1")
                .env("LOAMS_DESKTOP_ADAPTERS_DIR", root.path())
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if !loams_agentd_harness::acp::can_install(HarnessId::Antigravity) {
            return;
        }
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("server", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let digest = Box::leak(format!("{:x}", Sha512::digest(&bytes)).into_boxed_str());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Box::leak(
            format!("http://{}/archive.zip", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            let mut buf = [0; 4096];
            while !headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buf).await.unwrap();
                assert!(count > 0, "archive request closed before its headers");
                headers.extend_from_slice(&buf[..count]);
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(&bytes).await.unwrap();
        });
        let pin = ArchivePin {
            name: "explicit-install-test",
            version: "1",
            url,
            entry: "server",
            sha512: digest,
        };
        let registry = HarnessRegistry::new();
        let descriptor = serde_json::from_value(serde_json::json!({
            "id": "antigravity", "name": "Antigravity", "supportsSteering": true,
            "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": false
        }))
        .unwrap();
        registry.register_lazy(
            descriptor,
            Box::new(move || installed_entry(&pin).is_some()),
            Box::new(|| panic!("installation must not spawn the harness")),
        );
        assert!(!registry.descriptors()[0].installed);
        let result = install_harness_with(&registry, HarnessId::Antigravity, || async {
            ensure_installed(pin, "Test adapter").await.map(|_| ())
        })
        .await
        .unwrap();
        assert!(result[0].installed);
        assert_eq!(result[0].enabled, Some(true));
        assert!(result[0].can_install);
        assert!(installed_entry(&pin).unwrap().is_file());
        server.await.unwrap();
        // The verified marker makes another explicit install idempotent, even
        // after the archive server has stopped.
        install_harness_with(&registry, HarnessId::Antigravity, || async {
            ensure_installed(pin, "Test adapter").await.map(|_| ())
        })
        .await
        .unwrap();
        assert!(
            install_harness_with(&registry, HarnessId::Mock, || async {
                panic!("unsupported harness must not invoke an installer")
            })
            .await
            .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn local_opening_tail_arrives_before_full_mirror_and_keeps_all_history() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(loams_agentd_store::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "viewer".into(),
                default_harness: loams_agentd_proto::HarnessId::Mock,
                tool_outputs: dir.path().join("tool-outputs"),
            },
        );
        let handle = host.open("whale").unwrap();
        handle
            .doc()
            .push_message(&loams_agentd_doc::SessionMessageEntry {
                id: "turn".into(),
                role: loams_agentd_doc::MessageRole::Assistant,
                parts: (0..500)
                    .map(|i| loams_agentd_doc::MessagePart::Text {
                        id: format!("part-{i}"),
                        text: "local text".into(),
                    })
                    .collect(),
                created_at: 0,
                device_id: "host".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        // Hold publication blocked: the opening must not await the full mirror.
        let held = handle.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || {
            held.import_transcript(|| {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        locked_rx.recv().unwrap();
        let opening = tokio::time::timeout(
            Duration::from_secs(2),
            opening_doc_messages_stream(host.clone(), "whale".into()),
        )
        .await;
        release_tx.send(()).unwrap();
        blocker.join().unwrap();
        let mut stream = opening
            .expect("first paint must not wait for publication")
            .unwrap()
            .map(|update| serde_json::to_value(update).unwrap());
        let preview = stream.next().await.unwrap();
        assert_eq!(preview["historyPending"], true);
        assert_eq!(preview["reset"][0]["parts"].as_array().unwrap().len(), 128);
        assert_eq!(preview["reset"][0]["parts"][0]["id"], "part-372");
        // Changes between preview and subscribe must appear in the full reset.
        handle
            .write_user_message("arrived", "new local message", 1)
            .unwrap();
        let full = stream.next().await.unwrap();
        assert!(full.get("historyPending").is_none());
        assert_eq!(full["reset"][0]["parts"].as_array().unwrap().len(), 500);
        assert_eq!(full["reset"][1]["id"], "arrived");
        handle
            .write_user_message("live", "after attach", 2)
            .unwrap();
        let live = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        let mut entries = Vec::new();
        for value in [full, live] {
            let update: loams_agentd_doc::TranscriptUpdate = serde_json::from_value(value).unwrap();
            loams_agentd_doc::apply_transcript_frame(&mut entries, update.frame).unwrap();
        }
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.last().unwrap().id, "live");
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn antigravity_disable_does_not_launch_the_server() {
        let registry = HarnessRegistry::new();
        let executable = std::env::current_exe().unwrap();
        registry.register(std::sync::Arc::new(
            loams_agentd_harness::AcpHarness::grok().with_executable(executable.clone()),
        ));
        registry.register(std::sync::Arc::new(
            loams_agentd_harness::AcpHarness::antigravity().with_executable(executable),
        ));
        registry.set_enabled(HarnessId::Antigravity, true).unwrap();

        update_harness_enabled(&registry, HarnessId::Antigravity, false)
            .await
            .unwrap();
        assert!(!registry.enabled_set().contains(&HarnessId::Antigravity));
    }

    /// The UI's Switch/Forget calls send `{id, accountId, harness}` (+ optional
    /// `targetDeviceId`); the extra fields must be tolerated, `accountId` wins.
    #[test]
    fn list_models_force_is_optional_and_backward_compatible() {
        use loams_agentd_proto::params::ListModelsParams;
        let old: ListModelsParams =
            serde_json::from_value(serde_json::json!({"harness":"codex"})).unwrap();
        assert!(!old.force);
        let forced: ListModelsParams =
            serde_json::from_value(serde_json::json!({"harness":"codex","force":true})).unwrap();
        assert!(forced.force);
    }

    #[test]
    fn agent_account_params_accept_ui_shape() {
        use loams_agentd_proto::params::AgentAccountParams;
        let p: AgentAccountParams = loams_agentd_rpc::parse_params(serde_json::json!({
            "id": "acct-1",
            "accountId": "acct-1",
            "harness": "claude-code",
            "targetDeviceId": "dev-2",
        }))
        .expect("ui param shape");
        assert_eq!(p.account_id, "acct-1");
        assert_eq!(p.harness, HarnessId::ClaudeCode);
    }

    #[test]
    fn sidebar_preferences_mutation_accepts_desktop_wire_shape() {
        let p: MutateParams = loams_agentd_rpc::parse_params(serde_json::json!({
            "op": "changeSidebarPin",
            "change": {"action":"move","sessionId":"chat-b","before":"chat-a","after":null},
        }))
        .expect("sidebar preferences params");
        assert!(matches!(
            p,
            MutateParams::ChangeSidebarPin { change: loams_agentd_proto::SidebarPinChange::Move { session_id, before, .. } }
                if session_id == "chat-b" && before.as_deref() == Some("chat-a")
        ));
    }

    #[test]
    fn open_terminal_cwd_prefers_explicit_then_chat_then_space_then_home() {
        let home = crate::repos::session_home_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_open_terminal_cwd(
                Some("/proj".into()),
                Some("/chat".into()),
                Some("/space".into())
            )
            .unwrap(),
            "/proj"
        );
        assert_eq!(
            resolve_open_terminal_cwd(None, Some("/chat".into()), Some("/space".into())).unwrap(),
            "/chat"
        );
        assert_eq!(
            resolve_open_terminal_cwd(Some("~".into()), None, Some("/space".into())).unwrap(),
            "/space",
            "tilde is a fallback, not an override of the selected project"
        );
        assert_eq!(
            resolve_open_terminal_cwd(None, None, Some("/space".into())).unwrap(),
            "/space"
        );
        assert_eq!(resolve_open_terminal_cwd(None, None, None).unwrap(), home);
        assert_eq!(
            resolve_open_terminal_cwd(Some("~".into()), Some("/chat".into()), None).unwrap(),
            "/chat"
        );
        assert_eq!(
            resolve_open_terminal_cwd(Some("  ".into()), Some("/chat".into()), None).unwrap(),
            "/chat"
        );
        assert_eq!(canvas_space_id("space-canvas:s1"), Some("s1"));
        assert_eq!(canvas_space_id("space-canvas:"), None);
        assert_eq!(canvas_space_id("chat-1"), None);
    }

    #[test]
    fn tool_file_paths_keep_workspace_activity_only() {
        assert_eq!(
            tool_file_path(&ToolCall::EditFile {
                path: "src/main.rs".into(),
                old_string: None,
                new_string: None,
            }),
            Some("src/main.rs")
        );
        assert_eq!(
            tool_file_path(&ToolCall::Exec {
                command: "cargo test".into(),
            }),
            None
        );
    }
}

#[cfg(test)]
mod context_usage_tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn context_only_commits_reach_remote_watch_and_reconnect() {
        let host = loams_agentd_doc::SessionDoc::init("context-chat").unwrap();
        host.update_context_usage(Some(42000), Some(200000))
            .unwrap();
        // The viewing engine reads a replicated document, with no harness process.
        let remote = Arc::new(loams_agentd_doc::SessionDoc::from_doc(loro::LoroDoc::new()));
        remote
            .doc()
            .import(&host.export_snapshot().unwrap())
            .unwrap();
        let (tx, rx) = watch::channel(crate::doc_host::TranscriptSnapshot::default());
        let json = |update: TranscriptUpdate| serde_json::to_value(update).unwrap();
        let mut stream = doc_messages_stream(rx, remote.clone()).map(json);
        let first = stream.next().await.unwrap();
        assert_eq!(first["contextUsage"]["tokens"], 42000);
        assert!(first.get("reset").is_some());
        let version = host.doc().oplog_vv();
        host.update_context_usage(Some(0), None).unwrap();
        remote
            .doc()
            .import(
                &host
                    .doc()
                    .export(loro::ExportMode::updates(&version))
                    .unwrap(),
            )
            .unwrap();
        tx.send_replace(crate::doc_host::TranscriptSnapshot::default());
        let update = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(update["contextUsage"]["tokens"], 0);
        assert_eq!(update["contextUsage"]["window"], 200000);
        let mut reconnect = doc_messages_stream(tx.subscribe(), remote.clone()).map(json);
        assert_eq!(
            reconnect.next().await.unwrap()["contextUsage"],
            update["contextUsage"]
        );
        remote.clear_context_usage().unwrap();
        tx.send_replace(crate::doc_host::TranscriptSnapshot::default());
        assert!(stream.next().await.unwrap()["contextUsage"].is_null());
    }
}
