//! Each `loams.agentd.v1` method and its ndjson twin (design §53 §6.4, §6.5).
//!
//! The table is the join the plan names: Task 4's descriptors with the
//! ndjson method table (`METHODS`). It gives
//! - the renderer allowlist Electron main's protocol proxy enforces (a Connect
//!   method may be called from the renderer exactly when every ndjson method
//!   it runs is `renderer: true`; a gap is never on it until it is served),
//!   written to `apps/desktop-electron/src/main/agentd/allowlist.gen.ts`;
//! - Task 6's parity suite, one case per row with a twin.
//!
//! `tests/descriptors.rs` checks the table against the descriptors both
//! ways, and the allowlist file against the table.

use loams_agentd_proto::rpc::{MethodSpec, method_spec};

/// What a Connect method runs on the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Twin {
    /// The ndjson method of this name, with the same params and result.
    Method(&'static str),
    /// `Mutate` with this `op`.
    Mutate(&'static str),
    /// `QueueCommand` with this command `kind`.
    Command(&'static str),
    /// `EngineInfo`, joined with what `Hello` reports (`StudioService.GetHost`).
    Host,
    /// A §53 §6.5 gap, `unimplemented` until the named SD1 task lands.
    Gap {
        /// The gap's number, for example "G1".
        gap: &'static str,
        /// The task that implements it, for example "SD1 Task 9".
        task: &'static str,
    },
}

impl Twin {
    /// The ndjson methods the call runs, in `METHODS`.
    pub fn ndjson(self) -> Vec<&'static MethodSpec> {
        let names: &[&str] = match self {
            Twin::Method(name) => return method_spec(name).into_iter().collect(),
            Twin::Mutate(_) => &["Mutate"],
            Twin::Command(_) => &["QueueCommand"],
            Twin::Host => &["EngineInfo"],
            Twin::Gap { .. } => &[],
        };
        names.iter().filter_map(|name| method_spec(name)).collect()
    }

    /// Whether the renderer may call it through the desktop's proxy.
    pub fn renderer(self) -> bool {
        let specs = self.ndjson();
        !specs.is_empty() && specs.iter().all(|spec| spec.renderer)
    }
}

/// One Connect method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectMethod {
    /// The service, without the package (`ThreadService`).
    pub service: &'static str,
    /// The method (`WatchThreads`).
    pub method: &'static str,
    pub twin: Twin,
}

impl ConnectMethod {
    /// The Connect path, `/loams.agentd.v1.ThreadService/WatchThreads`.
    pub fn path(&self) -> String {
        format!("/{PACKAGE}.{}/{}", self.service, self.method)
    }
}

/// The proto package.
pub const PACKAGE: &str = "loams.agentd.v1";

macro_rules! methods {
    ($($service:ident { $($method:ident => $twin:expr),* $(,)? })*) => {
        /// Every `loams.agentd.v1` method, in proto order.
        pub const CONNECT_METHODS: &[ConnectMethod] = &[
            $($(ConnectMethod {
                service: stringify!($service),
                method: stringify!($method),
                twin: $twin,
            },)*)*
        ];
    };
}

use Twin::{Command, Host, Method, Mutate};

