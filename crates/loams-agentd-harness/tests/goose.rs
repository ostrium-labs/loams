//! Goose as a first-party ACP harness (MS1 Task 3, D1809): the launch
//! contract against the fake `goose acp` in `tests/fixtures/fake-goose-acp.sh`
//! (no real goose, no network), plus one `#[ignore]`d run of a real goose
//! (`LOAMS_TEST_GOOSE_BIN`) against a scripted model proxy.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod approvals_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use loams_agentd_harness::acp::GooseEnvironment;
use loams_agentd_harness::{AcpHarness, CancellationToken, Harness, RunControls, SteerMessage};
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, McpServer, RunRequest, SandboxLevel, SteeringMode,
    UserInputAnswer,
};

const PROXY: &str = "http://127.0.0.1:7801";

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-goose-acp.sh");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn harness(dir: &Path, instructions: Option<&str>) -> AcpHarness {
    AcpHarness::goose()
        .with_executable(fixture())
        .with_goose_environment(GooseEnvironment {
            dir: Some(dir.to_path_buf()),
            model_proxy: Some(PROXY.into()),
            instructions: instructions.map(str::to_owned),
        })
}

fn request(prompt: &str, cwd: &Path) -> RunRequest {
    RunRequest {
        mcp_servers: Vec::new(),
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Goose),
        // Not a `gpt-5`/`o*` id: goose sends those to the Responses API
        // whatever `OPENAI_BASE_PATH` says (verified, 1.54.0).
        model: Some("loams-test".into()),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        request_approval: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec!["Yes".into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
    };
    (controls, steer_tx, token)
}

async fn run_to_done(
    harness: &AcpHarness,
    req: RunRequest,
    controls: RunControls,
) -> Vec<AgentEvent> {
    let mut stream = harness.run(req, controls).await.expect("run starts");
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("the turn finishes")
}

fn record(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join("record").join(name))
        .unwrap_or_else(|e| panic!("record/{name}: {e}"))
}

