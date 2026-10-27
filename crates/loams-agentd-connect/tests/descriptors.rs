//! `loams.agentd.v1`'s descriptors against design §53 §6.4–§6.5, the twins
//! table and the ndjson method table (plan SD1 Tasks 4 and 5).
//!
//! - `every_studio_method_has_rpc`: each existing method §53 §6.4 maps, and
//!   each gap of §6.5, is served by (or declared as) a Connect method.
//! - `connect_methods_match_descriptors`: the twins table names exactly the
//!   descriptors' methods, and a stream twin is a server stream.
//! - `allowlist_gen_is_fresh`: Electron main's generated renderer allowlist,
//!   and the web client's copy (`@loams/agentd-client`), are the table's. `LOAMS_UPDATE_ALLOWLIST=1` rewrites it.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use buffa_descriptor::DescriptorPool;
use loams_agentd_connect::DESCRIPTOR_SET;
use loams_agentd_connect::twins::{CONNECT_METHODS, PACKAGE, Twin, renderer_allowlist_ts};
use loams_agentd_proto::rpc::{MethodKind, method_spec};

/// The ndjson methods of §53 §6.4's capability map ("Existing agentd
/// method(s)"), `Mutate` ops and `QueueCommand` kinds included.
const SECTION_6_4: &[&str] = &[
    "EngineInfo",
    "LocalDevice",
    "WatchSpaces",
    "Mutate:createSpace",
    "Mutate:renameSpace",
    "Mutate:deleteSpace",
    "ListRepos",
    "AddRepo",
    "CloneRepo",
    "CreateRepo",
    "ListFolders",
    "ListDrives",
    "WatchChats",
    "WatchSessions",
    "Mutate:createChat",
    "Mutate:renameChat",
    "Mutate:setChatArchived",
    "Mutate:deleteChat",
    "Mutate:changeSidebarPin",
    "Mutate:markChatSeen",
    "Mutate:setChatConfig",
    "Mutate:setChatBranch",
    "Mutate:setChatCwd",
    "WatchSidebarPreferences",
    "ForkSideChat",
    "QueueCommand:run",
    "QueueCommand:steer",
    "QueueCommand:interrupt",
    "QueueCommand:respondInput",
    "WatchQueue",
    "QueueMessage",
    "UpdateQueuedMessage",
    "BeginQueuedMessageEdit",
    "RenewQueuedMessageEdit",
    "FinishQueuedMessageEdit",
    "MoveQueuedMessage",
    "RemoveQueuedMessage",
    "SendQueuedMessageNow",
    "SteerQueuedMessageNow",
    "RetryDelivery",
    "WatchDocMessages",
    "ListHarnesses",
    "ListModels",
    "ListSkills",
    "ListCommands",
    "SetHarnessEnabled",
    "InstallHarness",
    "CancelInstall",
    "WatchHarnessUpdates",
    "CheckHarnessUpdates",
    "ApplyHarnessUpdate",
    "CancelHarnessUpdate",
    "DismissHarnessUpdate",
    "ListAgentAccounts",
    "ActivateAgentAccount",
    "ForgetAgentAccount",
    "StartAgentLogin",
    "CompleteAgentLogin",
    "PollAgentLogin",
    "CancelAgentLogin",
    "ListBranches",
    "ListRefs",
    "ListGitHistory",
    "SearchGitHistory",
    "ResolveGitAvatars",
    "SwitchRef",
    "FetchAll",
    "CreateWorktree",
    "DeleteWorktree",
    "WatchWorkspaceGitStatus",
    "WatchCheckoutDiffs",
    "GetCheckoutDiff",
    "GetCheckoutFileDiffText",
    "DiscardWorkingTree",
    "WatchCheckoutChangeRequest",
    "ListWorkspaceDirectory",
    "ReadWorkspaceFile",
    "ReadWorkspaceImage",
    "WriteWorkspaceFile",
    "MoveWorkspaceEntry",
    "DeleteWorkspaceEntry",
    "WatchWorkspaceFiles",
    "SearchWorkspaceFiles",
    "SearchFiles",
    "OpenTerminal",
    "SubscribeTerminal",
    "WriteTerminal",
    "ResizeTerminal",
    "CloseTerminal",
    "ListProjectActions",
    "UpsertProjectAction",
    "DeleteProjectAction",
    "RunProjectAction",
    "TakeProjectActionSetup",
    "WatchPreviews",
    "UploadChunk",
    "UploadCommit",
    "ReadAttachmentChunk",
    "FetchToolBlob",
    "GetTitleSettings",
    "SetTitleSettings",
];

