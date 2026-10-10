//! A chat made with an agent this daemon no longer ships (the Cursor shim,
//! plan DD1 Task 3) still opens, read-only: its stored harness reads as
//! `HarnessId::Unsupported`, and every send answers `harness_unsupported`
//! with the notice "This agent is no longer supported".
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use loams_agentd_doc::{
    MessageRole, SessionCommandEntry, SessionCommandPayload, SessionCommandStatus, WorkspaceDoc,
};
use loams_agentd_harness::mock::MockHarness;
use loams_agentd_proto::{HarnessId, RunRequest, SandboxLevel};
use loams_agentd_rpc::methods;
use loams_agentd_sessions::workspace_host::WORKSPACE_DOC_ID;
use loams_agentd_sessions::{EngineCore, HarnessRegistry};
use loams_agentd_store::DocsStore;

const CHAT: &str = "chat-cursor";

/// The chat row an older build wrote for a Cursor session: its `config`
/// names the harness `"cursor"`.
fn cursor_chat_fixture() -> Vec<u8> {
    let workspace = WorkspaceDoc::new();
    let row = workspace
        .doc()
        .get_map("chats")
        .insert_container(CHAT, loro::LoroMap::new())
        .unwrap();
    row.insert("id", CHAT).unwrap();
    row.insert("deviceId", "device-old").unwrap();
    row.insert("title", "Refactor with Cursor").unwrap();
    row.insert("createdAt", 1_000i64).unwrap();
    let config = serde_json::json!({
        "harness": "cursor",
        "model": "composer-1",
        "reasoning": null,
        "modelOptions": {},
        "sandbox": "workspace-write",
    });
    row.insert("config", loro::LoroValue::from(config)).unwrap();
    workspace.doc().commit();
    workspace.export_snapshot().unwrap()
}

fn run(harness: Option<HarnessId>, message_id: &str) -> SessionCommandPayload {
    SessionCommandPayload::Run {
        request: RunRequest {
            mcp: None,
            prompt: "one more change".into(),
            harness,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "/tmp".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
        },
        message_id: message_id.into(),
    }
}

fn assert_unsupported(error: impl std::fmt::Display) {
    let text = error.to_string();
    assert!(
        text.contains("harness_unsupported") && text.contains("This agent is no longer supported"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn old_cursor_session_opens_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    DocsStore::open(data.join("profiles/local"))
        .unwrap()
        .save_snapshot(WORKSPACE_DOC_ID, &cursor_chat_fixture())
        .unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script: Vec::new() }));
    let core = EngineCore::assemble(&data, Arc::new(registry), HarnessId::Mock).unwrap();
    core.workspace.set_chat_host(CHAT, &core.device_id).unwrap();

    // The chat loads, with its harness read as unsupported.
    let chat = core
        .workspace
        .chat(CHAT)
        .unwrap()
        .expect("the old chat loads");
    assert_eq!(chat.title.as_deref(), Some("Refactor with Cursor"));
    let config = chat.config.expect("the config survives");
    assert_eq!(config.harness, HarnessId::Unsupported);
    assert_eq!(config.model.as_deref(), Some("composer-1"));

    // Every send is refused: a Run (with or without a harness pick), a
    // Steer and a queued message.
    let client = loams_agentd_rpc::memory_client(core.rpc_service());
    for command in [
        run(None, "m-1"),
        run(Some(HarnessId::Mock), "m-2"),
        SessionCommandPayload::Steer {
            prompt: "steer".into(),
            message_id: Some("m-3".into()),
        },
    ] {
        let error = client
            .call(
                methods::QUEUE_COMMAND,
                serde_json::json!({ "chatId": CHAT, "command": command }),
            )
            .await
            .expect_err("a send to an unsupported agent's chat");
        assert_unsupported(error);
    }
    let error = client
        .call(
            methods::QUEUE_MESSAGE,
            serde_json::json!({ "chatId": CHAT, "text": "queued" }),
        )
        .await
        .expect_err("a queued message to an unsupported agent's chat");
    assert_unsupported(error);

    // A Run an older build left pending in the doc is rejected by the drain
    // with the same answer, and nothing runs.
    let handle = core.doc_host.open(CHAT).unwrap();
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-old".into(),
            payload: run(None, "m-old"),
            issued_by: "device-old".into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let resolution = loop {
        let command = handle
            .doc()
            .read_commands()
            .unwrap()
            .into_iter()
            .find(|c| c.id == "cmd-old")
            .unwrap();
        if command.status != SessionCommandStatus::Pending {
            assert_eq!(command.status, SessionCommandStatus::Rejected);
            break command.resolution.unwrap_or_default();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the old Run was never resolved"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_unsupported(resolution);
    assert!(
        !handle
            .doc()
            .read_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.role == MessageRole::Assistant),
        "no agent ran for the unsupported chat"
    );

    core.shutdown().await;
}
