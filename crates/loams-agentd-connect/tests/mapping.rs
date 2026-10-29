//! Every ndjson fixture (`crates/loams-agentd-proto/fixtures`) survives the
//! trip through `loams.agentd.v1`: JSON → Rust type → proto message → the
//! proto's binary encoding → proto message → Rust type → the same JSON.
//!
//! This is what Task 6's parity suite leans on: if the mapping keeps every
//! fixture, a value the engine answers reaches a Connect client unchanged.
//! A fixture of a type with no mapping must be listed in `NOT_MAPPED` with
//! its reason, so a new wire type cannot slip past.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use buffa::Message;
use loams_agentd_connect::map::Wire;
use loams_agentd_proto as rs;
use loams_agentd_proto::approvals as rs_approvals;
use loams_agentd_proto::checkpoints as rs_checkpoints;
use loams_agentd_proto::groups as rs_groups;
use loams_agentd_proto::invocation as rs_invocation;
use loams_agentd_proto::marketplace as rs_market;
use loams_agentd_proto::notify as rs_notify;
use loams_agentd_proto::transcript as rs_transcript;
use loams_agentd_proto::usage as rs_usage;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

type Roundtrip = fn(&Value) -> Value;

/// JSON → `T` → proto → bytes → proto → `T` → JSON.
fn message<T>(json: &Value) -> Value
where
    T: Wire + Serialize + DeserializeOwned,
    T::Pb: Message,
{
    let value: T = serde_json::from_value(json.clone()).unwrap();
    let bytes = value.to_pb().encode_to_vec();
    let decoded = T::Pb::decode_from_slice(&bytes).unwrap();
    serde_json::to_value(T::from_pb(decoded).unwrap()).unwrap()
}

/// A list newtype's items, each as [`message`].
fn list<T>(json: &Value) -> Value
where
    T: Wire + Serialize + DeserializeOwned,
    T::Pb: Message,
{
    Value::Array(json.as_array().unwrap().iter().map(message::<T>).collect())
}

macro_rules! table {
    ($($name:literal => $f:expr,)*) => {
        fn mapped() -> BTreeMap<&'static str, Roundtrip> {
            let mut out: BTreeMap<&'static str, Roundtrip> = BTreeMap::new();
            $(out.insert($name, $f);)*
            out
        }
    };
}

