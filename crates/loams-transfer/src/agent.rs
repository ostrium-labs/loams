//! `loams-transfer agent`: the desktop's sidecar (§85 §12, D2420).
//!
//! Electron main writes one JSON command per line on stdin and reads one
//! JSON event per line on stdout. Tokens arrive inside commands, never in
//! argv or the environment. The agent holds the state directory's exclusive
//! lock for its whole life (D2428), so one sidecar per install drives the
//! leases.
//!
//! Commands (`op`): `upload`, `download`, `pause`, `cancel`, `list`,
//! `run.start`, `run.tail`, `shutdown`. Events (`event`): `ready`,
//! `progress` (at most 4 per second per job, D2436), `state`, `log`,
//! `reply`, `error`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::Client;
use crate::client::{self, StateDir, Tracker, TransferError, UploadOpts};
use crate::net::{ConnectOpts, Mode, Trust};

/// How to reach a server, as the desktop sends it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerSpec {
    /// `https://host:port` (or `http://127.0.0.1:port` for the local engine).
    pub base: String,
    /// Pin the server certificate by SHA-256 (dev engines).
    #[serde(default)]
    pub pin: Option<String>,
    /// A PEM CA bundle for this server.
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub mode: Mode,
}

impl ServerSpec {
    pub fn connect_opts(&self, state: &StateDir) -> ConnectOpts {
        let trust = match (&self.pin, &self.ca_file) {
            (Some(p), _) => Trust::Pin(p.clone()),
            (None, Some(f)) => Trust::CaFile(f.clone()),
            (None, None) => Trust::System,
        };
        let mut o = ConnectOpts::new(self.base.clone(), trust);
        o.mode = self.mode;
        o.memory = Some(state.transport_memory());
        o
    }
}

/// One command line.
#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
enum Command {
    #[serde(rename = "upload")]
    Upload {
        id: String,
        server: ServerSpec,
        ns: String,
        token: String,
        path: PathBuf,
        #[serde(default)]
        chunk_size: Option<u64>,
        #[serde(default)]
        window: Option<usize>,
    },
    #[serde(rename = "download")]
    Download {
        id: String,
        server: ServerSpec,
        ns: String,
        token: String,
        tid: String,
        dest: PathBuf,
        #[serde(default)]
        window: Option<usize>,
    },
    #[serde(rename = "pause")]
    Pause { id: String, job: String },
    #[serde(rename = "cancel")]
    Cancel {
        id: String,
        job: String,
        #[serde(default)]
        server: Option<ServerSpec>,
        #[serde(default)]
        ns: Option<String>,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        tid: Option<String>,
    },
    #[serde(rename = "list")]
    List { id: String },
    #[serde(rename = "run.start")]
    RunStart {
        id: String,
        server: ServerSpec,
        ns: String,
        token: String,
        job: String,
        #[serde(default)]
        params: BTreeMap<String, String>,
    },
    #[serde(rename = "run.tail")]
    RunTail {
        id: String,
        server: ServerSpec,
        ns: String,
        token: String,
        rid: String,
    },
    /// The server's transfers and runs in the namespace.
    #[serde(rename = "remote.list")]
    RemoteList {
        id: String,
        server: ServerSpec,
        ns: String,
        token: String,
    },
    #[serde(rename = "shutdown")]
    Shutdown {},
}

type Out = mpsc::UnboundedSender<Value>;

fn emit(out: &Out, v: Value) {
    let _ = out.send(v);
}

struct Agent {
    state: StateDir,
    out: Out,
    jobs: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Agent {
    fn start(
        self: &Arc<Self>,
        job: String,
        fut: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let me = Arc::clone(self);
        let key = job.clone();
        let handle = tokio::spawn(async move {
            fut.await;
            me.jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&job);
        });
        let mut jobs = self.jobs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(old) = jobs.insert(key, handle) {
            old.abort();
        }
    }

    fn stop(&self, job: &str) -> bool {
        match self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(job)
        {
            Some(h) => {
                h.abort();
                true
            }
            None => false,
        }
    }
}

fn client_for(
    state: &StateDir,
    server: &ServerSpec,
    ns: &str,
    token: &str,
) -> Result<Client, TransferError> {
    Ok(Client::new(server.connect_opts(state), ns, token)?)
}

