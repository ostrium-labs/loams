//! loams-agentd-sessions — the headless backend: sessions engine, doc host + command executor,
//! run journal + crash recovery, and the IPC RPC server.
//!
//! Spec: ARCHITECTURE.md §5 and docs/research/feature-inventory.md §3. The engine is
//! local-only (D781): one device, one local profile, no edge sync, relay or sign-in.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use loams_agentd_proto::{EngineInfo, HarnessId, WorkspaceScope};

use loams_agentd_store::DocsStore;

pub mod agent_accounts;
pub mod approvals;
pub mod change_request_create;
pub mod change_requests;
mod chat_persistence;
pub mod checkpoints;
pub mod checks;
pub mod content_search;
pub mod diff_sync;
pub mod doc_host;
pub mod factory_loop;
pub mod fuzz;
pub mod git_write;
pub mod groups;
pub mod harness_updates;
pub mod instance_lock;
pub mod marketplace;
mod model_catalogs;
pub mod notify;
pub mod profile;
pub mod project_actions;
pub mod registry;
pub mod repos;
pub mod review;
pub mod rpc;
pub mod run_journal;
pub mod sessions;
pub mod source_control;
pub mod spaces;
pub mod terminals;
pub mod titles;
pub mod tool_outputs;
mod transcript_history;
pub mod uploads;
pub mod usage;
pub mod workspace_files;
pub mod workspace_host;
pub mod worktree_list;

pub use agent_accounts::{AgentAccounts, AgentAccountsConfig};
pub use approvals::{ApprovalSettings, Approvals};
pub use change_requests::{ChangeRequestCacheKey, CheckoutChangeRequests};
pub use checkpoints::Checkpoints;
pub use diff_sync::{
    CheckoutDiffSync, DiffFileTextPair, DiffSnapshot, TurnSnapshot, capture_commit_diff,
    capture_diff, capture_diff_against, capture_turn_diff, discard_working_tree, merge_base,
    read_diff_file_text, snapshot_tree, working_diff_base,
};
pub use doc_host::{ChatDocHandle, DocHost, DocHostConfig};
pub use groups::{RunGroupLimits, RunGroups};
pub use instance_lock::{InstanceLock, LockError};
pub use notify::Notifications;
pub use profile::EngineProfile;
pub use project_actions::ProjectActionsStore;
pub use registry::{HarnessDescriptor, HarnessRegistry, default_registry};
pub use repos::{CheckoutIdentity, Repos, worktree_branch_from_title};
pub use rpc::EngineRpc;
pub use run_journal::{JournalError, RunJournal};
pub use sessions::{JournaledEvent, SessionsEngine, SteerOutcome, TurnObserver};
pub use source_control::{
    BranchHeadContext, ChangeRequestError, ChangeRequestProvider, ChangeRequestResolution,
    ChangeRequestResolver, CheckoutChangeRequestLookup, CheckoutSourceContext, GitHubCli,
    GitRemote, parse_git_remote,
};
pub use spaces::SpacesSync;
pub use terminals::{TerminalShell, Terminals};
pub use titles::TitleGenerator;
pub use uploads::{AttachmentChunk, Uploads};
pub use usage::ThreadUsageLog;
pub use workspace_files::WorkspaceFiles;
pub use workspace_host::{WORKSPACE_DOC_ID, WorkspaceHost, WorkspaceHostConfig};

pub(crate) const LEGACY_UNKNOWN_DEVICE_NAME: &str = "unknown-device";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("doc: {0}")]
    Doc(#[from] loams_agentd_doc::DocError),
    #[error("journal: {0}")]
    Journal(#[from] run_journal::JournalError),
    #[error("store: {0}")]
    Store(#[from] loams_agentd_store::StoreError),
    #[error("harness: {0}")]
    Harness(#[from] loams_agentd_harness::HarnessError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A send to a chat on a harness this daemon no longer ships (plan DD1
    /// Task 3): the chat is read-only.
    #[error(
        "{}: {}",
        loams_agentd_proto::HARNESS_UNSUPPORTED,
        loams_agentd_proto::UNSUPPORTED_HARNESS_NOTICE
    )]
    HarnessUnsupported,
    #[error("{0}")]
    Other(String),
}

/// Epoch millis now — the doc/journal timestamp base.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Data directory (default `~/.loams-desktop`, dev `~/.loams-desktop-dev`).
    pub data_dir: PathBuf,
    /// Harness for doc-command runs on chats without a workspace `config` row.
    pub default_harness: HarnessId,
    /// A fixed shell for terminals and project actions; `None` = the user's `$SHELL`.
    pub terminal_shell: Option<TerminalShell>,
    /// Bounds of the full tool-output store (plan DD1 ruling T2-13).
    pub tool_outputs: tool_outputs::ToolOutputLimits,
    /// The daemon's port and run-token issuer for the injected MCP shim, set
    /// before stale runs are recovered so a resumed run gets the shim too.
    pub mcp_endpoint: Option<sessions::McpEndpoint>,
    /// The approval default and timeout (SD1 Task 13), set before stale runs
    /// are recovered so a resumed run is supervised as its thread says.
    pub approvals: ApprovalSettings,
}

