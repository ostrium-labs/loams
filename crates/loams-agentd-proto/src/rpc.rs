//! The RPC contract: the handshake, the error reasons and the method table
//! (design §50 §5.4, §7, §8; plan DD1 Tasks 6, 7 and 9).
//!
//! `Hello` is the first call on a connection. A client whose `protocol`
//! differs from [`AGENTD_PROTOCOL`] is answered with an error whose text
//! starts with [`PROTOCOL_MISMATCH`], and the daemon closes the connection.
//!
//! [`METHODS`] lists every method the daemon serves, with its kind, the
//! TypeScript names of its params and result, the role it needs and whether
//! the renderer may call it through preload. The same table generates the
//! method-name constants ([`methods`]) and one marker type per method
//! ([`calls`]) that ties the name to its Rust params and result types, so
//! the server parses and replies with exactly the types the TypeScript
//! client is generated from.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::approvals::{
    ApprovalList, ResolveApprovalParams, SetThreadApprovalModeParams, WatchApprovalsParams,
};
use crate::chat::{BeginQueueEditOutcome, FinishQueueEditOutcome, RenewQueueEditOutcome};
use crate::checkpoints::{
    CheckpointList, ListCheckpointsParams, RestoreCheckpointParams, RestoreCheckpointReply,
};
use crate::daemon::DaemonMode;
use crate::factory::{
    FactoryAppList, FactoryAppParams, FactoryAppReply, FactoryConfigureParams, FactoryQueryParams,
    FactoryQueryReply,
};
use crate::factory_loop::{
    CheckLogReply, ChecksReply, CloseChangeRequestParams, CloseChangeRequestReply,
    CreateRunWorktreeParams, DeployLiveAppParams, DeployLiveAppReply, GetApprovalParams,
    GetCheckLogParams, GetChecksParams, GetThreadDiffParams, MergeChangeRequestParams,
    MergeChangeRequestReply, OpenChangeRequestParams, OpenChangeRequestReply,
    PostReviewCommentParams, RequestApprovalParams, TypedApproval,
};
use crate::git_write::{
    CommitWorktreeParams, CommitWorktreeReply, CreateChangeRequestParams, CreateChangeRequestReply,
    DiscardPathsParams, MergeBranchParams, MergeBranchReply, PushBranchParams, PushBranchReply,
    WorktreeList,
};
use crate::groups::{
    CreateRunGroupParams, CreateRunGroupReply, GetRunGroupComparisonParams, ResolveRunGroupParams,
    ResolveRunGroupReply, RunGroupCheckParams, RunGroupCheckReply, RunGroupComparison,
    RunGroupList, WatchRunGroupsParams,
};
use crate::marketplace::{
    AddMarketplaceSourceParams, AvailableUpdateList, InstallListingParams, InstalledListingList,
    InstalledListingReply, ListingDetail, ListingRef, ListingSearchResults, MarketplaceSource,
    MarketplaceSourceList, PlanInstallReply, RemoveMarketplaceSourceParams, SearchListingsParams,
    UninstallListingParams, UpdateListingParams,
};
use crate::notify::Notification;
use crate::params::*;
use crate::review::{
    ContentSearchMatch, DeleteReviewCommentParams, PostReviewToChangeRequestParams,
    PostReviewToChangeRequestReply, ResolveReviewCommentParams, ReviewComment, ReviewCommentsFrame,
    SearchWorkspaceContentParams, SendReviewToThreadParams, WatchReviewCommentsParams,
};
use crate::transcript::TranscriptUpdate;
use crate::usage::{GetThreadUsageParams, ThreadUsage};
use crate::{
    AgentAccountsSnapshot, AgentLoginPoll, AgentLoginStart, AttachmentChunk, Chat,
    CheckoutChangeRequestStatus, CheckoutDiff, CheckoutFileDiffText, CreateWorktreeOutcome,
    DeleteWorkspaceEntryRequest, DriveListing, EngineInfo, FolderListing,
    GetCheckoutFileDiffTextRequest, GitHistoryPage, HarnessUpdateStatus,
    ListWorkspaceDirectoryRequest, MoveWorkspaceEntryRequest, PreviewSnapshot, ProjectActionRun,
    ProjectActionsSnapshot, ReadWorkspaceFileRequest, ReadWorkspaceImageRequest, Repo,
    SearchWorkspaceFilesRequest, SidebarPreferencesState, TerminalEvent, TerminalSession,
    TitleSettings, ToolOutputChunk, WatchPreviewsParams, WatchWorkspaceFilesRequest,
    WorkspaceDirectoryPage, WorkspaceFileChanges, WorkspaceFileText, WorkspaceGitStatusFrame,
    WorkspaceImageChunk, WorkspaceMutationOutcome, Worktree, WriteWorkspaceFileOutcome,
    WriteWorkspaceFileRequest,
};

/// The wire protocol. It changes only on a breaking change (§50 §5.3).
pub const AGENTD_PROTOCOL: u32 = 1;

/// Error reason (the `{err}` text prefix) for a `Hello` with another protocol.
pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";

/// Error reason for a call before a successful `Hello`, or on a connection
/// whose scoped run token was revoked. The connection closes after it.
pub const UNAUTHENTICATED: &str = "unauthenticated";

/// Error reason for a method the connection's role may not call, or a
/// scoped run token's call on another session (design §50 §7).
pub const FORBIDDEN_ROLE: &str = "forbidden_role";

/// Error reason for a stream beyond the per-connection limit (DD1 Task 7).
pub const LIMIT_EXCEEDED: &str = "limit_exceeded";

