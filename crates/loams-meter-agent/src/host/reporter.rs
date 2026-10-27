//! The host side of the host report (platform doc 06 §4, RN1 Ruling 3): a
//! wasmtime host links this [`Reporter`] and calls [`Reporter::record`] once
//! per invocation.
//!
//! * Invocations are batched into a `HostReport` every second or every
//!   1,000 invocations, numbered from 1 per `host_id`.
//! * Unacknowledged reports stay in a bounded in-memory buffer (64 MiB by
//!   default) and are resent after a reconnect. When the buffer is full the
//!   oldest report is dropped and counted; the agent learns of it through a
//!   `Gap` before the next report, so its ack never covers the loss.
//! * No consumer costs nothing beyond the buffer: `record` never blocks or
//!   fails.
//! * Each process gets a new random `host_id`, so sequence numbers never
//!   collide across restarts.
//! * Reports are signed when sent, with the epoch and key of the current
//!   session (PD64).
//!
//! Not built here (an honest gap, see doc 06 §3.3): the host's on-disk spool
//! that survives a host restart. A restart loses the buffer; the CPU it held
//! is still in the cgroup total, and the agent's reconciliation bills it.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use prost::Message;
use rand::Rng;
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::auth::{sign_gap, sign_report};
use super::proto::{
    AgentMessage, AgentMsg, Gap, Hello, HostMessage, HostMsg, HostReport, Invocation,
    SandboxFinished, read_frame, write_frame,
};
use super::server::DEFAULT_SOCKET;

/// The reporter's settings (RN1 Ruling 3).
#[derive(Debug, Clone)]
pub struct ReporterConfig {
    /// The agent's socket.
    pub socket: PathBuf,
    /// Seal a batch at least this often.
    pub flush_interval: Duration,
    /// Seal a batch at this many invocations.
    pub max_batch: usize,
    /// Bound on unacknowledged reports, in encoded bytes.
    pub max_buffer_bytes: usize,
    /// First reconnect delay.
    pub reconnect_min: Duration,
    /// Longest reconnect delay.
    pub reconnect_max: Duration,
    /// A write that takes longer drops the connection.
    pub write_timeout: Duration,
}

impl Default for ReporterConfig {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_SOCKET),
            flush_interval: Duration::from_secs(1),
            max_batch: 1_000,
            max_buffer_bytes: 64 << 20,
            reconnect_min: Duration::from_millis(100),
            reconnect_max: Duration::from_secs(10),
            write_timeout: Duration::from_secs(1),
        }
    }
}

/// Counters, for `loams_meter_*` metrics and tests.
#[derive(Debug, Default)]
pub struct ReporterStats {
    /// Reports sealed.
    pub sealed: AtomicU64,
    /// The highest `seq` the agent acknowledged.
    pub acked_seq: AtomicU64,
    /// Reports dropped from a full buffer
    /// (`loams_meter_reports_dropped_total`).
    pub dropped: AtomicU64,
    /// Connections made.
    pub connects: AtomicU64,
    /// Refusals from the agent.
    pub rejected: AtomicU64,
}

enum Cmd {
    Record(Invocation),
    SandboxFinished(String, oneshot::Sender<()>),
    Flush,
    Shutdown(oneshot::Sender<()>),
}

/// The per-invocation reporter of a host process.
#[derive(Debug)]
pub struct Reporter {
    host_id: String,
    tx: mpsc::UnboundedSender<Cmd>,
    stats: Arc<ReporterStats>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Cmd::Record(_) => "Record",
            Cmd::SandboxFinished(..) => "SandboxFinished",
            Cmd::Flush => "Flush",
            Cmd::Shutdown(_) => "Shutdown",
        })
    }
}