table! {
    "AddRepoParams" => |v| v.clone(),
    "ResolveApprovalParams" => message::<rs_approvals::ResolveApprovalParams>,
    "SetThreadApprovalModeParams" => message::<rs_approvals::SetThreadApprovalModeParams>,
    "WatchApprovalsParams" => message::<rs_approvals::WatchApprovalsParams>,
    "AgentAccountsSnapshot" => message::<rs::AgentAccountsSnapshot>,
    "AgentLoginPoll" => message::<rs::AgentLoginPoll>,
    "AgentLoginStart" => message::<rs::AgentLoginStart>,
    "ApprovalList" => message::<rs_approvals::ApprovalList>,
    "AttachmentChunk" => message::<rs::AttachmentChunk>,
    "BeginQueueEditOutcome" => message::<rs::BeginQueueEditOutcome>,
    "BeginQueuedMessageEditParams" => message::<rs::BeginQueuedMessageEditParams>,
    "Chat" => message::<rs::Chat>,
    "ChatList" => list::<rs::Chat>,
    "CheckoutChangeRequestStatus" => message::<rs::CheckoutChangeRequestStatus>,
    "CheckpointList" => message::<rs_checkpoints::CheckpointList>,
    "CheckoutDiff" => message::<rs::CheckoutDiff>,
    "CheckoutDiffList" => list::<rs::CheckoutDiff>,
    "CheckoutFileDiffText" => message::<rs::CheckoutFileDiffText>,
    "CommitWorktreeParams" => message::<rs::CommitWorktreeParams>,
    "CommitWorktreeReply" => message::<rs::CommitWorktreeReply>,
    "CompleteAgentLoginParams" => message::<rs::CompleteAgentLoginParams>,
    "ContentSearchMatch" => message::<rs::ContentSearchMatch>,
    "CreateChangeRequestParams" => message::<rs::CreateChangeRequestParams>,
    "CreateChangeRequestReply" => message::<rs::CreateChangeRequestReply>,
    "CreateRunGroupParams" => message::<rs_groups::CreateRunGroupParams>,
    "CreateRunGroupReply" => message::<rs_groups::CreateRunGroupReply>,
    "GetRunGroupComparisonParams" => message::<rs_groups::GetRunGroupComparisonParams>,
    "ResolveRunGroupParams" => message::<rs_groups::ResolveRunGroupParams>,
    "ResolveRunGroupReply" => message::<rs_groups::ResolveRunGroupReply>,
    "RunGroupCheckParams" => message::<rs_groups::RunGroupCheckParams>,
    "RunGroupCheckReply" => message::<rs_groups::RunGroupCheckReply>,
    "RunGroupComparison" => message::<rs_groups::RunGroupComparison>,
    "RunGroupList" => message::<rs_groups::RunGroupList>,
    "WatchRunGroupsParams" => message::<rs_groups::WatchRunGroupsParams>,
    "MarketplaceSourceList" => message::<rs_market::MarketplaceSourceList>,
    "MarketplaceSource" => message::<rs_market::MarketplaceSource>,
    "AddMarketplaceSourceParams" => message::<rs_market::AddMarketplaceSourceParams>,
    "RemoveMarketplaceSourceParams" => message::<rs_market::RemoveMarketplaceSourceParams>,
    "SearchListingsParams" => message::<rs_market::SearchListingsParams>,
    "ListingSearchResults" => message::<rs_market::ListingSearchResults>,
    "ListingRef" => message::<rs_market::ListingRef>,
    "ListingDetail" => message::<rs_market::ListingDetail>,
    "PlanInstallReply" => message::<rs_market::PlanInstallReply>,
    "InstallListingParams" => message::<rs_market::InstallListingParams>,
    "InstalledListingReply" => message::<rs_market::InstalledListingReply>,
    "UninstallListingParams" => message::<rs_market::UninstallListingParams>,
    "InstalledListingList" => message::<rs_market::InstalledListingList>,
    "AvailableUpdateList" => message::<rs_market::AvailableUpdateList>,
    "UpdateListingParams" => message::<rs_market::UpdateListingParams>,
    "CreateWorktreeOutcome" => message::<rs::CreateWorktreeOutcome>,
    "CreateWorktreeParams" => message::<rs::CreateWorktreeParams>,
    "DeleteReviewCommentParams" => message::<rs::DeleteReviewCommentParams>,
    "DeleteWorkspaceEntryRequest" => message::<rs::DeleteWorkspaceEntryRequest>,
    "DeleteWorktreeParams" => message::<rs::DeleteWorktreeParams>,
    "DiscardPathsParams" => message::<rs::DiscardPathsParams>,
    "DismissHarnessUpdateParams" => message::<rs::DismissHarnessUpdateParams>,
    "DriveListing" => |v| {
        let listing: rs::DriveListing = serde_json::from_value(v.clone()).unwrap();
        serde_json::json!({ "drives": list::<rs::DriveEntry>(&serde_json::to_value(listing.drives).unwrap()) })
    },
    "FileSearchMatchList" => list::<rs::FileSearchMatch>,
    "FinishQueueEditOutcome" => message::<rs::FinishQueueEditOutcome>,
    "FinishQueuedMessageEditParams" => message::<rs::FinishQueuedMessageEditParams>,
    "FolderListing" => message::<rs::FolderListing>,
    "ForkSideChatParams" => message::<rs::ForkSideChatParams>,
    "GetThreadUsageParams" => message::<rs_usage::GetThreadUsageParams>,
    "GitHistoryPage" => message::<rs::GitHistoryPage>,
    "HarnessCatalog" => list::<rs::HarnessDescriptor>,
    "HarnessUpdateList" => list::<rs::HarnessUpdateStatus>,
    "HarnessUpdateParams" => message::<rs::HarnessUpdateParams>,
    "HarnessUpdateStatus" => message::<rs::HarnessUpdateStatus>,
    "ListAgentAccountsParams" => message::<rs::ListAgentAccountsParams>,
    "ListCheckpointsParams" => message::<rs_checkpoints::ListCheckpointsParams>,
    "ListFoldersParams" => message::<rs::ListFoldersParams>,
    "ListModelsParams" => message::<rs::ListModelsParams>,
    "ListWorkspaceDirectoryRequest" => message::<rs::ListWorkspaceDirectoryRequest>,
    "MergeBranchParams" => message::<rs::MergeBranchParams>,
    "MergeBranchReply" => message::<rs::MergeBranchReply>,
    "MessagePart" => message::<rs::MessagePart>,
    "ModelList" => list::<rs::Model>,
    "MoveQueuedMessageParams" => message::<rs::MoveQueuedMessageParams>,
    "MoveWorkspaceEntryRequest" => message::<rs::MoveWorkspaceEntryRequest>,
    "Notification" => message::<rs_notify::Notification>,
    "PostReviewToChangeRequestParams" => message::<rs::PostReviewToChangeRequestParams>,
    "PostReviewToChangeRequestReply" => message::<rs::PostReviewToChangeRequestReply>,
    "PreviewSnapshot" => message::<rs::PreviewSnapshot>,
    "ProjectActionRun" => message::<rs::ProjectActionRun>,
    "ProjectActionSetup" => message::<rs::ProjectActionSetup>,
    "ProjectActionsSnapshot" => message::<rs::ProjectActionsSnapshot>,
    "PushBranchParams" => message::<rs::PushBranchParams>,
    "PushBranchReply" => message::<rs::PushBranchReply>,
    "QueueMessageParams" => message::<rs::QueueMessageParams>,
    "QueueSnapshot" => |v| {
        let snapshot: rs::QueueSnapshot = serde_json::from_value(v.clone()).unwrap();
        serde_json::json!({ "items": list::<rs::QueuedMessage>(&serde_json::to_value(snapshot.items).unwrap()) })
    },
    "ReadWorkspaceFileRequest" => message::<rs::ReadWorkspaceFileRequest>,
    "ReadWorkspaceImageRequest" => message::<rs::ReadWorkspaceImageRequest>,
    "RenewQueueEditOutcome" => message::<rs::RenewQueueEditOutcome>,
    "RenewQueuedMessageEditParams" => message::<rs::RenewQueuedMessageEditParams>,
    "Repo" => message::<rs::Repo>,
    "RepoList" => list::<rs::Repo>,
    "RepoRefList" => list::<rs::RepoRef>,
    "ResolveReviewCommentParams" => message::<rs::ResolveReviewCommentParams>,
    "RestoreCheckpointParams" => message::<rs_checkpoints::RestoreCheckpointParams>,
    "RestoreCheckpointReply" => message::<rs_checkpoints::RestoreCheckpointReply>,
    "ReviewComment" => message::<rs::ReviewComment>,
    "ReviewCommentsFrame" => message::<rs::ReviewCommentsFrame>,
    "SearchFilesParams" => message::<rs::SearchFilesParams>,
    "SearchWorkspaceContentParams" => message::<rs::SearchWorkspaceContentParams>,
    "SearchWorkspaceFilesRequest" => message::<rs::SearchWorkspaceFilesRequest>,
    "SendReviewToThreadParams" => message::<rs::SendReviewToThreadParams>,
    "SessionList" => list::<rs::Session>,
    "SetHarnessEnabledParams" => message::<rs::SetHarnessEnabledParams>,
    "SidebarPreferencesState" => message::<rs::SidebarPreferencesState>,
    "SkillList" => |v| match v {
        Value::Null => Value::Null,
        items => list::<rs_invocation::Skill>(items),
    },
    "SlashCommandList" => list::<rs::SlashCommand>,
    "SpaceList" => list::<rs::Space>,
    "StartAgentLoginParams" => message::<rs::StartAgentLoginParams>,
    "SwitchRefParams" => message::<rs::SwitchRefParams>,
    "TerminalEvent" => message::<rs::TerminalEvent>,
    "TerminalSession" => message::<rs::TerminalSession>,
    "ThreadUsage" => message::<rs_usage::ThreadUsage>,
    "TitleSettings" => message::<rs::TitleSettings>,
    "ToolCall" => message::<rs::ToolCall>,
    "ToolOutputChunk" => message::<rs::ToolOutputChunk>,
    "TranscriptUpdate" => message::<rs_transcript::TranscriptUpdate>,
    "UpdateQueuedMessageParams" => message::<rs::UpdateQueuedMessageParams>,
    "WatchCheckoutChangeRequestParams" => message::<rs::WatchCheckoutChangeRequestParams>,
    "WatchDocMessagesParams" => message::<rs::WatchDocMessagesParams>,
    "WatchReviewCommentsParams" => message::<rs::WatchReviewCommentsParams>,
    "WatchWorkspaceFilesRequest" => message::<rs::WatchWorkspaceFilesRequest>,
    "WorkspaceDirectoryPage" => message::<rs::WorkspaceDirectoryPage>,
    "WorkspaceFileChanges" => message::<rs::WorkspaceFileChanges>,
    "WorkspaceFileSearchMatchList" => list::<rs::WorkspaceFileSearchMatch>,
    "WorkspaceFileText" => message::<rs::WorkspaceFileText>,
    "WorkspaceGitStatusFrame" => |v| {
        let frame: rs::WorkspaceGitStatusFrame = serde_json::from_value(v.clone()).unwrap();
        let status = frame.status.map(|status| message::<rs::CheckoutGitStatus>(&serde_json::to_value(status).unwrap()));
        serde_json::json!({ "status": status })
    },
    "WorkspaceImageChunk" => message::<rs::WorkspaceImageChunk>,
    "WorkspaceMutationOutcome" => message::<rs::WorkspaceMutationOutcome>,
    "WorktreeList" => list::<rs::WorktreeInfo>,
    "WriteWorkspaceFileOutcome" => message::<rs::WriteWorkspaceFileOutcome>,
    "WriteWorkspaceFileRequest" => message::<rs::WriteWorkspaceFileRequest>,
}

