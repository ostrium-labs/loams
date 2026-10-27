//! agentd's side of an agent worker (SPIKE): spawns and drives one sealed
//! worker, answers its host calls through [`AgentHosts`] under the
//! install's [`Manifest`], runs its timers, feeds its entropy pool, and
//! keeps the busy watchdog.
//!
//! Every host call is answered here, on agentd's side, never in the
//! worker. In the spike the [`AgentHosts`] are fakes ([`super::fake`]); in
//! MS1 Task 2 they are agentd's egress proxy (fetch), model proxy (D1454)
//! and MCP shim (D1468). The manifest checks here are what the egress
//! proxy and the MCP shim enforce there.
//!
//! The watchdog: every frame sent to the worker starts (or extends) a busy
//! period, and the worker writes `AgentIdle` each time it has drained its
//! job queue and blocks on stdin. A worker busy for longer than its CPU
//! slice plus [`AgentConfig::kill_grace`] is killed. A turn's wall time is
//! not bounded by this: only uninterrupted JavaScript is.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use loams_live_proto::loams::live::worker::v1 as wire;
use serde_json::{Value, json};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use super::worker::MAX_CHUNK_BYTES;
use crate::ipc::{self, FrameError, ProbeKind, ToHost, ToWorker};

/// A host call's future.
pub type HostFuture<T> = BoxFuture<'static, Result<T, String>>;

/// A `fetch` the agent made, as agentd's egress proxy receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The egress proxy's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub url: String,
    pub body: Vec<u8>,
}

/// What answers an agent's host calls: agentd's egress proxy, model proxy
/// and MCP shim, and the Place token (D1453).
pub trait AgentHosts: Send + Sync + 'static {
    fn fetch(&self, request: FetchRequest) -> HostFuture<FetchResponse>;
    /// `api` is `chat`, `responses` or `messages`. A streaming call sends
    /// its events to `events` before it resolves.
    fn model(
        &self,
        api: String,
        request: Value,
        stream: bool,
        events: mpsc::UnboundedSender<Value>,
    ) -> HostFuture<Value>;
    /// `tools/list`: `{ "tools": [{ "name", … }] }`.
    fn mcp_list(&self, server: String) -> HostFuture<Value>;
    /// `tools/call`.
    fn mcp_call(&self, server: String, tool: String, args: Value) -> HostFuture<Value>;
    fn token(&self) -> HostFuture<String>;
}

/// What the install's manifest allows (D1468): the hosts `fetch` may reach
/// and the MCP tools (`server/tool`) the agent may see and call.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Host names; `*.example.com` matches subdomains.
    pub egress_hosts: Vec<String>,
    pub mcp_tools: Vec<String>,
}

impl Manifest {
    fn allows_host(&self, url: &str) -> Result<(), String> {
        let host = url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or_default()
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('@')
            .next()
            .unwrap_or_default();
        let host = host.rsplit_once(':').map_or(host, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                host
            }
        });
        let host = host.to_ascii_lowercase();
        let ok = self
            .egress_hosts
            .iter()
            .any(|allowed| match allowed.strip_prefix("*.") {
                Some(suffix) => host.ends_with(&format!(".{suffix}")),
                None => *allowed == host,
            });
        if ok {
            Ok(())
        } else {
            Err(format!(
                "egress refused: {host:?} is not a host this agent declared"
            ))
        }
    }

    fn allows_tool(&self, server: &str, tool: &str) -> bool {
        self.mcp_tools
            .iter()
            .any(|t| *t == format!("{server}/{tool}"))
    }
}

/// How to start an agent worker.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The program and arguments that run [`super::agent_worker_main`].
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The runtime's memory limit (the allocator's, uncatchable).
    pub memory_limit: u64,
    /// The most interpreter time one dispatch may take.
    pub cpu_slice: Duration,
    /// How long past `cpu_slice` the watchdog waits before it kills.
    pub kill_grace: Duration,
    pub manifest: Manifest,
    /// What `loams.config()` returns.
    pub config: BTreeMap<String, String>,
}

impl AgentConfig {
    /// `program worker` with the spike's defaults (64 MiB, 1 s, 500 ms).
    pub fn new(program: impl Into<PathBuf>) -> Self {
        AgentConfig {
            program: program.into(),
            args: vec!["worker".into()],
            memory_limit: 64 << 20,
            cpu_slice: Duration::from_secs(1),
            kill_grace: Duration::from_millis(500),
            manifest: Manifest::default(),
            config: BTreeMap::new(),
        }
    }
}

struct Busy {
    outstanding: u64,
    since: Instant,
}

