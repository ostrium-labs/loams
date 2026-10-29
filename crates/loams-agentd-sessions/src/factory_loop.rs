//! The engine's half of the Software Factory loop's tools (design §72 §5.2;
//! plan FT1, FT0 Task 4): branch worktrees for build threads, an idempotent
//! "open the pull request for this branch", and the code-host merge that
//! `MergeChangeRequest` runs once its approval checks out.
//!
//! Everything here runs in agentd, which holds the credentials: the Forgejo
//! token from the keyring (sent only to its own host, never across a
//! redirect) and the user's `gh` login for GitHub.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use loams_agentd_proto::git_write::{
    ChangeRequestProvider, CreateChangeRequestReply, MergeStrategy,
};
use loams_agentd_proto::{ChangeRequestState, Worktree};

use crate::change_request_create::{ChangeRequestCreator, forgejo_api_url, not_configured};
use crate::git_write::{
    GitWriteError, bad, failed, git_ok, git_try, redact, valid_branch, worktrees,
};
use crate::repos::Repos;
use crate::source_control::GitRemote;

const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// `owner/repository` of a remote, as merge approvals name it.
pub fn repo_slug(remote: &GitRemote) -> String {
    format!("{}/{}", remote.owner, remote.repository)
}

/// The folder a branch's worktree gets: `{root}/<repo>/<branch, `/` → `-`>`.
fn branch_worktree_path(root: &Path, repo: &Path, branch: &str) -> PathBuf {
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "repo".to_owned());
    let leaf: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    root.join(repo_name).join(leaf.trim_matches('.'))
}

/// A worktree of `repo` on `branch`, created from `base` when the branch is
/// new. Idempotent: when a worktree already has `branch` checked out, that
/// one is answered, so a resumed run never makes a second.
pub async fn create_branch_worktree(
    repos: &Repos,
    repo: &Path,
    branch: &str,
    base: &str,
) -> Result<Worktree, GitWriteError> {
    valid_branch(repo, branch).await?;
    if base.is_empty() || base.starts_with('-') {
        return Err(bad(format!("not a base: {base:?}")));
    }
    let existing = worktrees(repo)
        .await?
        .into_iter()
        .find(|stanza| stanza.branch.as_deref() == Some(branch));
    let path = match existing {
        Some(stanza) => stanza.path,
        None => {
            let path = branch_worktree_path(repos.worktrees_root(), repo, branch);
            if path.exists() {
                return Err(failed(format!(
                    "{} exists and is not a worktree of {branch}",
                    path.display()
                )));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| failed(e.to_string()))?;
            }
            let has_branch = git_try(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .await
            .is_some();
            let target = path.to_string_lossy().into_owned();
            if has_branch {
                git_ok(repo, &["worktree", "add", "--quiet", &target, branch]).await?;
            } else {
                git_ok(
                    repo,
                    &["worktree", "add", "--quiet", "-b", branch, &target, base],
                )
                .await?;
            }
            path
        }
    };
    let checkout = repos
        .checkout_identity(&path)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Worktree {
        repo_path: repo.to_string_lossy().to_string(),
        path: checkout.root.to_string_lossy().to_string(),
        branch: branch.to_owned(),
        name,
        checkout_id: Some(checkout.id),
    })
}

fn state_of(reply: &serde_json::Value) -> ChangeRequestState {
    match reply.get("state").and_then(serde_json::Value::as_str) {
        Some("closed" | "CLOSED")
            if reply.get("merged").and_then(serde_json::Value::as_bool) == Some(true) =>
        {
            ChangeRequestState::Merged
        }
        Some("MERGED") => ChangeRequestState::Merged,
        Some("closed" | "CLOSED") => ChangeRequestState::Closed,
        _ => ChangeRequestState::Open,
    }
}

impl ChangeRequestCreator {
    /// The remote `head`'s pull requests live on, and its `owner/repo`.
    pub async fn remote_for(&self, root: &Path, head: &str) -> Result<GitRemote, GitWriteError> {
        self.remote(root, head).await
    }

