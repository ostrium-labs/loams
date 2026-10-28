//! Matching for the Temporal gateway (design 74 §5.6, D1505; plan DX1 Task 7):
//! long polls wait here for workflow and activity tasks, and every hand-out
//! is claimed from Resonate first, so Resonate stays the only authority on
//! who holds a task.
//!
//! - Each task queue and kind is one Resonate poll group ([`QueueKey::group`]).
//!   The matcher subscribes to a group when its first poll arrives and turns
//!   each `execute` delivery into a ready task.
//! - A ready task goes to the oldest waiting poll, after `task_acquire`. With
//!   no poll waiting it is kept in a backlog (one entry per task: Resonate
//!   re-sends pending tasks every 30 s, so duplicates are normal); a later
//!   poll claims from the backlog first.
//! - A refused claim (the task moved on, or another replica took it) is
//!   skipped. A claim whose poll went away is released, so Resonate
//!   dispatches it again.
//! - The matcher keeps the leases it handed out alive ([`Matcher::run_leases`])
//!   until the gateway finishes the task ([`Matcher::finished`]): the
//!   gateway's own timeouts, not lease expiry, decide when an attempt is over
//!   (design 74 §5.3).
//! - One replica owns every queue in DX1 (plan X1.R8); partition leases and
//!   forwarding between replicas are DX3's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use loams_temporal_resonate::{Claim, Delivery, Durability, DurabilityError};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

/// Which kind of task a queue holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Workflow tasks.
    Workflow,
    /// Activity tasks.
    Activity,
}

/// A task queue of one namespace, for one kind of task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueueKey {
    /// The gateway's namespace ID.
    pub namespace_id: String,
    /// The task queue's name.
    pub task_queue: String,
    /// The kind of task.
    pub kind: TaskKind,
}

impl QueueKey {
    /// The Resonate poll group of this queue: `ltq-<w|a>-<24 hex>`, the hex
    /// being SHA-256 of the namespace ID and queue name (so no tenant name
    /// appears in Resonate, D147).
    pub fn group(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.namespace_id.as_bytes());
        h.update([0]);
        h.update(self.task_queue.as_bytes());
        let kind = match self.kind {
            TaskKind::Workflow => 'w',
            TaskKind::Activity => 'a',
        };
        format!("ltq-{kind}-{}", &hex::encode(h.finalize())[..24])
    }
}

/// A task Resonate says is pending at `version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// The task's (and its promise's) ID.
    pub task_id: String,
    /// The version to acquire at.
    pub version: i64,
}

/// What a poll gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handout {
    /// A task claimed from Resonate (held until [`Matcher::finished`] or
    /// [`Matcher::release`]).
    Claim(Claim),
    /// An in-memory item the gateway queued with [`Matcher::offer_local`]
    /// (a query for a live worker, which is never durable).
    Local(u64),
}

/// Why a poll failed (not a timeout: that is `Ok(None)`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MatchError {
    /// Resonate could not be reached.
    #[error("durability layer unavailable: {0}")]
    Unavailable(String),
}

#[derive(Default)]
struct Queue {
    waiters: VecDeque<(u64, oneshot::Sender<Handout>)>,
    backlog: VecDeque<Ready>,
    locals: VecDeque<u64>,
}

impl Queue {
    fn push_ready(&mut self, r: Ready) {
        if let Some(existing) = self.backlog.iter_mut().find(|x| x.task_id == r.task_id) {
            existing.version = existing.version.max(r.version);
        } else {
            self.backlog.push_back(r);
        }
    }
}

#[derive(Default)]
struct Inner {
    /// By poll group ([`QueueKey::group`]): a task can be offered for a
    /// group before any poll of it arrived (the relay created it first).
    queues: HashMap<String, Queue>,
    subscribed: HashSet<String>,
    /// Leases handed out and not finished: task ID → version.
    held: HashMap<String, i64>,
    /// Pollers seen, by poll group: identity → what they last reported
    /// (`DescribeTaskQueue`).
    pollers: HashMap<String, HashMap<String, Poller>>,
}

