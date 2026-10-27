//! The agent profile (SPIKE, MS1 Task 1, D1807): the fixtures under
//! `testdata/agents/` in sealed workers, driven over ACP, with the scripted
//! hosts of `agent::fake` standing in for agentd's model proxy, MCP shim
//! and egress proxy.

#![cfg(all(feature = "agent", target_os = "linux"))]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use loams_live_js::agent::fake::{FakeHosts, MODEL_TEXT, fixture_manifest};
use loams_live_js::agent::{AgentConfig, AgentWorker, ProbeKind};
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/testdata/agents/{name}/agent.js",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn config() -> AgentConfig {
    let mut c = AgentConfig::new(env!("CARGO_BIN_EXE_loams-agent-worker"));
    c.manifest = fixture_manifest();
    c
}

/// An ACP client over a worker, as `AcpHarness` would speak it.
struct Client {
    worker: AgentWorker,
    rx: mpsc::UnboundedReceiver<String>,
    next: u64,
    /// `session/update` notifications seen so far.
    updates: Vec<Value>,
}

impl Client {
    async fn start(name: &str, hosts: &FakeHosts) -> Self {
        Self::start_with(&config(), &fixture(name), hosts).await
    }

    async fn start_with(config: &AgentConfig, bundle: &str, hosts: &FakeHosts) -> Self {
        let (worker, rx) = AgentWorker::start(config, bundle, Arc::new(hosts.clone()))
            .await
            .expect("the agent loads");
        Client {
            worker,
            rx,
            next: 1,
            updates: Vec::new(),
        }
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.worker.acp_send(message.to_string());
        id
    }

    fn notify(&self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.worker.acp_send(message.to_string());
    }

    /// Waits for the response to `id`, keeping notifications.
    async fn response(&mut self, id: u64) -> Value {
        let deadline = Duration::from_secs(10);
        loop {
            let line = tokio::time::timeout(deadline, self.rx.recv())
                .await
                .expect("an ACP message in time")
                .unwrap_or_else(|| panic!("the agent went away: {:?}", self.worker.dead()));
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id && message.get("method").is_none() {
                return message;
            }
            if message["method"] == "session/update" {
                self.updates.push(message["params"]["update"].clone());
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.response(id).await
    }

    /// `initialize` and `session/new`; returns the session id.
    async fn open(&mut self) -> String {
        let init = self
            .request("initialize", json!({
                "protocolVersion": 1,
                "clientInfo": { "name": "loams-desktop", "version": "0" },
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            }))
            .await;
        assert_eq!(init["result"]["protocolVersion"], 1, "{init}");
        let new = self
            .request("session/new", json!({ "cwd": "/tmp", "mcpServers": [] }))
            .await;
        new["result"]["sessionId"].as_str().unwrap().to_string()
    }

    /// One prompt turn; returns the response and the text said during it.
    async fn prompt(&mut self, session: &str, text: &str) -> (Value, String) {
        self.updates.clear();
        let response = self
            .request(
                "session/prompt",
                json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }),
            )
            .await;
        (response, self.said())
    }

    fn said(&self) -> String {
        self.updates
            .iter()
            .filter(|u| u["sessionUpdate"] == "agent_message_chunk")
            .map(|u| u["content"]["text"].as_str().unwrap_or_default())
            .collect()
    }
}

fn stop_reason(response: &Value) -> &str {
    response["result"]["stopReason"]
        .as_str()
        .unwrap_or_else(|| panic!("no stop reason: {response}"))
}

#[tokio::test]
async fn echo_agent_answers_acp_initialize() {
    let mut c = Client::start("echo-acp", &FakeHosts::default()).await;
    assert_eq!(c.worker.name(), "echo-acp");
    let init = c
        .request("initialize", json!({ "protocolVersion": 1 }))
        .await;
    assert_eq!(init["result"]["agentInfo"]["name"], "echo-acp");
}

#[tokio::test]
async fn echo_agent_completes_a_prompt_turn() {
    let mut c = Client::start("echo-acp", &FakeHosts::default()).await;
    let session = c.open().await;
    for round in 0..3 {
        let (response, said) = c.prompt(&session, &format!("hello {round}")).await;
        assert_eq!(stop_reason(&response), "end_turn");
        assert_eq!(said, format!("echo: hello {round}"));
    }
}

#[tokio::test]
async fn acp_sdk_agent_answers_initialize() {
    let mut c = Client::start("acp-sdk", &FakeHosts::default()).await;
    let init = c
        .request(
            "initialize",
            json!({ "protocolVersion": 1, "clientCapabilities": {} }),
        )
        .await;
    assert_eq!(init["result"]["protocolVersion"], 1, "{init}");
    let session = c.open().await;
    assert_eq!(session.len(), 36, "a crypto.randomUUID(): {session}");
    let (response, said) = c.prompt(&session, "hi").await;
    assert_eq!(stop_reason(&response), "end_turn");
    assert_eq!(said, "sdk: hi");
}