/// The assembled engine core — also constructible without the IPC server for tests.
pub struct EngineCore {
    pub sessions: SessionsEngine,
    pub doc_host: DocHost,
    pub workspace: WorkspaceHost,
    pub registry: Arc<HarnessRegistry>,
    pub repos: Repos,
    pub workspace_files: WorkspaceFiles,
    pub terminals: Terminals,
    pub project_actions: ProjectActionsStore,
    pub previews: loams_agentd_preview::PreviewService,
    pub change_requests: CheckoutChangeRequests,
    pub diff_sync: CheckoutDiffSync,
    /// Per-turn checkpoints (SD1 Task 10).
    pub checkpoints: Checkpoints,
    /// Parallel run groups (SD1 Task 12).
    pub run_groups: RunGroups,
    /// Studio's notifications (SD1 Task 14, G19): run groups and approvals
    /// post theirs here too.
    pub notifications: Notifications,
    /// Each thread's per-turn usage (SD1 Task 14, G20).
    pub usage: ThreadUsageLog,
    /// Supervised approvals (SD1 Task 13, G17, G18, G22).
    pub approvals: Approvals,
    /// The daemon's marketplace, once attached (MK1 Task 4b).
    pub marketplace: marketplace::MarketplaceSlot,
    pub spaces_sync: SpacesSync,
    pub uploads: Uploads,
    pub agent_accounts: AgentAccounts,
    pub harness_updates: harness_updates::HarnessUpdateCoordinator,
    /// The software factory's app connections (SF1 amendment F1; §50 §13, D795).
    pub factory: Arc<loams_agentd_factory::FactoryHost>,
    pub device_id: String,
    workspace_scope: WorkspaceScope,
    /// Exclusive data-dir lock — held for the engine's lifetime (single-instance).
    _instance_lock: InstanceLock,
}

impl std::fmt::Debug for EngineCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineCore").finish_non_exhaustive()
    }
}

impl EngineCore {
    /// Open the local profile's stores under `data_dir`, wire sessions ⇄ doc host ⇄
    /// workspace host, and recover stale journals from a previous crash.
    pub fn assemble(
        data_dir: &Path,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
    ) -> Result<Self, EngineError> {
        let profile = EngineProfile::local(data_dir)?;
        Self::assemble_with_profile(profile, registry, default_harness)
    }

    /// [`Self::assemble`] with the approval settings (SD1 Task 13).
    pub fn assemble_with_approvals(
        data_dir: &Path,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
        approvals: ApprovalSettings,
    ) -> Result<Self, EngineError> {
        let profile = EngineProfile::local(data_dir)?;
        std::fs::create_dir_all(profile.device_root())?;
        let lock = InstanceLock::acquire(profile.device_root())?;
        Self::assemble_with_settings(profile, registry, default_harness, lock, None, approvals)
    }

    /// Assemble the engine against one resolved, immutable workspace profile.
    pub fn assemble_with_profile(
        profile: EngineProfile,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
    ) -> Result<Self, EngineError> {
        let data_dir = profile.device_root();
        std::fs::create_dir_all(data_dir)?;
        // Single-instance guard: two engines on one data dir would race the
        // SQLite snapshots + journals. Taken before any store opens or the IPC
        // port binds; held (and kernel-released on crash) for the engine's life.
        let lock = InstanceLock::acquire(data_dir)?;
        Self::assemble_with_profile_locked(profile, registry, default_harness, lock)
    }