/// A worker that polled a queue (`DescribeTaskQueue`'s pollers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poller {
    /// The worker's identity.
    pub identity: String,
    /// When it last polled (Unix ms).
    pub last_access_ms: i64,
    /// The build ID it reported, if any.
    pub build_id: String,
}

/// How long a poller is listed after its last poll (Temporal lists pollers
/// seen in the last few minutes).
pub const POLLER_TTL_MS: i64 = 5 * 60 * 1000;

/// A waiting poll; on drop it leaves the queue and gives back anything that
/// reached it unclaimed.
struct Wait {
    matcher: Arc<Matcher>,
    key: QueueKey,
    id: u64,
    rx: Option<oneshot::Receiver<Handout>>,
}

impl Drop for Wait {
    fn drop(&mut self) {
        let Some(mut rx) = self.rx.take() else {
            return;
        };
        {
            let mut inner = self.matcher.lock();
            if let Some(q) = inner.queues.get_mut(&self.key.group()) {
                q.waiters.retain(|(w, _)| *w != self.id);
            }
        }
        match rx.try_recv() {
            Ok(Handout::Claim(claim)) => {
                let m = self.matcher.clone();
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move { m.release(&claim).await });
                }
            }
            Ok(Handout::Local(id)) => self.matcher.offer_local(&self.key, id),
            Err(_) => {}
        }
    }
}

/// The matcher. Cheap to share (`Arc`).
pub struct Matcher {
    durability: Arc<dyn Durability>,
    pid: String,
    lease_ms: i64,
    next_waiter: AtomicU64,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher")
            .field("pid", &self.pid)
            .field("lease_ms", &self.lease_ms)
            .finish_non_exhaustive()
    }
}