/// §53 §6.5's gaps and the Connect method that declares each.
const SECTION_6_5: &[(&str, &str)] = &[
    ("G1", "GitService/ListWorktrees"),
    ("G2", "GitService/CommitWorktree"),
    ("G3", "GitService/PushBranch"),
    ("G4", "GitService/CreateChangeRequest"),
    ("G5", "GitService/MergeBranch"),
    ("G6", "ThreadService/ListCheckpoints"),
    ("G7", "ThreadService/RestoreCheckpoint"),
    ("G8", "ReviewService/WatchReviewComments"),
    ("G9", "ReviewService/UpsertReviewComment"),
    ("G9", "ReviewService/DeleteReviewComment"),
    ("G9", "ReviewService/ResolveReviewComment"),
    ("G10", "ReviewService/SendReviewToThread"),
    ("G11", "ReviewService/PostReviewToChangeRequest"),
    ("G12", "RunGroupService/CreateRunGroup"),
    ("G13", "RunGroupService/WatchRunGroups"),
    ("G14", "RunGroupService/GetRunGroupComparison"),
    ("G15", "RunGroupService/RunGroupCheck"),
    ("G16", "RunGroupService/ResolveRunGroup"),
    ("G17", "ApprovalService/WatchApprovals"),
    ("G18", "ApprovalService/ResolveApproval"),
    ("G19", "NotificationService/WatchNotifications"),
    ("G20", "ThreadService/GetThreadUsage"),
    ("G21", "DiffService/DiscardPaths"),
    ("G22", "ApprovalService/SetThreadApprovalMode"),
    ("G23", "FileService/SearchContent"),
];

/// SD1 Shared contracts: the services of `loams.agentd.v1`.
const SERVICES: &[&str] = &[
    "StudioService",
    "ProjectService",
    "ThreadService",
    "QueueService",
    "HarnessService",
    "GitService",
    "DiffService",
    "FileService",
    "TerminalService",
    "PreviewService",
    "ReviewService",
    "RunGroupService",
    "ApprovalService",
    "NotificationService",
    "AttachmentService",
    "FactoryService",
    // MK1 Task 4b.
    "MarketplaceService",
];

/// `(Service/Method, server streaming)` for every method in the descriptors.
fn descriptor_methods() -> BTreeSet<(String, bool)> {
    let pool = DescriptorPool::decode(DESCRIPTOR_SET).unwrap();
    let mut out = BTreeSet::new();
    for service in pool.services() {
        let Some(name) = service.full_name().strip_prefix(&format!("{PACKAGE}.")) else {
            continue;
        };
        for method in service.methods() {
            out.insert((
                format!("{name}/{}", method.name()),
                method.is_server_streaming(),
            ));
        }
    }
    out
}

fn served_by(row: Twin) -> Option<String> {
    match row {
        Twin::Method(name) => Some(name.to_owned()),
        Twin::Mutate(op) => Some(format!("Mutate:{op}")),
        Twin::Command(kind) => Some(format!("QueueCommand:{kind}")),
        Twin::Host => Some("EngineInfo".to_owned()),
        Twin::Gap { .. } => None,
    }
}