    /// The open pull request whose head is `head`, if there is one.
    pub async fn find_open(
        &self,
        root: &Path,
        provider: ChangeRequestProvider,
        head: &str,
    ) -> Result<Option<CreateChangeRequestReply>, GitWriteError> {
        valid_branch(root, head).await?;
        let remote = self.remote(root, head).await?;
        match provider {
            ChangeRequestProvider::Forgejo => self.forgejo_find_open(&remote, head).await,
            ChangeRequestProvider::Github => self.github_find_open(root, &remote, head).await,
        }
    }

    async fn forgejo_find_open(
        &self,
        remote: &GitRemote,
        head: &str,
    ) -> Result<Option<CreateChangeRequestReply>, GitWriteError> {
        let (base, token) = self.forgejo_credential_for(remote).await?;
        // One page of the newest open pull requests is enough: a run's branch
        // gets one pull request, opened moments ago.
        let mut url = forgejo_api_url(&base, remote, &["pulls"])?;
        url.query_pairs_mut()
            .append_pair("state", "open")
            .append_pair("sort", "recentupdate")
            .append_pair("limit", "50");
        let response = self
            .http()
            .get(url)
            .header("authorization", format!("token {token}"))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|error| {
                failed(format!(
                    "forgejo: the request failed: {}",
                    redact(&error.without_url().to_string())
                ))
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(not_configured(format!(
                "Forgejo refused the credential (HTTP {})",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(failed(format!("forgejo: HTTP {}", status.as_u16())));
        }
        let pulls: Vec<serde_json::Value> = response.json().await.unwrap_or_default();
        let found = pulls.into_iter().find(|pull| {
            pull.pointer("/head/ref")
                .and_then(serde_json::Value::as_str)
                == Some(head)
        });
        Ok(found.and_then(|pull| {
            Some(CreateChangeRequestReply {
                provider: ChangeRequestProvider::Forgejo,
                number: pull.get("number")?.as_u64()?,
                url: pull
                    .get("html_url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                state: state_of(&pull),
            })
        }))
    }

    pub(crate) async fn run_gh_text(
        &self,
        root: &Path,
        args: &[String],
    ) -> Result<String, GitWriteError> {
        let mut command = self.gh_command(root);
        command.args(args).stdin(Stdio::null());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(not_configured("gh is not installed"));
            }
            Err(error) => return Err(failed(format!("gh could not start: {error}"))),
        };
        let output = tokio::time::timeout(GH_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| failed("gh timed out"))?
            .map_err(|error| failed(format!("gh failed: {error}")))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            if stderr.contains("gh auth login") || stderr.contains("not logged in") {
                return Err(not_configured("gh is not signed in"));
            }
            let detail: String = stderr.trim().chars().take(1024).collect();
            return Err(failed(format!("gh: {}", redact(&detail))));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn github_find_open(
        &self,
        root: &Path,
        remote: &GitRemote,
        head: &str,
    ) -> Result<Option<CreateChangeRequestReply>, GitWriteError> {
        let stdout = self
            .run_gh_text(
                root,
                &[
                    "pr".into(),
                    "list".into(),
                    format!(
                        "--repo={}/{}/{}",
                        remote.host, remote.owner, remote.repository
                    ),
                    format!("--head={head}"),
                    "--state=open".into(),
                    "--json=number,url,state".into(),
                ],
            )
            .await?;
        let pulls: Vec<serde_json::Value> = serde_json::from_str(stdout.trim())
            .map_err(|_| failed("gh printed no pull request list"))?;
        Ok(pulls.first().and_then(|pull| {
            Some(CreateChangeRequestReply {
                provider: ChangeRequestProvider::Github,
                number: pull.get("number")?.as_u64()?,
                url: pull
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                state: state_of(pull),
            })
        }))
    }

    /// Merge pull request `number` on `remote` if, and only if, its head is
    /// still `head_sha` (the code host checks it). Callers check the
    /// approval first; this never runs without one.
    pub async fn merge_change_request(
        &self,
        root: &Path,
        remote: &GitRemote,
        provider: ChangeRequestProvider,
        number: u64,
        head_sha: &str,
        strategy: MergeStrategy,
    ) -> Result<(), GitWriteError> {
        if head_sha.len() < 7 || !head_sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad(format!("not a commit: {head_sha:?}")));
        }
        match provider {
            ChangeRequestProvider::Forgejo => {
                let (base, token) = self.forgejo_credential_for(remote).await?;
                let url = forgejo_api_url(&base, remote, &["pulls", &number.to_string(), "merge"])?;
                let style = match strategy {
                    MergeStrategy::Merge => "merge",
                    MergeStrategy::Squash => "squash",
                    MergeStrategy::Rebase => "rebase",
                    MergeStrategy::Ff => "fast-forward-only",
                };
                let response = self
                    .http()
                    .post(url)
                    .header("authorization", format!("token {token}"))
                    .header("accept", "application/json")
                    .json(&serde_json::json!({ "Do": style, "head_commit_id": head_sha }))
                    .send()
                    .await
                    .map_err(|error| {
                        failed(format!(
                            "forgejo: the request failed: {}",
                            redact(&error.without_url().to_string())
                        ))
                    })?;
                let status = response.status();
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    return Err(not_configured(format!(
                        "Forgejo refused the credential (HTTP {})",
                        status.as_u16()
                    )));
                }
                if !status.is_success() {
                    let reply: serde_json::Value = response.json().await.unwrap_or_default();
                    let message = reply
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .map(|m| redact(m.chars().take(512).collect::<String>().as_str()))
                        .unwrap_or_default();
                    return Err(failed(format!(
                        "forgejo: the merge was refused (HTTP {}) {message}",
                        status.as_u16()
                    )));
                }
                Ok(())
            }
            ChangeRequestProvider::Github => {
                let flag = match strategy {
                    MergeStrategy::Merge | MergeStrategy::Ff => "--merge",
                    MergeStrategy::Squash => "--squash",
                    MergeStrategy::Rebase => "--rebase",
                };
                self.run_gh_text(
                    root,
                    &[
                        "pr".into(),
                        "merge".into(),
                        number.to_string(),
                        format!(
                            "--repo={}/{}/{}",
                            remote.host, remote.owner, remote.repository
                        ),
                        flag.into(),
                        format!("--match-head-commit={head_sha}"),
                    ],
                )
                .await
                .map(drop)
            }
        }
    }
}