struct Inner {
    tx: mpsc::UnboundedSender<ToWorker>,
    acp: mpsc::UnboundedSender<String>,
    calls: Mutex<HashMap<u64, AbortHandle>>,
    timers: Mutex<HashMap<u64, AbortHandle>>,
    uploads: Mutex<HashMap<u64, (wire::FetchCall, Vec<u8>)>>,
    busy: Mutex<Busy>,
    probe: Mutex<Option<oneshot::Sender<String>>>,
    dead: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    hosts: Arc<dyn AgentHosts>,
    manifest: Manifest,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One running agent worker. Dropping every clone kills it.
#[derive(Clone)]
pub struct AgentWorker {
    inner: Arc<Inner>,
    name: String,
    version: String,
    pid: u32,
}

impl std::fmt::Debug for AgentWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentWorker")
            .field("name", &self.name)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

/// 32 bytes from the OS (agentd's side is not sealed).
fn os_entropy() -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf
}

impl AgentWorker {
    /// Spawns a worker, loads `bundle` and waits for `AgentLoaded`. The
    /// receiver yields the agent's ACP messages, one JSON text each.
    pub async fn start(
        config: &AgentConfig,
        bundle: &str,
        hosts: Arc<dyn AgentHosts>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<String>), String> {
        let mut child = Command::new(&config.program)
            .args(&config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawning the agent worker: {e}"))?;
        let pid = child.id().unwrap_or_default();
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let load = wire::AgentLoad {
            bundle: bundle.as_bytes().to_vec(),
            memory_limit: config.memory_limit,
            cpu_slice_ns: u64::try_from(config.cpu_slice.as_nanos()).unwrap_or(u64::MAX),
            entropy: os_entropy(),
            config: config.config.clone().into_iter().collect(),
            ..Default::default()
        };
        ipc::write_frame_async(
            &mut stdin,
            &ipc::to_worker(ToWorker::AgentLoad(Box::new(load))),
        )
        .await
        .map_err(|e| format!("sending AgentLoad: {e}"))?;
        let (name, version) = loop {
            match recv(&mut stdout).await? {
                ToHost::AgentLoaded(l) if !l.error.is_empty() => {
                    return Err(format!("the agent failed to load: {}", l.error));
                }
                ToHost::AgentLoaded(l) => break (l.name, l.version),
                ToHost::Log(l) => log(&l),
                other => return Err(format!("unexpected frame before AgentLoaded: {other:?}")),
            }
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let (acp, acp_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            tx,
            acp,
            calls: Mutex::default(),
            timers: Mutex::default(),
            uploads: Mutex::default(),
            busy: Mutex::new(Busy {
                outstanding: 0,
                since: Instant::now(),
            }),
            probe: Mutex::new(None),
            dead: Mutex::new(None),
            child: Mutex::new(Some(child)),
            hosts,
            manifest: config.manifest.clone(),
        });
        tokio::spawn(writer(Arc::downgrade(&inner), stdin, rx));
        tokio::spawn(reader(Arc::downgrade(&inner), stdout));
        tokio::spawn(watchdog(
            Arc::downgrade(&inner),
            config.cpu_slice + config.kill_grace,
        ));
        Ok((
            AgentWorker {
                inner,
                name,
                version,
                pid,
            },
            acp_rx,
        ))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// The worker's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Sends one ACP message (a JSON text) to the agent.
    pub fn acp_send(&self, json: String) {
        self.inner.send(ToWorker::Acp(Box::new(wire::AcpMessage {
            json,
            ..Default::default()
        })));
    }

    /// Makes the worker attempt a system call. Inside the seal it is
    /// killed, and the error says how.
    #[doc(hidden)]
    pub async fn probe(&self, kind: ProbeKind) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        *lock(&self.inner.probe) = Some(tx);
        self.inner.send(ToWorker::Probe(Box::new(wire::Probe {
            kind: kind.into(),
            ..Default::default()
        })));
        match rx.await {
            Ok(error) => Ok(error),
            Err(_) => Err(self.dead().unwrap_or_else(|| "the worker went away".into())),
        }
    }

    /// Why the worker is gone, if it is.
    pub fn dead(&self) -> Option<String> {
        lock(&self.inner.dead).clone()
    }

    /// Waits until the worker is gone; returns why.
    pub async fn wait(&self) -> String {
        loop {
            if let Some(reason) = self.dead() {
                return reason;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Kills the worker.
    pub fn kill(&self) {
        self.inner.die("killed by agentd".into());
    }
}

impl Inner {
    fn send(&self, message: ToWorker) {
        let _ = self.tx.send(message);
    }

    fn reply(&self, id: u64, answer: wire::__buffa::oneof::agent_reply::Answer) {
        self.send(ToWorker::AgentReply(Box::new(wire::AgentReply {
            id,
            answer: Some(answer),
            ..Default::default()
        })));
    }

    fn chunk(&self, id: u64, data: Vec<u8>, last: bool) {
        self.send(ToWorker::BodyChunk(Box::new(wire::BodyChunk {
            id,
            data,
            last,
            ..Default::default()
        })));
    }

    /// Marks the worker dead (first reason wins) and kills it.
    fn die(&self, reason: String) {
        {
            let mut dead = lock(&self.dead);
            if dead.is_none() {
                *dead = Some(reason);
            }
        }
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.start_kill();
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        for (_, h) in lock(&self.calls).drain() {
            h.abort();
        }
        for (_, h) in lock(&self.timers).drain() {
            h.abort();
        }
        lock(&self.probe).take();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.start_kill();
        }
    }
}

async fn recv(stdout: &mut ChildStdout) -> Result<ToHost, String> {
    match ipc::read_frame_async::<wire::WorkerMessage>(stdout).await {
        Ok(m) => m
            .message
            .ok_or_else(|| "the worker sent an empty frame".into()),
        Err(FrameError::Closed) => Err("the agent worker exited".into()),
        Err(e) => Err(format!("the agent worker {e}")),
    }
}

async fn writer(
    inner: Weak<Inner>,
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<ToWorker>,
) {
    while let Some(message) = rx.recv().await {
        let Some(i) = inner.upgrade() else { return };
        {
            let mut busy = lock(&i.busy);
            if busy.outstanding == 0 {
                busy.since = Instant::now();
            }
            busy.outstanding += 1;
        }
        drop(i);
        if let Err(e) = ipc::write_frame_async(&mut stdin, &ipc::to_worker(message)).await {
            if let Some(i) = inner.upgrade() {
                i.die(format!("the agent worker's stdin closed: {e}"));
            }
            return;
        }
    }
}

async fn watchdog(inner: Weak<Inner>, limit: Duration) {
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        tick.tick().await;
        let Some(inner) = inner.upgrade() else { return };
        if lock(&inner.dead).is_some() {
            return;
        }
        let stuck = {
            let busy = lock(&inner.busy);
            busy.outstanding > 0 && busy.since.elapsed() > limit
        };
        if stuck {
            inner.die(format!(
                "agent worker killed: it ran past its CPU slice without yielding ({} ms)",
                limit.as_millis()
            ));
            return;
        }
    }
}

async fn reader(inner: Weak<Inner>, mut stdout: ChildStdout) {
    loop {
        let message = recv(&mut stdout).await;
        let Some(inner) = inner.upgrade() else { return };
        let message = match message {
            Ok(m) => m,
            Err(e) => return inner.die(e),
        };
        match message {
            ToHost::Idle(_) => {
                let mut busy = lock(&inner.busy);
                busy.outstanding = busy.outstanding.saturating_sub(1);
                busy.since = Instant::now();
            }
            ToHost::Acp(m) => {
                let _ = inner.acp.send(m.json);
            }
            ToHost::AgentCall(c) => call(&inner, *c),
            ToHost::BodyChunk(chunk) => upload(&inner, *chunk),
            ToHost::TimerSet(t) => {
                let (id, ms) = (t.id, t.ms);
                let weak = Arc::downgrade(&inner);
                let task = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if let Some(inner) = weak.upgrade() {
                        lock(&inner.timers).remove(&id);
                        inner.send(ToWorker::TimerFired(Box::new(wire::TimerFired {
                            id,
                            ..Default::default()
                        })));
                    }
                });
                if let Some(old) = lock(&inner.timers).insert(id, task.abort_handle()) {
                    old.abort();
                }
            }
            ToHost::TimerClear(t) => {
                if let Some(h) = lock(&inner.timers).remove(&t.id) {
                    h.abort();
                }
            }
            ToHost::EntropyRequest(_) => inner.send(ToWorker::Entropy(Box::new(wire::Entropy {
                data: os_entropy(),
                ..Default::default()
            }))),
            ToHost::Log(l) => log(&l),
            ToHost::ProbeDone(p) => {
                if let Some(tx) = lock(&inner.probe).take() {
                    let _ = tx.send(p.error);
                }
            }
            ToHost::Stopped(s) => {
                return inner.die(format!("agent worker stopped: over its {} limit", s.reason));
            }
            other => {
                return inner.die(format!(
                    "the agent worker sent a frame out of turn: {other:?}"
                ));
            }
        }
    }
}

