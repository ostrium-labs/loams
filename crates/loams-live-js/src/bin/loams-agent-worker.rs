//! `loams-agent-worker` (SPIKE, MS1 Task 1): `worker` runs the sealed
//! QuickJS agent worker (MS1 Task 2: `loams-agentd agent-worker`);
//! `acp <bundle.js> [--fake-hosts]` is agentd's side as a process: it
//! starts `worker` and serves the bundle's ACP on stdio, so `AcpHarness`
//! spawns it like any ACP agent. Without `--fake-hosts` every host call
//! fails: the spike does not wire agentd's proxies.

use std::process::ExitCode;
use std::sync::Arc;

use loams_live_js::agent::fake::{FakeHosts, fixture_manifest};
use loams_live_js::agent::{
    AgentConfig, AgentHosts, AgentWorker, FetchRequest, FetchResponse, HostFuture, acp,
    agent_worker_main,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Before any runtime or thread: the worker seals itself
        // single-threaded.
        Some("worker") => agent_worker_main(),
        Some("acp") => {
            let Some(bundle) = args.get(1) else {
                eprintln!("usage: loams-agent-worker acp <bundle.js> [--fake-hosts]");
                return ExitCode::FAILURE;
            };
            let fake = args.iter().any(|a| a == "--fake-hosts");
            match front(bundle, fake) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("loams-agent-worker: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: loams-agent-worker worker | acp <bundle.js> [--fake-hosts]");
            ExitCode::FAILURE
        }
    }
}

/// Hosts that refuse everything (not wired to agentd in the spike).
struct Unwired;

impl AgentHosts for Unwired {
    fn fetch(&self, _: FetchRequest) -> HostFuture<FetchResponse> {
        Box::pin(async { Err("fetch is not wired to agentd in the spike".into()) })
    }
    fn model(
        &self,
        _: String,
        _: serde_json::Value,
        _: bool,
        _: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    ) -> HostFuture<serde_json::Value> {
        Box::pin(async { Err("the model proxy is not built (D1454)".into()) })
    }
    fn mcp_list(&self, _: String) -> HostFuture<serde_json::Value> {
        Box::pin(async { Err("MCP is not wired to agentd in the spike".into()) })
    }
    fn mcp_call(
        &self,
        _: String,
        _: String,
        _: serde_json::Value,
    ) -> HostFuture<serde_json::Value> {
        Box::pin(async { Err("MCP is not wired to agentd in the spike".into()) })
    }
    fn token(&self) -> HostFuture<String> {
        Box::pin(async { Err("no Place token in the spike".into()) })
    }
}

fn front(bundle: &str, fake: bool) -> Result<(), String> {
    let source = std::fs::read_to_string(bundle).map_err(|e| format!("{bundle}: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let hosts: Arc<dyn AgentHosts> = if fake {
        Arc::new(FakeHosts::default())
    } else {
        Arc::new(Unwired)
    };
    let mut config = AgentConfig::new(exe);
    if fake {
        config.manifest = fixture_manifest();
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let (worker, from_agent) = AgentWorker::start(&config, &source, hosts).await?;
        acp::serve_stdio(worker, from_agent, tokio::io::stdin(), tokio::io::stdout())
            .await
            .map_err(|e| e.to_string())
    })
}
