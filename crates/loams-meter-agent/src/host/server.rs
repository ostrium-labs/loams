//! The agent's end of the host-report socket (platform doc 06 §3.1, §3.3).
//!
//! The agent creates the node-local socket (`/run/loams/meter.sock` by
//! default) and wasmtime hosts connect to it; tenants cannot reach it. For
//! each connection the server:
//!
//! 1. reads the peer's credentials with `SO_PEERCRED` and refuses a process
//!    whose cgroup is not one of the allowed host cgroups
//!    (`wasm-host.scope` by default);
//! 2. answers `Hello` with the session's epoch and key ([`Sessions`]);
//! 3. checks every report and gap, hands what is new to the agent's loop
//!    ([`HostEvent`]), and acknowledges it only once the loop says it is
//!    durably spooled;
//! 4. answers `SandboxFinished` once the loop has taken its final read.
//!
//! The agent's loop owns the spool, so the server never writes it; it waits
//! on the loop's answer instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use super::auth::{Rejection, Sessions, Verdict};
use super::proto::{
    AgentMessage, AgentMsg, HostMessage, HostMsg, HostReportAck, Invocation, Reject,
    SandboxReadAck, Welcome, read_frame, write_frame,
};
use crate::cgroup::glob_match;

/// The default socket path.
pub const DEFAULT_SOCKET: &str = "/run/loams/meter.sock";
/// The default host cgroup pattern.
pub const DEFAULT_HOST_CGROUPS: &[&str] = &["wasm-host.scope"];

/// What the server asks of the agent's loop.
#[derive(Debug)]
pub enum HostEvent {
    /// Spool these invocations (report `seq` of `host_id`), then answer.
    Reports {
        /// The host.
        host_id: String,
        /// The report's sequence number.
        seq: u64,
        /// Its invocations.
        invocations: Vec<Invocation>,
        /// `Ok` once durable; `Err` (spool full, I/O) leaves it unacked.
        done: oneshot::Sender<Result<(), String>>,
    },
    /// Record a gap: the window it falls in is incomplete.
    Gap {
        /// The host.
        host_id: String,
        /// First missing `seq`.
        from_seq: u64,
        /// Last missing `seq`.
        to_seq: u64,
        /// Why.
        reason: String,
        /// `Ok` once durable.
        done: oneshot::Sender<Result<(), String>>,
    },
    /// Take a final read of `cgroup`, then answer.
    FinalRead {
        /// The cgroup, relative to the cgroup root.
        cgroup: String,
        /// Answered after the read.
        done: oneshot::Sender<()>,
    },
}

/// The server's settings.
#[derive(Debug, Clone)]
pub struct HostServerConfig {
    /// Where `/proc` is (for a peer's cgroup).
    pub proc_root: PathBuf,
    /// Patterns (as for cgroup targets) one of a peer's cgroup path
    /// components must match. Empty allows any peer (tests only).
    pub allowed_cgroups: Vec<String>,
}

impl Default for HostServerConfig {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            allowed_cgroups: DEFAULT_HOST_CGROUPS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Binds the socket at `path`, replacing a stale one, readable and writable
/// by its owner and group only.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

/// The cgroup v2 path of `pid` (the `0::` line of `/proc/<pid>/cgroup`).
pub fn cgroup_of(proc_root: &Path, pid: i32) -> Option<String> {
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|p| p.trim().trim_start_matches('/').to_string())
}

/// Whether a peer in `cgroup` may report.
pub fn peer_allowed(cgroup: &str, patterns: &[String]) -> bool {
    patterns.is_empty()
        || cgroup
            .split('/')
            .any(|part| patterns.iter().any(|p| glob_match(p, part)))
}

/// Accepts connections until the listener fails.
pub async fn serve(
    listener: UnixListener,
    config: Arc<HostServerConfig>,
    sessions: Arc<Mutex<Sessions>>,
    tx: mpsc::Sender<HostEvent>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let (config, sessions, tx) = (config.clone(), sessions.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, &config, &sessions, &tx).await {
                        debug!(error = %e, "host connection closed");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "host socket accept failed");
                return;
            }
        }
    }
}

async fn send(stream: &mut UnixStream, msg: AgentMsg) -> std::io::Result<()> {
    write_frame(stream, &AgentMessage { msg: Some(msg) }).await
}

async fn reject(stream: &mut UnixStream, why: &str) -> std::io::Result<()> {
    warn!(reason = why, "host report refused");
    send(
        stream,
        AgentMsg::Reject(Reject {
            reason: why.to_string(),
        }),
    )
    .await
}

fn closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "agent loop stopped")
}

