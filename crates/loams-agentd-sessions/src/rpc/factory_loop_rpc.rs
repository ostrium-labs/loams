//! `EngineRpc`'s Software Factory loop methods (design §72 §5.2; plan FT1,
//! FT0 Task 4; the wire types are `loams_agentd_proto::factory_loop`).
//!
//! Each names a chat; the daemon's policy has already checked that the chat
//! is in the calling run's session tree. Everything else comes from the
//! chat: its checkout, its repository, the remote and the code-host
//! credential. The existing methods run through their own dispatch arms
//! (`GetCheckoutDiff`, `PushBranch`, `CreateChangeRequest`), so their guards
//! apply unchanged.

use std::path::{Path, PathBuf};

use loams_agentd_proto::factory_loop::{
    CheckLogReply, ChecksReply, CloseChangeRequestParams, CloseChangeRequestReply,
    CreateRunWorktreeParams, DEPLOY_APPROVAL_KIND, DeployLiveAppParams, DeployLiveAppReply,
    GetApprovalParams, GetCheckLogParams, GetChecksParams, GetThreadDiffParams,
    MERGE_APPROVAL_KIND, MergeChangeRequestParams, MergeChangeRequestReply,
    OpenChangeRequestParams, OpenChangeRequestReply, PostReviewCommentParams,
    RequestApprovalParams, TypedApproval,
};
use loams_agentd_proto::git_write::{CreateChangeRequestParams, MergeStrategy, PushBranchParams};
use loams_agentd_proto::review::{DiffSide, ReviewComment};
use loams_agentd_proto::rpc::{Method, calls};
use loams_agentd_proto::{CheckoutDiff, GetCheckoutDiffParams, Worktree};
use loams_agentd_rpc::{RpcError, RpcReply, RpcService};

use super::{EngineRpc, git_write_error};
use crate::approvals::{TypedApprovalError, deploy_payload, merge_payload};
use crate::factory_loop::{create_branch_worktree, repo_slug};

fn failed(error: impl std::fmt::Display) -> RpcError {
    RpcError::Failed(error.to_string())
}

pub(super) fn typed_error(error: TypedApprovalError) -> RpcError {
    match error {
        TypedApprovalError::BadParams(text) => RpcError::BadParams(text),
        other => RpcError::Failed(other.to_string()),
    }
}

/// A chat's working tree on this device.
pub(super) struct ChatCheckout {
    pub identity: crate::repos::CheckoutIdentity,
}

impl ChatCheckout {
    fn root(&self) -> &Path {
        self.identity.root.as_path()
    }
}

impl EngineRpc {
    /// Another method through its own dispatch arm, guards and all.
    async fn factory_arm<M: Method>(&self, params: &M::Params) -> Result<M::Result, RpcError> {
        let value = serde_json::to_value(params).map_err(failed)?;
        match self.handle(M::NAME, value).await? {
            RpcReply::Value(value) => serde_json::from_value(value).map_err(failed),
            RpcReply::Stream(_) => Err(RpcError::Failed(format!("{} streams", M::NAME))),
        }
    }

    /// The checkout `chat_id` works in: its own `cwd`, else its project's
    /// folder. The chat must be hosted here.
    pub(super) async fn chat_checkout(&self, chat_id: &str) -> Result<ChatCheckout, RpcError> {
        let not_found = || RpcError::Failed(format!("not_found: chat {chat_id}"));
        let chat = self
            .workspace
            .chat(chat_id)
            .map_err(failed)?
            .ok_or_else(not_found)?;
        if chat.device_id != self.doc_host.device_id() {
            return Err(RpcError::BadParams(
                "the chat is hosted by another device".into(),
            ));
        }
        let cwd = match chat.cwd.clone() {
            Some(cwd) => cwd,
            None => {
                let space = chat
                    .space_id
                    .as_deref()
                    .and_then(|space| self.workspace.space(space).ok().flatten())
                    .ok_or_else(|| RpcError::BadParams("the chat has no working tree".into()))?;
                space.path
            }
        };
        let identity = self
            .repos
            .checkout_identity(Path::new(&cwd))
            .await
            .map_err(|e| {
                RpcError::BadParams(format!("the chat's folder is not a git checkout: {e}"))
            })?;
        Ok(ChatCheckout { identity })
    }