    /// Assemble against a pre-acquired [`InstanceLock`], so the listener owner and
    /// the data-dir owner cannot diverge.
    pub fn assemble_with_profile_locked(
        profile: EngineProfile,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
        lock: InstanceLock,
    ) -> Result<Self, EngineError> {
        Self::assemble_with_endpoint(profile, registry, default_harness, lock, None)
    }

    /// [`Self::assemble_with_profile_locked`] with the MCP shim's endpoint
    /// installed before stale runs are recovered.
    pub fn assemble_with_endpoint(
        profile: EngineProfile,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
        lock: InstanceLock,
        mcp_endpoint: Option<sessions::McpEndpoint>,
    ) -> Result<Self, EngineError> {
        Self::assemble_with_settings(
            profile,
            registry,
            default_harness,
            lock,
            mcp_endpoint,
            ApprovalSettings::default(),
        )
    }

    /// [`Self::assemble_with_endpoint`] with the approval settings (SD1 Task
    /// 13), applied before stale runs are recovered.
    pub fn assemble_with_settings(
        profile: EngineProfile,
        registry: Arc<HarnessRegistry>,
        default_harness: HarnessId,
        lock: InstanceLock,
        mcp_endpoint: Option<sessions::McpEndpoint>,
        approval_settings: ApprovalSettings,
    ) -> Result<Self, EngineError> {
        let data_dir = profile.device_root();
        std::fs::create_dir_all(data_dir)?;
        let device_id = load_or_create_device_id(data_dir)?;
        // This device's harness enablement (Settings → Providers) rides the
        // engine data dir — per-device, like the CLI installs it gates.
        registry.load_prefs(data_dir);
        let store = Arc::new(DocsStore::open(profile.store_root())?);
        let run_groups = RunGroups::open(store.clone())?;
        let approvals = Approvals::open(store.clone(), approval_settings)?;
        let journal = Arc::new(RunJournal::open(profile.store_root().join("journals"))?);
        let sessions = SessionsEngine::new(device_id.clone(), journal, registry.clone());
        let doc_host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: device_id.clone(),
                default_harness,
                tool_outputs: profile.store_root().join("tool-outputs"),
            },
        );
        let usage = ThreadUsageLog::new(store.clone());
        let workspace = WorkspaceHost::open(
            store,
            WorkspaceHostConfig {
                device_id: device_id.clone(),
                device_name: local_device_name(&device_id),
                platform: std::env::consts::OS.to_string(),
                org_id: profile.org_id().to_string(),
                user_id: profile.user_id().to_string(),
            },
        )?;
        doc_host.set_workspace(workspace.clone());
        doc_host.set_sessions(sessions.clone());
        sessions.set_doc_host(doc_host.clone());
        if let Some(endpoint) = mcp_endpoint {
            sessions.set_mcp_endpoint(endpoint.port, endpoint.tokens);
        }
        // Supervised approvals (SD1 Task 13), wired before stale runs are
        // recovered: a side chat takes its parent's mode, and each ask posts
        // an `approval` notification titled with its thread's title.
        let notifications = Notifications::new();
        let parents = workspace.clone();
        approvals.set_parent_of(Arc::new(move |chat_id: &str| {
            parents
                .chat(chat_id)
                .ok()
                .flatten()
                .and_then(|chat| chat.parent_chat_id)
        }));
        let titles = workspace.clone();
        approvals.set_notifications(
            notifications.clone(),
            Arc::new(move |chat_id: &str| {
                titles
                    .chat(chat_id)
                    .ok()
                    .flatten()
                    .and_then(|chat| chat.title.or(chat.last_message_preview))
            }),
        );
        sessions.set_approvals(approvals.clone());
        sessions.add_turn_observer(Arc::new(approvals.clone()));
        match sessions.recover_stale() {
            Ok(0) => {}
            Ok(recovered) => tracing::info!(recovered, "stale sessions recovered on boot"),
            Err(err) => tracing::error!(error = %err, "stale-session recovery failed"),
        }
        let repos = Repos::new(data_dir, &device_id);
        doc_host.set_repos(repos.clone());
        let change_requests = CheckoutChangeRequests::start(repos.clone(), &device_id);
        let workspace_files =
            WorkspaceFiles::new(repos.clone(), workspace.clone(), device_id.clone());
        let terminals = Terminals::new();
        let project_actions = ProjectActionsStore::open(profile.store_root())?;
        doc_host.set_project_action_runtime(project_actions.clone(), terminals.clone());
        let previews = loams_agentd_preview::PreviewService::new(
            profile.store_root().join("previews.json"),
            device_id.clone(),
            local_device_name(&device_id),
        )
        .map_err(|e| EngineError::Other(e.to_string()))?;
        let uploads = Uploads::from_root(profile.uploads_root());
        // Queued-attachment support: the doc host resolves `pending://` refs
        // against this store.
        doc_host.set_uploads(uploads.clone());
        let agent_accounts_config = AgentAccountsConfig::detect(data_dir);
        sessions.set_generated_images(
            uploads.clone(),
            agent_accounts_config.codex_home.join("generated_images"),
        );
        let agent_accounts = AgentAccounts::new(agent_accounts_config);
        let factory = Arc::new(
            loams_agentd_factory::open(data_dir)
                .map_err(|e| EngineError::Other(format!("factory: {e}")))?,
        );
        let harness_updates =
            harness_updates::HarnessUpdateCoordinator::new(data_dir, registry.clone());
        sessions.set_titles(TitleGenerator::new(
            workspace.clone(),
            registry.clone(),
            repos.clone(),
        ));
        let diff_sync = CheckoutDiffSync::start(repos.clone(), workspace.clone(), &device_id);
        // Turn starts snapshot the checkout tree — the "Latest turn" diff base.
        let turn_diff = diff_sync.clone();
        sessions.set_turn_listener(Arc::new(move |chat_id, cwd| {
            turn_diff.note_turn_start(chat_id, cwd);
        }));
        // Each turn's `pre` and `post` hidden commits (SD1 Task 10).
        let checkpoints = Checkpoints::new(repos.clone());
        // ... and the run groups' note of each member's turn (SD1 Task 12).
        sessions.set_turn_checkpoints(Arc::new(groups::GroupTurns {
            checkpoints: checkpoints.clone(),
            groups: run_groups.clone(),
        }));
        run_groups.start(
            doc_host.clone(),
            sessions.clone(),
            diff_sync.clone(),
            terminals.clone(),
        );
        // Notifications and per-turn usage follow every turn (SD1 Task 14).
        let titles = workspace.clone();
        sessions.add_turn_observer(Arc::new(notify::TurnNotifier::new(
            notifications.clone(),
            Arc::new(move |chat_id: &str| {
                titles
                    .chat(chat_id)
                    .ok()
                    .flatten()
                    .and_then(|chat| chat.title.or(chat.last_message_preview))
            }),
        )));
        sessions.add_turn_observer(Arc::new(usage.clone()));
        // Run groups post `group_ready` and `check_failed` (SD1 Tasks 12, 14).
        run_groups.set_notifications(notifications.clone());
        let spaces_sync = SpacesSync::start(repos.clone(), workspace.clone(), &device_id);
        let marketplace = marketplace::MarketplaceSlot::default();
        sessions.set_marketplace(marketplace.clone());
        Ok(Self {
            sessions,
            doc_host,
            workspace,
            registry,
            repos,
            workspace_files,
            terminals,
            project_actions,
            previews,
            change_requests,
            diff_sync,
            checkpoints,
            run_groups,
            notifications,
            usage,
            approvals,
            marketplace,
            spaces_sync,
            uploads,
            agent_accounts,
            harness_updates,
            factory,
            device_id,
            workspace_scope: profile.scope(),
            _instance_lock: lock,
        })
    }

    /// Attaches the daemon's marketplace (MK1 Task 4b): `MarketplaceService`
    /// answers from it, its harnesses join the registry and its MCP servers
    /// every run. Must run inside a Tokio runtime.
    pub fn attach_marketplace(&self, host: loams_agentd_marketplace::MarketplaceHost) {
        marketplace::attach(&self.marketplace, self.registry.clone(), host);
    }

    pub fn workspace_scope(&self) -> WorkspaceScope {
        self.workspace_scope
    }

    pub fn rpc_service(&self) -> Arc<EngineRpc> {
        let rpc = EngineRpc::new(
            self.sessions.clone(),
            self.doc_host.clone(),
            self.workspace.clone(),
            self.registry.clone(),
            self.repos.clone(),
            self.workspace_files.clone(),
            self.terminals.clone(),
            self.project_actions.clone(),
            self.change_requests.clone(),
            self.diff_sync.clone(),
            self.uploads.clone(),
            self.agent_accounts.clone(),
            self.workspace_scope,
        )
        .with_previews(self.previews.clone())
        .with_checkpoints(self.checkpoints.clone())
        .with_run_groups(self.run_groups.clone())
        .with_notifications(self.notifications.clone())
        .with_usage(self.usage.clone())
        .with_approvals(self.approvals.clone())
        .with_marketplace(self.marketplace.clone())
        .with_harness_updates(self.harness_updates.clone())
        .with_factory(self.factory.clone());
        Arc::new(rpc)
    }

    /// Graceful teardown: settle live runs (streaming entries stamped `aborted`),
    /// kill live PTYs, stamp our workspace `lastSeenAt`, and flush every open doc
    /// snapshot.
    pub async fn shutdown(&self) {
        // A turn the shutdown ends keeps its approvals: they are listed
        // again after the restart (SD1 Task 13).
        self.approvals.freeze();
        self.previews.shutdown().await;
        self.harness_updates.shutdown().await;
        // Stop sending queued group members before runs are settled.
        self.run_groups.shutdown().await;
        // A run interruption transitions its chat to Idle, and Idle normally
        // releases the next queued row. Freeze first so quitting never starts
        // recovered work while the engine is being torn down.
        self.doc_host.pause_all_queues();
        self.sessions.shutdown().await;
        self.terminals.shutdown();
        self.agent_accounts.shutdown();
        self.change_requests.shutdown();
        self.diff_sync.shutdown().await;
        self.workspace_files.shutdown().await;
        self.spaces_sync.shutdown().await;
        self.doc_host.shutdown_workers().await;
        self.doc_host.flush_all();
        self.workspace.shutdown();
        // Break the sessions ⇄ doc-host retain cycle so the replaced graph can
        // actually be freed once the last handle drops.
        self.sessions.clear_doc_host();
    }
}