/// Fixtures whose type the Connect surface maps field by field in its
/// handler (a params struct several RPCs share, or a one-field reply), or does
/// not carry at all, with the reason.
const NOT_MAPPED: &[(&str, &str)] = &[
    (
        "AgentAccountParams",
        "two string/enum fields, mapped in the handler",
    ),
    (
        "AgentEvent",
        "a harness event; the transcript carries its result, not the event",
    ),
    (
        "ApplyHarnessUpdateReply",
        "two scalars, mapped in the handler",
    ),
    ("BranchList", "repeated string"),
    ("CancelledReply", "one bool"),
    ("ChangedReply", "one bool"),
    ("ChatIdParams", "one string"),
    ("CloneRepoParams", "one string"),
    ("CreateRepoParams", "one string"),
    ("DeleteProjectActionParams", "two strings"),
    ("DiscardWorkingTreeParams", "three strings"),
    ("DiscardWorkingTreeReply", "two scalars"),
    ("Empty", "no fields"),
    (
        "EngineInfo",
        "joined with Hello's facts into Host by GetHost",
    ),
    (
        "EngineReadyReply",
        "EngineReady is not on the Connect surface",
    ),
    (
        "FactoryAppList",
        "mapped in the FactoryService handler; the parity suite compares both wires",
    ),
    ("FactoryAppParams", "one string, mapped in the handler"),
    (
        "FactoryAppReply",
        "mapped in the FactoryService handler; the parity suite compares both wires",
    ),
    (
        "FactoryConfigureParams",
        "carries secrets: ndjson only, never on the Connect surface (SF1 amendment F4)",
    ),
    (
        "FactoryQueryParams",
        "params cross as JSON text (SF1 amendment F2), mapped in the handler",
    ),
    (
        "FactoryQueryReply",
        "the value crosses as JSON text (SF1 amendment F2), mapped in the handler",
    ),
    ("FetchToolBlobParams", "three scalars"),
    ("GetCheckoutDiffParams", "five strings"),
    ("GetCheckoutFileDiffTextRequest", "eight strings"),
    ("GitAvatars", "map<string, string>"),
    (
        "HarnessCatalogParams",
        "shared by ListSkills and ListCommands",
    ),
    ("HarnessParams", "one enum, shared by five RPCs"),
    ("Hello", "the ndjson handshake"),
    (
        "HelloReply",
        "the ndjson handshake; GetHost reports its facts",
    ),
    ("IdReply", "one string"),
    (
        "ListGitHistoryParams",
        "its limit default is applied in the handler",
    ),
    ("ListProjectActionsParams", "one string"),
    ("LocalDeviceReply", "GetHost's device id"),
    ("LoginIdParams", "one string, shared by two RPCs"),
    (
        "MutateParams",
        "one Connect RPC per op, mapped in the handler",
    ),
    ("MutateReply", "ok, and ChangeSidebarPin's preferences"),
    ("OkReply", "one bool"),
    (
        "OpenTerminalParams",
        "four scalars, range-checked in the handler",
    ),
    ("QueueCommandParams", "one Connect RPC per command kind"),
    ("QueueCommandReply", "one string"),
    ("QueuedMessageRef", "two strings, shared by three RPCs"),
    ("ReadAttachmentChunkParams", "two scalars"),
    ("RemovedReply", "one bool"),
    ("RepoPathParams", "one string, shared by three RPCs"),
    ("ResizeTerminalParams", "three scalars"),
    (
        "ResolveGitAvatarsParams",
        "its limit default is applied in the handler",
    ),
    ("RunProjectActionParams", "five scalars"),
    (
        "SearchGitHistoryParams",
        "its limit default is applied in the handler",
    ),
    ("SentReply", "one bool"),
    (
        "SessionCommandPayload",
        "one Connect RPC per kind (Send, Steer, Interrupt, AnswerQuestion)",
    ),
    ("SubscribeTerminalParams", "two scalars"),
    ("SwitchRefReply", "one string"),
    ("TakeProjectActionSetupParams", "two strings"),
    ("TerminalIdParams", "one string"),
    ("UploadChunkParams", "three scalars"),
    ("UploadCommitParams", "two strings"),
    ("UploadCommitReply", "one string"),
    (
        "UpsertProjectActionParams",
        "the draft is mapped; the rest are strings",
    ),
    ("WatchPreviewsParams", "one string"),
    ("WriteTerminalParams", "two strings"),
    // The factory loop's `mcp`-role methods are ndjson only (FT0-4-1), not on
    // the Connect surface.
    (
        "CheckLogReply",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "ChecksReply",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "CreateRunWorktreeParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "GetApprovalParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "GetCheckLogParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "GetChecksParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "GetThreadDiffParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "MergeChangeRequestParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "MergeChangeRequestReply",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "OpenChangeRequestParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "OpenChangeRequestReply",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "PostReviewCommentParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "RequestApprovalParams",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "TypedApproval",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
    (
        "Worktree",
        "an `mcp`-role method's type, ndjson only (FT0-4-1)",
    ),
];

fn fixtures() -> Vec<(String, String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../loams-agentd-proto/fixtures");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".json") else {
            continue;
        };
        let ty = stem.split('.').next().unwrap().to_owned();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        out.push((file, ty, value));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        out.len() > 100,
        "the fixtures were not found in {}",
        dir.display()
    );
    out
}

