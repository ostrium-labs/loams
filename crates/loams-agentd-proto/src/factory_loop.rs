//! The Software Factory loop's agentd tools (design §72 §5.2, D1431; plan FT1,
//! FT0 Task 4).
//!
//! These are the `mcp`-role methods the factory host (Loams Bot, through the
//! injected MCP shim) uses to drive a run: each names a `chatId` in the run's
//! own session tree, and the daemon resolves the checkout, the repository
//! and the code-host credential from that chat. No credential ever leaves
//! agentd.
//!
//! - `CreateRunWorktree`: a worktree on a named branch (`factory/<run>/<n>`)
//!   of the repository the chat works in, for a new build thread.
//! - `GetThreadDiff`: a thread's diff, with the checksum review comments
//!   anchor to.
//! - `PostReviewComment`: an anchored review comment on a thread's checkout.
//! - `OpenChangeRequest`: pushes the thread's branch and opens a pull request
//!   on Forgejo or GitHub, or answers the open one for that branch
//!   (idempotent by branch).
//! - `RequestApproval`, `GetApproval`: a typed approval in Studio's queue. The
//!   daemon hashes the payload; only a person settles it (`ResolveApproval`
//!   stays role `owner`).
//! - `MergeChangeRequest` (destructive): refused unless `approvalId` names a
//!   settled, approved merge approval for this chat whose payload hash is the
//!   hash of `{repo, pr, head_sha}` of this very call. An approval is used
//!   once.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ChangeRequestState;
use crate::git_write::{ChangeRequestProvider, MergeStrategy};

/// `MergeChangeRequest` without a settled, approved approval.
pub const APPROVAL_REQUIRED: &str = "approval_required";
/// An approval whose kind, chat or payload hash does not match the call, or
/// one already used.
pub const APPROVAL_MISMATCH: &str = "approval_mismatch";

/// The approval kind `MergeChangeRequest` requires.
pub const MERGE_APPROVAL_KIND: &str = "merge";

/// The longest approval summary, in characters.
pub const MAX_APPROVAL_SUMMARY: usize = 2_000;

/// `CreateRunWorktree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunWorktreeParams {
    /// The chat whose repository gets the worktree (in the run's session).
    pub chat_id: String,
    /// The new branch, for example `factory/<run>/1`.
    pub branch: String,
    /// What the branch starts from; the chat's current branch when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// `GetThreadDiff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetThreadDiffParams {
    pub chat_id: String,
    /// Diff against merge-base(`baseRef`, HEAD), committed work included;
    /// without it, the working tree's uncommitted changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}

/// `PostReviewComment`: a comment on the chat's checkout, anchored to the
/// diff whose checksum `GetThreadDiff` answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PostReviewCommentParams {
    pub chat_id: String,
    /// Relative to the checkout, `/`-separated.
    pub path: String,
    /// `new` (the default) or `old`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<crate::review::DiffSide>,
    /// 1-based.
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub body: String,
    /// The checksum of the diff the comment was written against.
    pub diff_checksum: String,
}

/// `OpenChangeRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenChangeRequestParams {
    /// The thread whose checkout's branch is the head.
    pub chat_id: String,
    pub provider: ChangeRequestProvider,
    /// The branch to merge into.
    pub base: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
}

/// `OpenChangeRequest`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenChangeRequestReply {
    pub provider: ChangeRequestProvider,
    pub number: u64,
    pub url: String,
    pub state: ChangeRequestState,
    /// `owner/repository` on the code host: the `repo` of a merge approval's
    /// payload.
    pub repo: String,
    /// The head branch.
    pub head: String,
    /// The head commit that was pushed: the `head_sha` of a merge approval.
    pub head_sha: String,
    /// The pull request was already open for this branch.
    pub existed: bool,
}

/// `RequestApproval`: ask a person to approve one typed action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestApprovalParams {
    /// The asking chat (the factory host's own thread).
    pub chat_id: String,
    /// `plan`, `merge`, `deploy`, or another lowercase word.
    pub kind: String,
    /// What the person is asked, in plain words.
    pub summary: String,
    /// What exactly is approved. The daemon stores only its hash (SHA-256 of
    /// its canonical JSON, keys sorted); for `merge` it must be
    /// `{repo, pr, head_sha}`.
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
}

/// `GetApproval`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetApprovalParams {
    pub chat_id: String,
    pub approval_id: String,
}

/// Where a typed approval is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum TypedApprovalState {
    /// Waiting for a person.
    Pending,
    Approved,
    Denied,
    /// Approved, and spent by the action it approved.
    Used,
}

impl TypedApprovalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Used => "used",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "used" => Some(Self::Used),
            _ => None,
        }
    }
}

/// A typed approval (`RequestApproval`, `GetApproval`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TypedApproval {
    pub id: String,
    pub chat_id: String,
    pub kind: String,
    pub summary: String,
    /// SHA-256 (hex) of the payload's canonical JSON.
    pub payload_hash: String,
    pub state: TypedApprovalState,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<i64>,
}