impl Matcher {
    /// A matcher that claims as `pid` with leases of `lease_ms`.
    pub fn new(
        durability: Arc<dyn Durability>,
        pid: impl Into<String>,
        lease_ms: i64,
    ) -> Arc<Self> {
        Arc::new(Matcher {
            durability,
            pid: pid.into(),
            lease_ms,
            next_waiter: AtomicU64::new(1),
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Records that `identity` polled `key` at `now_ms`.
    pub fn record_poller(&self, key: &QueueKey, identity: &str, build_id: &str, now_ms: i64) {
        let mut inner = self.lock();
        let seen = inner.pollers.entry(key.group()).or_default();
        seen.retain(|_, p| now_ms - p.last_access_ms <= POLLER_TTL_MS);
        seen.insert(
            identity.to_string(),
            Poller {
                identity: identity.to_string(),
                last_access_ms: now_ms,
                build_id: build_id.to_string(),
            },
        );
    }

    /// The pollers of `key` seen within [`POLLER_TTL_MS`] of `now_ms`, most
    /// recent first.
    pub fn pollers(&self, key: &QueueKey, now_ms: i64) -> Vec<Poller> {
        let inner = self.lock();
        let mut out: Vec<Poller> = inner
            .pollers
            .get(&key.group())
            .map(|m| {
                m.values()
                    .filter(|p| now_ms - p.last_access_ms <= POLLER_TTL_MS)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| {
            b.last_access_ms
                .cmp(&a.last_access_ms)
                .then_with(|| a.identity.cmp(&b.identity))
        });
        out
    }

    /// How many tasks wait in `key`'s backlog (the poll answers'
    /// `backlog_count_hint`).
    pub fn backlog(&self, key: &QueueKey) -> usize {
        self.lock()
            .queues
            .get(&key.group())
            .map_or(0, |q| q.backlog.len())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Subscribes to `key`'s group (once) and forwards its deliveries.
    pub async fn ensure_subscribed(self: &Arc<Self>, key: &QueueKey) -> Result<(), MatchError> {
        let group = key.group();
        {
            let mut inner = self.lock();
            if !inner.subscribed.insert(group.clone()) {
                return Ok(());
            }
        }
        let mut rx = match self.durability.subscribe(&group).await {
            Ok(rx) => rx,
            Err(e) => {
                self.lock().subscribed.remove(&group);
                return Err(MatchError::Unavailable(e.to_string()));
            }
        };
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(d) = rx.recv().await {
                let Some(me) = me.upgrade() else { break };
                if let Delivery::TaskReady {
                    task_id, version, ..
                } = d
                {
                    me.offer_group(&group, Ready { task_id, version }).await;
                }
            }
            if let Some(me) = me.upgrade() {
                me.lock().subscribed.remove(&group);
            }
        });
        Ok(())
    }

    /// A task of the queue whose group is `group` is ready (a delivery, or
    /// the gateway's own fast path right after it created the task).
    pub async fn offer_group(self: &Arc<Self>, group: &str, ready: Ready) {
        self.offer_in(group, ready).await;
    }

    /// A task of `key` is ready: claim it for the oldest waiting poll, or
    /// keep it for the next one.
    pub async fn offer(self: &Arc<Self>, key: &QueueKey, ready: Ready) {
        self.offer_in(&key.group(), ready).await;
    }

    async fn offer_in(self: &Arc<Self>, group: &str, ready: Ready) {
        {
            let mut inner = self.lock();
            if inner.held.contains_key(&ready.task_id) {
                return;
            }
            let q = inner.queues.entry(group.to_string()).or_default();
            q.waiters.retain(|(_, tx)| !tx.is_closed());
            if q.waiters.is_empty() {
                q.push_ready(ready);
                return;
            }
        }
        let Ok(claim) = self.acquire(&ready).await else {
            return;
        };
        let mut handout = Handout::Claim(claim);
        loop {
            let waiter = {
                let mut inner = self.lock();
                inner
                    .queues
                    .get_mut(group)
                    .and_then(|q| q.waiters.pop_front())
            };
            let Some((_, tx)) = waiter else { break };
            match tx.send(handout) {
                Ok(()) => return,
                Err(back) => handout = back,
            }
        }
        if let Handout::Claim(claim) = handout {
            self.give_back(&claim).await;
        }
    }

    /// Queues an in-memory item for the next poll of `key` (the oldest
    /// waiting one, if any). Never durable: a gateway restart loses it.
    pub fn offer_local(&self, key: &QueueKey, id: u64) {
        let mut inner = self.lock();
        let q = inner.queues.entry(key.group()).or_default();
        let mut handout = Handout::Local(id);
        while let Some((_, tx)) = q.waiters.pop_front() {
            match tx.send(handout) {
                Ok(()) => return,
                Err(back) => handout = back,
            }
        }
        q.locals.push_back(id);
    }

    /// Removes a queued in-memory item that was not handed out yet; returns
    /// whether it was still queued.
    pub fn cancel_local(&self, key: &QueueKey, id: u64) -> bool {
        let mut inner = self.lock();
        let Some(q) = inner.queues.get_mut(&key.group()) else {
            return false;
        };
        let before = q.locals.len();
        q.locals.retain(|x| *x != id);
        before != q.locals.len()
    }

    async fn acquire(&self, r: &Ready) -> Result<Claim, DurabilityError> {
        let claim = self
            .durability
            .task_acquire(&r.task_id, r.version, &self.pid, self.lease_ms)
            .await?;
        self.lock()
            .held
            .insert(claim.task_id.clone(), claim.version);
        Ok(claim)
    }

    async fn give_back(&self, claim: &Claim) {
        self.lock().held.remove(&claim.task_id);
        let _ = self
            .durability
            .task_release(&claim.task_id, claim.version)
            .await;
    }

    /// Waits up to `timeout` for a task of `key`; `Ok(None)` when none came.
    /// A claim is held until [`Matcher::finished`] or [`Matcher::release`].
    /// Dropping the returned future (a client that went away) never loses a
    /// task: a claim that reached it is released.
    pub async fn poll(
        self: &Arc<Self>,
        key: &QueueKey,
        timeout: Duration,
    ) -> Result<Option<Handout>, MatchError> {
        self.ensure_subscribed(key).await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // In-memory items first, then the backlog.
            loop {
                let (local, next) = {
                    let mut inner = self.lock();
                    let q = inner.queues.entry(key.group()).or_default();
                    match q.locals.pop_front() {
                        Some(id) => (Some(id), None),
                        None => (None, q.backlog.pop_front()),
                    }
                };
                if let Some(id) = local {
                    return Ok(Some(Handout::Local(id)));
                }
                let Some(r) = next else { break };
                match self.acquire(&r).await {
                    Ok(claim) => return Ok(Some(Handout::Claim(claim))),
                    Err(DurabilityError::Unavailable(m)) => {
                        self.lock()
                            .queues
                            .entry(key.group())
                            .or_default()
                            .push_ready(r);
                        return Err(MatchError::Unavailable(m));
                    }
                    // Moved on, taken elsewhere, or gone: skip it.
                    Err(_) => {}
                }
            }
            let id = self.next_waiter.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            {
                let mut inner = self.lock();
                let q = inner.queues.entry(key.group()).or_default();
                if !q.backlog.is_empty() || !q.locals.is_empty() {
                    continue;
                }
                q.waiters.push_back((id, tx));
            }
            let mut wait = Wait {
                matcher: self.clone(),
                key: key.clone(),
                id,
                rx: Some(rx),
            };
            let rx = wait.rx.as_mut().expect("set above");
            return match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(h)) => {
                    wait.rx = None;
                    Ok(Some(h))
                }
                Ok(Err(_)) => Ok(None),
                // `wait`'s drop removes the waiter and gives back a claim
                // that arrived as the deadline passed.
                Err(_) => Ok(None),
            };
        }
    }