    /// `CreateRunWorktree`.
    pub(super) async fn create_run_worktree(
        &self,
        p: CreateRunWorktreeParams,
    ) -> Result<Worktree, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        // The repository's main checkout: worktrees hang off the repository,
        // not off another worktree.
        let common = crate::git_write::git_ok(
            checkout.root(),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        .map_err(git_write_error)?;
        let common = PathBuf::from(common);
        let repo = if common.file_name().is_some_and(|name| name == ".git") {
            common.parent().map(Path::to_path_buf).unwrap_or(common)
        } else {
            checkout.root().to_path_buf()
        };
        let base = match p.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(base) => base.to_owned(),
            None => self
                .repos
                .current_branch(checkout.root())
                .await
                .map_err(failed)?,
        };
        let base = if base.is_empty() {
            "HEAD".to_owned()
        } else {
            base
        };
        let _write = self.git_writes.lock().await;
        create_branch_worktree(&self.repos, &repo, p.branch.trim(), &base)
            .await
            .map_err(git_write_error)
    }

    /// `GetThreadDiff`: `GetCheckoutDiff` of the chat's checkout.
    pub(super) async fn thread_diff(
        &self,
        p: GetThreadDiffParams,
    ) -> Result<CheckoutDiff, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let base_ref = p
            .base_ref
            .map(|b| b.trim().to_owned())
            .filter(|b| !b.is_empty());
        let params = GetCheckoutDiffParams {
            cwd: checkout.root().to_string_lossy().into_owned(),
            mode: if base_ref.is_some() {
                "branch"
            } else {
                "working"
            }
            .to_owned(),
            base_ref,
            commit_sha: None,
            chat_id: None,
        };
        self.factory_arm::<calls::GetCheckoutDiff>(&params).await
    }

    /// `PostReviewComment`: a comment on the chat's checkout.
    pub(super) async fn post_review_comment(
        &self,
        p: PostReviewCommentParams,
    ) -> Result<String, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let comment = ReviewComment {
            id: String::new(),
            checkout_id: checkout.identity.id.clone(),
            path: p.path,
            side: p.side.unwrap_or(DiffSide::New),
            line: p.line,
            end_line: p.end_line,
            body: p.body,
            diff_checksum: p.diff_checksum,
            created_at_ms: 0,
            resolved: false,
            outdated: false,
        };
        let id: loams_agentd_proto::IdReply = self
            .factory_arm::<calls::UpsertReviewComment>(&comment)
            .await?;
        Ok(id.id)
    }

    /// `OpenChangeRequest`: push the chat's branch, then answer its open pull
    /// request or open one.
    pub(super) async fn open_change_request(
        &self,
        p: OpenChangeRequestParams,
    ) -> Result<OpenChangeRequestReply, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let root = checkout.root().to_path_buf();
        let head = self.repos.current_branch(&root).await.map_err(failed)?;
        if head.is_empty() {
            return Err(RpcError::BadParams(
                "the chat's checkout is not on a branch".into(),
            ));
        }
        let head_sha = crate::git_write::git_ok(&root, &["rev-parse", "HEAD"])
            .await
            .map_err(git_write_error)?;
        let cwd = root.to_string_lossy().into_owned();
        self.factory_arm::<calls::PushBranch>(&PushBranchParams {
            cwd: cwd.clone(),
            remote: None,
            branch: head.clone(),
            set_upstream: true,
            force_with_lease: false,
        })
        .await?;
        let remote = self
            .change_request_creator
            .remote_for(&root, &head)
            .await
            .map_err(git_write_error)?;
        let existing = self
            .change_request_creator
            .find_open(&root, p.provider, &head)
            .await
            .map_err(git_write_error)?;
        let (opened, existed) = match existing {
            Some(open) => (open, true),
            None => (
                self.factory_arm::<calls::CreateChangeRequest>(&CreateChangeRequestParams {
                    cwd,
                    provider: p.provider,
                    base: p.base,
                    head: head.clone(),
                    title: p.title,
                    body: p.body,
                    draft: p.draft,
                    link_issue: None,
                })
                .await?,
                false,
            ),
        };
        Ok(OpenChangeRequestReply {
            provider: opened.provider,
            number: opened.number,
            url: opened.url,
            state: opened.state,
            repo: repo_slug(&remote),
            head,
            head_sha,
            existed,
        })
    }

    /// `RequestApproval`.
    pub(super) fn request_typed_approval(
        &self,
        p: RequestApprovalParams,
    ) -> Result<TypedApproval, RpcError> {
        self.approvals()?
            .request_typed(&p.chat_id, p.kind.trim(), &p.summary, &p.payload)
            .map_err(typed_error)
    }

    /// `GetApproval`.
    pub(super) fn get_typed_approval(
        &self,
        p: GetApprovalParams,
    ) -> Result<TypedApproval, RpcError> {
        self.approvals()?
            .typed(&p.chat_id, &p.approval_id)
            .map_err(typed_error)
    }

    /// `MergeChangeRequest`: the approval is checked and spent first, on the
    /// payload this call computes from its own arguments and the chat's
    /// remote; then the code host merges, refusing a head that moved.
    pub(super) async fn merge_change_request(
        &self,
        p: MergeChangeRequestParams,
    ) -> Result<MergeChangeRequestReply, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let root = checkout.root().to_path_buf();
        let head = self.repos.current_branch(&root).await.map_err(failed)?;
        let remote = self
            .change_request_creator
            .remote_for(&root, if head.is_empty() { "HEAD" } else { &head })
            .await
            .map_err(git_write_error)?;
        let repo = repo_slug(&remote);
        let payload = merge_payload(&repo, p.number, p.head_sha.trim());
        self.approvals()?
            .use_typed(&p.chat_id, &p.approval_id, MERGE_APPROVAL_KIND, &payload)
            .map_err(typed_error)?;
        self.change_request_creator
            .merge_change_request(
                &root,
                &remote,
                p.provider,
                p.number,
                p.head_sha.trim(),
                p.strategy.unwrap_or(MergeStrategy::Merge),
            )
            .await
            .map_err(git_write_error)?;
        Ok(MergeChangeRequestReply {
            provider: p.provider,
            number: p.number,
            repo,
            merged: true,
        })
    }

    /// The chat's remote, for the checks of its branch.
    async fn chat_remote(&self, root: &Path) -> Result<crate::source_control::GitRemote, RpcError> {
        let head = self.repos.current_branch(root).await.map_err(failed)?;
        self.change_request_creator
            .remote_for(root, if head.is_empty() { "HEAD" } else { &head })
            .await
            .map_err(git_write_error)
    }

    /// `GetChecks` (FT0 Task 5).
    pub(super) async fn get_checks(&self, p: GetChecksParams) -> Result<ChecksReply, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let root = checkout.root().to_path_buf();
        let sha = match p.sha.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) {
            Some(sha) => sha,
            None => crate::git_write::git_ok(&root, &["rev-parse", "HEAD"])
                .await
                .map_err(git_write_error)?,
        };
        let remote = self.chat_remote(&root).await?;
        self.change_request_creator
            .checks(&root, &remote, p.provider, &sha)
            .await
            .map_err(git_write_error)
    }

    /// `GetCheckLog` (FT0 Task 5).
    pub(super) async fn get_check_log(
        &self,
        p: GetCheckLogParams,
    ) -> Result<CheckLogReply, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let root = checkout.root().to_path_buf();
        let remote = self.chat_remote(&root).await?;
        self.change_request_creator
            .check_log_tail(&root, &remote, p.provider, p.log_id.trim(), p.lines)
            .await
            .map_err(git_write_error)
    }

    /// `DeployLiveApp` (FT0 Task 8): the release is read from the commit,
    /// then the approval is spent (or found spent by an earlier call of the
    /// same chat, app and commit), then Live deploys, checks health and
    /// rolls back on failure. One deploy at a time per daemon.
    pub(super) async fn deploy_live_app(
        &self,
        p: DeployLiveAppParams,
    ) -> Result<DeployLiveAppReply, RpcError> {
        let app = p.app.trim();
        let commit = p.commit.trim().to_ascii_lowercase();
        if !crate::live_deploy::valid_app(app) {
            return Err(RpcError::BadParams(format!("not a Live app name: {app:?}")));
        }
        if !crate::live_deploy::valid_commit(&commit) {
            return Err(RpcError::BadParams(format!(
                "a deploy names a full commit id, not {commit:?}"
            )));
        }
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let release = crate::live_deploy::read_release(checkout.root(), &commit)
            .await
            .map_err(git_write_error)?;
        if let Some(named) = release.config.app.as_deref()
            && named != app
        {
            return Err(RpcError::BadParams(format!(
                "the commit's {} deploys app {named:?}, not {app:?}",
                crate::live_deploy::FACTORY_TOML
            )));
        }
        let _one = self.live_deployer.serialize().await;
        let payload = deploy_payload(app, &commit);
        let (_, resumed) = self
            .approvals()?
            .use_typed_or_resume(&p.chat_id, &p.approval_id, DEPLOY_APPROVAL_KIND, &payload)
            .map_err(typed_error)?;
        self.live_deployer
            .deploy(
                app,
                &commit,
                &p.approval_id,
                p.message.as_deref().unwrap_or_default(),
                &release,
                resumed,
            )
            .await
            .map_err(git_write_error)
    }

    /// `CloseChangeRequest` (FT0 Task 10): close the pull request on the
    /// chat's remote; the branch stays.
    pub(super) async fn close_change_request(
        &self,
        p: CloseChangeRequestParams,
    ) -> Result<CloseChangeRequestReply, RpcError> {
        let checkout = self.chat_checkout(&p.chat_id).await?;
        let root = checkout.root().to_path_buf();
        let remote = self.chat_remote(&root).await?;
        self.change_request_creator
            .close_change_request(&root, &remote, p.provider, p.number)
            .await
            .map_err(git_write_error)?;
        Ok(CloseChangeRequestReply {
            provider: p.provider,
            number: p.number,
            repo: repo_slug(&remote),
            closed: true,
        })
    }
}