pub struct Engine {
    pub config: EngineConfig,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

/// A fully assembled engine for the local profile, served by `loams-agentd run`.
pub struct EngineRuntime {
    core: EngineCore,
}

impl std::fmt::Debug for EngineRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineRuntime").finish_non_exhaustive()
    }
}

impl EngineRuntime {
    pub fn core(&self) -> &EngineCore {
        &self.core
    }

    pub fn workspace_scope(&self) -> WorkspaceScope {
        self.core.workspace_scope()
    }

    pub async fn shutdown(&self) {
        self.core.shutdown().await;
    }
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    /// Resolve the one-shot identity served before profile stores are available.
    pub fn engine_info(
        config: &EngineConfig,
        workspace_scope: WorkspaceScope,
    ) -> Result<EngineInfo, EngineError> {
        std::fs::create_dir_all(&config.data_dir)?;
        Ok(EngineInfo {
            device_id: load_or_create_device_id(&config.data_dir)?,
            workspace_scope,
            capabilities: loams_agentd_proto::capabilities::current(),
        })
    }

    /// Open one already-resolved profile and start its local services.
    pub async fn assemble_runtime(
        config: &EngineConfig,
        profile: EngineProfile,
    ) -> anyhow::Result<EngineRuntime> {
        Self::assemble_runtime_inner(config, profile, None).await
    }

