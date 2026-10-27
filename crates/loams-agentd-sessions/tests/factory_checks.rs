//! Plan FT1, FT0 Task 5 (design §72 §5.2, D1433): CI check status and log
//! tails, against a local fake Forgejo and a fake `gh`. No real service.
#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use loams_agentd_proto::factory_loop::CiState;
use loams_agentd_proto::git_write::ChangeRequestProvider;
use loams_agentd_sessions::change_request_create::{
    ChangeRequestCreator, ForgejoCredential, StaticForgeCredentials,
};
use loams_agentd_sessions::source_control::parse_git_remote;
use serde_json::{Value, json};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo(dir: &Path, remote: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["remote", "add", "origin", remote]);
    dir.to_path_buf()
}

fn forgejo(method: &str, path: &str) -> (u16, Value) {
    match (method, path) {
        ("GET", p) if p == format!("/api/v1/repos/dev/project/commits/{SHA}/status") => (
            200,
            json!({
                "state": "failure",
                "statuses": [
                    {"context": "ci / test", "status": "failure",
                     "target_url": "http://forgejo.invalid/dev/project/actions/runs/12/jobs/1"},
                    {"context": "ci / lint", "status": "success",
                     "target_url": "http://forgejo.invalid/dev/project/actions/runs/12/jobs/0"},
                    {"context": "external", "status": "pending", "target_url": ""},
                ]
            }),
        ),
        ("GET", "/dev/project/actions/runs/12/jobs/1/logs") => (
            200,
            Value::String(
                "setup\nexport TOKEN=ghp_0123456789abcdefghijklmnopqrstuvwxyzAB\nFAIL test_signup_rate_limit".into(),
            ),
        ),
        _ => (404, json!({"message": "not found"})),
    }
}

#[tokio::test]
async fn checks_forgejo_fixture() {
    let (base, seen) = support::forgejo::fake_forgejo(forgejo).await;
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"), &format!("{base}/dev/project.git"));
    let creator = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(Some(
        ForgejoCredential {
            base_url: base.clone(),
            token: "forgejo-test-token".into(),
        },
    ))));
    let remote = parse_git_remote(&format!("{base}/dev/project.git")).unwrap();
    let checks = creator
        .checks(&root, &remote, ChangeRequestProvider::Forgejo, SHA)
        .await
        .unwrap();
    assert_eq!(checks.repo, "dev/project");
    assert_eq!(checks.state, CiState::Failure);
    assert_eq!(checks.checks.len(), 3);
    assert_eq!(checks.checks[0].name, "ci / test");
    assert_eq!(checks.checks[0].state, CiState::Failure);
    assert_eq!(checks.checks[0].log_id.as_deref(), Some("12/1"));
    assert_eq!(checks.checks[2].state, CiState::Pending);
    assert_eq!(checks.checks[2].url, None);

    let log = creator
        .check_log_tail(
            &root,
            &remote,
            ChangeRequestProvider::Forgejo,
            "12/1",
            Some(2),
        )
        .await
        .unwrap();
    assert!(
        log.text.ends_with("FAIL test_signup_rate_limit"),
        "{}",
        log.text
    );
    assert!(
        !log.text.contains("ghp_0123456789"),
        "scrubbed: {}",
        log.text
    );
    assert_eq!(log.lines, 2);
    assert!(log.truncated);
    for recorded in seen.lock().unwrap().iter() {
        assert_eq!(recorded.authorization, "token forgejo-test-token");
    }
    // A log id that is not run/job digits never becomes a URL.
    assert!(
        creator
            .check_log_tail(
                &root,
                &remote,
                ChangeRequestProvider::Forgejo,
                "../../admin",
                None
            )
            .await
            .is_err()
    );
    // The token is only for the credential's host.
    let other = parse_git_remote("https://elsewhere.example/dev/project.git").unwrap();
    let error = creator
        .checks(&root, &other, ChangeRequestProvider::Forgejo, SHA)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("provider_not_configured"),
        "{error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn checks_github_fixture() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().unwrap();
    let root = repo(
        &tmp.path().join("repo"),
        "https://github.com/acme/widgets.git",
    );
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let runs = json!({"check_runs": [
        {"id": 901, "name": "test", "status": "completed", "conclusion": "failure",
         "html_url": "https://github.com/acme/widgets/runs/901", "app": {"slug": "github-actions"}},
        {"id": 902, "name": "build", "status": "in_progress", "conclusion": null,
         "html_url": "https://github.com/acme/widgets/runs/902", "app": {"slug": "github-actions"}},
        {"id": 77, "name": "codecov", "status": "completed", "conclusion": "success",
         "app": {"slug": "codecov"}}
    ]});
    let status = json!({"statuses": [{"context": "deploy/preview", "state": "success",
                                      "target_url": "https://preview.example/1"}]});
    std::fs::write(tmp.path().join("runs.json"), runs.to_string()).unwrap();
    std::fs::write(tmp.path().join("status.json"), status.to_string()).unwrap();
    let log: String = (1..=300).map(|n| format!("step {n}\n")).collect();
    std::fs::write(tmp.path().join("job.log"), log).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ndir='{}'\nfor a in \"$@\"; do last=\"$a\"; done\ncase \"$last\" in\n\
             *check-runs*) cat \"$dir/runs.json\";;\n*/status) cat \"$dir/status.json\";;\n\
             *jobs/901/logs) cat \"$dir/job.log\";;\n*) echo \"unexpected $last\" >&2; exit 1;;\nesac\n",
            tmp.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let creator = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(None)))
        .with_gh_path(vec![bin]);
    let remote = parse_git_remote("https://github.com/acme/widgets.git").unwrap();
    let checks = creator
        .checks(&root, &remote, ChangeRequestProvider::Github, SHA)
        .await
        .unwrap();
    assert_eq!(checks.repo, "acme/widgets");
    assert_eq!(checks.state, CiState::Failure);
    let names: Vec<_> = checks
        .checks
        .iter()
        .map(|c| (c.name.as_str(), c.state, c.log_id.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("test", CiState::Failure, Some("901")),
            ("build", CiState::Pending, Some("902")),
            ("codecov", CiState::Success, None),
            ("deploy/preview", CiState::Success, None),
        ]
    );
    let log = creator
        .check_log_tail(&root, &remote, ChangeRequestProvider::Github, "901", None)
        .await
        .unwrap();
    assert_eq!(log.lines, 200);
    assert!(log.text.starts_with("step 101"));
    assert!(log.truncated);
    assert!(
        creator
            .check_log_tail(
                &root,
                &remote,
                ChangeRequestProvider::Github,
                "901; rm -rf /",
                None
            )
            .await
            .is_err()
    );
}