/// `MergeChangeRequest` (destructive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergeChangeRequestParams {
    /// The thread whose checkout's remote holds the pull request.
    pub chat_id: String,
    /// A settled, approved `merge` approval of this chat for exactly
    /// `{repo, pr, head_sha}`.
    pub approval_id: String,
    pub provider: ChangeRequestProvider,
    /// The pull request's number.
    pub number: u64,
    /// The head commit the person approved; the code host refuses the merge
    /// when the pull request's head has moved.
    pub head_sha: String,
    /// `merge` (the default), `squash` or `rebase`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<MergeStrategy>,
}

/// `MergeChangeRequest`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergeChangeRequestReply {
    pub provider: ChangeRequestProvider,
    pub number: u64,
    pub repo: String,
    pub merged: bool,
}

/// The default and the ceiling of `GetCheckLog`'s tail, in lines.
pub const CHECK_LOG_LINES: usize = 200;
pub const MAX_CHECK_LOG_LINES: usize = 1_000;
/// The ceiling of `GetCheckLog`'s tail, in bytes.
pub const MAX_CHECK_LOG_BYTES: usize = 64 * 1024;

/// One CI check's state, provider-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CiState {
    /// Queued or running.
    Pending,
    Success,
    Failure,
    /// Skipped, cancelled or neutral: neither passes nor fails the run.
    Neutral,
}

/// `GetChecks` (FT0 Task 5, D1433): the checks of a commit on the chat's
/// remote; the chat's `HEAD` when `sha` is omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetChecksParams {
    pub chat_id: String,
    pub provider: ChangeRequestProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

/// One check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CiCheck {
    pub name: String,
    pub state: CiState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What `GetCheckLog` takes as `logId`, when the check has a readable log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_id: Option<String>,
}

/// `GetChecks`' answer. `state` is `failure` when any check failed, else
/// `pending` while any runs, else `success` (no checks at all is `pending`:
/// CI may not have started).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChecksReply {
    pub provider: ChangeRequestProvider,
    pub repo: String,
    pub sha: String,
    pub state: CiState,
    pub checks: Vec<CiCheck>,
}

/// `GetCheckLog`: the last `lines` of one check's log, scrubbed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetCheckLogParams {
    pub chat_id: String,
    pub provider: ChangeRequestProvider,
    /// A check's `logId` from `GetChecks`.
    pub log_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CheckLogReply {
    /// The tail, secrets scrubbed.
    pub text: String,
    /// Lines in `text`.
    pub lines: u32,
    /// The log had more than the tail.
    pub truncated: bool,
}

/// The approval kind `DeployLiveApp` requires.
pub const DEPLOY_APPROVAL_KIND: &str = "deploy";

/// `DeployLiveApp` (destructive; FT0 Task 8, D1434): deploy the Live bundle a
/// repository's `.loams/factory.toml` names at `commit` to the Live app
/// `app`, check its health, and roll back to the previous deployment when
/// the check fails. Refused unless `approvalId` names a settled, approved
/// `deploy` approval of this chat whose payload was
/// `{target: "live", app, commit}`. The Live credential stays in agentd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeployLiveAppParams {
    /// The thread whose repository holds the commit (the run's own thread).
    pub chat_id: String,
    pub approval_id: String,
    /// The Live app, as the person approved it.
    pub app: String,
    /// The full commit id (40 or 64 hex digits) of the merge to deploy.
    pub commit: String,
    /// A note for the deployment's record, such as the pull request's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Where a factory deploy ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LiveDeployState {
    /// Active and healthy.
    Active,
    /// Its health check failed and the previous deployment is active again.
    RolledBack,
    /// Its health check failed and there was no previous deployment to go
    /// back to (the app's first): it stays active, unhealthy.
    Unhealthy,
    /// Live refused it (`failureReason`); the previous deployment serves.
    Failed,
    /// Still validating or backfilling (a resumed call found it in flight).
    Pending,
    /// A later deployment replaced it before this call looked again.
    Superseded,
}

/// `DeployLiveApp`'s answer: the deploy record's fields (D1167) until
/// `loams.deploy.v1` exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeployLiveAppReply {
    /// Always `live` in FT0.
    pub target: String,
    pub app: String,
    pub commit: String,
    /// The Live deployment (a ULID).
    pub deployment_id: String,
    /// The bundle's SHA-256: the artifact digest of the deploy record.
    pub bundle_sha256: String,
    /// The deployment that was active before; empty for the app's first.
    #[serde(default)]
    pub previous_deployment_id: String,
    pub state: LiveDeployState,
    /// What the health check saw, in plain words.
    #[serde(default)]
    pub health: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// The approval had been spent by an earlier call (a crash or a retry):
    /// this answer is that call's deployment, found again, not a new one.
    pub resumed: bool,
}

/// `CloseChangeRequest` (FT0 Task 10, D1432): close a run's pull request
/// when the run is killed. The branch is kept, on the remote and here, so the
/// work can be picked up again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CloseChangeRequestParams {
    /// The thread whose checkout's remote holds the pull request.
    pub chat_id: String,
    pub provider: ChangeRequestProvider,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CloseChangeRequestReply {
    pub provider: ChangeRequestProvider,
    pub number: u64,
    pub repo: String,
    /// It is closed now (it may have been closed already).
    pub closed: bool,
}