/// Emits the tracker's progress at most every 250 ms while `fut` runs.
async fn with_progress<T>(
    out: Out,
    job: String,
    tracker: Arc<Mutex<Option<Tracker>>>,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let reporter = {
        let (out, job, tracker) = (out.clone(), job.clone(), Arc::clone(&tracker));
        tokio::spawn(async move {
            let mut last = None;
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let snap = tracker
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .map(Tracker::snapshot);
                if let Some(p) = snap
                    && last.as_ref() != Some(&p)
                {
                    emit(
                        &out,
                        json!({"event": "progress", "job": job, "progress": p}),
                    );
                    last = Some(p);
                }
            }
        })
    };
    let r = fut.await;
    reporter.abort();
    if let Some(p) = tracker
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(Tracker::snapshot)
    {
        emit(
            &out,
            json!({"event": "progress", "job": job, "progress": p}),
        );
    }
    r
}

fn failed(out: &Out, job: &str, e: &TransferError) {
    emit(
        out,
        json!({"event": "state", "job": job, "state": "failed", "code": e.code(), "error": e.to_string()}),
    );
}

/// An unfinished upload of the same file to the same place, if any.
pub fn find_resumable(state: &StateDir, client: &Client, path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = client::local::mtime_ns(&meta);
    state.uploads().ok()?.into_iter().find_map(|u| {
        (u.path == path
            && u.size == meta.len()
            && u.mtime_ns == mtime
            && u.server == client.origin()
            && u.ns == client.ns())
        .then_some(u.tid)
    })
}

async fn handle(agent: &Arc<Agent>, cmd: Command) -> bool {
    let out = agent.out.clone();
    match cmd {
        Command::Upload {
            id,
            server,
            ns,
            token,
            path,
            chunk_size,
            window,
        } => {
            let client = match client_for(&agent.state, &server, &ns, &token) {
                Ok(c) => c,
                Err(e) => {
                    failed(&out, &id, &e);
                    return true;
                }
            };
            let state = agent.state.clone();
            let mut opts = UploadOpts {
                chunk_size,
                ..UploadOpts::default()
            };
            if let Some(w) = window {
                opts.window = w.clamp(1, 64);
            }
            let job = id.clone();
            agent.start(id, async move {
                let resume = find_resumable(&state, &client, &path);
                emit(&out, json!({"event": "state", "job": job, "state": "started", "resumed": resume.is_some()}));
                let tracker = Arc::new(Mutex::new(None));
                let t2 = Arc::clone(&tracker);
                let r = with_progress(out.clone(), job.clone(), Arc::clone(&tracker), async {
                    client::upload(&client, &state, &path, resume.as_deref(), &opts, move |t| {
                        *t2.lock().unwrap_or_else(|p| p.into_inner()) = Some(t);
                    })
                    .await
                })
                .await;
                match r {
                    Ok(v) => emit(&out, json!({"event": "state", "job": job, "state": "done", "transfer": v})),
                    Err(e) => failed(&out, &job, &e),
                }
            });
        }
        Command::Download {
            id,
            server,
            ns,
            token,
            tid,
            dest,
            window,
        } => {
            let client = match client_for(&agent.state, &server, &ns, &token) {
                Ok(c) => c,
                Err(e) => {
                    failed(&out, &id, &e);
                    return true;
                }
            };
            let mut opts = UploadOpts::default();
            if let Some(w) = window {
                opts.window = w.clamp(1, 64);
            }
            let job = id.clone();
            agent.start(id, async move {
                let resumed = client::local::part_paths(&dest).1.exists();
                emit(
                    &out,
                    json!({"event": "state", "job": job, "state": "started", "resumed": resumed}),
                );
                let tracker = Arc::new(Mutex::new(None));
                let t2 = Arc::clone(&tracker);
                let r = with_progress(out.clone(), job.clone(), Arc::clone(&tracker), async {
                    client::download(&client, &tid, &dest, &opts, move |t| {
                        *t2.lock().unwrap_or_else(|p| p.into_inner()) = Some(t);
                    })
                    .await
                })
                .await;
                match r {
                    Ok(p) => emit(
                        &out,
                        json!({"event": "state", "job": job, "state": "done", "path": p}),
                    ),
                    Err(e) => failed(&out, &job, &e),
                }
            });
        }
        Command::Pause { id, job } => {
            let stopped = agent.stop(&job);
            if stopped {
                emit(
                    &out,
                    json!({"event": "state", "job": job, "state": "paused"}),
                );
            }
            emit(&out, json!({"event": "reply", "id": id, "ok": stopped}));
        }
        Command::Cancel {
            id,
            job,
            server,
            ns,
            token,
            tid,
        } => {
            agent.stop(&job);
            // An upload's server side is abandoned and its local state goes;
            // a download's part file stays for the desktop to delete.
            if let (Some(server), Some(ns), Some(token), Some(tid)) = (server, ns, token, tid) {
                if let Ok(c) = client_for(&agent.state, &server, &ns, &token) {
                    let _ = c.abandon(&tid).await;
                }
                let _ = agent.state.remove_upload(&tid);
            }
            emit(
                &out,
                json!({"event": "state", "job": job, "state": "cancelled"}),
            );
            emit(&out, json!({"event": "reply", "id": id, "ok": true}));
        }
        Command::List { id } => {
            let uploads = agent.state.uploads().unwrap_or_default();
            let running: Vec<String> = agent
                .jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .keys()
                .cloned()
                .collect();
            emit(
                &out,
                json!({"event": "reply", "id": id, "uploads": uploads, "running": running}),
            );
        }
        Command::RunStart {
            id,
            server,
            ns,
            token,
            job,
            params,
        } => {
            let r = async {
                let c = client_for(&agent.state, &server, &ns, &token)?;
                let rid = crate::keys::new_id();
                Ok::<_, TransferError>(c.create_run(&rid, &job, params).await?)
            }
            .await;
            match r {
                Ok(v) => emit(&out, json!({"event": "reply", "id": id, "run": v})),
                Err(e) => emit(
                    &out,
                    json!({"event": "reply", "id": id, "code": e.code(), "error": e.to_string()}),
                ),
            }
        }
        Command::RunTail {
            id,
            server,
            ns,
            token,
            rid,
        } => {
            let client = match client_for(&agent.state, &server, &ns, &token) {
                Ok(c) => c,
                Err(e) => {
                    failed(&out, &id, &e);
                    return true;
                }
            };
            let log = agent.state.run_log(&rid);
            let job = id.clone();
            agent.start(id, async move {
                match client::tail::tail(&client, &rid, &log, |offset, bytes| {
                    emit(
                        &out,
                        json!({
                            "event": "log", "job": job, "rid": rid, "offset": offset,
                            "len": bytes.len(), "text": String::from_utf8_lossy(bytes),
                        }),
                    );
                })
                .await
                {
                    Ok(v) => emit(
                        &out,
                        json!({"event": "state", "job": job, "state": "done", "run": v}),
                    ),
                    Err(e) => failed(&out, &job, &e),
                }
            });
        }
        Command::RemoteList {
            id,
            server,
            ns,
            token,
        } => {
            let r = async {
                let c = client_for(&agent.state, &server, &ns, &token)?;
                let transfers = c.list().await?;
                let runs = c.runs().await?;
                Ok::<_, TransferError>((transfers, runs))
            }
            .await;
            match r {
                Ok((t, r)) => emit(
                    &out,
                    json!({"event": "reply", "id": id, "transfers": t, "runs": r}),
                ),
                Err(e) => emit(
                    &out,
                    json!({"event": "reply", "id": id, "code": e.code(), "error": e.to_string()}),
                ),
            }
        }
        Command::Shutdown {} => return false,
    }
    true
}