#[tokio::test]
async fn agent_model_call_reaches_scripted_proxy() {
    let hosts = FakeHosts::default();
    let mut c = Client::start("tool-agent", &hosts).await;
    let session = c.open().await;
    let (response, said) = c.prompt(&session, "Weather in Paris?").await;
    assert_eq!(stop_reason(&response), "end_turn");
    assert!(said.starts_with(MODEL_TEXT), "{said}");
    // The model's words arrive one chunk each.
    let chunks = c
        .updates
        .iter()
        .take_while(|u| u["sessionUpdate"] == "agent_message_chunk")
        .count();
    assert_eq!(chunks, MODEL_TEXT.split_inclusive(' ').count());
    let (kind, detail) = hosts.seen.lock().unwrap()[0].clone();
    assert_eq!(
        (kind.as_str(), detail),
        ("model", json!({ "api": "chat", "stream": true }))
    );
}

#[tokio::test]
async fn agent_mcp_call_reaches_shim() {
    let hosts = FakeHosts::default();
    let mut c = Client::start("tool-agent", &hosts).await;
    let session = c.open().await;
    let (_, said) = c.prompt(&session, "Weather in Paris?").await;
    assert!(
        said.contains("weather/get_forecast: Sunny, 21°C in Paris"),
        "{said}"
    );
    let tool = c
        .updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(tool["title"], "weather/get_forecast");
    let done = c
        .updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call_update")
        .unwrap();
    assert_eq!(done["status"], "completed");
    assert_eq!(hosts.kinds(), ["model", "mcp.call", "fetch"]);
}

#[tokio::test]
async fn agent_undeclared_mcp_tool_hidden() {
    let hosts = FakeHosts::default();
    let mut c = Client::start("tool-agent", &hosts).await;
    let session = c.open().await;
    let (_, said) = c.prompt(&session, "list").await;
    assert_eq!(
        said, "get_forecast",
        "the shim lists delete_city too; agentd hides it"
    );
    let (_, said) = c.prompt(&session, "call delete_city").await;
    assert!(said.contains("not declared"), "{said}");
    assert_eq!(
        hosts.kinds(),
        ["mcp.list"],
        "the undeclared call never reached the shim"
    );
}

#[tokio::test]
async fn agent_fetch_goes_through_egress_proxy() {
    let hosts = FakeHosts::default();
    let mut c = Client::start("tool-agent", &hosts).await;
    let session = c.open().await;
    let (_, said) = c
        .prompt(&session, "fetch https://api.example.com/v1/items")
        .await;
    assert!(said.starts_with("fetched 200"), "{said}");
    let (kind, detail) = hosts.seen.lock().unwrap()[0].clone();
    assert_eq!(kind, "fetch");
    assert_eq!(
        detail,
        json!({ "url": "https://api.example.com/v1/items", "method": "POST", "bytes": 17 })
    );
    // A 3 MiB body arrives in 1 MiB frames.
    let (_, said) = c
        .prompt(&session, "fetch https://api.example.com/bytes/3145728")
        .await;
    assert_eq!(said, "fetched 200 3145728 bytes");
}

#[tokio::test]
async fn agent_fetch_undeclared_host_refused() {
    let hosts = FakeHosts::default();
    let mut c = Client::start("tool-agent", &hosts).await;
    let session = c.open().await;
    for url in [
        "https://evil.example.org/steal",
        "https://api.example.com.evil.org/",
        "https://user@evil.example.org:443/",
        "http://127.0.0.1:8080/",
    ] {
        let (_, said) = c.prompt(&session, &format!("fetch {url}")).await;
        assert!(said.contains("egress refused"), "{url}: {said}");
    }
    assert!(hosts.kinds().is_empty(), "nothing reached the egress proxy");
}

#[tokio::test]
async fn agent_timers_fire_in_order() {
    let mut c = Client::start("tool-agent", &FakeHosts::default()).await;
    let session = c.open().await;
    let (_, said) = c.prompt(&session, "timers").await;
    assert_eq!(said, "tick1,tick2,t40,micro,t5");
}

#[tokio::test]
async fn agent_token_entropy_and_config_are_host_supplied() {
    let mut c = Client::start("tool-agent", &FakeHosts::default()).await;
    let session = c.open().await;
    let (_, said) = c.prompt(&session, "token").await;
    assert_eq!(said, "place-token-fake");
    let (_, said) = c.prompt(&session, "random").await;
    let v: Value = serde_json::from_str(&said).unwrap();
    assert_eq!(v["uuid"].as_str().unwrap().len(), 36);
    assert_eq!(v["r"], true);
}

