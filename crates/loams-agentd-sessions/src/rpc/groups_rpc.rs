//! `EngineRpc`'s run-group methods (design §53 §5.5, §6.5 G12–G16; plan SD1
//! Task 12). The records and the scheduler are `crate::groups::RunGroups`.
//!
//! - Every path is jailed as Task 9's git writes are: the repository must be
//!   one this device knows (`known_repo`), a comparison's `path` is a literal
//!   relative path, and a member's worktree is the one the group made.
//! - `ResolveRunGroup` runs Task 9's methods through their own dispatch arms
//!   (`CommitWorktree`, `MergeBranch`, `PushBranch`, `CreateChangeRequest`),
//!   so their guards apply unchanged: the known-checkout jail, the live-run
//!   refusal, the serialized writes, the checksum and source-head checks
//!   that carry the person's confirmation, and no plain force push.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use futures::stream::BoxStream;
use loams_agentd_proto::git_write::{CommitWorktreeParams, PushBranchParams, STALE_DIFF};
use loams_agentd_proto::groups::{
    CheckRun, CreateRunGroupParams, CreateRunGroupReply, GROUP_NOT_READY,
    GetRunGroupComparisonParams, ResolveAction, ResolveRunGroupParams, ResolveRunGroupReply,
    RunGroupCheckParams, RunGroupCheckReply, RunGroupComparison, RunGroupList, RunGroupResolution,
    WatchRunGroupsParams,
};
use loams_agentd_proto::params::MutateParams;
use loams_agentd_proto::rpc::{Method, calls};
use loams_agentd_proto::{ChatConfig, CheckoutDiff, HarnessId, SandboxLevel};
use loams_agentd_rpc::{RpcError, RpcReply, RpcService};

use super::{EngineRpc, git_write_error};
use crate::groups::{GroupRecord, MAX_VARIANTS, MemberRecord, Phase, RunGroups};

/// A failure as the RPC's.
fn failed(error: impl std::fmt::Display) -> RpcError {
    RpcError::Failed(error.to_string())
}

impl EngineRpc {
    fn groups(&self) -> Result<&RunGroups, RpcError> {
        self.run_groups
            .as_ref()
            .ok_or_else(|| RpcError::Failed("run groups unavailable".into()))
    }

    /// One of Task 9's methods through its own dispatch arm, guards and all.
    async fn call_arm<M: Method>(&self, params: &M::Params) -> Result<M::Result, RpcError> {
        let value = serde_json::to_value(params).map_err(failed)?;
        match self.handle(M::NAME, value).await? {
            RpcReply::Value(value) => serde_json::from_value(value).map_err(failed),
            RpcReply::Stream(_) => Err(RpcError::Failed(format!("{} streams", M::NAME))),
        }
    }

    fn group_record(&self, group_id: &str) -> Result<GroupRecord, RpcError> {
        self.groups()?
            .group(group_id)
            .ok_or_else(|| RpcError::BadParams(format!("no run group {group_id}")))
    }