#[test]
fn every_studio_method_has_rpc() {
    let descriptors: BTreeSet<String> = descriptor_methods().into_iter().map(|(m, _)| m).collect();
    let services: BTreeSet<&str> = descriptors
        .iter()
        .map(|m| m.split('/').next().unwrap())
        .collect();
    assert_eq!(
        services,
        SERVICES.iter().copied().collect(),
        "the Shared contracts' services"
    );

    let served: BTreeSet<String> = CONNECT_METHODS
        .iter()
        .filter_map(|row| served_by(row.twin))
        .collect();
    let mut missing: Vec<&str> = SECTION_6_4
        .iter()
        .copied()
        .filter(|name| !served.contains(*name))
        .collect();
    // `LocalDevice` is the legacy half of `GetHost`, whose `EngineInfo`
    // carries the same device id; no client needs both.
    missing.retain(|name| *name != "LocalDevice");
    assert!(
        missing.is_empty(),
        "§53 §6.4 methods with no Connect RPC: {missing:?}"
    );

    for (gap, method) in SECTION_6_5 {
        assert!(
            descriptors.contains(*method),
            "{gap}: {method} is not declared"
        );
        let row = CONNECT_METHODS
            .iter()
            .find(|row| format!("{}/{}", row.service, row.method) == *method)
            .unwrap();
        // A gap is marked as one until its task serves it with the ndjson
        // method of the same name, or of its §53 §6.5 name where the Connect
        // RPC is named after its service (G23: `FileService.SearchContent`
        // is `SearchWorkspaceContent`).
        let served = SERVED_GAPS.contains(gap);
        let rpc = match *gap {
            "G23" => "SearchWorkspaceContent",
            _ => method.split('/').nth(1).unwrap(),
        };
        assert!(
            if served {
                row.twin == Twin::Method(rpc)
            } else {
                matches!(row.twin, Twin::Gap { gap: declared, .. } if declared == *gap)
            },
            "{method}: gap {gap} is {}",
            if served {
                "served, but not by its ndjson twin"
            } else {
                "not marked as a gap"
            }
        );
    }
}

/// The §53 §6.5 gaps a task has served: G1–G5 and G21 (SD1 Task 9), G6 and
/// G7 (SD1 Task 10), G8–G11 and G23 (SD1 Task 11), G12–G16 (SD1 Task 12), G19 and
/// G20 (SD1 Task 14), G17, G18 and G22 (SD1 Task 13): every one.
const SERVED_GAPS: &[&str] = &[
    "G1", "G2", "G3", "G4", "G5", "G21", // SD1 Task 9
    "G6", "G7", // SD1 Task 10
    "G8", "G9", "G10", "G11", "G23", // SD1 Task 11
    "G12", "G13", "G14", "G15", "G16", // SD1 Task 12
    "G19", "G20", // SD1 Task 14
    "G17", "G18", "G22", // SD1 Task 13
];

#[test]
fn connect_methods_match_descriptors() {
    let descriptors = descriptor_methods();
    let table: BTreeSet<String> = CONNECT_METHODS
        .iter()
        .map(|row| format!("{}/{}", row.service, row.method))
        .collect();
    let described: BTreeSet<String> = descriptors.iter().map(|(m, _)| m.clone()).collect();
    assert_eq!(
        table, described,
        "the twins table and the descriptors differ"
    );
    for row in CONNECT_METHODS {
        let path = format!("{}/{}", row.service, row.method);
        let streams = descriptors.contains(&(path.clone(), true));
        if let Twin::Method(name) = row.twin {
            let spec = method_spec(name).unwrap();
            assert_eq!(
                streams,
                spec.kind == MethodKind::Stream,
                "{path} and its twin {name} disagree on streaming"
            );
        }
    }
}

#[test]
fn the_renderer_reaches_no_gap_and_no_lifecycle() {
    let allowlist = loams_agentd_connect::twins::renderer_allowlist();
    for row in CONNECT_METHODS {
        if matches!(row.twin, Twin::Gap { .. }) {
            assert!(!allowlist.contains(&row.path()), "{} is a gap", row.path());
        }
    }
    assert!(
        allowlist
            .iter()
            .all(|path| path.starts_with("/loams.agentd.v1."))
    );
    assert!(allowlist.contains(&"/loams.agentd.v1.ThreadService/WatchTranscript".to_owned()));
}

/// Electron main's copy (the proxy's allowlist) and the web client's (the
/// `agentd:git-write` check of `rpc.agentd`, SD1 Task 6): the same file.
fn allowlist_paths() -> [PathBuf; 2] {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    [
        root.join("apps/desktop-electron/src/main/agentd/allowlist.gen.ts"),
        root.join("web/packages/agentd-client/src/methods.gen.ts"),
    ]
}

#[test]
fn allowlist_gen_is_fresh() {
    let generated = renderer_allowlist_ts();
    for path in allowlist_paths() {
        if std::env::var_os("LOAMS_UPDATE_ALLOWLIST").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == generated,
            "{} is stale; run `LOAMS_UPDATE_ALLOWLIST=1 cargo test -p loams-agentd-connect --test descriptors`",
            path.display()
        );
    }
}
