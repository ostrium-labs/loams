//! [`FakeDurability`]: the port in process, deterministic, on a manual clock.
//!
//! It follows Resonate's reference model at the pinned revision (31ffb201,
//! `resonate-oracle`), checked against a real server by `tests/profile.rs`:
//!
//! - A promise with `resonate:target` carries a task with the same ID,
//!   created `pending` at version 0 and dispatched (`execute`) at once, or,
//!   with `resonate:delay`, when the delay ends.
//! - A pending task is dispatched again every 30 s (`PENDING_RETRY_TTL`)
//!   until it is acquired or fulfilled. An acquire moves the version on by
//!   one; a lease expiry or a release makes it pending at that version and
//!   dispatches it.
//! - An awaitable promise (a task, a timer, `resonate:scope = global`)
//!   settles at its deadline: a timer **resolves**, anything else is
//!   `rejected_timedout`. Any other promise times out only when a call next
//!   names it. Settling a task's promise fulfils the task.
//! - A listener needs an awaitable promise (`422` otherwise), fires once and
//!   is dropped; on a settled promise none is registered (the port then
//!   delivers to the local subscription, see [`crate::Durability`]).
//! - A message for a group nobody subscribes to is lost, as the poll
//!   transport loses it.
//!
//! Guessed rather than read (and not observable through the port): the
//! order of deliveries due at the same instant (promise deadlines, then
//! lease expiries, then redispatches, by ID within each, as the model's
//! `debug.tick` does them).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::port::{
    Claim, CreatePromise, DelaySupport, Delivery, Durability, DurabilityError, PromiseStatus,
    PromiseView, SettleAs, TAG_SCOPE, TAG_TARGET, TAG_TIMER, valid_group,
};

/// How long a pending task waits before it is dispatched again (31ffb201
/// `resonate-oracle::PENDING_RETRY_TTL`).
pub const PENDING_RETRY_TTL_MS: i64 = 30_000;

/// The fake's clock starts here unless told otherwise (2023-11-14).
pub const DEFAULT_START_MS: i64 = 1_700_000_000_000;

/// A task's state as the fake holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeTaskState {
    Pending,
    Acquired,
    Fulfilled,
}

/// A task, for tests to inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTask {
    pub state: FakeTaskState,
    pub version: i64,
    pub pid: Option<String>,
}

/// A fault applied to the next delivery (any group).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFault {
    /// Lose it.
    Drop,
    /// Deliver it twice.
    Duplicate,
    /// Hold it until the clock has moved on by this many ms.
    Delay(i64),
}

#[derive(Debug, Clone, Copy)]
enum CallFault {
    /// Answer `Unavailable` without applying the call.
    Before,
    /// Apply the call, then answer `Unavailable`.
    After,
}

#[derive(Debug)]
struct P {
    status: PromiseStatus,
    param: Option<Vec<u8>>,
    tags: BTreeMap<String, String>,
    timeout_at: i64,
    settled_at: Option<i64>,
    listeners: BTreeSet<String>,
}

impl P {
    fn awaitable(&self) -> bool {
        self.tags.contains_key(TAG_TARGET)
            || self.tags.get(TAG_TIMER).map(String::as_str) == Some("true")
            || self.tags.get(TAG_SCOPE).map(String::as_str) == Some("global")
            || self.tags.get("resonate:external").map(String::as_str) == Some("true")
    }

    fn timeout_status(&self) -> PromiseStatus {
        if self.tags.get(TAG_TIMER).map(String::as_str) == Some("true") {
            PromiseStatus::Resolved
        } else {
            PromiseStatus::RejectedTimedout
        }
    }

    fn view(&self, id: &str) -> PromiseView {
        PromiseView {
            id: id.to_string(),
            status: self.status,
            param: self.param.clone(),
            tags: self.tags.clone(),
            timeout_at_ms: self.timeout_at,
            settled_at_ms: self.settled_at,
        }
    }
}

#[derive(Debug)]
struct T {
    state: FakeTaskState,
    version: i64,
    pid: Option<String>,
    ttl: Option<i64>,
    lease_at: Option<i64>,
    retry_at: Option<i64>,
}

#[derive(Debug)]
struct State {
    now: i64,
    promises: BTreeMap<String, P>,
    tasks: BTreeMap<String, T>,
    subs: BTreeMap<String, mpsc::Sender<Delivery>>,
    delivery_faults: VecDeque<DeliveryFault>,
    call_faults: VecDeque<CallFault>,
    held: Vec<(i64, String, Delivery)>,
    calls: Vec<String>,
    ignores_delay: bool,
}