#[test]
fn every_fixture_survives_the_proto() {
    let mapped = mapped();
    let mut unlisted = Vec::new();
    let mut checked = 0;
    for (file, ty, value) in fixtures() {
        if let Some(roundtrip) = mapped.get(ty.as_str()) {
            assert_eq!(
                roundtrip(&value),
                value,
                "{file} changes through loams.agentd.v1"
            );
            checked += 1;
        } else if !NOT_MAPPED.iter().any(|(name, _)| *name == ty) {
            unlisted.push(file);
        }
    }
    assert!(
        unlisted.is_empty(),
        "fixtures of types neither mapped nor listed in NOT_MAPPED: {unlisted:?}"
    );
    assert!(checked > 60, "only {checked} fixtures were checked");
}

#[test]
fn an_unset_required_enum_is_bad_params() {
    use loams_agentd_connect::proto::loams::agentd::v1 as pb;
    let request = pb::ListModelsRequest::default();
    let error = rs::ListModelsParams::from_pb(request).unwrap_err();
    assert!(error.0.contains("HarnessId"), "{error}");
}

#[test]
fn an_unset_target_names_no_checkout() {
    use loams_agentd_connect::proto::loams::agentd::v1 as pb;
    let request =
        rs::WatchWorkspaceFilesRequest::from_pb(pb::WatchFilesRequest::default()).unwrap();
    assert_eq!(request.target.chat_id, None);
    assert_eq!(request.target.space_id, None);
    assert_eq!(request.target.checkout_path, None);
}