    /// G12.
    pub(super) async fn create_run_group(
        &self,
        p: CreateRunGroupParams,
    ) -> Result<CreateRunGroupReply, RpcError> {
        let groups = self.groups()?.clone();
        if p.prompt.trim().is_empty() {
            return Err(RpcError::BadParams("the prompt is empty".into()));
        }
        if p.variants.is_empty() || p.variants.len() > MAX_VARIANTS {
            return Err(RpcError::BadParams(format!(
                "a run group has 1 to {MAX_VARIANTS} variants"
            )));
        }
        for variant in &p.variants {
            if variant.harness == HarnessId::Unsupported {
                return Err(RpcError::BadParams("unknown harness".into()));
            }
            self.registry
                .resolve(variant.harness)
                .map_err(|e| RpcError::BadParams(e.to_string()))?;
        }
        let repo = self.known_repo(&p.repo_path).await?;
        if p.base.is_empty() || p.base.starts_with('-') {
            return Err(RpcError::BadParams("base must name a ref".into()));
        }
        let base_sha = crate::groups::resolve_commit(&repo, &p.base)
            .await
            .ok_or_else(|| RpcError::BadParams(format!("base {} is not a commit", p.base)))?;
        if let Some(space_id) = &p.space_id {
            let space = self.local_project_action_space(space_id)?;
            let same = std::fs::canonicalize(&space.path).is_ok_and(|path| path == repo);
            if !same {
                return Err(RpcError::BadParams(
                    "the project's folder is not repoPath".into(),
                ));
            }
        }
        if let Some(action_id) = &p.check_action_id {
            let space_id = p.space_id.as_deref().ok_or_else(|| {
                RpcError::BadParams("a check action needs the project (spaceId)".into())
            })?;
            let space = self.local_project_action_space(space_id)?;
            self.project_actions
                .action(&space.id, Path::new(&space.path), action_id)
                .map_err(failed)?
                .ok_or_else(|| RpcError::BadParams(format!("no project action {action_id}")))?;
        }

        let group_id = crate::new_id();
        let mut members: Vec<MemberRecord> = Vec::new();
        for variant in &p.variants {
            let made = self.make_member(&repo, &base_sha, &p, variant).await;
            match made {
                Ok(member) => members.push(member),
                Err(error) => {
                    // Undo the members made so far; nothing was sent yet.
                    for member in &members {
                        self.discard_member(&repo, member).await;
                    }
                    return Err(error);
                }
            }
        }
        let chat_ids: Vec<String> = members.iter().map(|m| m.chat_id.clone()).collect();
        // One approval mode for the group (§53 §8.1), kept on each member's
        // thread before any is sent (SD1 Task 13; ruling T12-10).
        if let Some(approvals) = &self.approvals {
            for chat_id in &chat_ids {
                approvals
                    .set_mode(chat_id, p.approval_mode)
                    .map_err(failed)?;
            }
        }
        groups
            .create(GroupRecord {
                id: group_id.clone(),
                space_id: p.space_id.clone(),
                repo_path: repo.to_string_lossy().into_owned(),
                prompt: p.prompt.clone(),
                base: p.base.clone(),
                base_sha,
                created_at_ms: crate::now_ms(),
                approval_mode: p.approval_mode,
                check_action_id: p.check_action_id.clone(),
                attachments: p.attachments.clone(),
                members,
                resolution: None,
            })
            .map_err(failed)?;
        Ok(CreateRunGroupReply { group_id, chat_ids })
    }