fn log(line: &wire::AgentLog) {
    eprintln!("[agent {:?}] {}", line.level.as_known(), line.line);
}

/// A request body arrives in chunks after its `FetchCall`.
fn upload(inner: &Arc<Inner>, chunk: wire::BodyChunk) {
    let done = {
        let mut uploads = lock(&inner.uploads);
        let Some((_, body)) = uploads.get_mut(&chunk.id) else {
            return;
        };
        body.extend_from_slice(&chunk.data);
        if chunk.last {
            uploads.remove(&chunk.id)
        } else {
            None
        }
    };
    if let Some((fetch, body)) = done {
        start_fetch(inner, chunk.id, fetch, body);
    }
}

/// Answers one `AgentCall` on its own task.
fn call(inner: &Arc<Inner>, call: wire::AgentCall) {
    use wire::__buffa::oneof::agent_call::Kind;
    use wire::__buffa::oneof::agent_reply::Answer;
    let id = call.id;
    let Some(kind) = call.kind else {
        return inner.reply(id, Answer::Error("an empty call".into()));
    };
    let task_inner = inner.clone();
    let task = match kind {
        Kind::Fetch(fetch) => {
            if fetch.body_follows {
                lock(&inner.uploads).insert(id, (*fetch, Vec::new()));
            } else {
                start_fetch(inner, id, *fetch, Vec::new());
            }
            return;
        }
        Kind::Model(m) => tokio::spawn(async move {
            let request: Value = serde_json::from_str(&m.json).unwrap_or(Value::Null);
            let (events, mut rx) = mpsc::unbounded_channel();
            let host = tokio::spawn(task_inner.hosts.model(
                m.api.clone(),
                request,
                m.stream,
                events,
            ));
            while let Some(event) = rx.recv().await {
                task_inner.chunk(id, event.to_string().into_bytes(), false);
            }
            let result = host
                .await
                .unwrap_or_else(|e| Err(format!("the model host panicked: {e}")));
            finish(&task_inner, id, result.map(|v| v.to_string()));
        }),
        Kind::Mcp(m) => tokio::spawn(async move {
            let manifest = &task_inner.manifest;
            let result = match m.method.as_str() {
                "list" => task_inner
                    .hosts
                    .mcp_list(m.server.clone())
                    .await
                    .map(|mut list| {
                        // Undeclared tools are hidden (D1468).
                        if let Some(tools) = list["tools"].as_array_mut() {
                            tools.retain(|t| {
                                manifest
                                    .allows_tool(&m.server, t["name"].as_str().unwrap_or_default())
                            });
                        }
                        list.to_string()
                    }),
                "call" if !manifest.allows_tool(&m.server, &m.tool) => Err(format!(
                    "MCP tool {}/{} is not declared by this agent",
                    m.server, m.tool
                )),
                "call" => {
                    let args: Value = serde_json::from_str(&m.json).unwrap_or(json!({}));
                    task_inner
                        .hosts
                        .mcp_call(m.server.clone(), m.tool.clone(), args)
                        .await
                        .map(|v| v.to_string())
                }
                other => Err(format!("unknown MCP method {other:?}")),
            };
            finish(&task_inner, id, result);
        }),
        Kind::Token(_) => tokio::spawn(async move {
            let result = task_inner
                .hosts
                .token()
                .await
                .map(|t| Value::String(t).to_string());
            finish(&task_inner, id, result);
        }),
    };
    lock(&inner.calls).insert(id, task.abort_handle());
}

