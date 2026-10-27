//! SD1 Task 8: the Mock harness's edit knob (`LOAMS_DESKTOP_MOCK_EDIT`) writes
//! files in the run's cwd, so a Studio test sees a real checkout diff.
//!
//! The knob is read from the process environment, as the daemon reads it.
//! This file holds ONE test, so its process sets the variable before anything
//! else reads it.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use loams_agentd_doc::{
    MessageRole, MessageStatus, SessionCommandEntry, SessionCommandPayload, SessionCommandStatus,
};
use loams_agentd_harness::mock::MockHarness;
use loams_agentd_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall};
use loams_agentd_rpc::methods;
use loams_agentd_sessions::{EngineCore, HarnessRegistry};

const CHAT: &str = "chat-mock-edit";

async fn git(cwd: &Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_request(cwd: &Path) -> RunRequest {
    RunRequest {
        mcp_servers: Vec::new(),
        mcp: None,
        prompt: "edit the notes".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

#[tokio::test]
async fn mock_edit_produces_checkout_diff() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: test-only; this file's only test sets these before the engine
    // (or anything else in the process) reads them.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("LOAMS_DESKTOP_MOCK_EDIT", "notes.txt,src/new.txt");
        std::env::set_var("LOAMS_DESKTOP_WORKTREES_DIR", tmp.path().join("worktrees"));
    }

    // A repository with a committed `notes.txt`.
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "initial"]).await;

    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness {
        script: vec![
            AgentEvent::TextDelta {
                text: "Editing the notes.".into(),
            },
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            },
        ],
    }));
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(registry),
        HarnessId::Mock,
    )
    .unwrap();

    let handle = core.doc_host.open(CHAT).unwrap();
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-edit-1".into(),
            payload: SessionCommandPayload::Run {
                request: run_request(&repo),
                message_id: "msg-edit-1".into(),
            },
            issued_by: "viewer-device".into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let complete = core
            .doc_host
            .open(CHAT)
            .ok()
            .and_then(|h| h.doc().read_entries().ok())
            .unwrap_or_default()
            .iter()
            .any(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete));
        if complete {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the run"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // The files were written in the run's cwd.
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
        "first\nmock edit 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("src/new.txt")).unwrap(),
        "mock edit 1\n"
    );

    // The journal carries the matching editFile calls and results.
    let journal = core.sessions.subscribe(CHAT, 0).unwrap().0;
    let edits: Vec<String> = journal
        .iter()
        .filter_map(|j| match &j.event {
            AgentEvent::ToolCall {
                call: ToolCall::EditFile { path, .. },
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(edits, vec!["notes.txt", "src/new.txt"]);
    let results = journal
        .iter()
        .filter(|j| {
            matches!(
                &j.event,
                AgentEvent::ToolResult { id, is_error: false, diff: Some(_), .. }
                    if id.starts_with("mock-edit-")
            )
        })
        .count();
    assert_eq!(results, 2);

    // GetCheckoutDiff over the RPC surface shows both files.
    let client = loams_agentd_rpc::memory_client(core.rpc_service());
    let diff = client
        .call(
            methods::GET_CHECKOUT_DIFF,
            serde_json::json!({ "cwd": repo.to_string_lossy() }),
        )
        .await
        .unwrap();
    let patch = diff["patch"].as_str().unwrap();
    assert!(
        patch.contains("diff --git a/notes.txt b/notes.txt"),
        "{patch}"
    );
    assert!(patch.contains("+mock edit 1"), "{patch}");
    assert!(
        patch.contains("diff --git a/src/new.txt b/src/new.txt"),
        "{patch}"
    );
    let mut files: Vec<(String, String)> = diff["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                f["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            ("notes.txt".to_string(), "modified".to_string()),
            ("src/new.txt".to_string(), "added".to_string()),
        ]
    );
}