/// A new random host id: 16 random bytes in hex.
pub fn new_host_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Reporter {
    /// Starts the reporter on the current Tokio runtime with a new host id.
    pub fn start(config: ReporterConfig) -> Self {
        let host_id = new_host_id();
        let stats = Arc::new(ReporterStats::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker::new(host_id.clone(), config, stats.clone());
        let task = tokio::spawn(worker.run(rx));
        Self {
            host_id,
            tx,
            stats,
            task,
        }
    }

    /// This process's host id.
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// The counters.
    pub fn stats(&self) -> &ReporterStats {
        &self.stats
    }

    /// Records one invocation. Never blocks; never fails.
    pub fn record(&self, invocation: Invocation) {
        let _ = self.tx.send(Cmd::Record(invocation));
    }

    /// Seals the current batch now.
    pub fn flush(&self) {
        let _ = self.tx.send(Cmd::Flush);
    }

    /// Tells the agent that the sandbox in `cgroup` finished and waits, up to
    /// `timeout`, until the agent has read its final counters. `true` means
    /// the cgroup may be removed now; `false` means keep it for the
    /// retention window (doc 06 §3.0).
    pub async fn sandbox_finished(&self, cgroup: impl Into<String>, timeout: Duration) -> bool {
        let (done, wait) = oneshot::channel();
        if self
            .tx
            .send(Cmd::SandboxFinished(cgroup.into(), done))
            .is_err()
        {
            return false;
        }
        matches!(tokio::time::timeout(timeout, wait).await, Ok(Ok(())))
    }

    /// Seals the last batch and waits up to `grace` for the agent to ack
    /// everything, then stops.
    pub async fn shutdown(self, grace: Duration) {
        let (done, wait) = oneshot::channel();
        if self.tx.send(Cmd::Shutdown(done)).is_ok() {
            let _ = tokio::time::timeout(grace, wait).await;
        }
        self.task.abort();
    }
}

struct Session {
    write: OwnedWriteHalf,
    key: Vec<u8>,
    /// Everything up to this seq has been written on this connection.
    sent: u64,
}

struct Worker {
    host_id: String,
    config: ReporterConfig,
    stats: Arc<ReporterStats>,
    batch: Vec<Invocation>,
    /// Sealed, unacknowledged reports (unsigned), oldest first.
    buffer: VecDeque<HostReport>,
    buffer_bytes: usize,
    next_seq: u64,
    acked: u64,
    /// Why the reports between the ack and the buffer's head are missing.
    gap_reason: &'static str,
    epoch: Vec<u8>,
    session: Option<Session>,
    retry_at: Instant,
    backoff: Duration,
    finished: BTreeMap<String, Vec<oneshot::Sender<()>>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Worker {
    fn new(host_id: String, config: ReporterConfig, stats: Arc<ReporterStats>) -> Self {
        let backoff = config.reconnect_min;
        Self {
            host_id,
            config,
            stats,
            batch: Vec::new(),
            buffer: VecDeque::new(),
            buffer_bytes: 0,
            next_seq: 1,
            acked: 0,
            gap_reason: "buffer_full",
            epoch: Vec::new(),
            session: None,
            retry_at: Instant::now(),
            backoff,
            finished: BTreeMap::new(),
            shutdown: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        let mut flush = tokio::time::interval(self.config.flush_interval);
        flush.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // What the agent sends on the current connection; kept out of
        // `self` so the select below borrows only this local.
        let mut incoming: Option<mpsc::UnboundedReceiver<AgentMessage>> = None;
        loop {
            if self.session.is_none() {
                incoming = None;
                if Instant::now() >= self.retry_at {
                    incoming = self.connect().await;
                }
            }
            let connected = self.session.is_some();
            let retry_at = self.retry_at;
            let next_message = async {
                match incoming.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Record(inv)) => {
                        self.batch.push(inv);
                        if self.batch.len() >= self.config.max_batch {
                            self.seal();
                            self.pump().await;
                        }
                    }
                    Some(Cmd::Flush) => {
                        self.seal();
                        self.pump().await;
                    }
                    Some(Cmd::SandboxFinished(cgroup, done)) => {
                        self.finished.entry(cgroup.clone()).or_default().push(done);
                        self.send_finished(&cgroup).await;
                    }
                    Some(Cmd::Shutdown(done)) => {
                        self.seal();
                        self.shutdown = Some(done);
                        self.pump().await;
                        self.check_shutdown();
                    }
                    None => return,
                },
                msg = next_message => match msg {
                    Some(msg) => self.on_message(msg).await,
                    None => self.disconnect(),
                },
                _ = flush.tick() => {
                    self.seal();
                    self.pump().await;
                }
                _ = tokio::time::sleep_until(retry_at), if !connected => {}
            }
        }
    }

    /// Seals the current batch into a numbered report.
    fn seal(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let report = HostReport {
            host_id: self.host_id.clone(),
            seq: self.next_seq,
            invocations: std::mem::take(&mut self.batch),
            epoch: Vec::new(),
            mac: Vec::new(),
        };
        self.next_seq += 1;
        self.buffer_bytes += report.encoded_len();
        self.buffer.push_back(report);
        self.stats.sealed.fetch_add(1, Ordering::Relaxed);
        // Drop the oldest, never the newest, and count it; the gap is
        // announced before the next report.
        while self.buffer_bytes > self.config.max_buffer_bytes && self.buffer.len() > 1 {
            if let Some(old) = self.buffer.pop_front() {
                self.buffer_bytes -= old.encoded_len();
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                self.gap_reason = "buffer_full";
            }
        }
    }

    /// Connects and handshakes; returns the agent's messages on success.
    async fn connect(&mut self) -> Option<mpsc::UnboundedReceiver<AgentMessage>> {
        let Ok(stream) = UnixStream::connect(&self.config.socket).await else {
            self.backoff();
            return None;
        };
        let (mut read, mut write) = stream.into_split();
        let hello = HostMessage {
            msg: Some(HostMsg::Hello(Hello {
                host_id: self.host_id.clone(),
                epoch: self.epoch.clone(),
                acked_seq: self.acked,
            })),
        };
        if write_frame(&mut write, &hello).await.is_err() {
            self.backoff();
            return None;
        }
        let welcome = match read_frame::<_, AgentMessage>(&mut read).await {
            Ok(Some(AgentMessage {
                msg: Some(AgentMsg::Welcome(w)),
            })) => w,
            _ => {
                self.stats.rejected.fetch_add(1, Ordering::Relaxed);
                // Possibly an epoch from an earlier agent boot that this one
                // will not resume; it issues a fresh one either way.
                self.backoff();
                return None;
            }
        };
        self.epoch = welcome.epoch;
        self.on_ack(welcome.acked_seq);
        let (tx, incoming) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(msg)) = read_frame::<_, AgentMessage>(&mut read).await {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        self.session = Some(Session {
            write,
            key: welcome.key,
            sent: self.acked,
        });
        self.backoff = self.config.reconnect_min;
        self.stats.connects.fetch_add(1, Ordering::Relaxed);
        self.pump().await;
        let pending: Vec<String> = self.finished.keys().cloned().collect();
        for cgroup in pending {
            self.send_finished(&cgroup).await;
        }
        Some(incoming)
    }

    fn backoff(&mut self) {
        self.session = None;
        self.retry_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(self.config.reconnect_max);
    }

    fn disconnect(&mut self) {
        self.backoff();
    }

    async fn write(&mut self, msg: HostMessage) -> bool {
        let timeout = self.config.write_timeout;
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        match tokio::time::timeout(timeout, write_frame(&mut session.write, &msg)).await {
            Ok(Ok(())) => true,
            _ => {
                self.disconnect();
                false
            }
        }
    }

    /// Sends the gap (if any) and every buffered report not yet written on
    /// this connection, signed for this session.
    async fn pump(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let key = session.key.clone();
        let mut sent = session.sent;
        let head = self.buffer.front().map_or(self.next_seq, |r| r.seq);
        if sent < head.saturating_sub(1) {
            let mut gap = Gap {
                host_id: self.host_id.clone(),
                from_seq: sent + 1,
                to_seq: head - 1,
                reason: self.gap_reason.to_string(),
                epoch: self.epoch.clone(),
                mac: Vec::new(),
            };
            sign_gap(&key, &mut gap);
            if !self
                .write(HostMessage {
                    msg: Some(HostMsg::Gap(gap)),
                })
                .await
            {
                return;
            }
            sent = head - 1;
        }
        let to_send: Vec<HostReport> = self
            .buffer
            .iter()
            .filter(|r| r.seq > sent)
            .cloned()
            .collect();
        for mut report in to_send {
            report.epoch = self.epoch.clone();
            sign_report(&key, &mut report);
            let seq = report.seq;
            if !self
                .write(HostMessage {
                    msg: Some(HostMsg::Report(report)),
                })
                .await
            {
                return;
            }
            sent = seq;
        }
        if let Some(session) = self.session.as_mut() {
            session.sent = session.sent.max(sent);
        }
    }

    async fn send_finished(&mut self, cgroup: &str) {
        let msg = HostMessage {
            msg: Some(HostMsg::SandboxFinished(SandboxFinished {
                cgroup: cgroup.to_string(),
            })),
        };
        self.write(msg).await;
    }

    fn on_ack(&mut self, seq: u64) {
        if seq < self.acked {
            return;
        }
        self.acked = seq;
        self.stats.acked_seq.store(seq, Ordering::Relaxed);
        while self.buffer.front().is_some_and(|r| r.seq <= seq) {
            if let Some(r) = self.buffer.pop_front() {
                self.buffer_bytes -= r.encoded_len();
            }
        }
        if self.next_seq <= seq {
            self.next_seq = seq + 1;
        }
    }

    async fn on_message(&mut self, msg: AgentMessage) {
        match msg.msg {
            Some(AgentMsg::Ack(ack)) => {
                self.on_ack(ack.seq);
                self.check_shutdown();
            }
            Some(AgentMsg::SandboxRead(read)) => {
                for done in self.finished.remove(&read.cgroup).unwrap_or_default() {
                    let _ = done.send(());
                }
            }
            Some(AgentMsg::Reject(_)) => {
                self.stats.rejected.fetch_add(1, Ordering::Relaxed);
                // A refused epoch means the agent restarted: start a fresh
                // session on the next connection.
                self.epoch.clear();
                self.disconnect();
            }
            Some(AgentMsg::Welcome(_)) | None => {}
        }
    }

    fn check_shutdown(&mut self) {
        if self.buffer.is_empty() && self.batch.is_empty() {
            if let Some(done) = self.shutdown.take() {
                let _ = done.send(());
            }
        }
    }
}