/// The `TranscriptUpdate` fixtures as a Connect client receives them: each
/// mapped to `loams.agentd.v1.TranscriptUpdate` and encoded, in hex. The
/// web client's `reducer_accepts_connect_updates` decodes these with
/// protobuf-es, maps them back with its one mapping function and folds them
/// (plan SD1 Task 6). A proto map's entries have no fixed order on the wire, so
/// the committed file is checked by decoding it: each entry must decode to its
/// fixture, and every fixture must have one. `LOAMS_UPDATE_CONNECT_FIXTURES=1`
/// rewrites the file.
#[test]
fn connect_transcript_fixtures_are_fresh() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/packages/agentd-client/test/transcript.connect.gen.json");
    let wanted: BTreeMap<String, Value> = fixtures()
        .into_iter()
        .filter(|(_, ty, _)| ty == "TranscriptUpdate")
        .map(|(file, _, value)| (file.trim_end_matches(".json").to_owned(), value))
        .collect();
    assert!(wanted.len() >= 2, "the transcript fixtures");
    if std::env::var_os("LOAMS_UPDATE_CONNECT_FIXTURES").is_some() {
        let mut out = serde_json::Map::new();
        for (name, value) in &wanted {
            let update: rs_transcript::TranscriptUpdate =
                serde_json::from_value(value.clone()).unwrap();
            let bytes = update.to_pb().encode_to_vec();
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            out.insert(name.clone(), Value::String(hex));
        }
        let text = serde_json::to_string_pretty(&Value::Object(out)).unwrap();
        std::fs::write(&path, format!("{text}\n")).unwrap();
    }
    let stale = format!(
        "{} is stale; run `LOAMS_UPDATE_CONNECT_FIXTURES=1 cargo test -p loams-agentd-connect --test mapping`",
        path.display()
    );
    let committed: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default()).expect(&stale);
    assert_eq!(
        committed.keys().collect::<Vec<_>>(),
        wanted.keys().collect::<Vec<_>>(),
        "{stale}"
    );
    for (name, hex) in &committed {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect(&stale))
            .collect();
        let decoded =
            <rs_transcript::TranscriptUpdate as Wire>::Pb::decode_from_slice(&bytes).expect(&stale);
        let back = rs_transcript::TranscriptUpdate::from_pb(decoded).expect(&stale);
        assert_eq!(
            serde_json::to_value(back).unwrap(),
            wanted[name],
            "{name}: {stale}"
        );
    }
}
