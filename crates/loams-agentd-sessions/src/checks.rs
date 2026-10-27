//! CI check status and log tails for the factory loop (design §72 §5.2, D1433;
//! plan FT1, FT0 Task 5, ruling FT0-0-2): the repository's own CI, read with
//! the credentials agentd already holds for change requests.
//!
//! - **Forgejo**: the combined commit status,
//!   `GET /api/v1/repos/{owner}/{repo}/commits/{sha}/status`. A Forgejo Actions
//!   status's `target_url` names its run and job
//!   (`…/actions/runs/{run}/jobs/{job}`); its log is that page's `/logs`, read
//!   with the same token, only on the credential's own host.
//! - **GitHub**: check runs and commit statuses through `gh api`; an Actions
//!   check run's id is its job id, whose log is
//!   `repos/{owner}/{repo}/actions/jobs/{id}/logs`.
//!
//! A log is cut to its last lines (at most [`MAX_CHECK_LOG_LINES`] and
//! [`MAX_CHECK_LOG_BYTES`]) and scrubbed with the harness's output redaction
//! before it leaves agentd: it is untrusted text, and the factory host quotes
//! it as data.

use std::path::Path;

use loams_agentd_proto::factory_loop::{
    CHECK_LOG_LINES, CheckLogReply, ChecksReply, CiCheck, CiState, MAX_CHECK_LOG_BYTES,
    MAX_CHECK_LOG_LINES,
};
use loams_agentd_proto::git_write::ChangeRequestProvider;

use crate::change_request_create::{ChangeRequestCreator, forgejo_api_url, not_configured};
use crate::factory_loop::repo_slug;
use crate::git_write::{GitWriteError, bad, failed, redact};
use crate::source_control::GitRemote;

/// The overall state of a set of checks (see [`ChecksReply`]).
pub fn overall(checks: &[CiCheck]) -> CiState {
    if checks.iter().any(|c| c.state == CiState::Failure) {
        CiState::Failure
    } else if checks.is_empty() || checks.iter().any(|c| c.state == CiState::Pending) {
        CiState::Pending
    } else {
        CiState::Success
    }
}

/// A Forgejo commit status word.
fn forgejo_state(word: &str) -> CiState {
    match word {
        "success" => CiState::Success,
        "failure" | "error" => CiState::Failure,
        "pending" | "running" | "queued" | "waiting" | "blocked" => CiState::Pending,
        _ => CiState::Neutral,
    }
}

/// A GitHub check run's status and conclusion.
fn github_run_state(status: &str, conclusion: Option<&str>) -> CiState {
    if status != "completed" {
        return CiState::Pending;
    }
    match conclusion {
        Some("success") => CiState::Success,
        Some("failure" | "timed_out" | "action_required" | "startup_failure") => CiState::Failure,
        _ => CiState::Neutral,
    }
}