methods! {
    StudioService {
        GetHost => Host,
        GetTitleSettings => Method("GetTitleSettings"),
        SetTitleSettings => Method("SetTitleSettings"),
    }
    TerminalService {
        OpenTerminal => Method("OpenTerminal"),
        SubscribeTerminal => Method("SubscribeTerminal"),
        WriteTerminal => Method("WriteTerminal"),
        ResizeTerminal => Method("ResizeTerminal"),
        CloseTerminal => Method("CloseTerminal"),
    }
    ProjectService {
        WatchProjects => Method("WatchSpaces"),
        CreateProject => Mutate("createSpace"),
        RenameProject => Mutate("renameSpace"),
        DeleteProject => Mutate("deleteSpace"),
        ListRepos => Method("ListRepos"),
        AddRepo => Method("AddRepo"),
        CloneRepo => Method("CloneRepo"),
        CreateRepo => Method("CreateRepo"),
        ListProjectActions => Method("ListProjectActions"),
        UpsertProjectAction => Method("UpsertProjectAction"),
        DeleteProjectAction => Method("DeleteProjectAction"),
        RunProjectAction => Method("RunProjectAction"),
        TakeProjectActionSetup => Method("TakeProjectActionSetup"),
    }
    ThreadService {
        WatchThreads => Method("WatchChats"),
        WatchSessions => Method("WatchSessions"),
        WatchSidebarPreferences => Method("WatchSidebarPreferences"),
        CreateThread => Mutate("createChat"),
        RenameThread => Mutate("renameChat"),
        SetThreadArchived => Mutate("setChatArchived"),
        DeleteThread => Mutate("deleteChat"),
        ChangeSidebarPin => Mutate("changeSidebarPin"),
        MarkThreadSeen => Mutate("markChatSeen"),
        SetThreadConfig => Mutate("setChatConfig"),
        SetThreadBranch => Mutate("setChatBranch"),
        SetThreadCwd => Mutate("setChatCwd"),
        ForkThread => Method("ForkSideChat"),
        Send => Command("run"),
        Steer => Command("steer"),
        Interrupt => Command("interrupt"),
        AnswerQuestion => Command("respondInput"),
        WatchTranscript => Method("WatchDocMessages"),
        ListCheckpoints => Method("ListCheckpoints"),
        RestoreCheckpoint => Method("RestoreCheckpoint"),
        GetThreadUsage => Method("GetThreadUsage"),
    }
    QueueService {
        WatchQueue => Method("WatchQueue"),
        QueueMessage => Method("QueueMessage"),
        UpdateQueuedMessage => Method("UpdateQueuedMessage"),
        BeginQueuedMessageEdit => Method("BeginQueuedMessageEdit"),
        RenewQueuedMessageEdit => Method("RenewQueuedMessageEdit"),
        FinishQueuedMessageEdit => Method("FinishQueuedMessageEdit"),
        MoveQueuedMessage => Method("MoveQueuedMessage"),
        RemoveQueuedMessage => Method("RemoveQueuedMessage"),
        SendQueuedMessageNow => Method("SendQueuedMessageNow"),
        SteerQueuedMessageNow => Method("SteerQueuedMessageNow"),
        RetryDelivery => Method("RetryDelivery"),
    }
    HarnessService {
        ListHarnesses => Method("ListHarnesses"),
        ListModels => Method("ListModels"),
        ListSkills => Method("ListSkills"),
        ListCommands => Method("ListCommands"),
        SetHarnessEnabled => Method("SetHarnessEnabled"),
        InstallHarness => Method("InstallHarness"),
        CancelInstall => Method("CancelInstall"),
        WatchHarnessUpdates => Method("WatchHarnessUpdates"),
        CheckHarnessUpdates => Method("CheckHarnessUpdates"),
        ApplyHarnessUpdate => Method("ApplyHarnessUpdate"),
        CancelHarnessUpdate => Method("CancelHarnessUpdate"),
        DismissHarnessUpdate => Method("DismissHarnessUpdate"),
        ListAgentAccounts => Method("ListAgentAccounts"),
        ActivateAgentAccount => Method("ActivateAgentAccount"),
        ForgetAgentAccount => Method("ForgetAgentAccount"),
        StartAgentLogin => Method("StartAgentLogin"),
        CompleteAgentLogin => Method("CompleteAgentLogin"),
        PollAgentLogin => Method("PollAgentLogin"),
        CancelAgentLogin => Method("CancelAgentLogin"),
    }
    GitService {
        ListBranches => Method("ListBranches"),
        ListRefs => Method("ListRefs"),
        ListGitHistory => Method("ListGitHistory"),
        SearchGitHistory => Method("SearchGitHistory"),
        ResolveGitAvatars => Method("ResolveGitAvatars"),
        SwitchRef => Method("SwitchRef"),
        FetchAll => Method("FetchAll"),
        CreateWorktree => Method("CreateWorktree"),
        DeleteWorktree => Method("DeleteWorktree"),
        WatchStatus => Method("WatchWorkspaceGitStatus"),
        WatchChangeRequest => Method("WatchCheckoutChangeRequest"),
        ListWorktrees => Method("ListWorktrees"),
        CommitWorktree => Method("CommitWorktree"),
        PushBranch => Method("PushBranch"),
        CreateChangeRequest => Method("CreateChangeRequest"),
        MergeBranch => Method("MergeBranch"),
    }
    DiffService {
        WatchCheckoutDiffs => Method("WatchCheckoutDiffs"),
        GetCheckoutDiff => Method("GetCheckoutDiff"),
        GetCheckoutFileDiffText => Method("GetCheckoutFileDiffText"),
        Discard => Method("DiscardWorkingTree"),
        DiscardPaths => Method("DiscardPaths"),
    }
    FileService {
        ListFolders => Method("ListFolders"),
        ListDrives => Method("ListDrives"),
        SearchMentions => Method("SearchFiles"),
        Search => Method("SearchWorkspaceFiles"),
        ListDirectory => Method("ListWorkspaceDirectory"),
        ReadFile => Method("ReadWorkspaceFile"),
        ReadImage => Method("ReadWorkspaceImage"),
        WriteFile => Method("WriteWorkspaceFile"),
        MoveEntry => Method("MoveWorkspaceEntry"),
        DeleteEntry => Method("DeleteWorkspaceEntry"),
        WatchFiles => Method("WatchWorkspaceFiles"),
        SearchContent => Method("SearchWorkspaceContent"),
    }
    PreviewService {
        WatchPreviews => Method("WatchPreviews"),
    }
    AttachmentService {
        UploadChunk => Method("UploadChunk"),
        UploadCommit => Method("UploadCommit"),
        ReadAttachmentChunk => Method("ReadAttachmentChunk"),
        FetchToolBlob => Method("FetchToolBlob"),
    }
    ReviewService {
        WatchReviewComments => Method("WatchReviewComments"),
        UpsertReviewComment => Method("UpsertReviewComment"),
        DeleteReviewComment => Method("DeleteReviewComment"),
        ResolveReviewComment => Method("ResolveReviewComment"),
        SendReviewToThread => Method("SendReviewToThread"),
        PostReviewToChangeRequest => Method("PostReviewToChangeRequest"),
    }
    ApprovalService {
        WatchApprovals => Method("WatchApprovals"),
        ResolveApproval => Method("ResolveApproval"),
        SetThreadApprovalMode => Method("SetThreadApprovalMode"),
    }
    RunGroupService {
        CreateRunGroup => Method("CreateRunGroup"),
        WatchRunGroups => Method("WatchRunGroups"),
        GetRunGroupComparison => Method("GetRunGroupComparison"),
        RunGroupCheck => Method("RunGroupCheck"),
        ResolveRunGroup => Method("ResolveRunGroup"),
    }
    NotificationService {
        WatchNotifications => Method("WatchNotifications"),
    }
    FactoryService {
        ListFactoryApps => Method("FactoryList"),
        TestFactoryApp => Method("FactoryTest"),
        QueryFactory => Method("FactoryQuery"),
        ConfigureFactoryApp => Method("FactoryConfigure"),
        RemoveFactoryApp => Method("FactoryRemove"),
    }
    MarketplaceService {
        ListSources => Method("ListMarketplaceSources"),
        AddSource => Method("AddMarketplaceSource"),
        RemoveSource => Method("RemoveMarketplaceSource"),
        SearchListings => Method("SearchListings"),
        GetListing => Method("GetListing"),
        PlanInstall => Method("PlanInstall"),
        Install => Method("InstallListing"),
        Uninstall => Method("UninstallListing"),
        ListInstalled => Method("ListInstalledListings"),
        CheckUpdates => Method("CheckListingUpdates"),
        Update => Method("UpdateListing"),
    }
}