/// The in-process, deterministic [`Durability`] (module docs). Clones share
/// one state.
#[derive(Debug, Clone)]
pub struct FakeDurability {
    inner: Arc<Mutex<State>>,
}

impl Default for FakeDurability {
    fn default() -> Self {
        Self::new()
    }
}

fn not_found() -> DurabilityError {
    DurabilityError::NotFound
}

fn conflict(m: &str) -> DurabilityError {
    DurabilityError::Conflict(m.to_string())
}

impl State {
    fn send(&mut self, group: &str, d: Delivery) {
        match self.delivery_faults.pop_front() {
            Some(DeliveryFault::Drop) => {}
            Some(DeliveryFault::Duplicate) => {
                self.push(group, d.clone());
                self.push(group, d);
            }
            Some(DeliveryFault::Delay(ms)) => self.held.push((self.now + ms, group.to_string(), d)),
            None => self.push(group, d),
        }
    }

    fn push(&mut self, group: &str, d: Delivery) {
        if let Some(tx) = self.subs.get(group) {
            // A full or closed channel loses the message, as the poll
            // transport's `try_send` does.
            let _ = tx.try_send(d);
        }
    }

    fn execute(&mut self, id: &str, version: i64) {
        let Some(group) = self
            .promises
            .get(id)
            .and_then(|p| p.tags.get(TAG_TARGET))
            .and_then(|a| a.strip_prefix("poll://any@"))
            .map(str::to_string)
        else {
            return;
        };
        self.send(
            &group,
            Delivery::TaskReady {
                task_id: id.to_string(),
                version,
                promise_id: id.to_string(),
            },
        );
    }

    /// Fulfil the task and fire the listeners of a promise that just settled.
    fn trigger(&mut self, id: &str) {
        if let Some(t) = self.tasks.get_mut(id)
            && t.state != FakeTaskState::Fulfilled
        {
            t.state = FakeTaskState::Fulfilled;
            t.pid = None;
            t.ttl = None;
            t.lease_at = None;
            t.retry_at = None;
        }
        let Some(p) = self.promises.get_mut(id) else {
            return;
        };
        let status = p.status;
        let listeners = std::mem::take(&mut p.listeners);
        for group in listeners {
            self.send(
                &group,
                Delivery::Settled {
                    promise_id: id.to_string(),
                    status,
                },
            );
        }
    }

    /// A pending promise past its deadline times out when a call names it.
    fn try_timeout(&mut self, id: &str) {
        let now = self.now;
        if let Some(p) = self.promises.get_mut(id)
            && p.status == PromiseStatus::Pending
            && now >= p.timeout_at
        {
            p.status = p.timeout_status();
            p.settled_at = Some(p.timeout_at);
            self.trigger(id);
        }
    }

    fn next_due(&self) -> Option<i64> {
        let promises = self
            .promises
            .values()
            .filter(|p| p.status == PromiseStatus::Pending && p.awaitable())
            .map(|p| p.timeout_at);
        let tasks = self.tasks.values().filter_map(|t| match t.state {
            FakeTaskState::Acquired => t.lease_at,
            FakeTaskState::Pending => t.retry_at,
            FakeTaskState::Fulfilled => None,
        });
        let held = self.held.iter().map(|(at, _, _)| *at);
        promises.chain(tasks).chain(held).min()
    }