    /// One member: a worktree from the base and a chat in it, titled so no
    /// title model runs (which would also rename the worktree's branch).
    async fn make_member(
        &self,
        repo: &Path,
        base_sha: &str,
        p: &CreateRunGroupParams,
        variant: &loams_agentd_proto::groups::RunVariant,
    ) -> Result<MemberRecord, RpcError> {
        let worktree = self
            .repos
            .create_worktree(repo, base_sha)
            .await
            .map_err(failed)?;
        let chat_id = crate::new_id();
        let created = self.mutate(MutateParams::CreateChat {
            chat_id: chat_id.clone(),
            space_id: p.space_id.clone(),
            device_id: Some(self.doc_host.device_id().to_string()),
            config: Some(ChatConfig {
                harness: variant.harness,
                model: variant.model.clone(),
                reasoning: variant.reasoning,
                model_options: variant.model_options.clone(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            branch: Some(worktree.branch.clone()),
            cwd: Some(worktree.path.clone()),
            parent_chat_id: None,
        });
        let member = MemberRecord {
            chat_id: chat_id.clone(),
            variant: variant.clone(),
            cwd: worktree.path.clone(),
            branch: worktree.branch.clone(),
            checkout_id: worktree.checkout_id.clone(),
            message_id: crate::new_id(),
            command_id: None,
            phase: Phase::Queued,
            started_at_ms: None,
            finished_at_ms: None,
            tokens: None,
            check: None,
            detached: false,
            archived: false,
        };
        if let Err(error) = created {
            self.discard_member(repo, &member).await;
            return Err(error);
        }
        self.workspace
            .rename_chat(&chat_id, &crate::groups::member_title(&p.prompt, variant))
            .map_err(failed)?;
        Ok(member)
    }

    /// Remove a member that was never sent: its chat and its worktree.
    async fn discard_member(&self, repo: &Path, member: &MemberRecord) {
        let _ = self.workspace.delete_chat(&member.chat_id);
        if let Err(error) = self
            .repos
            .delete_worktree(repo, Path::new(&member.cwd))
            .await
        {
            tracing::warn!(cwd = %member.cwd, %error, "run group worktree not removed");
        }
    }

    /// G13.
    pub(super) fn watch_run_groups(
        &self,
        p: WatchRunGroupsParams,
    ) -> Result<BoxStream<'static, RunGroupList>, RpcError> {
        let rx = self.groups()?.watch();
        let space = p.space_id;
        Ok(
            futures::stream::unfold((rx, false), move |(mut rx, emitted)| {
                let space = space.clone();
                async move {
                    if emitted {
                        rx.changed().await.ok()?;
                    }
                    let groups = rx
                        .borrow_and_update()
                        .iter()
                        .filter(|group| {
                            space
                                .as_deref()
                                .is_none_or(|space| group.space_id.as_deref() == Some(space))
                        })
                        .cloned()
                        .collect();
                    Some((RunGroupList { groups }, (rx, true)))
                }
            })
            .boxed(),
        )
    }

    /// G14: each member's result is snapshotted to a tree (a throwaway
    /// index, nothing committed) and diffed against the group's base commit
    /// for the matrix, and against the other member for the pair.
    pub(super) async fn run_group_comparison(
        &self,
        p: GetRunGroupComparisonParams,
    ) -> Result<RunGroupComparison, RpcError> {
        let group = self.group_record(&p.group_id)?;
        if let Some(path) = &p.path {
            crate::git_write::checkout_relative(path).map_err(git_write_error)?;
        }
        let a = group
            .member(&p.a)
            .ok_or_else(|| RpcError::BadParams(format!("{} is not in the group", p.a)))?
            .clone();
        let b = match &p.b {
            Some(b) => Some(
                group
                    .member(b)
                    .ok_or_else(|| RpcError::BadParams(format!("{b} is not in the group")))?
                    .clone(),
            ),
            None => None,
        };
        let repo = PathBuf::from(&group.repo_path);
        let mut trees: Vec<(String, String)> = Vec::new();
        for member in &group.members {
            if !Path::new(&member.cwd).is_dir() {
                continue; // its worktree was removed
            }
            let tree = crate::diff_sync::snapshot_tree(Path::new(&member.cwd))
                .await
                .map_err(failed)?;
            trees.push((member.chat_id.clone(), tree));
        }
        let tree_of = |chat_id: &str| {
            trees
                .iter()
                .find(|(id, _)| id == chat_id)
                .map(|(_, tree)| tree.clone())
                .ok_or_else(|| RpcError::Failed(format!("{chat_id}'s worktree is gone")))
        };
        let mut per_member = Vec::new();
        for (chat_id, tree) in &trees {
            let stats = crate::groups::numstat(&repo, &group.base_sha, tree, p.path.as_deref())
                .await
                .map_err(failed)?;
            per_member.push((chat_id.clone(), stats));
        }
        let (from, to) = match &b {
            Some(b) => (tree_of(&a.chat_id)?, tree_of(&b.chat_id)?),
            None => (group.base_sha.clone(), tree_of(&a.chat_id)?),
        };
        let snapshot =
            crate::diff_sync::capture_tree_diff(&self.repos, &repo, &from, &to, p.path.as_deref())
                .await
                .map_err(failed)?;
        Ok(RunGroupComparison {
            matrix: crate::groups::matrix(per_member),
            pair_diff: Some(CheckoutDiff {
                checkout_id: a.checkout_id.clone().unwrap_or_default(),
                device_id: self.doc_host.device_id().to_string(),
                cwd: a.cwd.clone(),
                patch: snapshot.patch,
                files: snapshot.files,
                additions: snapshot.additions,
                deletions: snapshot.deletions,
                truncated: snapshot.truncated,
                checksum: snapshot.checksum,
                updated_at: chrono::Utc::now(),
            }),
        })
    }

    /// G15: the group's check action in each finished member's worktree, as
    /// a PTY whose shell exits with the check's status.
    pub(super) async fn run_group_check(
        &self,
        p: RunGroupCheckParams,
    ) -> Result<RunGroupCheckReply, RpcError> {
        let groups = self.groups()?.clone();
        let group = self.group_record(&p.group_id)?;
        let action_id = group
            .check_action_id
            .as_deref()
            .ok_or_else(|| RpcError::BadParams("the run group has no check action".into()))?;
        let space_id = group
            .space_id
            .as_deref()
            .ok_or_else(|| RpcError::BadParams("the run group has no project".into()))?;
        let space = self.local_project_action_space(space_id)?;
        let project_root = std::fs::canonicalize(&space.path)
            .map_err(|_| failed("Project root is unavailable"))?;
        let action = self
            .project_actions
            .action(&space.id, Path::new(&space.path), action_id)
            .map_err(failed)?
            .ok_or_else(|| RpcError::BadParams(format!("no project action {action_id}")))?;
        let targets: Vec<&MemberRecord> =
            match &p.chat_id {
                Some(chat_id) => vec![group.member(chat_id).ok_or_else(|| {
                    RpcError::BadParams(format!("{chat_id} is not in the group"))
                })?],
                None => group
                    .members
                    .iter()
                    .filter(|m| !m.detached && m.phase != Phase::Cancelled)
                    .collect(),
            };
        if let Some(live) = targets.iter().find(|m| m.phase.is_live()) {
            return Err(RpcError::Failed(format!(
                "{GROUP_NOT_READY}: {} is still running",
                live.chat_id
            )));
        }
        let mut runs = Vec::new();
        for member in targets {
            let run = crate::project_actions::launch_project_check(
                &self.terminals,
                &action,
                &project_root,
                Path::new(&member.cwd),
                120,
                32,
            )
            .map_err(failed)?;
            groups
                .start_check(&group.id, &member.chat_id, &run.terminal.id)
                .map_err(failed)?;
            runs.push(CheckRun {
                chat_id: member.chat_id.clone(),
                terminal_id: run.terminal.id,
            });
        }
        Ok(RunGroupCheckReply { runs })
    }

    /// G16.
    pub(super) async fn resolve_run_group(
        &self,
        p: ResolveRunGroupParams,
    ) -> Result<ResolveRunGroupReply, RpcError> {
        let groups = self.groups()?.clone();
        let group = self.group_record(&p.group_id)?;
        if group.resolution.is_some() {
            return Err(RpcError::Failed("the run group is already resolved".into()));
        }
        let winner = group
            .member(&p.winner_chat_id)
            .filter(|m| !m.detached && m.phase != Phase::Cancelled)
            .ok_or_else(|| {
                RpcError::BadParams(format!("{} is not a member to pick", p.winner_chat_id))
            })?
            .clone();
        let live: Vec<String> = group
            .live_members()
            .iter()
            .map(|m| m.chat_id.clone())
            .collect();
        if !live.is_empty() {
            if !p.force {
                return Err(RpcError::Failed(format!(
                    "{GROUP_NOT_READY}: {} of the group's members are still running",
                    live.len()
                )));
            }
            // `force`: queued members are never sent, running ones stop.
            groups
                .update(&group.id, |group| {
                    for member in &mut group.members {
                        if member.phase == Phase::Queued {
                            member.phase = Phase::Cancelled;
                        }
                    }
                })
                .map_err(failed)?;
            for chat_id in &live {
                if let Err(error) = self.sessions.interrupt(chat_id).await {
                    tracing::debug!(chat = %chat_id, %error, "run group interrupt skipped");
                }
            }
        }

        let (result, sha, url) = match p.action {
            ResolveAction::Merge => {
                let mut merge = p
                    .merge
                    .clone()
                    .ok_or_else(|| RpcError::BadParams("merge needs its parameters".into()))?;
                let same_repo = std::fs::canonicalize(&merge.repo_path)
                    .is_ok_and(|path| path == Path::new(&group.repo_path));
                if !same_repo {
                    return Err(RpcError::BadParams(
                        "merge.repoPath is not the group's repository".into(),
                    ));
                }
                let branch = self.winner_branch(&winner).await?;
                if merge.source != branch {
                    return Err(RpcError::BadParams(format!(
                        "merge.source must be the winner's branch {branch}"
                    )));
                }
                // The person confirmed the branch at this head; a commit of
                // the winner's uncommitted work moves it, so check it first.
                let head_now = crate::groups::resolve_commit(Path::new(&winner.cwd), "HEAD")
                    .await
                    .unwrap_or_default();
                if merge.expected_source_head != head_now {
                    return Err(RpcError::Failed(format!(
                        "{STALE_DIFF}: {branch} moved since the merge was confirmed"
                    )));
                }
                let head = self
                    .commit_winner(&group, &winner, &p, merge.message.as_deref())
                    .await?;
                if let Some(committed) = head {
                    merge.expected_source_head = committed;
                }
                let merged = self.call_arm::<calls::MergeBranch>(&merge).await?;
                (
                    format!("merged {} into {}", merge.source, merge.target),
                    Some(merged.sha),
                    None,
                )
            }
            ResolveAction::Pr => {
                let mut pr = p.pr.clone().ok_or_else(|| {
                    RpcError::BadParams("pr needs the change request's parameters".into())
                })?;
                let branch = self.winner_branch(&winner).await?;
                if !pr.head.is_empty() && pr.head != branch {
                    return Err(RpcError::BadParams(format!(
                        "pr.head must be the winner's branch {branch}"
                    )));
                }
                self.commit_winner(&group, &winner, &p, Some(&pr.title))
                    .await?;
                self.call_arm::<calls::PushBranch>(&PushBranchParams {
                    cwd: winner.cwd.clone(),
                    remote: None,
                    branch: branch.clone(),
                    set_upstream: true,
                    force_with_lease: false,
                })
                .await?;
                pr.cwd = winner.cwd.clone();
                pr.head = branch;
                if pr.base.is_empty() {
                    pr.base = group.base.clone();
                }
                let created = self.call_arm::<calls::CreateChangeRequest>(&pr).await?;
                (
                    format!(
                        "opened {} change request #{}",
                        created.provider.as_str(),
                        created.number
                    ),
                    None,
                    Some(created.url),
                )
            }
            ResolveAction::Keep => (format!("kept {} as a thread", winner.chat_id), None, None),
        };

        let others: Vec<MemberRecord> = group
            .members
            .iter()
            .filter(|m| m.chat_id != winner.chat_id && !m.detached)
            .cloned()
            .collect();
        let mut archived = Vec::new();
        if p.archive_others {
            for member in &others {
                let done = self.mutate(MutateParams::SetChatArchived {
                    chat_id: member.chat_id.clone(),
                    archived: true,
                });
                match done {
                    Ok(()) => archived.push(member.chat_id.clone()),
                    Err(error) => {
                        tracing::warn!(chat = %member.chat_id, %error, "run group member not archived");
                    }
                }
            }
        }
        if p.remove_other_worktrees {
            let repo = PathBuf::from(&group.repo_path);
            for member in &others {
                let cwd = Path::new(&member.cwd);
                if !cwd.is_dir() {
                    continue;
                }
                let identity = self.repos.checkout_identity(cwd).await.map_err(failed)?;
                self.ensure_no_live_run(&identity).await?;
                let _write = self.git_writes.lock().await;
                self.repos
                    .delete_worktree(&repo, cwd)
                    .await
                    .map_err(failed)?;
            }
            self.diff_sync.sync_all();
        }
        let resolution = RunGroupResolution {
            action: p.action,
            winner_chat_id: winner.chat_id.clone(),
            result: result.clone(),
            sha: sha.clone(),
            url: url.clone(),
            resolved_at_ms: crate::now_ms(),
        };
        groups
            .update(&group.id, |group| {
                for member in &mut group.members {
                    if member.chat_id == winner.chat_id {
                        member.detached = p.action == ResolveAction::Keep;
                    }
                    if archived.contains(&member.chat_id) {
                        member.archived = true;
                    }
                }
                group.resolution = Some(resolution);
            })
            .map_err(failed)?;
        Ok(ResolveRunGroupReply { result, sha, url })
    }

    /// The branch the winner's worktree is on now (a person may have renamed
    /// it since the group was made).
    async fn winner_branch(&self, winner: &MemberRecord) -> Result<String, RpcError> {
        let branch = self
            .repos
            .current_branch(Path::new(&winner.cwd))
            .await
            .map_err(failed)?;
        if branch.is_empty() {
            return Err(RpcError::Failed(
                "the winner's worktree is not on a branch".into(),
            ));
        }
        Ok(branch)
    }

    /// Commit the winner's uncommitted work, if any, through Task 9's
    /// `CommitWorktree`, against the checksum the person confirmed. Answers
    /// the new head, or `None` when there was nothing to commit.
    async fn commit_winner(
        &self,
        group: &GroupRecord,
        winner: &MemberRecord,
        p: &ResolveRunGroupParams,
        message: Option<&str>,
    ) -> Result<Option<String>, RpcError> {
        let cwd = Path::new(&winner.cwd);
        if !crate::groups::is_dirty(cwd).await.map_err(failed)? {
            return Ok(None);
        }
        let expected_checksum = p.expected_checksum.clone().ok_or_else(|| {
            RpcError::Failed(format!(
                "{STALE_DIFF}: the winner has uncommitted changes; confirm its diff (expectedChecksum)"
            ))
        })?;
        let identity = self.repos.checkout_identity(cwd).await.map_err(failed)?;
        let subject = message
            .map(str::to_owned)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| crate::groups::commit_subject(&group.prompt));
        let committed = self
            .call_arm::<calls::CommitWorktree>(&CommitWorktreeParams {
                checkout_id: identity.id,
                cwd: winner.cwd.clone(),
                message: subject,
                paths: Vec::new(),
                all: true,
                amend: false,
                signoff: false,
                expected_checksum,
            })
            .await?;
        Ok(Some(committed.sha))
    }
}