/// The methods that write to git: a plugin calls them only with the
/// `agentd:git-write` permission (§53 §8.3), and the UI asks the person to
/// confirm each one (§8.1). `(service, method)`.
pub const GIT_WRITE_METHODS: &[(&str, &str)] = &[
    ("GitService", "CommitWorktree"),
    ("GitService", "PushBranch"),
    ("GitService", "CreateChangeRequest"),
    ("GitService", "MergeBranch"),
    ("GitService", "DeleteWorktree"),
    ("DiffService", "Discard"),
    ("DiffService", "DiscardPaths"),
    // A review posted to the pull request writes to the forge, as
    // CreateChangeRequest does (SD1 Task 11).
    ("ReviewService", "PostReviewToChangeRequest"),
    // SD1 Task 12: a resolve commits, merges or pushes the winner, and may
    // remove the other members' worktrees.
    ("RunGroupService", "ResolveRunGroup"),
];

/// The Connect paths of [`GIT_WRITE_METHODS`], sorted.
pub fn git_write_paths() -> Vec<String> {
    let mut paths: Vec<String> = GIT_WRITE_METHODS
        .iter()
        .map(|(service, method)| format!("/{PACKAGE}.{service}/{method}"))
        .collect();
    paths.sort();
    paths
}

/// The row of a Connect method.
pub fn twin_of(service: &str, method: &str) -> Option<&'static ConnectMethod> {
    CONNECT_METHODS
        .iter()
        .find(|row| row.service == service && row.method == method)
}