impl ChangeRequestCreator {
    /// Close pull request `number` on `remote`, keeping its branch: Forgejo's
    /// `PATCH …/pulls/{n}` with `state: closed`, or `gh pr close` (never with
    /// `--delete-branch`). Closing a closed pull request is not an error.
    pub async fn close_change_request(
        &self,
        root: &Path,
        remote: &GitRemote,
        provider: ChangeRequestProvider,
        number: u64,
    ) -> Result<(), GitWriteError> {
        match provider {
            ChangeRequestProvider::Forgejo => {
                let (base, token) = self.forgejo_credential_for(remote).await?;
                let url = forgejo_api_url(&base, remote, &["pulls", &number.to_string()])?;
                let response = self
                    .http()
                    .patch(url)
                    .header("authorization", format!("token {token}"))
                    .header("accept", "application/json")
                    .json(&serde_json::json!({ "state": "closed" }))
                    .send()
                    .await
                    .map_err(|error| {
                        failed(format!(
                            "forgejo: the request failed: {}",
                            redact(&error.without_url().to_string())
                        ))
                    })?;
                let status = response.status();
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    return Err(not_configured(format!(
                        "Forgejo refused the credential (HTTP {})",
                        status.as_u16()
                    )));
                }
                if !status.is_success() {
                    return Err(failed(format!(
                        "forgejo: closing #{number} was refused (HTTP {})",
                        status.as_u16()
                    )));
                }
                Ok(())
            }
            ChangeRequestProvider::Github => {
                let closed = self
                    .run_gh_text(
                        root,
                        &[
                            "pr".into(),
                            "close".into(),
                            number.to_string(),
                            format!(
                                "--repo={}/{}/{}",
                                remote.host, remote.owner, remote.repository
                            ),
                        ],
                    )
                    .await;
                match closed {
                    Ok(_) => Ok(()),
                    Err(GitWriteError::Failed(text)) if text.contains("already closed") => Ok(()),
                    Err(error) => Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_worktree_paths_stay_one_folder_deep() {
        let root = Path::new("/w");
        assert_eq!(
            branch_worktree_path(root, Path::new("/src/comet"), "factory/r1/2"),
            PathBuf::from("/w/comet/factory-r1-2")
        );
        assert_eq!(
            branch_worktree_path(root, Path::new("/src/comet"), "../../etc"),
            PathBuf::from("/w/comet/-..-etc")
        );
    }
}