    /// Like [`Self::assemble_runtime`], but against an [`InstanceLock`] the
    /// caller already holds on the profile's device root.
    pub async fn assemble_runtime_with_lock(
        config: &EngineConfig,
        profile: EngineProfile,
        lock: InstanceLock,
    ) -> anyhow::Result<EngineRuntime> {
        Self::assemble_runtime_inner(config, profile, Some(lock)).await
    }

    async fn assemble_runtime_inner(
        config: &EngineConfig,
        profile: EngineProfile,
        lock: Option<InstanceLock>,
    ) -> anyhow::Result<EngineRuntime> {
        let lock = match lock {
            Some(lock) => lock,
            None => {
                std::fs::create_dir_all(profile.device_root())?;
                InstanceLock::acquire(profile.device_root())?
            }
        };
        let core = EngineCore::assemble_with_settings(
            profile,
            Arc::new(default_registry()),
            config.default_harness,
            lock,
            config.mcp_endpoint.clone(),
            config.approvals,
        )?;
        core.terminals.set_shell(config.terminal_shell.clone());
        core.doc_host.set_tool_output_limits(config.tool_outputs);
        let preview_workspace = core.workspace.clone();
        let preview_device = core.device_id.clone();
        let projects = Arc::new(move || {
            preview_workspace
                .read_chats()
                .unwrap_or_default()
                .into_iter()
                .filter(|chat| chat.device_id == preview_device)
                .filter_map(|chat| chat.cwd.map(std::path::PathBuf::from))
                .collect()
        });
        core.previews.start(projects).await;
        tracing::info!(device_id = %core.device_id, "engine core assembled");
        // Managed ACP adapters install in the background at boot (agents
        // whose CLI is present but whose adapter isn't yet), so a first chat
        // never waits on — or dies inside — an npm run.
        loams_agentd_harness::acp::prewarm_managed_adapters();
        Ok(EngineRuntime { core })
    }
}