/// The Connect paths the renderer may call, sorted.
pub fn renderer_allowlist() -> Vec<String> {
    let mut paths: Vec<String> = CONNECT_METHODS
        .iter()
        .filter(|row| row.twin.renderer())
        .map(ConnectMethod::path)
        .collect();
    paths.sort();
    paths
}

/// `apps/desktop-electron/src/main/agentd/allowlist.gen.ts`.
pub fn renderer_allowlist_ts() -> String {
    let mut out = String::from(
        "// @generated by loams-agentd-connect (`twins::renderer_allowlist_ts`). Do not edit:\n\
         // `LOAMS_UPDATE_ALLOWLIST=1 cargo test -p loams-agentd-connect --test descriptors`\n\
         // rewrites it, and `allowlist_gen_is_fresh` fails when it is stale.\n\
         //\n\
         // The loams.agentd.v1 methods the renderer may call through the protocol\n\
         // proxy (SD1 Task 5): those whose ndjson twin is `renderer: true`. A §53 §6.5\n\
         // gap joins the list when its task serves it.\n\
         export const AGENTD_RENDERER_METHODS: ReadonlySet<string> = new Set([\n",
    );
    for path in renderer_allowlist() {
        out.push_str(&format!("\t\"{path}\",\n"));
    }
    out.push_str("]);\n");
    out.push_str(
        "\n// The methods that write to git (SD1 Task 9): a plugin calls them only with\n\
         // the `agentd:git-write` permission (§53 §8.3), after the person confirms.\n\
         export const AGENTD_GIT_WRITE_METHODS: ReadonlySet<string> = new Set([\n",
    );
    for path in git_write_paths() {
        out.push_str(&format!("\t\"{path}\",\n"));
    }
    out.push_str("]);\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_twin_names_an_ndjson_method() {
        for row in CONNECT_METHODS {
            if let Twin::Method(name) = row.twin {
                assert!(
                    method_spec(name).is_some(),
                    "{}: no ndjson method {name}",
                    row.path()
                );
            }
        }
    }

    #[test]
    fn the_lifecycle_is_never_on_the_connect_surface() {
        for row in CONNECT_METHODS {
            assert!(
                !matches!(row.twin, Twin::Method("Hello" | "Shutdown")),
                "{} runs a lifecycle method",
                row.path()
            );
        }
    }

    #[test]
    fn git_writes_are_served_owner_methods() {
        for (service, method) in GIT_WRITE_METHODS {
            let row = twin_of(service, method).expect("a Connect method");
            let Twin::Method(name) = row.twin else {
                panic!("{} is not served", row.path());
            };
            let spec = method_spec(name).expect("in METHODS");
            assert_eq!(
                spec.role,
                loams_agentd_proto::rpc::Role::Owner,
                "{name} is never open to mcp"
            );
        }
    }

    #[test]
    fn rows_are_unique() {
        let mut paths: Vec<String> = CONNECT_METHODS.iter().map(ConnectMethod::path).collect();
        paths.sort();
        let count = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), count);
    }
}