fn recorded_env(dir: &Path) -> BTreeMap<String, String> {
    record(dir, "env")
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        // What the fixture's own shell adds.
        .filter(|(k, _)| !matches!(k.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_"))
        .collect()
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn goose_spec_launches_goose_acp() {
    let harness = AcpHarness::goose();
    assert_eq!(harness.id(), HarnessId::Goose);
    assert_eq!(harness.display_name(), "Goose");
    assert_eq!(harness.steering_mode(), SteeringMode::TurnBoundary);
    assert!(harness.reasoning_levels().is_empty());
    assert_eq!(
        serde_json::to_value(HarnessId::Goose).unwrap(),
        serde_json::json!("goose")
    );
}

#[tokio::test]
async fn goose_run_is_private_and_passes_mcp_servers() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let harness = harness(dir.path(), Some("You are the Loams release agent."));
    let mut req = request("hello", work.path());
    req.mcp = Some(McpServer {
        name: "loams-desktop".into(),
        command: "/opt/loams/loams-agentd".into(),
        args: vec!["mcp".into()],
        env: [("LOAMS_AGENTD_RUN_TOKEN".to_owned(), "tok-123".to_owned())]
            .into_iter()
            .collect(),
    });
    let (controls, _steer, _token) = controls();
    let events = run_to_done(&harness, req, controls).await;
    assert_eq!(text(&events), "hello from goose", "{events:?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )),
        "{events:?}"
    );

    // `goose acp`, nothing else.
    assert_eq!(record(dir.path(), "args").trim(), "acp");

    // goose_env_is_private: exactly the contract's variables reach goose.
    let env = recorded_env(dir.path());
    let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
    keys.retain(|k| *k != "LANG");
    assert_eq!(
        keys,
        vec![
            "CONTEXT_FILE_NAMES",
            "GOOSE_DISABLE_KEYRING",
            "GOOSE_DISABLE_SESSION_NAMING",
            "GOOSE_MODE",
            "GOOSE_MODEL",
            "GOOSE_PATH_ROOT",
            "GOOSE_PROVIDER",
            "GOOSE_TELEMETRY_ENABLED",
            "HOME",
            "OPENAI_API_KEY",
            "OPENAI_BASE_PATH",
            "OPENAI_HOST",
            "PATH",
            "TMPDIR",
        ]
    );
    let goose_dir = dir.path().display().to_string();
    assert_eq!(env["HOME"], format!("{goose_dir}/home"));
    assert_eq!(env["GOOSE_PATH_ROOT"], format!("{goose_dir}/root"));
    assert_eq!(env["GOOSE_MODE"], "approve");
    assert_eq!(env["GOOSE_MODEL"], "loams-test");
    assert_eq!(env["OPENAI_HOST"], format!("{PROXY}/openai"));
    assert_eq!(env["OPENAI_API_KEY"], "loams-model-proxy");
    let fixtures = fixture().parent().unwrap().display().to_string();
    assert!(
        env["PATH"].starts_with(&format!("{fixtures}:")),
        "{}",
        env["PATH"]
    );

    // The persona reaches goose's global hints file.
    assert_eq!(
        record(dir.path(), "hints"),
        "You are the Loams release agent.\n"
    );

    // goose_session_new_passes_mcp_servers: the MCP shim and its run token.
    let session_new: serde_json::Value =
        serde_json::from_str(record(dir.path(), "session_new").trim()).unwrap();
    let params = &session_new["params"];
    assert_eq!(params["cwd"], work.path().display().to_string());
    assert_eq!(
        params["mcpServers"],
        serde_json::json!([{
            "name": "loams-desktop",
            "command": "/opt/loams/loams-agentd",
            "args": ["mcp"],
            "env": [{ "name": "LOAMS_AGENTD_RUN_TOKEN", "value": "tok-123" }],
        }])
    );
}

/// goose runs in `approve` mode, so a tool call asks over
/// `session/request_permission`, and a supervised thread's approval decides.
#[tokio::test]
async fn goose_permission_request_maps_to_approval() {
    use loams_agentd_proto::approvals::ApprovalDecision;

    for (decision, expected, option) in [
        (ApprovalDecision::Once, "tool allowed", "allow_once"),
        (ApprovalDecision::Deny, "tool rejected", "reject_once"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let harness = harness(dir.path(), None);
        let (mut controls, _steer, _token) = controls();
        let (bridge, mut asks) = approvals_support::bridge();
        controls.request_approval = Some(bridge);
        let mut stream = harness
            .run(request("scenario:permission", work.path()), controls)
            .await
            .unwrap();
        let (ask, answer) = approvals_support::next_ask(&mut asks).await;
        assert_eq!(ask.call_id, "call_x");
        assert_eq!(ask.tool, "shell");
        assert_eq!(ask.args["rawInput"]["command"], "rm -rf build");
        answer.send(decision).unwrap();
        let events = tokio::time::timeout(Duration::from_secs(20), async {
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let done = matches!(event, AgentEvent::Done { .. });
                events.push(event);
                if done {
                    break;
                }
            }
            events
        })
        .await
        .unwrap();
        assert!(text(&events).contains(expected), "{decision:?}: {events:?}");
        assert!(
            record(dir.path(), "permission_answer").contains(&format!("\"optionId\":\"{option}\"")),
            "{decision:?}"
        );
        // No persona configured: no hints file.
        assert!(!dir.path().join("root/config/.goosehints").exists());
    }
}

#[tokio::test]
async fn goose_unselectable_without_model_proxy() {
    if std::env::var_os(loams_agentd_harness::acp::MODEL_PROXY_ENV).is_some() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let harness = AcpHarness::goose()
        .with_executable(fixture())
        .with_goose_environment(GooseEnvironment {
            dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        });
    assert!(!harness.installed(), "listed but not selectable");
    let (controls, _steer, _token) = controls();
    let error = match harness.run(request("hello", dir.path()), controls).await {
        Ok(_) => panic!("a run without the model proxy must not start"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("model proxy"), "{error}");
    assert!(!dir.path().join("record").exists(), "goose never launched");

    // With the proxy, the same harness is installed.
    assert!(
        harness
            .with_goose_environment(GooseEnvironment {
                dir: Some(dir.path().to_path_buf()),
                model_proxy: Some(PROXY.into()),
                instructions: None,
            })
            .installed()
    );
}

/// A scripted OpenAI-compatible model proxy: every chat completion answers
/// "hello from the proxy"; requests are recorded.
async fn scripted_proxy() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0; 8192];
                let header_end = loop {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < header_end + length {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                let request_line = head.lines().next().unwrap_or_default().to_owned();
                log.lock().unwrap().push(format!("{request_line}\n{body}"));
                let (content_type, payload) = if request_line.starts_with("GET") {
                    (
                        "application/json",
                        r#"{"object":"list","data":[{"id":"loams-test","object":"model"}]}"#
                            .to_owned(),
                    )
                } else if body.contains("please-run-tool") && !body.contains("\"tool_call_id\"") {
                    // First round of the tool scenario: call developer's shell.
                    let call = r#"{"id":"c2","object":"chat.completion.chunk","created":1,"model":"loams-test","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"shell","arguments":"{\"command\":\"echo hi\"}"}}]},"finish_reason":null}]}"#;
                    let stop = r#"{"id":"c2","object":"chat.completion.chunk","created":1,"model":"loams-test","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#;
                    (
                        "text/event-stream",
                        format!("data: {call}\n\ndata: {stop}\n\ndata: [DONE]\n\n"),
                    )
                } else if body.contains("\"stream\":true") {
                    let delta = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"loams-test","choices":[{"index":0,"delta":{"role":"assistant","content":"hello from the proxy"},"finish_reason":null}]}"#;
                    let stop = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"loams-test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}}"#;
                    (
                        "text/event-stream",
                        format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n"),
                    )
                } else {
                    (
                        "application/json",
                        r#"{"id":"c1","object":"chat.completion","created":1,"model":"loams-test","choices":[{"index":0,"message":{"role":"assistant","content":"hello from the proxy"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}}"#.to_owned(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (url, seen)
}

/// A real `goose acp` (set `LOAMS_TEST_GOOSE_BIN` to the pinned 1.54.0
/// build) against the scripted proxy: the reply streams back, the requests
/// reach the proxy with the dummy key, and the persona is in the prompt.
#[tokio::test]
#[ignore = "needs a real goose binary in LOAMS_TEST_GOOSE_BIN"]
async fn real_goose_acp_against_a_scripted_proxy() {
    let Some(bin) = std::env::var_os("LOAMS_TEST_GOOSE_BIN") else {
        eprintln!("LOAMS_TEST_GOOSE_BIN is not set; skipping");
        return;
    };
    let (proxy, seen) = scripted_proxy().await;
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let harness = AcpHarness::goose()
        .with_executable(PathBuf::from(bin))
        .with_goose_environment(GooseEnvironment {
            dir: Some(dir.path().to_path_buf()),
            model_proxy: Some(proxy),
            instructions: Some("Always sign off as LOAMS-PERSONA-MARKER.".into()),
        });
    let (controls, _steer, _token) = controls();
    let events = run_to_done(&harness, request("say hello", work.path()), controls).await;
    assert!(text(&events).contains("hello from the proxy"), "{events:?}");
    let seen = seen.lock().unwrap().join("\n---\n");
    assert!(
        seen.contains("POST /openai/v1/chat/completions"),
        "the provider reached the proxy: {seen}"
    );
    assert!(
        seen.contains("LOAMS-PERSONA-MARKER"),
        "the hints reached the prompt"
    );
    assert!(
        dir.path().join("root").is_dir(),
        "goose kept its state under GOOSE_PATH_ROOT"
    );
}