/// `…/actions/runs/{run}/jobs/{job}` in a Forgejo status URL, as `run/job`.
fn forgejo_log_id(target_url: &str) -> Option<String> {
    let path = target_url.split(['?', '#']).next()?;
    let at = path.find("/actions/runs/")?;
    let mut parts = path[at + "/actions/runs/".len()..].split('/');
    let run = parts.next()?;
    let (Some("jobs"), Some(job)) = (parts.next(), parts.next()) else {
        return None;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (digits(run) && digits(job)).then(|| format!("{run}/{job}"))
}

/// The last `lines` lines of `text`, at most [`MAX_CHECK_LOG_BYTES`], scrubbed.
pub fn tail(text: &str, lines: usize) -> CheckLogReply {
    let lines = lines.clamp(1, MAX_CHECK_LOG_LINES);
    let all: Vec<&str> = text.lines().collect();
    let mut start = all.len().saturating_sub(lines);
    let mut kept = all[start..].join("\n");
    while kept.len() > MAX_CHECK_LOG_BYTES && start < all.len() {
        start += 1;
        kept = all[start..].join("\n");
    }
    if kept.len() > MAX_CHECK_LOG_BYTES {
        // One enormous line: keep its end, on a character boundary.
        let mut cut = kept.len() - MAX_CHECK_LOG_BYTES;
        while !kept.is_char_boundary(cut) {
            cut += 1;
        }
        kept = kept[cut..].to_owned();
    }
    let scrubbed = loams_agentd_harness::redact::redact_output(&kept);
    CheckLogReply {
        lines: u32::try_from(scrubbed.lines().count()).unwrap_or(u32::MAX),
        truncated: start > 0 || kept.len() < all.join("\n").len(),
        text: scrubbed,
    }
}

impl ChangeRequestCreator {
    /// The checks of `sha` on `remote`.
    pub async fn checks(
        &self,
        root: &Path,
        remote: &GitRemote,
        provider: ChangeRequestProvider,
        sha: &str,
    ) -> Result<ChecksReply, GitWriteError> {
        if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad(format!("not a commit: {sha:?}")));
        }
        let checks = match provider {
            ChangeRequestProvider::Forgejo => self.forgejo_checks(remote, sha).await?,
            ChangeRequestProvider::Github => self.github_checks(root, remote, sha).await?,
        };
        Ok(ChecksReply {
            provider,
            repo: repo_slug(remote),
            sha: sha.to_owned(),
            state: overall(&checks),
            checks,
        })
    }

    async fn forgejo_get(
        &self,
        remote: &GitRemote,
        url: reqwest::Url,
        token: &str,
    ) -> Result<reqwest::Response, GitWriteError> {
        let response = self
            .http()
            .get(url)
            .header("authorization", format!("token {token}"))
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
                "Forgejo refused the credential for {} (HTTP {})",
                remote.host,
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(failed(format!("forgejo: HTTP {}", status.as_u16())));
        }
        Ok(response)
    }

    async fn forgejo_checks(
        &self,
        remote: &GitRemote,
        sha: &str,
    ) -> Result<Vec<CiCheck>, GitWriteError> {
        let (base, token) = self.forgejo_credential_for(remote).await?;
        let url = forgejo_api_url(&base, remote, &["commits", sha, "status"])?;
        let reply: serde_json::Value = self
            .forgejo_get(remote, url, &token)
            .await?
            .json()
            .await
            .map_err(|_| failed("forgejo: the status is not JSON"))?;
        let statuses = reply
            .get("statuses")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(statuses
            .iter()
            .map(|status| {
                let text = |key: &str| status.get(key).and_then(serde_json::Value::as_str);
                let url = text("target_url")
                    .filter(|u| !u.is_empty())
                    .map(str::to_owned);
                CiCheck {
                    name: text("context").unwrap_or("status").to_owned(),
                    state: forgejo_state(text("status").or(text("state")).unwrap_or_default()),
                    log_id: url.as_deref().and_then(forgejo_log_id),
                    url,
                }
            })
            .collect())
    }

    async fn github_checks(
        &self,
        root: &Path,
        remote: &GitRemote,
        sha: &str,
    ) -> Result<Vec<CiCheck>, GitWriteError> {
        let api = |rest: &str| {
            vec![
                "api".to_owned(),
                format!("--hostname={}", remote.host),
                format!(
                    "repos/{}/{}/commits/{sha}/{rest}",
                    remote.owner, remote.repository
                ),
            ]
        };
        let runs: serde_json::Value = serde_json::from_str(
            self.run_gh_text(root, &api("check-runs?per_page=100"))
                .await?
                .trim(),
        )
        .map_err(|_| failed("gh printed no check runs"))?;
        let statuses: serde_json::Value =
            serde_json::from_str(self.run_gh_text(root, &api("status")).await?.trim())
                .map_err(|_| failed("gh printed no commit status"))?;
        let mut checks = Vec::new();
        for run in runs
            .get("check_runs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let text = |key: &str| run.get(key).and_then(serde_json::Value::as_str);
            let is_actions = run.pointer("/app/slug").and_then(serde_json::Value::as_str)
                == Some("github-actions");
            checks.push(CiCheck {
                name: text("name").unwrap_or("check").to_owned(),
                state: github_run_state(text("status").unwrap_or_default(), text("conclusion")),
                url: text("html_url").map(str::to_owned),
                log_id: is_actions
                    .then(|| run.get("id").and_then(serde_json::Value::as_u64))
                    .flatten()
                    .map(|id| id.to_string()),
            });
        }
        for status in statuses
            .get("statuses")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let text = |key: &str| status.get(key).and_then(serde_json::Value::as_str);
            checks.push(CiCheck {
                name: text("context").unwrap_or("status").to_owned(),
                state: forgejo_state(text("state").unwrap_or_default()),
                url: text("target_url").map(str::to_owned),
                log_id: None,
            });
        }
        Ok(checks)
    }

    /// The scrubbed tail of a check's log (`log_id` from [`Self::checks`]).
    pub async fn check_log_tail(
        &self,
        root: &Path,
        remote: &GitRemote,
        provider: ChangeRequestProvider,
        log_id: &str,
        lines: Option<u32>,
    ) -> Result<CheckLogReply, GitWriteError> {
        let lines = lines.map_or(CHECK_LOG_LINES, |n| n as usize);
        let text = match provider {
            ChangeRequestProvider::Forgejo => {
                let (run, job) = log_id
                    .split_once('/')
                    .filter(|(r, j)| {
                        [r, j]
                            .iter()
                            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                    })
                    .ok_or_else(|| bad(format!("not a Forgejo log id: {log_id:?}")))?;
                let (base, token) = self.forgejo_credential_for(remote).await?;
                let mut url = base.clone();
                url.path_segments_mut()
                    .map_err(|_| not_configured("the Forgejo credential's URL is not valid"))?
                    .pop_if_empty()
                    .extend([
                        remote.owner.as_str(),
                        remote.repository.as_str(),
                        "actions",
                        "runs",
                        run,
                        "jobs",
                        job,
                        "logs",
                    ]);
                self.forgejo_get(remote, url, &token)
                    .await?
                    .text()
                    .await
                    .map_err(|_| failed("forgejo: the log could not be read"))?
            }
            ChangeRequestProvider::Github => {
                if log_id.is_empty() || !log_id.chars().all(|c| c.is_ascii_digit()) {
                    return Err(bad(format!("not a GitHub job id: {log_id:?}")));
                }
                self.run_gh_text(
                    root,
                    &[
                        "api".to_owned(),
                        format!("--hostname={}", remote.host),
                        format!(
                            "repos/{}/{}/actions/jobs/{log_id}/logs",
                            remote.owner, remote.repository
                        ),
                    ],
                )
                .await?
            }
        };
        Ok(tail(&text, lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_log_ids_come_from_actions_urls() {
        assert_eq!(
            forgejo_log_id("https://f.example/dev/app/actions/runs/12/jobs/0"),
            Some("12/0".into())
        );
        assert_eq!(forgejo_log_id("https://ci.example/build/7"), None);
        assert_eq!(
            forgejo_log_id("https://f.example/x/actions/runs/../jobs/1"),
            None
        );
    }

    #[test]
    fn overall_state() {
        let check = |state| CiCheck {
            name: "c".into(),
            state,
            url: None,
            log_id: None,
        };
        assert_eq!(overall(&[]), CiState::Pending);
        assert_eq!(
            overall(&[check(CiState::Success), check(CiState::Neutral)]),
            CiState::Success
        );
        assert_eq!(
            overall(&[check(CiState::Success), check(CiState::Pending)]),
            CiState::Pending
        );
        assert_eq!(
            overall(&[check(CiState::Pending), check(CiState::Failure)]),
            CiState::Failure
        );
        assert_eq!(github_run_state("in_progress", None), CiState::Pending);
        assert_eq!(
            github_run_state("completed", Some("timed_out")),
            CiState::Failure
        );
        assert_eq!(
            github_run_state("completed", Some("skipped")),
            CiState::Neutral
        );
    }

    #[test]
    fn check_log_tail_is_bounded() {
        let log: String = (1..=5000).map(|n| format!("line {n}\n")).collect();
        let reply = tail(&log, 50);
        assert_eq!(reply.lines, 50);
        assert!(reply.truncated);
        assert!(reply.text.starts_with("line 4951"));
        assert!(reply.text.ends_with("line 5000"));
        let reply = tail(&log, 1_000_000);
        assert_eq!(reply.lines as usize, MAX_CHECK_LOG_LINES);
        let wide = "é".repeat(MAX_CHECK_LOG_BYTES);
        let reply = tail(&wide, 10);
        assert!(reply.text.len() <= MAX_CHECK_LOG_BYTES);
        assert!(reply.truncated);
        assert!(!tail("short\nlog", 10).truncated);
    }

    #[test]
    fn check_log_is_scrubbed() {
        let log = "step 1\nAuthorization: Bearer abc.def.ghi\nusing token ghp_0123456789abcdefghijklmnopqrstuvwxyzAB\nclone https://user:hunter2@git.example/x.git\nFAIL test_signup";
        let reply = tail(log, 10);
        for secret in [
            "abc.def.ghi",
            "ghp_0123456789abcdefghijklmnopqrstuvwxyzAB",
            "hunter2",
        ] {
            assert!(!reply.text.contains(secret), "{secret} in {}", reply.text);
        }
        assert!(reply.text.contains("FAIL test_signup"));
    }
}