/// Runs the agent on stdin and stdout until `shutdown` or end of input.
pub async fn run(state_dir: PathBuf) -> anyhow::Result<()> {
    let state = StateDir::new(&state_dir)?;
    let _lock = state.lock()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(v) = rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let agent = Arc::new(Agent {
        state: state.clone(),
        out: tx.clone(),
        jobs: Mutex::new(HashMap::new()),
    });
    emit(
        &tx,
        json!({
            "event": "ready",
            "version": env!("CARGO_PKG_VERSION"),
            "install_id": state.install_id()?,
            "unfinished_uploads": state.uploads().unwrap_or_default(),
        }),
    );
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Command>(&line) {
            Ok(cmd) => {
                if !handle(&agent, cmd).await {
                    break;
                }
            }
            Err(e) => emit(
                &tx,
                json!({"event": "error", "error": format!("bad command: {e}")}),
            ),
        }
    }
    for (_, h) in agent.jobs.lock().unwrap_or_else(|p| p.into_inner()).drain() {
        h.abort();
    }
    drop(agent);
    drop(tx);
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        let c: Command = serde_json::from_str(
            r#"{"op":"upload","id":"j1","server":{"base":"https://h:1","pin":"ab"},"ns":"acme","token":"t","path":"/x"}"#,
        )
        .expect("parse");
        assert!(matches!(c, Command::Upload { .. }));
        let c: Command =
            serde_json::from_str(r#"{"op":"pause","id":"r","job":"j1"}"#).expect("parse");
        assert!(matches!(c, Command::Pause { .. }));
        assert!(serde_json::from_str::<Command>(r#"{"op":"rm -rf"}"#).is_err());
    }
}