/// Best-effort human name for this device's registry row.
fn local_device_name(device_id: &str) -> String {
    select_local_device_name(
        [
            std::env::var("LOAMS_DESKTOP_DEVICE_NAME").ok(),
            native_friendly_device_name(),
            std::env::var("HOSTNAME").ok(),
            gethostname::gethostname().into_string().ok(),
            std::fs::read_to_string("/etc/hostname").ok(),
        ],
        device_id,
        std::env::consts::OS,
    )
}

fn select_local_device_name(
    candidates: impl IntoIterator<Item = Option<String>>,
    device_id: &str,
    platform: &str,
) -> String {
    candidates
        .into_iter()
        .flatten()
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let platform = match platform {
                "macos" => "macOS",
                "windows" => "Windows",
                "linux" => "Linux",
                _ => "Local",
            };
            let short_id: String = device_id.chars().take(8).collect();
            format!("{platform} device {short_id}")
        })
}

#[cfg(target_os = "macos")]
fn native_friendly_device_name() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn native_friendly_device_name() -> Option<String> {
    #[cfg(target_os = "windows")]
    return std::env::var("COMPUTERNAME").ok();

    #[cfg(not(target_os = "windows"))]
    None
}

/// Stable per-installation device id, persisted at `{data_dir}/device-id`.
fn load_or_create_device_id(data_dir: &Path) -> Result<String, EngineError> {
    std::fs::create_dir_all(data_dir)?;
    // EngineInfo is resolved before the lifetime InstanceLock is acquired, so
    // identity creation and legacy repair need their own short critical section.
    // The OS releases this lock after a crash; unlike a create_new lockfile it
    // cannot strand an installation permanently.
    let _identity_lock = DeviceIdentityLock::acquire(data_dir)?;
    let path = data_dir.join("device-id");
    let recovering_empty = match std::fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => return Ok(id.trim().to_string()),
        // Older releases used truncate+write. A crash between those operations
        // left a zero-byte file that is safe to replace with a fresh identity.
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };

    let id = new_id();
    let temp_path = data_dir.join(format!(
        ".device-id.tmp-{}-{}",
        std::process::id(),
        new_id()
    ));
    let write_result = (|| -> Result<(), EngineError> {
        let mut temp = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        temp.write_all(id.as_bytes())?;
        temp.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }

    // Fresh installs use create-if-absent. Legacy empty files need an atomic
    // same-directory replacement on Unix; the Windows fallback runs under the
    // identity lock and remains recoverable if interrupted.
    let publish_result = if recovering_empty {
        match std::fs::read_to_string(&path) {
            Ok(id) if !id.trim().is_empty() => {
                let _ = std::fs::remove_file(&temp_path);
                return Ok(id.trim().to_string());
            }
            Ok(_) => replace_empty_device_id(&temp_path, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::hard_link(&temp_path, &path)
            }
            Err(err) => Err(err),
        }
    } else {
        std::fs::hard_link(&temp_path, &path)
    };
    let _ = std::fs::remove_file(&temp_path);
    match publish_result {
        Ok(()) => Ok(id),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let winner = std::fs::read_to_string(&path)?;
            if winner.trim().is_empty() {
                Err(EngineError::Other(format!(
                    "invalid device identity {}: file is empty",
                    path.display()
                )))
            } else {
                Ok(winner.trim().to_string())
            }
        }
        Err(err) => Err(err.into()),
    }
}