    /// Everything due at `self.now`, in the model's order.
    fn tick(&mut self) {
        let now = self.now;
        let expired: Vec<String> = self
            .promises
            .iter()
            .filter(|(_, p)| p.status == PromiseStatus::Pending && p.awaitable())
            .filter(|(_, p)| p.timeout_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            if let Some(p) = self.promises.get_mut(id) {
                p.status = p.timeout_status();
                p.settled_at = Some(p.timeout_at);
            }
        }
        for id in &expired {
            self.trigger(id);
        }
        let leases: Vec<(String, i64)> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.state == FakeTaskState::Acquired)
            .filter(|(_, t)| t.lease_at.is_some_and(|at| at <= now))
            .map(|(id, t)| (id.clone(), t.version))
            .collect();
        for (id, version) in leases {
            if let Some(t) = self.tasks.get_mut(&id) {
                t.state = FakeTaskState::Pending;
                t.pid = None;
                t.ttl = None;
                t.lease_at = None;
                t.retry_at = Some(now + PENDING_RETRY_TTL_MS);
            }
            self.execute(&id, version);
        }
        let retries: Vec<(String, i64)> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.state == FakeTaskState::Pending)
            .filter(|(_, t)| t.retry_at.is_some_and(|at| at <= now))
            .map(|(id, t)| (id.clone(), t.version))
            .collect();
        for (id, version) in retries {
            if let Some(t) = self.tasks.get_mut(&id) {
                t.retry_at = Some(now + PENDING_RETRY_TTL_MS);
            }
            self.execute(&id, version);
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|(at, _, _)| *at <= now);
        self.held = later;
        for (_, group, d) in due {
            self.push(&group, d);
        }
    }

    fn create(&mut self, req: &CreatePromise) -> Result<PromiseView, DurabilityError> {
        req.validate()?;
        self.try_timeout(&req.id);
        let now = self.now;
        if !self.promises.contains_key(&req.id) {
            let tags = req.wire_tags();
            let timed_out = now >= req.timeout_at_ms;
            let mut p = P {
                status: PromiseStatus::Pending,
                param: req.param.clone(),
                tags,
                timeout_at: req.timeout_at_ms,
                settled_at: None,
                listeners: BTreeSet::new(),
            };
            if timed_out {
                p.status = p.timeout_status();
                p.settled_at = Some(req.timeout_at_ms);
            }
            self.promises.insert(req.id.clone(), p);
            if req.target_group.is_some() {
                let mut t = T {
                    state: FakeTaskState::Pending,
                    version: 0,
                    pid: None,
                    ttl: None,
                    lease_at: None,
                    retry_at: None,
                };
                if timed_out {
                    t.state = FakeTaskState::Fulfilled;
                    self.tasks.insert(req.id.clone(), t);
                } else {
                    match req.delay_until_ms.filter(|_| !self.ignores_delay) {
                        Some(at) if now < at => {
                            t.retry_at = Some(at);
                            self.tasks.insert(req.id.clone(), t);
                        }
                        _ => {
                            t.retry_at = Some(now + PENDING_RETRY_TTL_MS);
                            self.tasks.insert(req.id.clone(), t);
                            self.execute(&req.id, 0);
                        }
                    }
                }
            }
        }
        let p = self.promises.get(&req.id).ok_or_else(not_found)?;
        if p.param != req.param {
            return Err(DurabilityError::IdConflict { id: req.id.clone() });
        }
        let view = p.view(&req.id);
        match &req.listener_group {
            Some(g) => self.listen(&req.id, g),
            None => Ok(view),
        }
    }

    fn listen(&mut self, id: &str, group: &str) -> Result<PromiseView, DurabilityError> {
        if !valid_group(group) {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: "invalid poll group".into(),
            });
        }
        self.try_timeout(id);
        let p = self.promises.get_mut(id).ok_or_else(not_found)?;
        if !p.awaitable() {
            return Err(DurabilityError::Rejected {
                status: 422,
                message: "Awaited promise is not awaitable".into(),
            });
        }
        let view = p.view(id);
        if p.status == PromiseStatus::Pending {
            p.listeners.insert(group.to_string());
        } else {
            self.push(
                group,
                Delivery::Settled {
                    promise_id: id.to_string(),
                    status: view.status,
                },
            );
        }
        Ok(view)
    }

    fn settle(&mut self, id: &str, state: SettleAs) -> Result<PromiseView, DurabilityError> {
        self.try_timeout(id);
        let now = self.now;
        let p = self.promises.get_mut(id).ok_or_else(not_found)?;
        if p.status.is_settled() {
            return Ok(p.view(id));
        }
        p.status = state.status();
        p.settled_at = Some(now);
        let view = p.view(id);
        self.trigger(id);
        Ok(view)
    }

    fn get(&mut self, id: &str) -> Result<PromiseView, DurabilityError> {
        self.try_timeout(id);
        self.promises
            .get(id)
            .map(|p| p.view(id))
            .ok_or_else(not_found)
    }

    fn acquire(
        &mut self,
        id: &str,
        version: i64,
        pid: &str,
        ttl: i64,
    ) -> Result<Claim, DurabilityError> {
        if ttl < 1 || version < 0 || pid.is_empty() {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: "invalid task.acquire".into(),
            });
        }
        self.try_timeout(id);
        let now = self.now;
        let t = self.tasks.get_mut(id).ok_or_else(not_found)?;
        if t.state != FakeTaskState::Pending {
            return Err(conflict("Task is not pending"));
        }
        if t.version != version {
            return Err(conflict("Version mismatch"));
        }
        t.state = FakeTaskState::Acquired;
        t.version = version + 1;
        t.pid = Some(pid.to_string());
        t.ttl = Some(ttl);
        t.lease_at = Some(now + ttl);
        t.retry_at = None;
        let param = self.promises.get(id).and_then(|p| p.param.clone());
        Ok(Claim {
            task_id: id.to_string(),
            version: version + 1,
            promise_id: id.to_string(),
            param,
        })
    }

    fn heartbeat(&mut self, pid: &str, tasks: &[(&str, i64)]) {
        let now = self.now;
        for (id, version) in tasks {
            let live = self
                .promises
                .get(*id)
                .is_some_and(|p| p.status != PromiseStatus::Pending || p.timeout_at > now);
            if let Some(t) = self.tasks.get_mut(*id)
                && live
                && t.state == FakeTaskState::Acquired
                && t.version == *version
                && t.pid.as_deref() == Some(pid)
                && let Some(ttl) = t.ttl
            {
                t.lease_at = Some(now + ttl);
            }
        }
    }

    fn fulfill(&mut self, id: &str, version: i64, state: SettleAs) -> Result<(), DurabilityError> {
        self.try_timeout(id);
        let t = self.tasks.get(id).ok_or_else(not_found)?;
        if t.state != FakeTaskState::Acquired || t.version != version {
            return Err(conflict("Task version mismatch or invalid state"));
        }
        let settled = self
            .promises
            .get(id)
            .ok_or_else(not_found)?
            .status
            .is_settled();
        if settled {
            self.trigger(id);
            return Ok(());
        }
        self.settle(id, state).map(|_| ())
    }

    fn release(&mut self, id: &str, version: i64) -> Result<(), DurabilityError> {
        self.try_timeout(id);
        let now = self.now;
        let t = self.tasks.get_mut(id).ok_or_else(not_found)?;
        if t.state != FakeTaskState::Acquired || t.version != version {
            return Err(conflict("Task version mismatch or invalid state"));
        }
        t.state = FakeTaskState::Pending;
        t.pid = None;
        t.ttl = None;
        t.lease_at = None;
        t.retry_at = Some(now + PENDING_RETRY_TTL_MS);
        let v = t.version;
        self.execute(id, v);
        Ok(())
    }
}