#[tokio::test]
async fn agent_cancel_stops_turn() {
    let mut c = Client::start("tool-agent", &FakeHosts::default()).await;
    let session = c.open().await;
    let id = c.send(
        "session/prompt",
        json!({ "sessionId": session, "prompt": [{ "type": "text", "text": "hang" }] }),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    c.notify("session/cancel", json!({ "sessionId": session }));
    let response = c.response(id).await;
    assert_eq!(stop_reason(&response), "cancelled");
    assert_eq!(c.said(), "waiting");
    // The agent serves the next turn.
    let (response, said) = c.prompt(&session, "token").await;
    assert_eq!(stop_reason(&response), "end_turn");
    assert_eq!(said, "place-token-fake");
}

#[tokio::test]
async fn agent_sigterm_exits_within_10s() {
    let c = Client::start("echo-acp", &FakeHosts::default()).await;
    let started = Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &c.worker.pid().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let reason = tokio::time::timeout(Duration::from_secs(10), c.worker.wait())
        .await
        .unwrap();
    assert!(reason.contains("exited"), "{reason}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn agent_hostile_javascript_finds_no_escape_hatch() {
    let mut c = Client::start("hostile", &FakeHosts::default()).await;
    let session = c.open().await;
    let (_, said) = c.prompt(&session, "probe").await;
    let expected = [
        "eval!ReferenceError",
        "Function!EvalError",
        "ctorChain!EvalError",
        "asyncCtor!EvalError",
        "require=undefined",
        "process=undefined",
        "WebAssembly=undefined",
        "std=undefined/undefined",
    ];
    for e in expected {
        assert!(said.contains(e), "{e} in {said}");
    }
    for module in ["node:fs", "fs", "qjs:os", "os", "std", "loams:server"] {
        assert!(
            said.contains(&format!("import({module})!")),
            "{module}: {said}"
        );
    }
}

async fn probe_kills(kind: ProbeKind) {
    let c = Client::start("echo-acp", &FakeHosts::default()).await;
    assert_eq!(
        c.worker.probe(ProbeKind::PROBE_KIND_PING).await,
        Ok(String::new())
    );
    // Killed by `SIGSYS` before it could answer. (A core dump can hold
    // the pipes open past the watchdog's limit, so either reason counts.)
    let err = c.worker.probe(kind).await.unwrap_err();
    assert!(
        err.contains("exited") || err.contains("CPU slice"),
        "{kind:?}: {err}"
    );
}

#[tokio::test]
async fn agent_worker_has_no_socket() {
    probe_kills(ProbeKind::PROBE_KIND_OPEN_SOCKET).await;
}

#[tokio::test]
async fn agent_worker_cannot_open_files() {
    probe_kills(ProbeKind::PROBE_KIND_OPEN_FILE).await;
}

#[tokio::test]
async fn agent_worker_cannot_spawn_thread() {
    probe_kills(ProbeKind::PROBE_KIND_SPAWN_THREAD).await;
}

#[tokio::test]
async fn agent_worker_cannot_exec() {
    probe_kills(ProbeKind::PROBE_KIND_EXEC).await;
}

fn limited() -> AgentConfig {
    let mut c = config();
    c.cpu_slice = Duration::from_millis(200);
    c.kill_grace = Duration::from_millis(300);
    c.memory_limit = 32 << 20;
    c
}

/// Sends a hostile prompt; returns why the worker died and when.
async fn hostile(command: &str) -> (String, Duration) {
    let mut c = Client::start_with(&limited(), &fixture("hostile"), &FakeHosts::default()).await;
    let session = c.open().await;
    let started = Instant::now();
    c.send(
        "session/prompt",
        json!({ "sessionId": session, "prompt": [{ "type": "text", "text": command }] }),
    );
    let reason = tokio::time::timeout(Duration::from_secs(10), c.worker.wait())
        .await
        .unwrap();
    (reason, started.elapsed())
}

#[tokio::test]
async fn agent_spin_loop_killed() {
    let (reason, took) = hostile("spin").await;
    assert!(reason.contains("over its cpu limit"), "{reason}");
    assert!(took < Duration::from_millis(450), "{took:?}");
    eprintln!("spin loop: stopped by the interrupt handler after {took:?}");
}

#[tokio::test]
async fn agent_c_builtin_killed_by_the_watchdog() {
    // QuickJS's Array.prototype.reverse loops in C without polling the
    // interrupt handler; on an array-like of length 2^53 - 1 it never ends.
    let (reason, took) = hostile("builtin").await;
    assert!(reason.contains("ran past its CPU slice"), "{reason}");
    assert!(took >= Duration::from_millis(500), "{took:?}");
    eprintln!("C built-in: killed by the watchdog after {took:?}");
}

#[tokio::test]
async fn agent_memory_limit_enforced() {
    let (reason, _) = hostile("memory").await;
    assert!(reason.contains("over its memory limit"), "{reason}");
}

#[tokio::test]
async fn ai_sdk_generate_text_through_the_egress_proxy() {
    // The AI SDK runs only with a bundled Web-streams polyfill (QuickJS has
    // no TransformStream); its provider's fetch reaches the model proxy's
    // URL through the egress proxy.
    let hosts = FakeHosts::default();
    let mut c = Client::start("ai-sdk", &hosts).await;
    let session = c.open().await;
    let (response, said) = c.prompt(&session, "weather?").await;
    assert_eq!(stop_reason(&response), "end_turn", "{response}");
    assert_eq!(said, "It is sunny in Paris.");
    assert_eq!(hosts.kinds(), ["fetch"]);
}