struct DeviceIdentityLock {
    _file: std::fs::File,
}

impl DeviceIdentityLock {
    #[allow(unsafe_code)]
    fn acquire(data_dir: &Path) -> Result<Self, EngineError> {
        let path = data_dir.join("device-id.lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
            let mut retries = 200;
            let file = loop {
                match options.open(&path) {
                    Ok(file) => break file,
                    // share_mode(0) means a concurrent holder fails the open
                    // with ERROR_SHARING_VIOLATION (raw os error 32), which
                    // Rust maps to ErrorKind::Uncategorized, not
                    // PermissionDenied. Retry through both.
                    Err(err)
                        if (err.raw_os_error()
                            == Some(
                                windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32,
                            )
                            || err.kind() == std::io::ErrorKind::PermissionDenied)
                            && retries > 0 =>
                    {
                        retries -= 1;
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(err) => return Err(err.into()),
                }
            };
            return Ok(Self { _file: file });
        }

        #[cfg(not(windows))]
        let file = options.open(&path)?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            loop {
                // SAFETY: flock on a descriptor this function owns; it touches no memory.
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                    break;
                }
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINTR) {
                    return Err(err.into());
                }
            }
        }

        #[cfg(not(windows))]
        Ok(Self { _file: file })
    }
}

fn replace_empty_device_id(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::rename(temp_path, path)
    }
    #[cfg(not(unix))]
    {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        std::fs::hard_link(temp_path, path)
    }
}

#[cfg(all(test, windows))]
mod identity_lock_retry_tests {
    use super::*;

    /// A concurrent holder of the lock file (share_mode(0)) fails the open
    /// with ERROR_SHARING_VIOLATION, which Rust reports as
    /// ErrorKind::Uncategorized, not PermissionDenied. acquire must retry
    /// through that error until the holder releases; before the fix the
    /// retry loop never matched it and startup failed outright.
    #[test]
    fn acquire_retries_through_sharing_violations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-id.lock");

        // Hold the file exclusively for 50ms, then release. The retry loop
        // has a 200 x 5ms budget, so the timing is comfortable.
        {
            use std::os::windows::fs::OpenOptionsExt;
            let holder = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&path)
                .unwrap();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                drop(holder);
            });
        }

        let lock = DeviceIdentityLock::acquire(dir.path());
        assert!(
            lock.is_ok(),
            "acquire did not retry through the sharing violation"
        );
    }
}

#[cfg(test)]
mod device_name_tests {
    use super::select_local_device_name;

    fn name(candidates: &[Option<&str>], device_id: &str, platform: &str) -> String {
        select_local_device_name(
            candidates
                .iter()
                .map(|candidate| candidate.map(str::to_string)),
            device_id,
            platform,
        )
    }

    #[test]
    fn explicit_override_wins_and_is_trimmed() {
        assert_eq!(
            name(
                &[Some("  Studio Mac  "), Some("system-host")],
                "17bc0aa2-rest",
                "macos"
            ),
            "Studio Mac"
        );
    }

    #[test]
    fn native_friendly_name_wins_over_hostnames() {
        assert_eq!(
            name(
                &[
                    None,
                    Some("MacBook Pro de Jose"),
                    None,
                    Some("MacBook-Pro.local"),
                ],
                "17bc0aa2-rest",
                "macos"
            ),
            "MacBook Pro de Jose"
        );
    }

    #[test]
    fn windows_computer_name_is_used_when_present() {
        assert_eq!(
            name(
                &[None, Some("DESKTOP-123"), Some("shell-host")],
                "17bc0aa2-rest",
                "windows"
            ),
            "DESKTOP-123"
        );
    }

    #[test]
    fn blank_candidates_are_ignored() {
        assert_eq!(
            name(
                &[Some("  "), None, Some("\n"), Some("linux-box")],
                "17bc0aa2-rest",
                "linux"
            ),
            "linux-box"
        );
    }

    #[test]
    fn final_fallback_is_platform_specific_and_distinct() {
        assert_eq!(
            name(&[None, Some(" ")], "17bc0aa2-rest", "linux"),
            "Linux device 17bc0aa2"
        );
    }
}