impl FakeDurability {
    /// A fake whose clock reads [`DEFAULT_START_MS`].
    pub fn new() -> Self {
        Self::starting_at(DEFAULT_START_MS)
    }

    /// A fake whose clock reads `now_ms`.
    pub fn starting_at(now_ms: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                now: now_ms,
                promises: BTreeMap::new(),
                tasks: BTreeMap::new(),
                subs: BTreeMap::new(),
                delivery_faults: VecDeque::new(),
                call_faults: VecDeque::new(),
                held: Vec::new(),
                calls: Vec::new(),
                ignores_delay: false,
            })),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while the lock was held leaves state a test is about to
        // fail on anyway; keep going rather than poison every later call.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The clock, Unix ms.
    pub fn now(&self) -> i64 {
        self.state().now
    }

    /// Move the clock on by `ms`, firing everything that comes due on the
    /// way, each at its own time.
    pub fn advance(&self, ms: i64) {
        let mut s = self.state();
        let target = s.now + ms.max(0);
        while let Some(at) = s.next_due().filter(|at| *at <= target) {
            s.now = s.now.max(at);
            s.tick();
        }
        s.now = target;
        s.tick();
    }

    /// Apply `fault` to the next delivery the fake makes (queued in order).
    pub fn fault_next_delivery(&self, fault: DeliveryFault) {
        self.state().delivery_faults.push_back(fault);
    }

    /// The next `n` port calls answer `Unavailable`; with `applied`, after
    /// taking effect (the answer was lost), otherwise before.
    pub fn fail_next_calls(&self, n: usize, applied: bool) {
        let f = if applied {
            CallFault::After
        } else {
            CallFault::Before
        };
        let mut s = self.state();
        for _ in 0..n {
            s.call_faults.push_back(f);
        }
    }

    /// Forget every promise, task and held delivery (§74 §5.5 rule 7: a
    /// Resonate whose state is gone). Subscriptions and the clock stay.
    pub fn lose_all_state(&self) {
        let mut s = self.state();
        s.promises.clear();
        s.tasks.clear();
        s.held.clear();
    }

    /// A task, for tests.
    pub fn task(&self, id: &str) -> Option<FakeTask> {
        self.state().tasks.get(id).map(|t| FakeTask {
            state: t.state,
            version: t.version,
            pid: t.pid.clone(),
        })
    }

    /// Behave as Resonate's SQL stores do at 31ffb201: ignore
    /// `resonate:delay` (dispatch at create) and report
    /// [`DelaySupport::Defer`], so the relay defers delayed creates.
    pub fn set_ignores_delay(&self, ignores: bool) {
        self.state().ignores_delay = ignores;
    }

    /// How many promises exist.
    pub fn promise_count(&self) -> usize {
        self.state().promises.len()
    }

    /// Every port call made so far, as `"<kind> <id>"`.
    pub fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }

    /// Record the call and apply any call fault around `f`.
    fn call<R>(
        &self,
        what: String,
        f: impl FnOnce(&mut State) -> Result<R, DurabilityError>,
    ) -> Result<R, DurabilityError> {
        let mut s = self.state();
        s.calls.push(what.clone());
        match s.call_faults.pop_front() {
            Some(CallFault::Before) => Err(DurabilityError::Unavailable(format!(
                "injected before {what}"
            ))),
            Some(CallFault::After) => {
                let _ = f(&mut s);
                Err(DurabilityError::Unavailable(format!(
                    "injected after {what}"
                )))
            }
            None => f(&mut s),
        }
    }
}

