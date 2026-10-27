//! The agent under test as an ACP client sees it: a process with a cleared
//! environment in its own process group, spoken to with agentd's own ACP
//! JSON-RPC client (`loams-agentd-harness::jsonrpc`). Permission requests
//! are answered by the manifest (Task 0 ruling 6): a declared tool is
//! allowed once, any other is rejected and recorded. The runner offers no
//! client methods (`fs/*`, `terminal/*`).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loams_agentd_harness::jsonrpc::{Incoming, RpcClient};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::declared_tool;

const STDERR_TAIL: usize = 8 * 1024;
const TEXT_CAP: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub(super) struct PermissionAsked {
    pub seq: u64,
    /// The call's title, else its kind.
    pub name: String,
    /// The declared tool it matched.
    pub declared: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ToolCallSeen {
    pub name: String,
    pub declared: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct AcpLog {
    pub permissions: Vec<PermissionAsked>,
    pub tool_calls: Vec<ToolCallSeen>,
    /// `agent_message_chunk` text, in order (capped).
    pub text: String,
    /// Client methods the agent called that the runner does not offer.
    pub refused_methods: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Agent {
    child: tokio::process::Child,
    pub pid: i32,
    pub rpc: RpcClient,
    log: Arc<Mutex<AcpLog>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The option to pick from a permission request: the first of the kinds
/// given, in order of preference.
fn pick_option<'a>(options: &'a [Value], kinds: &[&str]) -> Option<&'a str> {
    kinds.iter().find_map(|kind| {
        options
            .iter()
            .find(|o| str_of(o, "kind") == *kind)
            .and_then(|o| o.get("optionId").and_then(Value::as_str))
    })
}

impl Agent {
    /// Starts `program args` with exactly `env`, in `cwd`.
    pub(super) fn spawn(
        program: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
        tools: Vec<String>,
        seq: Arc<AtomicU64>,
    ) -> std::io::Result<Self> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or_else(|| std::io::Error::other("the agent has no pid"))?;
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(std::io::Error::other("the agent's stdio is not piped"));
        };
        let (rpc, incoming) = RpcClient::new(stdin, stdout);
        let log = Arc::new(Mutex::new(AcpLog::default()));
        tokio::spawn(pump(incoming, rpc.clone(), Arc::clone(&log), tools, seq));
        let tail = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut tail = lock(&sink);
                tail.extend_from_slice(&buf[..n]);
                let excess = tail.len().saturating_sub(STDERR_TAIL);
                tail.drain(..excess);
            }
        });
        Ok(Self {
            child,
            pid,
            rpc,
            log,
            stderr: tail,
        })
    }

    /// A request with a deadline; the error is plain words.
    pub(super) async fn request(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<(Value, Duration), String> {
        let started = Instant::now();
        match tokio::time::timeout(limit, self.rpc.request(method, params)).await {
            Ok(Ok(value)) => Ok((value, started.elapsed())),
            Ok(Err(error)) => Err(format!("{method} failed: {error}")),
            Err(_) => Err(format!(
                "{method} had no answer within {} ms",
                limit.as_millis()
            )),
        }
    }

    pub(super) fn log(&self) -> std::sync::MutexGuard<'_, AcpLog> {
        lock(&self.log)
    }

    pub(super) fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.stderr)).into_owned()
    }

    /// Whether the agent process has exited.
    pub(super) fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Waits up to `limit` for the agent process to exit.
    pub(super) async fn wait_exit(&mut self, limit: Duration) -> Option<Duration> {
        let started = Instant::now();
        match tokio::time::timeout(limit, self.child.wait()).await {
            Ok(Ok(_)) => Some(started.elapsed()),
            _ => None,
        }
    }

    /// Sends SIGTERM to the agent process only (not its group).
    pub(super) fn terminate(&self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }

    /// Whether any process is left in the agent's process group.
    pub(super) fn group_alive(&self) -> bool {
        rustix::process::Pid::from_raw(self.pid)
            .is_some_and(|pid| rustix::process::test_kill_process_group(pid).is_ok())
    }

    /// Kills the whole process group and reaps the agent.
    pub(super) async fn kill(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

async fn pump(
    mut incoming: tokio::sync::mpsc::Receiver<Incoming>,
    rpc: RpcClient,
    log: Arc<Mutex<AcpLog>>,
    tools: Vec<String>,
    seq: Arc<AtomicU64>,
) {
    while let Some(message) = incoming.recv().await {
        match message {
            Incoming::Request { id, method, params } => {
                if method != "session/request_permission" {
                    lock(&log).refused_methods.push(method.clone());
                    rpc.respond_error(&id, -32601, "the LAPC runner offers no client methods");
                    continue;
                }
                let call = &params["toolCall"];
                let title = str_of(call, "title");
                let kind = str_of(call, "kind");
                let declared = declared_tool(&tools, title, kind).map(str::to_owned);
                let options = params["options"].as_array().cloned().unwrap_or_default();
                let choice = if declared.is_some() {
                    pick_option(&options, &["allow_once", "allow_always"])
                } else {
                    pick_option(&options, &["reject_once", "reject_always"])
                };
                lock(&log).permissions.push(PermissionAsked {
                    seq: seq.fetch_add(1, Ordering::SeqCst),
                    name: if title.is_empty() { kind } else { title }.to_owned(),
                    declared,
                });
                let outcome = match choice {
                    Some(option) => json!({ "outcome": "selected", "optionId": option }),
                    None => json!({ "outcome": "cancelled" }),
                };
                rpc.respond(&id, json!({ "outcome": outcome }));
            }
            Incoming::Notification { method, params } => {
                if method != "session/update" {
                    continue;
                }
                let update = &params["update"];
                match str_of(update, "sessionUpdate") {
                    "agent_message_chunk" => {
                        let mut log = lock(&log);
                        if log.text.len() < TEXT_CAP {
                            let text = str_of(&update["content"], "text");
                            log.text.push_str(text);
                        }
                    }
                    "tool_call" => {
                        let title = str_of(update, "title");
                        let kind = str_of(update, "kind");
                        let declared = declared_tool(&tools, title, kind).map(str::to_owned);
                        lock(&log).tool_calls.push(ToolCallSeen {
                            name: if title.is_empty() { kind } else { title }.to_owned(),
                            declared,
                        });
                    }
                    _ => {}
                }
            }
            Incoming::Eof => break,
        }
    }
}