/// Every error reason a client can see as the `{err}` text prefix, which
/// the TypeScript client turns into `AgentdError.code` (Shared contracts,
/// plus [`LIMIT_EXCEEDED`] from Task 7, `harness_unsupported` from Task 3 and
/// SD1 Task 9's git-write reasons).
/// Texts without one of these prefixes are plain failures.
pub const ERROR_REASONS: &[&str] = &[
    UNAUTHENTICATED,
    FORBIDDEN_ROLE,
    PROTOCOL_MISMATCH,
    "agentd_draining",
    "unknown_method",
    "bad_params",
    "key_required",
    "unknown_provider",
    "invalid_url",
    "tool_unavailable_remote_server",
    "not_found",
    "engine_not_ready",
    "keyring_unavailable",
    LIMIT_EXCEEDED,
    crate::HARNESS_UNSUPPORTED,
    crate::git_write::STALE_DIFF,
    crate::git_write::MERGE_CONFLICT,
    crate::git_write::NOTHING_TO_COMMIT,
    crate::git_write::PUSH_REJECTED,
    crate::git_write::PROVIDER_NOT_CONFIGURED,
    crate::checkpoints::CHECKPOINT_MISSING,
    crate::groups::GROUP_NOT_READY,
    crate::approvals::APPROVAL_NOT_PENDING,
    crate::factory_loop::APPROVAL_REQUIRED,
    crate::factory_loop::APPROVAL_MISMATCH,
    crate::marketplace::PLAN_CHANGED,
    crate::marketplace::CONSENT_REQUIRED,
    crate::marketplace::LISTING_REVOKED,
    crate::marketplace::UNTRUSTED_LISTING,
];

/// The MCP shim's environment, set only by the daemon when it injects the
/// shim into a harness run (Shared contracts). These three keys and no
/// others: never the main token.
///
/// The run's scoped token (role `mcp`), valid until the run ends.
pub const ENV_RUN_TOKEN: &str = "LOAMS_AGENTD_RUN_TOKEN";
/// The daemon's loopback RPC port.
pub const ENV_PORT: &str = "LOAMS_AGENTD_PORT";
/// The session (chat) the run belongs to; the token is bound to it.
pub const ENV_SESSION_ID: &str = "LOAMS_AGENTD_SESSION_ID";

/// Who is calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ClientKind {
    /// Loams Desktop's main process, the one general client.
    ElectronMain,
    /// `loams-agentd` itself: `status`, or a second `run` checking the first.
    Cli,
    /// The MCP shim injected into a harness run.
    Mcp,
}

/// What a connection may call (§50 §7). The main token gives `Owner`; a
/// scoped run token gives `Mcp`. In the method table it is the least role a
/// method needs: `Owner` methods are the main token's alone, `Mcp` methods
/// are open to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Mcp,
}

/// `Hello`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub client: ClientKind,
    pub client_version: String,
    pub protocol: u32,
}

/// `Hello`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub protocol: u32,
    pub version: String,
    pub build_sha: String,
    pub mode: DaemonMode,
    pub pid: u32,
    pub started_at_ms: i64,
    pub role: Role,
    pub features: Vec<String>,
}

/// A unary call answers once (`{id, ok}`); a stream sends items until
/// `{id, done}` or the client cancels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    Unary,
    Stream,
}

/// One row of the method table (design §50 §8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    /// The flat PascalCase method name (Ruling 3).
    pub name: &'static str,
    pub kind: MethodKind,
    /// The TypeScript name of the params type.
    pub params: &'static str,
    /// The TypeScript name of the result, or of each stream item.
    pub result: &'static str,
    /// The least role that may call it.
    pub role: Role,
    /// Whether Electron's preload forwards it from the renderer.
    pub renderer: bool,
}

/// A method's types, implemented by its marker in [`calls`].
pub trait Method: 'static {
    const NAME: &'static str;
    /// What the call's `params` parse into.
    type Params: Serialize + DeserializeOwned + TS + 'static;
    /// The unary result, or each stream item.
    type Result: Serialize + DeserializeOwned + TS + 'static;

    /// The method's row in [`METHODS`].
    fn spec() -> &'static MethodSpec {
        // Cannot fail: `method_table!` emits each marker together with its
        // row (checked by `markers_name_their_rows`).
        method_spec(Self::NAME).expect("every marker in `calls` has a row in METHODS")
    }
}

/// `Self` goes on the wire as the JSON of `T`: the identity, and a list
/// newtype's `Vec` (the engine returns the `Vec`, the table names the
/// newtype). The server's typed reply helpers require it, so a handler
/// cannot answer a method with another type than its row names.
pub trait WireAs<T> {}

impl<T> WireAs<T> for T {}

/// Walks the method table with each method's marker type.
pub trait MethodVisitor {
    fn visit<M: Method>(&mut self);
}

/// The row of `name`, if the daemon serves it.
pub fn method_spec(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.name == name)
}

macro_rules! list_wire {
    ($($newtype:ident($inner:ty)),* $(,)?) => {
        $(impl WireAs<$newtype> for $inner {})*
    };
}