    /// The gateway finished the task (fulfilled or settled it): stop
    /// keeping its lease.
    pub fn finished(&self, task_id: &str) {
        self.lock().held.remove(task_id);
    }

    /// Gives a claim back to Resonate (the poll that took it could not use
    /// it).
    pub async fn release(&self, claim: &Claim) {
        self.give_back(claim).await;
    }

    /// The number of leases held.
    pub fn held(&self) -> usize {
        self.lock().held.len()
    }

    /// Heartbeats every held lease once.
    pub async fn heartbeat_once(&self) -> Result<(), DurabilityError> {
        let held: Vec<(String, i64)> = self
            .lock()
            .held
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        if held.is_empty() {
            return Ok(());
        }
        let tasks: Vec<(&str, i64)> = held.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        self.durability.task_heartbeat(&self.pid, &tasks).await
    }

    /// Keeps the held leases alive, every third of the lease, until
    /// `shutdown`.
    pub async fn run_leases(self: Arc<Self>, shutdown: impl std::future::Future<Output = ()>) {
        let every = Duration::from_millis((self.lease_ms / 3).max(100) as u64);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return,
                () = tokio::time::sleep(every) => {
                    if let Err(e) = self.heartbeat_once().await {
                        tracing::warn!(error = %e, "lease heartbeat failed");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod poller_tests {
    use super::*;

    #[tokio::test]
    async fn pollers_are_listed_newest_first_and_expire() {
        let m = Matcher::new(
            Arc::new(loams_temporal_resonate::FakeDurability::new()),
            "p",
            30_000,
        );
        let key = QueueKey {
            namespace_id: "ns".into(),
            task_queue: "q".into(),
            kind: TaskKind::Workflow,
        };
        m.record_poller(&key, "w1", "", 1_000);
        m.record_poller(&key, "w2", "b2", 2_000);
        m.record_poller(&key, "w1", "", 3_000);
        let p = m.pollers(&key, 3_000);
        assert_eq!(
            p.iter().map(|p| p.identity.as_str()).collect::<Vec<_>>(),
            ["w1", "w2"]
        );
        assert_eq!(p[1].build_id, "b2");
        assert!(m.pollers(&key, 3_000 + POLLER_TTL_MS + 1).is_empty());
        assert_eq!(m.backlog(&key), 0);
    }
}