fn finish(inner: &Arc<Inner>, id: u64, result: Result<String, String>) {
    use wire::__buffa::oneof::agent_reply::Answer;
    lock(&inner.calls).remove(&id);
    match result {
        Ok(json) => inner.reply(id, Answer::Json(json)),
        Err(e) => inner.reply(id, Answer::Error(e)),
    }
}

fn start_fetch(inner: &Arc<Inner>, id: u64, fetch: wire::FetchCall, body: Vec<u8>) {
    use wire::__buffa::oneof::agent_reply::Answer;
    let task_inner = inner.clone();
    let task = tokio::spawn(async move {
        if let Err(e) = task_inner.manifest.allows_host(&fetch.url) {
            return finish(&task_inner, id, Err(e));
        }
        let request = FetchRequest {
            url: fetch.url,
            method: fetch.method,
            headers: fetch
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
            body,
        };
        match task_inner.hosts.fetch(request).await {
            Err(e) => finish(&task_inner, id, Err(e)),
            Ok(response) => {
                lock(&task_inner.calls).remove(&id);
                task_inner.reply(
                    id,
                    Answer::Fetch(Box::new(wire::FetchHead {
                        status: u32::from(response.status),
                        status_text: response.status_text,
                        headers: response
                            .headers
                            .into_iter()
                            .map(|(name, value)| wire::Header {
                                name,
                                value,
                                ..Default::default()
                            })
                            .collect(),
                        url: response.url,
                        ..Default::default()
                    })),
                );
                let body = response.body;
                if body.is_empty() {
                    task_inner.chunk(id, Vec::new(), true);
                }
                let pieces = body.chunks(MAX_CHUNK_BYTES).count();
                for (i, piece) in body.chunks(MAX_CHUNK_BYTES).enumerate() {
                    task_inner.chunk(id, piece.to_vec(), i + 1 == pieces);
                }
            }
        }
    });
    lock(&inner.calls).insert(id, task.abort_handle());
}