list_wire!(
    HarnessCatalog(Vec<crate::HarnessDescriptor>),
    ModelList(Vec<crate::Model>),
    SkillList(Option<Vec<crate::invocation::Skill>>),
    SlashCommandList(Vec<crate::SlashCommand>),
    HarnessUpdateList(Vec<HarnessUpdateStatus>),
    ChatList(Vec<Chat>),
    SpaceList(Vec<crate::Space>),
    SessionList(Vec<crate::Session>),
    RepoList(Vec<Repo>),
    BranchList(Vec<String>),
    RepoRefList(Vec<crate::RepoRef>),
    GitAvatars(std::collections::HashMap<String, String>),
    FileSearchMatchList(Vec<crate::FileSearchMatch>),
    WorkspaceFileSearchMatchList(Vec<crate::WorkspaceFileSearchMatch>),
    CheckoutDiffList(Vec<CheckoutDiff>),
    WorktreeList(Vec<crate::git_write::WorktreeInfo>),
);

macro_rules! method_table {
    ($(
        $(#[doc = $doc:literal])*
        $konst:ident = $name:ident : $kind:ident($params:ident -> $result:ident), $role:ident, renderer: $renderer:literal;
    )*) => {
        /// RPC method names, one per row of [`METHODS`]; the single source of
        /// truth for both ends (re-exported as `loams_agentd_rpc::methods`).
        pub mod methods {
            $(
                $(#[doc = $doc])*
                pub const $konst: &str = stringify!($name);
            )*
        }

        /// One marker type per method, binding its name to its Rust types.
        pub mod calls {
            $(
                $(#[doc = $doc])*
                #[derive(Debug, Clone, Copy)]
                pub struct $name;

                impl super::Method for $name {
                    const NAME: &'static str = stringify!($name);
                    type Params = super::$params;
                    type Result = super::$result;
                }
            )*
        }

        /// Every method the daemon serves (design §50 §8.2).
        pub const METHODS: &[MethodSpec] = &[
            $(
                MethodSpec {
                    name: stringify!($name),
                    kind: MethodKind::$kind,
                    params: stringify!($params),
                    result: stringify!($result),
                    role: Role::$role,
                    renderer: $renderer,
                },
            )*
        ];

        /// Visit every method, in table order, with its marker type.
        pub fn visit_methods(visitor: &mut impl MethodVisitor) {
            $(visitor.visit::<calls::$name>();)*
        }
    };
}

method_table! {
    /// The handshake, the first call on a connection (design §50 §5.4).
    /// Another protocol is answered `protocol_mismatch: …` and the connection
    /// closes. `loams-agentd` serves it in front of the engine.
    HELLO = Hello: Unary(Hello -> HelloReply), Mcp, renderer: false;
    /// Stop the daemon: it closes the listener and every connection, shuts
    /// the engine down and exits 0 (the fork's headless `StopEngine`, ruling
    /// T0-1). Served by `loams-agentd`.
    SHUTDOWN = Shutdown: Unary(Empty -> OkReply), Owner, renderer: false;
    WATCH_PREVIEWS = WatchPreviews: Stream(WatchPreviewsParams -> PreviewSnapshot), Owner, renderer: true;
    LIST_HARNESSES = ListHarnesses: Unary(Empty -> HarnessCatalog), Mcp, renderer: true;
    CANCEL_INSTALL = CancelInstall: Unary(HarnessParams -> Empty), Owner, renderer: true;
    INSTALL_HARNESS = InstallHarness: Unary(HarnessParams -> HarnessCatalog), Owner, renderer: true;
    GET_TITLE_SETTINGS = GetTitleSettings: Unary(Empty -> TitleSettings), Owner, renderer: true;
    SET_TITLE_SETTINGS = SetTitleSettings: Unary(TitleSettings -> TitleSettings), Owner, renderer: true;
    /// Flip a harness's enablement (Settings → Providers); replies with the
    /// fresh `ListHarnesses` catalog.
    SET_HARNESS_ENABLED = SetHarnessEnabled: Unary(SetHarnessEnabledParams -> HarnessCatalog), Owner, renderer: true;
    LIST_MODELS = ListModels: Unary(ListModelsParams -> ModelList), Mcp, renderer: true;
    LIST_SKILLS = ListSkills: Unary(HarnessCatalogParams -> SkillList), Owner, renderer: true;
    LIST_COMMANDS = ListCommands: Unary(HarnessCatalogParams -> SlashCommandList), Owner, renderer: true;
    QUEUE_COMMAND = QueueCommand: Unary(QueueCommandParams -> QueueCommandReply), Mcp, renderer: true;
    /// User-driven retry of a chat's failed sends: re-sends every dead
    /// Run/Steer attempt (Rejected, Expired, or consumed with no result)
    /// under a new id, then drains.
    RETRY_DELIVERY = RetryDelivery: Unary(ChatIdParams -> Empty), Owner, renderer: true;
    TAKE_PROJECT_ACTION_SETUP = TakeProjectActionSetup: Unary(TakeProjectActionSetupParams -> ProjectActionSetup), Owner, renderer: true;
    FORK_SIDE_CHAT = ForkSideChat: Unary(ForkSideChatParams -> Chat), Owner, renderer: true;
    /// A thread's per-turn restore points, oldest first (SD1 Task 10, G6).
    LIST_CHECKPOINTS = ListCheckpoints: Unary(ListCheckpointsParams -> CheckpointList), Owner, renderer: true;
    /// Restore the thread's checkout to a turn's `pre` checkpoint, refusing a
    /// stale checksum; `worktree+transcript` also forks the thread at that
    /// turn (SD1 Task 10, G7). Never in the `mcp` role.
    RESTORE_CHECKPOINT = RestoreCheckpoint: Unary(RestoreCheckpointParams -> RestoreCheckpointReply), Owner, renderer: true;
    /// A thread's per-turn tokens and durations, oldest first, with their
    /// totals; informational, never billing (SD1 Task 14, G20).
    GET_THREAD_USAGE = GetThreadUsage: Unary(GetThreadUsageParams -> ThreadUsage), Owner, renderer: true;
    /// The last 200 notifications, then each new one: turns ended, approvals
    /// and questions waiting, errors, run groups ready, checks failed (SD1
    /// Task 14, G19).
    WATCH_NOTIFICATIONS = WatchNotifications: Stream(Empty -> Notification), Owner, renderer: true;
    /// The transcript: a `reset` frame, then deltas.
    WATCH_DOC_MESSAGES = WatchDocMessages: Stream(WatchDocMessagesParams -> TranscriptUpdate), Mcp, renderer: true;
    /// Messages typed while the agent was busy, held on the chat doc.
    WATCH_QUEUE = WatchQueue: Stream(ChatIdParams -> QueueSnapshot), Owner, renderer: true;
    /// Append to the queue.
    QUEUE_MESSAGE = QueueMessage: Unary(QueueMessageParams -> IdReply), Mcp, renderer: true;
    /// Retype a queued message; empty text deletes it.
    UPDATE_QUEUED_MESSAGE = UpdateQueuedMessage: Unary(UpdateQueuedMessageParams -> ChangedReply), Owner, renderer: true;
    /// Acquire a host-authoritative edit lease for one queued row.
    BEGIN_QUEUED_MESSAGE_EDIT = BeginQueuedMessageEdit: Unary(BeginQueuedMessageEditParams -> BeginQueueEditOutcome), Owner, renderer: true;
    /// Renew an acquired queue edit lease.
    RENEW_QUEUED_MESSAGE_EDIT = RenewQueuedMessageEdit: Unary(RenewQueuedMessageEditParams -> RenewQueueEditOutcome), Owner, renderer: true;
    /// Commit, cancel, discard, or explicitly release an acquired edit.
    FINISH_QUEUED_MESSAGE_EDIT = FinishQueuedMessageEdit: Unary(FinishQueuedMessageEditParams -> FinishQueueEditOutcome), Owner, renderer: true;
    MOVE_QUEUED_MESSAGE = MoveQueuedMessage: Unary(MoveQueuedMessageParams -> ChangedReply), Owner, renderer: true;
    REMOVE_QUEUED_MESSAGE = RemoveQueuedMessage: Unary(QueuedMessageRef -> RemovedReply), Owner, renderer: true;
    /// Interrupt whatever is running and send this one.
    SEND_QUEUED_MESSAGE_NOW = SendQueuedMessageNow: Unary(QueuedMessageRef -> SentReply), Owner, renderer: true;
    /// Steer this row into the live turn without interrupting it.
    STEER_QUEUED_MESSAGE_NOW = SteerQueuedMessageNow: Unary(QueuedMessageRef -> SentReply), Owner, renderer: true;
    WATCH_CHATS = WatchChats: Stream(Empty -> ChatList), Mcp, renderer: true;
    WATCH_SIDEBAR_PREFERENCES = WatchSidebarPreferences: Stream(Empty -> SidebarPreferencesState), Owner, renderer: true;
    WATCH_SESSIONS = WatchSessions: Stream(Empty -> SessionList), Mcp, renderer: true;
    /// The spaces (projects: device and folder pairs) from the workspace doc.
    WATCH_SPACES = WatchSpaces: Stream(Empty -> SpaceList), Mcp, renderer: true;
    /// Entity mutations against the workspace doc, tagged by `op`.
    MUTATE = Mutate: Unary(MutateParams -> MutateReply), Mcp, renderer: true;
    /// This engine's device id.
    LOCAL_DEVICE = LocalDevice: Unary(Empty -> LocalDeviceReply), Mcp, renderer: true;
    /// This engine runtime's fixed device and workspace identity.
    ENGINE_INFO = EngineInfo: Unary(Empty -> EngineInfo), Mcp, renderer: true;
    /// Readiness barrier: completes once stores and journals are assembled.
    ENGINE_READY = EngineReady: Unary(Empty -> EngineReadyReply), Owner, renderer: true;
    LIST_REPOS = ListRepos: Unary(Empty -> RepoList), Owner, renderer: true;
    ADD_REPO = AddRepo: Unary(AddRepoParams -> Repo), Owner, renderer: true;
    CLONE_REPO = CloneRepo: Unary(CloneRepoParams -> Repo), Owner, renderer: true;
    CREATE_REPO = CreateRepo: Unary(CreateRepoParams -> Repo), Owner, renderer: true;
    LIST_BRANCHES = ListBranches: Unary(RepoPathParams -> BranchList), Owner, renderer: true;
    LIST_REFS = ListRefs: Unary(RepoPathParams -> RepoRefList), Owner, renderer: true;
    LIST_GIT_HISTORY = ListGitHistory: Unary(ListGitHistoryParams -> GitHistoryPage), Owner, renderer: true;
    /// Fuzzy commit-subject / SHA search over the complete public history.
    SEARCH_GIT_HISTORY = SearchGitHistory: Unary(SearchGitHistoryParams -> GitHistoryPage), Owner, renderer: true;
    /// Resolve hosted profile images for a page of Git commit authors.
    RESOLVE_GIT_AVATARS = ResolveGitAvatars: Unary(ResolveGitAvatarsParams -> GitAvatars), Owner, renderer: true;
    /// Update remote-tracking refs without changing HEAD, the index, or files.
    FETCH_ALL = FetchAll: Unary(RepoPathParams -> OkReply), Owner, renderer: true;
    SWITCH_REF = SwitchRef: Unary(SwitchRefParams -> SwitchRefReply), Owner, renderer: true;
    LIST_FOLDERS = ListFolders: Unary(ListFoldersParams -> FolderListing), Owner, renderer: true;
    /// The device's browse roots: home plus mounted drives and volumes.
    LIST_DRIVES = ListDrives: Unary(Empty -> DriveListing), Owner, renderer: true;
    /// Fuzzy relative-path search rooted in a known chat or space checkout.
    SEARCH_FILES = SearchFiles: Unary(SearchFilesParams -> FileSearchMatchList), Owner, renderer: true;
    LIST_WORKSPACE_DIRECTORY = ListWorkspaceDirectory: Unary(ListWorkspaceDirectoryRequest -> WorkspaceDirectoryPage), Owner, renderer: true;
    SEARCH_WORKSPACE_FILES = SearchWorkspaceFiles: Unary(SearchWorkspaceFilesRequest -> WorkspaceFileSearchMatchList), Owner, renderer: true;
    READ_WORKSPACE_IMAGE = ReadWorkspaceImage: Unary(ReadWorkspaceImageRequest -> WorkspaceImageChunk), Owner, renderer: true;
    READ_WORKSPACE_FILE = ReadWorkspaceFile: Unary(ReadWorkspaceFileRequest -> WorkspaceFileText), Owner, renderer: true;
    MOVE_WORKSPACE_ENTRY = MoveWorkspaceEntry: Unary(MoveWorkspaceEntryRequest -> WorkspaceMutationOutcome), Owner, renderer: true;
    DELETE_WORKSPACE_ENTRY = DeleteWorkspaceEntry: Unary(DeleteWorkspaceEntryRequest -> WorkspaceMutationOutcome), Owner, renderer: true;
    WRITE_WORKSPACE_FILE = WriteWorkspaceFile: Unary(WriteWorkspaceFileRequest -> WriteWorkspaceFileOutcome), Owner, renderer: true;
    /// The checkout-scoped filesystem change stream.
    WATCH_WORKSPACE_FILES = WatchWorkspaceFiles: Stream(WatchWorkspaceFilesRequest -> WorkspaceFileChanges), Owner, renderer: true;
    CREATE_WORKTREE = CreateWorktree: Unary(CreateWorktreeParams -> CreateWorktreeOutcome), Owner, renderer: true;
    DELETE_WORKTREE = DeleteWorktree: Unary(DeleteWorktreeParams -> OkReply), Owner, renderer: true;
    /// Project Actions are private state on the device that owns the project.
    LIST_PROJECT_ACTIONS = ListProjectActions: Unary(ListProjectActionsParams -> ProjectActionsSnapshot), Owner, renderer: true;
    UPSERT_PROJECT_ACTION = UpsertProjectAction: Unary(UpsertProjectActionParams -> ProjectActionsSnapshot), Owner, renderer: true;
    DELETE_PROJECT_ACTION = DeleteProjectAction: Unary(DeleteProjectActionParams -> ProjectActionsSnapshot), Owner, renderer: true;
    RUN_PROJECT_ACTION = RunProjectAction: Unary(RunProjectActionParams -> ProjectActionRun), Owner, renderer: true;
    OPEN_TERMINAL = OpenTerminal: Unary(OpenTerminalParams -> TerminalSession), Owner, renderer: true;
    /// Replay from `afterSeq`, then the live tail.
    SUBSCRIBE_TERMINAL = SubscribeTerminal: Stream(SubscribeTerminalParams -> TerminalEvent), Owner, renderer: true;
    WRITE_TERMINAL = WriteTerminal: Unary(WriteTerminalParams -> OkReply), Owner, renderer: true;
    RESIZE_TERMINAL = ResizeTerminal: Unary(ResizeTerminalParams -> OkReply), Owner, renderer: true;
    CLOSE_TERMINAL = CloseTerminal: Unary(TerminalIdParams -> OkReply), Owner, renderer: true;
    /// Checkout diffs of this device's chats.
    WATCH_CHECKOUT_DIFFS = WatchCheckoutDiffs: Stream(Empty -> CheckoutDiffList), Owner, renderer: true;
    WATCH_WORKSPACE_GIT_STATUS = WatchWorkspaceGitStatus: Stream(WatchWorkspaceFilesRequest -> WorkspaceGitStatusFrame), Owner, renderer: true;
    /// The current pull request of one checkout.
    WATCH_CHECKOUT_CHANGE_REQUEST = WatchCheckoutChangeRequest: Stream(WatchCheckoutChangeRequestParams -> CheckoutChangeRequestStatus), Owner, renderer: true;
    GET_CHECKOUT_DIFF = GetCheckoutDiff: Unary(GetCheckoutDiffParams -> CheckoutDiff), Owner, renderer: true;
    /// Permanently restore one chat-owned checkout to its current HEAD and
    /// remove only its untracked, non-ignored paths.
    DISCARD_WORKING_TREE = DiscardWorkingTree: Unary(DiscardWorkingTreeParams -> DiscardWorkingTreeReply), Owner, renderer: true;
    /// G21 (SD1 Task 9): restore only these paths of one checkout to HEAD,
    /// refused (`stale_diff`) unless the diff checksum is still the one the
    /// person confirmed.
    DISCARD_PATHS = DiscardPaths: Unary(DiscardPathsParams -> Empty), Owner, renderer: true;
    /// G1: a repository's worktrees, joined with this device's chats.
    LIST_WORKTREES = ListWorktrees: Unary(RepoPathParams -> WorktreeList), Owner, renderer: true;
    /// G2: stage and commit in one checkout, with the user's own identity.
    COMMIT_WORKTREE = CommitWorktree: Unary(CommitWorktreeParams -> CommitWorktreeReply), Owner, renderer: true;
    /// G3: push one branch; a forced push is only ever `--force-with-lease`.
    PUSH_BRANCH = PushBranch: Unary(PushBranchParams -> PushBranchReply), Owner, renderer: true;
    /// G4: open a pull request on Forgejo (REST API) or GitHub (`gh`).
    CREATE_CHANGE_REQUEST = CreateChangeRequest: Unary(CreateChangeRequestParams -> CreateChangeRequestReply), Owner, renderer: true;
    /// G5: merge, squash, rebase or fast-forward one branch into another;
    /// stops on conflicts with a clean tree (`merge_conflict`).
    MERGE_BRANCH = MergeBranch: Unary(MergeBranchParams -> MergeBranchReply), Owner, renderer: true;
    /// G8 (SD1 Task 11): a checkout's review comments, as a reset and then
    /// upserts and removals; `outdated` follows the checkout's diff.
    WATCH_REVIEW_COMMENTS = WatchReviewComments: Stream(WatchReviewCommentsParams -> ReviewCommentsFrame), Owner, renderer: true;
    /// G9: create (empty `id`) or edit one review comment.
    UPSERT_REVIEW_COMMENT = UpsertReviewComment: Unary(ReviewComment -> IdReply), Owner, renderer: true;
    /// G9.
    DELETE_REVIEW_COMMENT = DeleteReviewComment: Unary(DeleteReviewCommentParams -> IdReply), Owner, renderer: true;
    /// G9.
    RESOLVE_REVIEW_COMMENT = ResolveReviewComment: Unary(ResolveReviewCommentParams -> IdReply), Owner, renderer: true;
    /// G10: queue one run on the thread, a fixed template quoting the
    /// comments as data.
    SEND_REVIEW_TO_THREAD = SendReviewToThread: Unary(SendReviewToThreadParams -> QueueCommandReply), Owner, renderer: true;
    /// G11: post the comments as a review on the branch's pull request.
    POST_REVIEW_TO_CHANGE_REQUEST = PostReviewToChangeRequest: Unary(PostReviewToChangeRequestParams -> PostReviewToChangeRequestReply), Owner, renderer: true;
    /// G23: search a workspace's file contents, one match per item.
    SEARCH_WORKSPACE_CONTENT = SearchWorkspaceContent: Stream(SearchWorkspaceContentParams -> ContentSearchMatch), Owner, renderer: true;
    /// G12 (SD1 Task 12): one chat and one worktree per variant from the same
    /// base, and the same run queued in each within the concurrency limits.
    CREATE_RUN_GROUP = CreateRunGroup: Unary(CreateRunGroupParams -> CreateRunGroupReply), Owner, renderer: true;
    /// G13: every run group, again on each change.
    WATCH_RUN_GROUPS = WatchRunGroups: Stream(WatchRunGroupsParams -> RunGroupList), Owner, renderer: true;
    /// G14: the diff matrix and a pair's diff, from snapshot trees.
    GET_RUN_GROUP_COMPARISON = GetRunGroupComparison: Unary(GetRunGroupComparisonParams -> RunGroupComparison), Owner, renderer: true;
    /// G15: the group's check action in each member's worktree.
    RUN_GROUP_CHECK = RunGroupCheck: Unary(RunGroupCheckParams -> RunGroupCheckReply), Owner, renderer: true;
    /// G16: merge, push and open a change request, or keep the winner, through
    /// Task 9's git writes; `group_not_ready` while members run.
    RESOLVE_RUN_GROUP = ResolveRunGroup: Unary(ResolveRunGroupParams -> ResolveRunGroupReply), Owner, renderer: true;
    /// MK1 Task 4b (`MarketplaceService`): the sources, in search order.
    LIST_MARKETPLACE_SOURCES = ListMarketplaceSources: Unary(Empty -> MarketplaceSourceList), Owner, renderer: true;
    /// Adds a source pinned to its signing identity, kept only once its
    /// index verifies.
    ADD_MARKETPLACE_SOURCE = AddMarketplaceSource: Unary(AddMarketplaceSourceParams -> MarketplaceSource), Owner, renderer: true;
    /// Removes a source; its installs stay.
    REMOVE_MARKETPLACE_SOURCE = RemoveMarketplaceSource: Unary(RemoveMarketplaceSourceParams -> OkReply), Owner, renderer: true;
    /// Searches every source's verified index.
    SEARCH_LISTINGS = SearchListings: Unary(SearchListingsParams -> ListingSearchResults), Owner, renderer: true;
    /// One listing with its versions and revocations.
    GET_LISTING = GetListing: Unary(ListingRef -> ListingDetail), Owner, renderer: true;
    /// The consent plan; creates nothing (D1466).
    PLAN_INSTALL = PlanInstall: Unary(ListingRef -> PlanInstallReply), Owner, renderer: true;
    /// Installs exactly the confirmed plan, by its digest (`plan_changed`).
    INSTALL_LISTING = InstallListing: Unary(InstallListingParams -> InstalledListingReply), Owner, renderer: true;
    /// Removes the package, its registration and its secrets; data stays.
    UNINSTALL_LISTING = UninstallListing: Unary(UninstallListingParams -> OkReply), Owner, renderer: true;
    LIST_INSTALLED_LISTINGS = ListInstalledListings: Unary(Empty -> InstalledListingList), Owner, renderer: true;
    /// Newer versions of installs, and whether each widens a permission.
    CHECK_LISTING_UPDATES = CheckListingUpdates: Unary(Empty -> AvailableUpdateList), Owner, renderer: true;
    /// Updates one install (`consent_required` without the new plan's digest
    /// when the update widens a permission).
    UPDATE_LISTING = UpdateListing: Unary(UpdateListingParams -> InstalledListingReply), Owner, renderer: true;
    /// G17 (SD1 Task 13): the pending approvals, again on each change; with a
    /// `chatId`, that thread's only and its effective mode.
    WATCH_APPROVALS = WatchApprovals: Stream(WatchApprovalsParams -> ApprovalList), Owner, renderer: true;
    /// G18: DD1 Task 24's answer to an approval. `owner` (and, on the remote
    /// listener, `studio`) only: never `mcp` (DD1 Ruling 8).
    RESOLVE_APPROVAL = ResolveApproval: Unary(ResolveApprovalParams -> OkReply), Owner, renderer: true;
    /// G22: a thread's approval mode, `full-access` or `supervised`.
    SET_THREAD_APPROVAL_MODE = SetThreadApprovalMode: Unary(SetThreadApprovalModeParams -> OkReply), Owner, renderer: true;
    GET_CHECKOUT_FILE_DIFF_TEXT = GetCheckoutFileDiffText: Unary(GetCheckoutFileDiffTextRequest -> CheckoutFileDiffText), Owner, renderer: true;
    LIST_AGENT_ACCOUNTS = ListAgentAccounts: Unary(ListAgentAccountsParams -> AgentAccountsSnapshot), Owner, renderer: true;
    ACTIVATE_AGENT_ACCOUNT = ActivateAgentAccount: Unary(AgentAccountParams -> AgentAccountsSnapshot), Owner, renderer: true;
    FORGET_AGENT_ACCOUNT = ForgetAgentAccount: Unary(AgentAccountParams -> AgentAccountsSnapshot), Owner, renderer: true;
    START_AGENT_LOGIN = StartAgentLogin: Unary(StartAgentLoginParams -> AgentLoginStart), Owner, renderer: true;
    COMPLETE_AGENT_LOGIN = CompleteAgentLogin: Unary(CompleteAgentLoginParams -> AgentAccountsSnapshot), Owner, renderer: true;
    POLL_AGENT_LOGIN = PollAgentLogin: Unary(LoginIdParams -> AgentLoginPoll), Owner, renderer: true;
    CANCEL_AGENT_LOGIN = CancelAgentLogin: Unary(LoginIdParams -> OkReply), Owner, renderer: true;
    UPLOAD_CHUNK = UploadChunk: Unary(UploadChunkParams -> OkReply), Owner, renderer: true;
    UPLOAD_COMMIT = UploadCommit: Unary(UploadCommitParams -> UploadCommitReply), Owner, renderer: true;
    /// Transcript images, 45 KB of base64 per chunk, path-jailed to the
    /// uploads directory and the workspace's chat checkouts.
    READ_ATTACHMENT_CHUNK = ReadAttachmentChunk: Unary(ReadAttachmentChunkParams -> AttachmentChunk), Owner, renderer: true;
    /// A tool result's full output from the daemon's local tool-output store
    /// (plan DD1 rulings T0-16, T2-13), one page at a time.
    FETCH_TOOL_BLOB = FetchToolBlob: Unary(FetchToolBlobParams -> ToolOutputChunk), Owner, renderer: true;
    /// Device-local agent CLI updates, on request only (DD1 Task 3): the
    /// complete ordered status list first and after every transition.
    WATCH_HARNESS_UPDATES = WatchHarnessUpdates: Stream(Empty -> HarnessUpdateList), Owner, renderer: true;
    CHECK_HARNESS_UPDATES = CheckHarnessUpdates: Unary(HarnessUpdateParams -> HarnessUpdateList), Owner, renderer: true;
    APPLY_HARNESS_UPDATE = ApplyHarnessUpdate: Unary(HarnessParams -> ApplyHarnessUpdateReply), Owner, renderer: true;
    CANCEL_HARNESS_UPDATE = CancelHarnessUpdate: Unary(HarnessParams -> CancelledReply), Owner, renderer: true;
    DISMISS_HARNESS_UPDATE = DismissHarnessUpdate: Unary(DismissHarnessUpdateParams -> HarnessUpdateStatus), Owner, renderer: true;
    /// The software factory's apps (SF1 amendment F1-F6; §50 §13, D795): every app
    /// with its state; the non-secret fields only.
    FACTORY_LIST = FactoryList: Unary(Empty -> FactoryAppList), Owner, renderer: true;
    /// Check an app's stored connection.
    FACTORY_TEST = FactoryTest: Unary(FactoryAppParams -> FactoryAppReply), Owner, renderer: true;
    /// One read from the factory's op allowlist (D660).
    FACTORY_QUERY = FactoryQuery: Unary(FactoryQueryParams -> FactoryQueryReply), Owner, renderer: true;
    /// Store an app's URL and credentials, then check them. Write-only for secrets,
    /// and Electron main's alone: not on the Connect surface (F4).
    FACTORY_CONFIGURE = FactoryConfigure: Unary(FactoryConfigureParams -> FactoryAppReply), Owner, renderer: false;
    /// Forget an app and its credentials (F4: main's alone).
    FACTORY_REMOVE = FactoryRemove: Unary(FactoryAppParams -> Empty), Owner, renderer: false;
    // The Software Factory loop's tools (design §72 §5.2; plan FT1, FT0 Task 4):
    // role `mcp`, each on a `chatId` in the run's own session tree. The daemon
    // resolves checkouts, remotes and credentials; none reaches the caller.
    /// A worktree on a named branch of the chat's repository (idempotent by
    /// branch), for a build thread.
    CREATE_RUN_WORKTREE = CreateRunWorktree: Unary(CreateRunWorktreeParams -> Worktree), Mcp, renderer: false;
    /// A thread's diff with its checksum.
    GET_THREAD_DIFF = GetThreadDiff: Unary(GetThreadDiffParams -> CheckoutDiff), Mcp, renderer: false;
    /// An anchored review comment on a thread's checkout (D905).
    POST_REVIEW_COMMENT = PostReviewComment: Unary(PostReviewCommentParams -> IdReply), Mcp, renderer: false;
    /// Push the thread's branch and open its pull request, or answer the open
    /// one (idempotent by branch).
    OPEN_CHANGE_REQUEST = OpenChangeRequest: Unary(OpenChangeRequestParams -> OpenChangeRequestReply), Mcp, renderer: false;
    /// A typed approval in Studio's queue; only a person settles it.
    REQUEST_APPROVAL = RequestApproval: Unary(RequestApprovalParams -> TypedApproval), Mcp, renderer: false;
    GET_APPROVAL = GetApproval: Unary(GetApprovalParams -> TypedApproval), Mcp, renderer: false;
    /// Destructive: merge a pull request, only with a settled, approved merge
    /// approval whose payload hash is this call's `{repo, pr, head_sha}`.
    MERGE_CHANGE_REQUEST = MergeChangeRequest: Unary(MergeChangeRequestParams -> MergeChangeRequestReply), Mcp, renderer: false;
    /// FT0 Task 5 (D1433): a commit's CI checks on Forgejo or GitHub.
    GET_CHECKS = GetChecks: Unary(GetChecksParams -> ChecksReply), Mcp, renderer: false;
    /// The scrubbed, bounded tail of one check's log.
    GET_CHECK_LOG = GetCheckLog: Unary(GetCheckLogParams -> CheckLogReply), Mcp, renderer: false;
    /// FT0 Task 8 (D1434), destructive: deploy a commit's Live bundle to a
    /// Live app, only with a settled, approved deploy approval whose payload
    /// hash is this call's `{target: "live", app, commit}`; health check and
    /// rollback included. The Live credential stays in agentd.
    DEPLOY_LIVE_APP = DeployLiveApp: Unary(DeployLiveAppParams -> DeployLiveAppReply), Mcp, renderer: false;
    /// FT0 Task 10 (D1432): close a killed run's pull request; the branch is
    /// kept.
    CLOSE_CHANGE_REQUEST = CloseChangeRequest: Unary(CloseChangeRequestParams -> CloseChangeRequestReply), Mcp, renderer: false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_wire_shape_matches_the_design() {
        let hello: Hello = serde_json::from_value(serde_json::json!({
            "client": "electron-main", "clientVersion": "0.2.1", "protocol": 1
        }))
        .expect("parse Hello");
        assert_eq!(hello.client, ClientKind::ElectronMain);
        let reply = HelloReply {
            protocol: AGENTD_PROTOCOL,
            version: "0.2.1".into(),
            build_sha: "abc123".into(),
            mode: DaemonMode::Child,
            pid: 7,
            started_at_ms: 1,
            role: Role::Owner,
            features: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&reply).expect("serialize"),
            serde_json::json!({
                "protocol": 1, "version": "0.2.1", "buildSha": "abc123", "mode": "child",
                "pid": 7, "startedAtMs": 1, "role": "owner", "features": []
            })
        );
    }

    #[test]
    fn the_mcp_role_reaches_exactly_the_shims_methods() {
        // Ruling T7-3: the methods the injected MCP shim uses, and no others.
        let mcp: Vec<&str> = METHODS
            .iter()
            .filter(|spec| spec.role == Role::Mcp)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(
            mcp,
            [
                "Hello",
                "ListHarnesses",
                "ListModels",
                "QueueCommand",
                "WatchDocMessages",
                "QueueMessage",
                "WatchChats",
                "WatchSessions",
                "WatchSpaces",
                "Mutate",
                "LocalDevice",
                "EngineInfo",
                // The factory loop's tools (plan FT1, FT0 Task 4).
                "CreateRunWorktree",
                "GetThreadDiff",
                "PostReviewComment",
                "OpenChangeRequest",
                "RequestApproval",
                "GetApproval",
                "MergeChangeRequest",
                "GetChecks",
                "GetCheckLog",
                "DeployLiveApp",
                "CloseChangeRequest",
            ]
        );
    }

    #[test]
    fn the_renderer_never_reaches_the_lifecycle() {
        for name in [methods::HELLO, methods::SHUTDOWN] {
            let spec = method_spec(name).expect("in the table");
            assert!(
                !spec.renderer,
                "{name} must not be forwarded from the renderer"
            );
        }
    }

    #[test]
    fn markers_name_their_rows() {
        struct Check(usize);
        impl MethodVisitor for Check {
            fn visit<M: Method>(&mut self) {
                assert_eq!(M::spec().name, M::NAME);
                assert_eq!(METHODS[self.0].name, M::NAME, "visit order is table order");
                self.0 += 1;
            }
        }
        let mut check = Check(0);
        visit_methods(&mut check);
        assert_eq!(check.0, METHODS.len());
    }
}