#[async_trait]
impl Durability for FakeDurability {
    async fn promise_create(&self, req: &CreatePromise) -> Result<PromiseView, DurabilityError> {
        self.call(format!("promise.create {}", req.id), |s| s.create(req))
    }

    async fn promise_settle(
        &self,
        id: &str,
        state: SettleAs,
    ) -> Result<PromiseView, DurabilityError> {
        self.call(format!("promise.settle {id}"), |s| s.settle(id, state))
    }

    async fn promise_register_listener(
        &self,
        id: &str,
        group: &str,
    ) -> Result<PromiseView, DurabilityError> {
        self.call(format!("promise.register_listener {id}"), |s| {
            s.listen(id, group)
        })
    }

    async fn promise_get(&self, id: &str) -> Result<PromiseView, DurabilityError> {
        self.call(format!("promise.get {id}"), |s| s.get(id))
    }

    async fn task_acquire(
        &self,
        task_id: &str,
        version: i64,
        pid: &str,
        ttl_ms: i64,
    ) -> Result<Claim, DurabilityError> {
        self.call(format!("task.acquire {task_id}"), |s| {
            s.acquire(task_id, version, pid, ttl_ms)
        })
    }

    async fn task_heartbeat(
        &self,
        pid: &str,
        tasks: &[(&str, i64)],
    ) -> Result<(), DurabilityError> {
        self.call(format!("task.heartbeat {pid}"), |s| {
            s.heartbeat(pid, tasks);
            Ok(())
        })
    }

    async fn task_fulfill(
        &self,
        task_id: &str,
        version: i64,
        state: SettleAs,
    ) -> Result<(), DurabilityError> {
        self.call(format!("task.fulfill {task_id}"), |s| {
            s.fulfill(task_id, version, state)
        })
    }

    async fn task_release(&self, task_id: &str, version: i64) -> Result<(), DurabilityError> {
        self.call(format!("task.release {task_id}"), |s| {
            s.release(task_id, version)
        })
    }

    fn delay_support(&self) -> DelaySupport {
        if self.state().ignores_delay {
            DelaySupport::Defer
        } else {
            DelaySupport::Native
        }
    }

    fn now_ms(&self) -> i64 {
        self.now()
    }

    async fn subscribe(&self, group: &str) -> Result<mpsc::Receiver<Delivery>, DurabilityError> {
        if !valid_group(group) {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: "invalid poll group".into(),
            });
        }
        let mut s = self.state();
        if s.subs.get(group).is_some_and(|tx| !tx.is_closed()) {
            return Err(conflict("group already subscribed in this process"));
        }
        let (tx, rx) = mpsc::channel(1024);
        s.subs.insert(group.to_string(), tx);
        Ok(rx)
    }
}