/// Serves one host connection.
pub async fn handle(
    mut stream: UnixStream,
    config: &HostServerConfig,
    sessions: &Mutex<Sessions>,
    tx: &mpsc::Sender<HostEvent>,
) -> std::io::Result<()> {
    let cred = stream.peer_cred()?;
    if !config.allowed_cgroups.is_empty() {
        let cgroup = cred.pid().and_then(|pid| cgroup_of(&config.proc_root, pid));
        if !cgroup.is_some_and(|c| peer_allowed(&c, &config.allowed_cgroups)) {
            return reject(&mut stream, "peer_not_allowed").await;
        }
    }
    let Some(HostMessage {
        msg: Some(HostMsg::Hello(hello)),
    }) = read_frame::<_, HostMessage>(&mut stream).await?
    else {
        return reject(&mut stream, "expected_hello").await;
    };
    let host_id = hello.host_id;
    let welcome = lock(sessions)
        .hello(&host_id, &hello.epoch, cred.uid(), hello.acked_seq)
        .map(|s| Welcome {
            epoch: s.epoch.to_vec(),
            key: s.key.to_vec(),
            acked_seq: s.acked,
        });
    let welcome = match welcome {
        Ok(welcome) => welcome,
        Err(r) => return reject(&mut stream, r.as_str()).await,
    };
    send(&mut stream, AgentMsg::Welcome(welcome)).await?;

    while let Some(message) = read_frame::<_, HostMessage>(&mut stream).await? {
        match message.msg {
            Some(HostMsg::Report(report)) => {
                if report.host_id != host_id {
                    return reject(&mut stream, Rejection::PeerMismatch.as_str()).await;
                }
                let verdict = lock(sessions).check_report(&report);
                match verdict {
                    Verdict::Reject(r) => return reject(&mut stream, r.as_str()).await,
                    Verdict::Duplicate => {}
                    Verdict::Record => {
                        let (done, wait) = oneshot::channel();
                        tx.send(HostEvent::Reports {
                            host_id: host_id.clone(),
                            seq: report.seq,
                            invocations: report.invocations,
                            done,
                        })
                        .await
                        .map_err(|_| closed())?;
                        match wait.await.map_err(|_| closed())? {
                            Ok(()) => {
                                lock(sessions).acked(&host_id, report.seq);
                            }
                            // Not durable: no ack, so the host keeps it and
                            // resends after reconnecting.
                            Err(e) => {
                                warn!(host_id = %host_id, seq = report.seq, error = %e, "report not spooled");
                                return Ok(());
                            }
                        }
                    }
                }
                let acked = lock(sessions).get(&host_id).map_or(0, |s| s.acked);
                send(&mut stream, AgentMsg::Ack(HostReportAck { seq: acked })).await?;
            }
            Some(HostMsg::Gap(gap)) => {
                if gap.host_id != host_id {
                    return reject(&mut stream, Rejection::PeerMismatch.as_str()).await;
                }
                let verdict = lock(sessions).check_gap(&gap);
                match verdict {
                    Verdict::Reject(r) => return reject(&mut stream, r.as_str()).await,
                    Verdict::Duplicate => {}
                    Verdict::Record => {
                        warn!(
                            host_id = %host_id,
                            from = gap.from_seq,
                            to = gap.to_seq,
                            reason = %gap.reason,
                            "host reports lost; window marked incomplete"
                        );
                        let (done, wait) = oneshot::channel();
                        tx.send(HostEvent::Gap {
                            host_id: host_id.clone(),
                            from_seq: gap.from_seq,
                            to_seq: gap.to_seq,
                            reason: gap.reason.clone(),
                            done,
                        })
                        .await
                        .map_err(|_| closed())?;
                        if wait.await.map_err(|_| closed())?.is_err() {
                            return Ok(());
                        }
                        lock(sessions).acked(&host_id, gap.to_seq);
                    }
                }
                let acked = lock(sessions).get(&host_id).map_or(0, |s| s.acked);
                send(&mut stream, AgentMsg::Ack(HostReportAck { seq: acked })).await?;
            }
            Some(HostMsg::SandboxFinished(finished)) => {
                let (done, wait) = oneshot::channel();
                tx.send(HostEvent::FinalRead {
                    cgroup: finished.cgroup.clone(),
                    done,
                })
                .await
                .map_err(|_| closed())?;
                wait.await.map_err(|_| closed())?;
                send(
                    &mut stream,
                    AgentMsg::SandboxRead(SandboxReadAck {
                        cgroup: finished.cgroup,
                    }),
                )
                .await?;
            }
            Some(HostMsg::Hello(_)) | None => {
                return reject(&mut stream, "unexpected_message").await;
            }
        }
    }
    Ok(())
}

fn lock(sessions: &Mutex<Sessions>) -> std::sync::MutexGuard<'_, Sessions> {
    sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_cgroup_must_be_a_host_cgroup() {
        let patterns: Vec<String> = DEFAULT_HOST_CGROUPS.iter().map(|s| s.to_string()).collect();
        assert!(peer_allowed("loams.slice/wasm-host.scope", &patterns));
        assert!(!peer_allowed(
            "loams.slice/tenant-org_a.slice/workerd.scope",
            &patterns
        ));
        assert!(!peer_allowed(
            "kubepods.slice/kubepods-pod1.slice",
            &patterns
        ));
        assert!(peer_allowed("anything", &[]));
    }

    #[test]
    fn cgroup_is_read_from_proc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("42")).unwrap();
        std::fs::write(
            dir.path().join("42/cgroup"),
            "0::/loams.slice/wasm-host.scope\n",
        )
        .unwrap();
        assert_eq!(
            cgroup_of(dir.path(), 42).as_deref(),
            Some("loams.slice/wasm-host.scope")
        );
        assert_eq!(cgroup_of(dir.path(), 7), None);
    }
}
