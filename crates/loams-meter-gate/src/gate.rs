//! The gate (design §63 §10, D1125; ME1 Task 18): what a product asks before
//! it spends an entitlement.
//!
//! - [`Gate::check`]: may `cost` units of `feature` be spent for `subject` now?
//! - [`Gate::reserve`]: hold an estimate for work whose cost is known only
//!   after it runs; [`Reservation::settle`] spends the actual amount and
//!   returns the rest, and can emit the usage event with `loamslease`.
//!
//! [`NoGate`] allows everything and counts nothing: a product without the
//! meter behaves exactly as before (D202, D551).
//!
//! [`LeaseGate`] keeps one lease per (subject, feature) on this node in a
//! sharded map of atomics. A **hit** is a read lock on one shard, two map
//! lookups by `&str` and an atomic compare-and-swap: no allocation, no I/O.
//! Leases come from a [`LeaseSource`] (the counter owner's `LeaseService`,
//! §63 §11), refreshed by a background thread when half the chunk is spent or
//! `refresh_after` passes. Only a **cold miss** (no lease, an expired one, or a
//! limiting lease that cannot cover the cost) calls the source on the request
//! path, with a deadline (50 ms); when that fails the gate fails closed with
//! `meter_unavailable` and caches the refusal for a second, so a refused
//! subject cannot load the owner. Task 19 (policy) decides fail-open for paid
//! subjects.
//!
//! Amounts are integer base units (D2180).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::event::UsageEvent;
use crate::policy::{FailClosedLookup, FailOpenLookup};
use crate::sink::{MeterSink, lock};

/// An amount in the feature's base unit (D2180).
pub type Amount = u64;

/// Why the gate refused (§63 §10.1; the strings are `loams-meter-core`'s
/// `RefusalReason::reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefusalReason {
    EntitlementExhausted,
    PaymentMethodRequired,
    SpendCapReached,
    NotEntitled,
    MeterUnavailable,
}

impl RefusalReason {
    /// The wire reason (`entitlement_exhausted`, …).
    pub fn reason(self) -> &'static str {
        match self {
            Self::EntitlementExhausted => "entitlement_exhausted",
            Self::PaymentMethodRequired => "payment_method_required",
            Self::SpendCapReached => "spend_cap_reached",
            Self::NotEntitled => "not_entitled",
            Self::MeterUnavailable => "meter_unavailable",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::EntitlementExhausted => 0,
            Self::PaymentMethodRequired => 1,
            Self::SpendCapReached => 2,
            Self::NotEntitled => 3,
            Self::MeterUnavailable => 4,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => Self::PaymentMethodRequired,
            2 => Self::SpendCapReached,
            3 => Self::NotEntitled,
            4 => Self::MeterUnavailable,
            _ => Self::EntitlementExhausted,
        }
    }
}

impl fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

/// A refusal, which each product maps to its own surface (§63 §17.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub feature: String,
    pub retry_after: Option<Duration>,
}

impl Refusal {
    pub fn new(reason: RefusalReason, feature: &str) -> Self {
        Self {
            reason,
            feature: feature.to_owned(),
            retry_after: None,
        }
    }

    pub fn retry_after(mut self, d: Duration) -> Self {
        self.retry_after = Some(d);
        self
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: entitlement `{}`", self.reason, self.feature)
    }
}

impl std::error::Error for Refusal {}

/// A lease id: up to 10 characters `[a-z0-9]` (the `loamslease` value),
/// held as its base-36 value so a hit copies a `u64`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LeaseId(pub u64);

impl LeaseId {
    /// No lease (work [`NoGate`] admitted).
    pub const NONE: LeaseId = LeaseId(0);

    /// Parses 1..=10 characters of `[a-z0-9]`.
    pub fn parse(s: &str) -> Option<LeaseId> {
        if s.is_empty() || s.len() > 10 {
            return None;
        }
        let mut v: u64 = 0;
        for b in s.bytes() {
            let d = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'z' => b - b'a' + 10,
                _ => return None,
            };
            v = v * 36 + u64::from(d);
        }
        Some(LeaseId(v))
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 13];
        let mut i = buf.len();
        let mut v = self.0;
        loop {
            i -= 1;
            let d = (v % 36) as u8;
            buf[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
            v /= 36;
            if v == 0 {
                break;
            }
        }
        f.write_str(std::str::from_utf8(&buf[i..]).unwrap_or("0"))
    }
}

/// What [`Gate::check`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow {
        lease: LeaseId,
    },
    /// Allowed past the included amount; the usage is billed as overage.
    AllowOverage {
        lease: LeaseId,
    },
    Refuse(Refusal),
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        !matches!(self, Self::Refuse(_))
    }

    /// `Ok(lease)` when allowed.
    pub fn into_result(self) -> Result<LeaseId, Refusal> {
        match self {
            Self::Allow { lease } | Self::AllowOverage { lease } => Ok(lease),
            Self::Refuse(r) => Err(r),
        }
    }
}

/// One request of a batched [`Gate::check_many`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckRequest<'a> {
    pub subject: &'a str,
    pub feature: &'a str,
    pub cost: Amount,
}

/// What products call (§63 §10.1).
pub trait Gate: Send + Sync + fmt::Debug + 'static {
    /// May `cost` units of `feature` be spent for `subject` now? A lease hit
    /// reads local state only.
    fn check(&self, subject: &str, feature: &str, cost: Amount) -> Decision;

    /// Holds `estimate` units for work whose cost is known only after it runs.
    fn reserve(
        &self,
        subject: &str,
        feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal>;

    /// Decides a batch of requests at once (pipelining, BQ1): `out[i]` is
    /// the decision for `requests[i]`, as if each were `check`ed in turn.
    /// A product that drains a socket or a queue in batches calls this once
    /// per batch. The default checks one by one.
    fn check_many(&self, requests: &[CheckRequest<'_>], out: &mut Vec<Decision>) {
        out.clear();
        out.extend(
            requests
                .iter()
                .map(|r| self.check(r.subject, r.feature, r.cost)),
        );
    }
}

impl<T: Gate + ?Sized> Gate for Arc<T> {
    fn check(&self, subject: &str, feature: &str, cost: Amount) -> Decision {
        (**self).check(subject, feature, cost)
    }

    fn reserve(
        &self,
        subject: &str,
        feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal> {
        (**self).reserve(subject, feature, estimate)
    }

    fn check_many(&self, requests: &[CheckRequest<'_>], out: &mut Vec<Decision>) {
        (**self).check_many(requests, out);
    }
}

/// Allows everything, counts nothing (D202, D551).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoGate;

impl Gate for NoGate {
    fn check(&self, _subject: &str, _feature: &str, _cost: Amount) -> Decision {
        Decision::Allow {
            lease: LeaseId::NONE,
        }
    }

    fn reserve(
        &self,
        _subject: &str,
        _feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal> {
        Ok(Reservation {
            slot: None,
            lease: LeaseId::NONE,
            held: estimate,
            overage: false,
        })
    }
}

/// Units held for one piece of work. Dropping it unsettled returns them.
#[derive(Debug)]
pub struct Reservation {
    slot: Option<Arc<Slot>>,
    lease: LeaseId,
    held: Amount,
    overage: bool,
}

impl Reservation {
    pub fn lease(&self) -> LeaseId {
        self.lease
    }

    pub fn held(&self) -> Amount {
        self.held
    }

    /// The work runs past the included amount (billed as overage).
    pub fn is_overage(&self) -> bool {
        self.overage
    }

    /// Spends `actual`: the rest of the hold goes back to the local lease,
    /// and an `actual` above the hold is taken from it (work whose cost is
    /// known after it runs may overshoot by one operation, §63 §11.4).
    pub fn settle(mut self, actual: Amount) -> LeaseId {
        if let Some(slot) = self.slot.take() {
            slot.adjust(self.held, actual);
        }
        self.lease
    }

    /// [`Reservation::settle`], then emits `event` with `loamslease` set.
    pub fn settle_and_emit(self, actual: Amount, event: UsageEvent, sink: &dyn MeterSink) {
        let lease = self.settle(actual);
        let event = if lease == LeaseId::NONE {
            event
        } else {
            event.lease(lease.to_string())
        };
        sink.emit(event);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.adjust(self.held, 0);
        }
    }
}

/// What a node holds and spent on its current lease (§63 §11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseReport {
    pub lease: LeaseId,
    pub granted: Amount,
    pub spent: Amount,
}

/// `LeaseService.AcquireLease`'s request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseRequest<'a> {
    pub subject: &'a str,
    pub feature: &'a str,
    pub node: &'a str,
    pub want: Amount,
    pub held: Option<LeaseReport>,
}

/// A lease from the counter owner (§63 §11.2). Durations are relative so the
/// gate needs no clock agreement with the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseGrant {
    pub lease: LeaseId,
    /// What this node may spend from now on.
    pub amount: Amount,
    /// `false` with `amount = 0` is a refusal the gate caches until
    /// `refresh_after`.
    pub has_access: bool,
    /// Why access is refused, when it is.
    pub refusal: Option<RefusalReason>,
    /// Soft limit, or overage under the cap: the lease only counts and never
    /// refuses locally.
    pub counting: bool,
    /// Usage under this lease is overage ([`Decision::AllowOverage`]).
    pub overage: bool,
    pub ttl: Duration,
    pub refresh_after: Duration,
    pub owner_epoch: u64,
    /// What this node may spend for the subject if the owner later cannot
    /// be reached and this lease has expired: the owner's
    /// [`PolicyTable::when_unreachable`](crate::policy::PolicyTable::when_unreachable)
    /// budget (0 fails closed, the free tier's row).
    pub fail_open: Amount,
}

impl LeaseGrant {
    /// A limiting lease of `amount` (tests, single-node stacks).
    pub fn limit(lease: LeaseId, amount: Amount, owner_epoch: u64) -> Self {
        Self {
            lease,
            amount,
            has_access: true,
            refusal: None,
            counting: false,
            overage: false,
            ttl: Duration::from_secs(30),
            refresh_after: Duration::from_secs(10),
            owner_epoch,
            fail_open: 0,
        }
    }

    /// A refusal for `reason`.
    pub fn refused(reason: RefusalReason, owner_epoch: u64) -> Self {
        Self {
            has_access: false,
            refusal: Some(reason),
            ..Self::limit(LeaseId::NONE, 0, owner_epoch)
        }
    }
}

/// Why the source gave no lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The deadline passed.
    Timeout,
    /// The owner could not be reached or answered with an error.
    Unavailable(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("lease request timed out"),
            Self::Unavailable(e) => write!(f, "lease owner unavailable: {e}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Where leases come from: the client of the counter owner's internal
/// `LeaseService` (ME-svc), routed by the subject's partition. Called on the
/// request path for a cold miss and on the refresh thread otherwise; it must
/// return within `deadline`.
pub trait LeaseSource: Send + Sync + fmt::Debug + 'static {
    fn acquire(
        &self,
        req: &LeaseRequest<'_>,
        deadline: Duration,
    ) -> Result<LeaseGrant, SourceError>;
}

/// Milliseconds on a monotonic clock.
pub trait Clock: Send + Sync + fmt::Debug + 'static {
    fn now_ms(&self) -> u64;
}

/// [`Instant`] since the gate started.
#[derive(Debug)]
pub struct MonotonicClock(Instant);

impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A clock tests move by hand.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

const ACCESS: u8 = 1;
const COUNTING: u8 = 2;
const OVERAGE: u8 = 4;
const REASON_SHIFT: u8 = 3;

/// One (subject, feature)'s lease on this node.
#[derive(Debug)]
pub(crate) struct Slot {
    subject: Box<str>,
    feature: Box<str>,
    /// Units left on the lease; negative after an overshoot.
    remaining: AtomicI64,
    granted: AtomicU64,
    /// Spent on the current lease.
    spent: AtomicU64,
    /// `spent` when the last report was sent.
    spent_at_report: AtomicU64,
    lease: AtomicU64,
    /// 0 until the first grant.
    expires_ms: AtomicU64,
    refresh_ms: AtomicU64,
    state: AtomicU8,
    epoch: AtomicU64,
    last_used_ms: AtomicU64,
    /// A cold-miss acquire is running (single flight).
    fetching: AtomicBool,
    /// A background refresh is queued or running.
    queued: AtomicBool,
    /// Fail-open budget left (§63 §13); [`UNSET`] until a grant or the
    /// [`FailOpenLookup`] sets it.
    fail_open_left: AtomicI64,
    /// The owner missed its deadline: until then, no lease request is sent
    /// on the request path.
    down_until_ms: AtomicU64,
}

/// `fail_open_left` before anything set it.
const UNSET: i64 = i64::MIN;

impl Slot {
    fn new(subject: &str, feature: &str) -> Self {
        Self {
            subject: subject.into(),
            feature: feature.into(),
            remaining: AtomicI64::new(0),
            granted: AtomicU64::new(0),
            spent: AtomicU64::new(0),
            spent_at_report: AtomicU64::new(0),
            lease: AtomicU64::new(0),
            expires_ms: AtomicU64::new(0),
            refresh_ms: AtomicU64::new(0),
            state: AtomicU8::new(0),
            epoch: AtomicU64::new(0),
            last_used_ms: AtomicU64::new(0),
            fetching: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            fail_open_left: AtomicI64::new(UNSET),
            down_until_ms: AtomicU64::new(0),
        }
    }

    /// Marks the slot used at `now`. Writes only when the millisecond
    /// changed, so concurrent hits on one subject do not bounce the line.
    fn touch(&self, now: u64) {
        if self.last_used_ms.load(Ordering::Relaxed) != now {
            self.last_used_ms.store(now, Ordering::Relaxed);
        }
    }

    fn lease_id(&self) -> LeaseId {
        LeaseId(self.lease.load(Ordering::Acquire))
    }

    fn allowed(&self, state: u8) -> Decision {
        let lease = self.lease_id();
        if state & OVERAGE != 0 {
            Decision::AllowOverage { lease }
        } else {
            Decision::Allow { lease }
        }
    }

    fn refusal(&self, state: u8, now: u64) -> Refusal {
        let reason = RefusalReason::from_code(state >> REASON_SHIFT);
        let wait = self.refresh_ms.load(Ordering::Acquire).saturating_sub(now);
        Refusal::new(reason, &self.feature).retry_after(Duration::from_millis(wait.max(1_000)))
    }

    /// Takes `cost` from a limiting lease; `false` if it does not cover it.
    fn take(&self, cost: Amount) -> bool {
        let cost = i64::try_from(cost).unwrap_or(i64::MAX);
        let mut cur = self.remaining.load(Ordering::Acquire);
        loop {
            if cur < cost {
                return false;
            }
            match self.remaining.compare_exchange_weak(
                cur,
                cur - cost,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(now) => cur = now,
            }
        }
    }

    /// Counts `cost` without refusing.
    fn count(&self, cost: Amount) {
        let cost = i64::try_from(cost).unwrap_or(i64::MAX);
        self.remaining.fetch_sub(cost, Ordering::AcqRel);
    }

    /// Settles a hold of `held` at `actual`.
    fn adjust(&self, held: Amount, actual: Amount) {
        if actual >= held {
            let extra = actual - held;
            self.count(extra);
            self.spent.fetch_add(extra, Ordering::AcqRel);
        } else {
            let back = held - actual;
            self.remaining
                .fetch_add(i64::try_from(back).unwrap_or(i64::MAX), Ordering::AcqRel);
            // `spent` counted the whole hold at reserve time.
            let _ = self
                .spent
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |s| {
                    Some(s.saturating_sub(back))
                });
        }
    }

    /// Half the chunk is spent.
    fn half_spent(&self) -> bool {
        let granted = self.granted.load(Ordering::Relaxed);
        let remaining = self.remaining.load(Ordering::Relaxed);
        remaining < i64::try_from(granted / 2).unwrap_or(i64::MAX)
    }

    fn report(&self) -> Option<LeaseReport> {
        let lease = self.lease_id();
        if lease == LeaseId::NONE {
            return None;
        }
        let spent = self.spent.load(Ordering::Acquire);
        self.spent_at_report.store(spent, Ordering::Release);
        Some(LeaseReport {
            lease,
            granted: self.granted.load(Ordering::Acquire),
            spent,
        })
    }

    /// Installs `g` unless it comes from an older owner epoch.
    fn apply(&self, g: &LeaseGrant, now: u64) -> bool {
        if g.owner_epoch < self.epoch.load(Ordering::Acquire) {
            return false;
        }
        let since_report = self
            .spent
            .load(Ordering::Acquire)
            .saturating_sub(self.spent_at_report.load(Ordering::Acquire));
        let amount = i64::try_from(g.amount).unwrap_or(i64::MAX);
        self.remaining.store(
            amount.saturating_sub(i64::try_from(since_report).unwrap_or(i64::MAX)),
            Ordering::Release,
        );
        self.granted.store(g.amount, Ordering::Release);
        self.spent.store(since_report, Ordering::Release);
        self.spent_at_report.store(0, Ordering::Release);
        self.lease.store(g.lease.0, Ordering::Release);
        let mut state = 0;
        if g.has_access {
            state |= ACCESS;
        }
        if g.counting {
            state |= COUNTING;
        }
        if g.overage {
            state |= OVERAGE;
        }
        let reason = g.refusal.unwrap_or(RefusalReason::EntitlementExhausted);
        state |= reason.code() << REASON_SHIFT;
        self.state.store(state, Ordering::Release);
        self.epoch.store(g.owner_epoch, Ordering::Release);
        self.fail_open_left.store(
            i64::try_from(g.fail_open).unwrap_or(i64::MAX),
            Ordering::Release,
        );
        self.down_until_ms.store(0, Ordering::Release);
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.refresh_ms
            .store(now.saturating_add(ms(g.refresh_after)), Ordering::Release);
        self.expires_ms
            .store(now.saturating_add(ms(g.ttl)).max(1), Ordering::Release);
        true
    }

    /// Caches a refusal of `reason` for `for_ms`.
    fn refuse_for(&self, reason: RefusalReason, now: u64, for_ms: u64) {
        self.remaining.store(0, Ordering::Release);
        self.state
            .store(reason.code() << REASON_SHIFT, Ordering::Release);
        self.refresh_ms
            .store(now.saturating_add(for_ms), Ordering::Release);
        self.expires_ms
            .store(now.saturating_add(for_ms).max(1), Ordering::Release);
    }
}

/// The gate's settings.
#[derive(Clone, Debug)]
pub struct GateConfig {
    /// This node's name in lease requests.
    pub node: String,
    /// How long a cold miss waits for a lease (§63 §10.3: 50 ms).
    pub deadline: Duration,
    /// How long a `meter_unavailable` refusal is cached (`Retry-After: 1`).
    pub unavailable_for: Duration,
    /// How often the refresh thread looks for leases past `refresh_after`.
    pub sweep_interval: Duration,
    /// A slot unused this long is not refreshed and is forgotten at expiry.
    pub idle_after: Duration,
    /// Run the refresh thread. Off, refreshes happen only through
    /// [`LeaseGate::refresh_now`] (deterministic tests).
    pub background: bool,
    /// Fail-open budgets for subjects this node holds no grant for
    /// (default: every one fails closed).
    pub fail_open: Arc<dyn FailOpenLookup>,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            node: String::new(),
            deadline: Duration::from_millis(50),
            unavailable_for: Duration::from_secs(1),
            sweep_interval: Duration::from_millis(250),
            idle_after: Duration::from_secs(60),
            background: true,
            fail_open: Arc::new(FailClosedLookup),
        }
    }
}

/// Counters a role exports as metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GateStats {
    pub cold_misses: u64,
    /// `loams_meter_gate_refresh_failures_total`.
    pub refresh_failures: u64,
    pub stale_grants: u64,
    /// Checks allowed fail-open while the owner was unreachable.
    pub fail_open_allowed: u64,
}

const SHARDS: usize = 64;

type Shard = RwLock<HashMap<Box<str>, HashMap<Box<str>, Arc<Slot>>>>;

#[derive(Debug)]
struct Inner {
    shards: Vec<Shard>,
    source: Arc<dyn LeaseSource>,
    clock: Arc<dyn Clock>,
    config: GateConfig,
    refresh_queue: Mutex<RefreshQueue>,
    refresh_wake: Condvar,
    /// Wakes checks waiting for another thread's cold-miss acquire.
    ready: Mutex<()>,
    ready_cv: Condvar,
    cold_misses: AtomicU64,
    refresh_failures: AtomicU64,
    stale_grants: AtomicU64,
    fail_open_allowed: AtomicU64,
}

#[derive(Debug, Default)]
struct RefreshQueue {
    slots: VecDeque<Arc<Slot>>,
    closed: bool,
}

/// The leasing gate (§63 §10.1, §11).
#[derive(Debug)]
pub struct LeaseGate {
    inner: Arc<Inner>,
    refresher: Mutex<Option<JoinHandle<()>>>,
}

impl LeaseGate {
    /// A gate on the monotonic clock, with its refresh thread.
    pub fn new(source: Arc<dyn LeaseSource>, config: GateConfig) -> std::io::Result<Self> {
        Self::with_clock(source, Arc::new(MonotonicClock::default()), config)
    }

    pub fn with_clock(
        source: Arc<dyn LeaseSource>,
        clock: Arc<dyn Clock>,
        config: GateConfig,
    ) -> std::io::Result<Self> {
        let inner = Arc::new(Inner {
            shards: (0..SHARDS).map(|_| RwLock::new(HashMap::new())).collect(),
            source,
            clock,
            config,
            refresh_queue: Mutex::new(RefreshQueue::default()),
            refresh_wake: Condvar::new(),
            ready: Mutex::new(()),
            ready_cv: Condvar::new(),
            cold_misses: AtomicU64::new(0),
            refresh_failures: AtomicU64::new(0),
            stale_grants: AtomicU64::new(0),
            fail_open_allowed: AtomicU64::new(0),
        });
        let refresher = if inner.config.background {
            let worker = Arc::clone(&inner);
            Some(
                std::thread::Builder::new()
                    .name("loams-meter-gate".into())
                    .spawn(move || refresh_loop(&worker))?,
            )
        } else {
            None
        };
        Ok(Self {
            inner,
            refresher: Mutex::new(refresher),
        })
    }

    pub fn stats(&self) -> GateStats {
        GateStats {
            cold_misses: self.inner.cold_misses.load(Ordering::Relaxed),
            refresh_failures: self.inner.refresh_failures.load(Ordering::Relaxed),
            stale_grants: self.inner.stale_grants.load(Ordering::Relaxed),
            fail_open_allowed: self.inner.fail_open_allowed.load(Ordering::Relaxed),
        }
    }

    /// Installs a grant the caller obtained itself (tests, a pushed grant);
    /// `false` if it is from an older owner epoch.
    pub fn install(&self, subject: &str, feature: &str, grant: &LeaseGrant) -> bool {
        let slot = self.inner.slot_or_insert(subject, feature);
        let now = self.inner.clock.now_ms();
        let ok = slot.apply(grant, now);
        if !ok {
            self.inner.stale_grants.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    /// Runs the refresh pass now (tests; the thread does this on its own).
    pub fn refresh_now(&self) {
        self.inner.sweep();
        let queued: Vec<Arc<Slot>> = lock(&self.inner.refresh_queue).slots.drain(..).collect();
        for slot in queued {
            self.inner.refresh(&slot);
        }
    }

    /// Stops the refresh thread; called by `Drop`.
    pub fn close(&self) {
        lock(&self.inner.refresh_queue).closed = true;
        self.inner.refresh_wake.notify_all();
        if let Some(h) = lock(&self.refresher).take() {
            let _ = h.join();
        }
    }
}

impl Drop for LeaseGate {
    fn drop(&mut self) {
        self.close();
    }
}

impl Gate for LeaseGate {
    fn check(&self, subject: &str, feature: &str, cost: Amount) -> Decision {
        // The hit path borrows the slot under the shard's read lock instead
        // of cloning its `Arc`: two fewer atomic read-modify-writes on a
        // cache line every thread checking that subject shares (BQ1-meter).
        if let Some(d) = self.inner.check_hit(subject, feature, cost) {
            return d;
        }
        self.inner.spend(subject, feature, cost).0
    }

    /// The batched hit path: one clock read per batch, one read lock per
    /// shard the batch touches, and one atomic take per (subject, feature)
    /// for the sum of its requests' costs (amortised lease use). A key whose
    /// lease cannot cover the sum, or that has no live lease, falls back to
    /// one `check` per request after the locks are released, so every
    /// decision is one a sequence of `check`s could have produced.
    fn check_many(&self, requests: &[CheckRequest<'_>], out: &mut Vec<Decision>) {
        out.clear();
        out.resize(
            requests.len(),
            Decision::Allow {
                lease: LeaseId::NONE,
            },
        );
        let mut order: Vec<(usize, usize)> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| (shard_of(r.subject, r.feature), i))
            .collect();
        order.sort_unstable_by(|a, b| {
            let (ra, rb) = (&requests[a.1], &requests[b.1]);
            a.0.cmp(&b.0)
                .then_with(|| ra.subject.cmp(rb.subject))
                .then_with(|| ra.feature.cmp(rb.feature))
                .then_with(|| a.1.cmp(&b.1))
        });
        let now = self.inner.clock.now_ms();
        let mut fallback = Vec::new();
        let mut i = 0;
        while i < order.len() {
            let shard_index = order[i].0;
            let shard = self.inner.shards[shard_index]
                .read()
                .unwrap_or_else(|e| e.into_inner());
            while i < order.len() && order[i].0 == shard_index {
                let first = &requests[order[i].1];
                let mut j = i + 1;
                let mut total = first.cost;
                while j < order.len() && order[j].0 == shard_index {
                    let r = &requests[order[j].1];
                    if r.subject != first.subject || r.feature != first.feature {
                        break;
                    }
                    total = total.saturating_add(r.cost);
                    j += 1;
                }
                let group = &order[i..j];
                let decided = shard
                    .get(first.subject)
                    .and_then(|features| features.get(first.feature))
                    .and_then(|slot| {
                        slot.touch(now);
                        if now >= slot.expires_ms.load(Ordering::Acquire) {
                            return None;
                        }
                        self.inner.hit(slot, total, now)
                    });
                match decided {
                    Some(d) => {
                        for &(_, k) in group {
                            out[k] = d.clone();
                        }
                    }
                    None => fallback.extend(group.iter().map(|&(_, k)| k)),
                }
                i = j;
            }
        }
        for k in fallback {
            let r = &requests[k];
            out[k] = self.check(r.subject, r.feature, r.cost);
        }
    }

    fn reserve(
        &self,
        subject: &str,
        feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal> {
        let (decision, slot) = self.inner.spend(subject, feature, estimate);
        let overage = matches!(decision, Decision::AllowOverage { .. });
        let lease = decision.into_result()?;
        Ok(Reservation {
            slot,
            lease,
            held: estimate,
            overage,
        })
    }
}

/// Takes `cost` from `counter` if it covers it.
fn take_from(counter: &AtomicI64, cost: Amount) -> bool {
    let cost = i64::try_from(cost).unwrap_or(i64::MAX);
    let mut cur = counter.load(Ordering::Acquire);
    loop {
        if cur == UNSET || cur < cost {
            return false;
        }
        match counter.compare_exchange_weak(cur, cur - cost, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(now) => cur = now,
        }
    }
}

/// FNV-1a over subject ‖ 0xff ‖ feature: a shard index without allocating.
fn shard_of(subject: &str, feature: &str) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in subject
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(feature.bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h % SHARDS as u64) as usize
}

impl Inner {
    fn lookup(&self, subject: &str, feature: &str) -> Option<Arc<Slot>> {
        let shard = self.shards[shard_of(subject, feature)]
            .read()
            .unwrap_or_else(|e| e.into_inner());
        shard.get(subject)?.get(feature).cloned()
    }

    /// A lease hit decided under the shard's read lock; `None` sends the
    /// caller to [`Inner::spend`] (no slot, expired, or not coverable).
    fn check_hit(&self, subject: &str, feature: &str, cost: Amount) -> Option<Decision> {
        let now = self.clock.now_ms();
        let shard = self.shards[shard_of(subject, feature)]
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let slot = shard.get(subject)?.get(feature)?;
        slot.touch(now);
        if now >= slot.expires_ms.load(Ordering::Acquire) {
            return None;
        }
        self.hit(slot, cost, now)
    }

    fn slot_or_insert(&self, subject: &str, feature: &str) -> Arc<Slot> {
        if let Some(s) = self.lookup(subject, feature) {
            return s;
        }
        let mut shard = self.shards[shard_of(subject, feature)]
            .write()
            .unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            shard
                .entry(subject.into())
                .or_default()
                .entry(feature.into())
                .or_insert_with(|| Arc::new(Slot::new(subject, feature))),
        )
    }

    /// The hit path, falling back to a cold acquire. Returns the slot so a
    /// reservation can settle into it.
    fn spend(&self, subject: &str, feature: &str, cost: Amount) -> (Decision, Option<Arc<Slot>>) {
        let now = self.clock.now_ms();
        let slot = match self.lookup(subject, feature) {
            Some(s) => s,
            None => self.slot_or_insert(subject, feature),
        };
        slot.touch(now);
        if now < slot.expires_ms.load(Ordering::Acquire)
            && let Some(d) = self.hit(&slot, cost, now)
        {
            return (d, Some(slot));
        }
        if now < slot.down_until_ms.load(Ordering::Acquire) {
            // The owner just missed its deadline: answer by the policy
            // without asking again.
            let d = self.owner_unreachable(&slot, cost, now);
            return (d, Some(slot));
        }
        let d = self.cold(&slot, cost);
        (d, Some(slot))
    }

    /// `None` when only the owner can answer (the lease cannot cover `cost`).
    fn hit(&self, slot: &Arc<Slot>, cost: Amount, now: u64) -> Option<Decision> {
        let state = slot.state.load(Ordering::Acquire);
        if state & ACCESS == 0 {
            // A cached refusal: no network until `refresh_after`.
            return Some(Decision::Refuse(slot.refusal(state, now)));
        }
        if state & COUNTING != 0 {
            slot.count(cost);
        } else if !slot.take(cost) {
            return None;
        }
        slot.spent.fetch_add(cost, Ordering::AcqRel);
        if slot.half_spent() || now >= slot.refresh_ms.load(Ordering::Relaxed) {
            self.queue_refresh(slot);
        }
        Some(slot.allowed(state))
    }

    /// A synchronous acquire for `cost`, single-flight per slot.
    fn cold(&self, slot: &Arc<Slot>, cost: Amount) -> Decision {
        self.cold_misses.fetch_add(1, Ordering::Relaxed);
        let deadline = self.config.deadline;
        if slot
            .fetching
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Another thread is asking: wait for it, up to the deadline.
            let started = Instant::now();
            let mut g = lock(&self.ready);
            while slot.fetching.load(Ordering::Acquire) {
                let Some(left) = deadline.checked_sub(started.elapsed()) else {
                    break;
                };
                g = match self.ready_cv.wait_timeout(g, left) {
                    Ok((g, _)) => g,
                    Err(p) => p.into_inner().0,
                };
            }
            drop(g);
            let now = self.clock.now_ms();
            if now < slot.expires_ms.load(Ordering::Acquire)
                && let Some(d) = self.hit(slot, cost, now)
            {
                return d;
            }
            return self.owner_unreachable(slot, cost, now);
        }
        let held = slot.report();
        let want = cost.max(slot.granted.load(Ordering::Acquire)).max(1);
        let result = self.source.acquire(
            &LeaseRequest {
                subject: &slot.subject,
                feature: &slot.feature,
                node: &self.config.node,
                want,
                held,
            },
            deadline,
        );
        let now = self.clock.now_ms();
        let decision = match result {
            Ok(grant) => {
                if !slot.apply(&grant, now) {
                    self.stale_grants.fetch_add(1, Ordering::Relaxed);
                }
                match self.hit(slot, cost, now) {
                    Some(d) => d,
                    // The owner granted less than this one operation needs:
                    // the balance is exhausted for it.
                    None => {
                        let state = slot.state.load(Ordering::Acquire);
                        let reason = if state & ACCESS == 0 {
                            RefusalReason::from_code(state >> REASON_SHIFT)
                        } else {
                            RefusalReason::EntitlementExhausted
                        };
                        Decision::Refuse(
                            Refusal::new(reason, &slot.feature).retry_after(Duration::from_secs(1)),
                        )
                    }
                }
            }
            Err(_) => {
                self.refresh_failures.fetch_add(1, Ordering::Relaxed);
                if now < slot.expires_ms.load(Ordering::Acquire)
                    && slot.state.load(Ordering::Acquire) & ACCESS != 0
                {
                    // The owner is unreachable but this lease still holds:
                    // what it cannot cover is refused like exhaustion.
                    Decision::Refuse(
                        Refusal::new(RefusalReason::EntitlementExhausted, &slot.feature)
                            .retry_after(Duration::from_secs(1)),
                    )
                } else {
                    self.owner_unreachable(slot, cost, now)
                }
            }
        };
        slot.fetching.store(false, Ordering::Release);
        let _g = lock(&self.ready);
        self.ready_cv.notify_all();
        decision
    }

    /// No usable lease and no answer from the owner within the deadline
    /// (§63 §13). A subject with a fail-open budget (paid: the spend-cap
    /// headroom, or one period's included amount) spends it locally; the
    /// owner reconciles it later. Everyone else, and a spent budget, fails
    /// closed with `meter_unavailable`. The owner is not asked again for
    /// `unavailable_for`.
    fn owner_unreachable(&self, slot: &Arc<Slot>, cost: Amount, now: u64) -> Decision {
        let for_ms = u64::try_from(self.config.unavailable_for.as_millis()).unwrap_or(1_000);
        slot.down_until_ms
            .fetch_max(now.saturating_add(for_ms), Ordering::AcqRel);
        if slot.fail_open_left.load(Ordering::Acquire) == UNSET {
            let budget = self
                .config
                .fail_open
                .fail_open_budget(&slot.subject, &slot.feature);
            let _ = slot.fail_open_left.compare_exchange(
                UNSET,
                i64::try_from(budget).unwrap_or(i64::MAX),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        if take_from(&slot.fail_open_left, cost) {
            self.fail_open_allowed.fetch_add(1, Ordering::Relaxed);
            return Decision::Allow {
                lease: LeaseId::NONE,
            };
        }
        if now >= slot.expires_ms.load(Ordering::Acquire) {
            slot.refuse_for(RefusalReason::MeterUnavailable, now, for_ms);
        }
        Decision::Refuse(
            Refusal::new(RefusalReason::MeterUnavailable, &slot.feature)
                .retry_after(self.config.unavailable_for),
        )
    }

    fn queue_refresh(&self, slot: &Arc<Slot>) {
        if slot
            .queued
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            lock(&self.refresh_queue).slots.push_back(Arc::clone(slot));
            self.refresh_wake.notify_one();
        }
    }

    /// Refreshes one slot off the request path.
    fn refresh(&self, slot: &Arc<Slot>) {
        let held = slot.report();
        let want = slot.granted.load(Ordering::Acquire).max(1);
        let result = self.source.acquire(
            &LeaseRequest {
                subject: &slot.subject,
                feature: &slot.feature,
                node: &self.config.node,
                want,
                held,
            },
            self.config.deadline,
        );
        let now = self.clock.now_ms();
        match result {
            Ok(g) => {
                if !slot.apply(&g, now) {
                    self.stale_grants.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                // Keep spending the chunk until it expires (§63 §14).
                self.refresh_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        slot.queued.store(false, Ordering::Release);
    }

    /// Queues slots past `refresh_after` that were used recently, and
    /// forgets idle expired ones.
    fn sweep(&self) {
        let now = self.clock.now_ms();
        let idle = u64::try_from(self.config.idle_after.as_millis()).unwrap_or(u64::MAX);
        let keep = |slot: &Slot| {
            let used = slot.last_used_ms.load(Ordering::Relaxed);
            now.saturating_sub(used) < idle
                || now < slot.expires_ms.load(Ordering::Acquire)
                || slot.fetching.load(Ordering::Acquire)
                || slot.queued.load(Ordering::Acquire)
        };
        let mut due = Vec::new();
        for shard in &self.shards {
            // A read lock finds what is due; the write lock, which stalls
            // every check on the shard, is taken only when a slot is evicted
            // (BQ1-meter: the sweep wrote-locked all 64 shards every 250 ms).
            let evict = {
                let shard = shard.read().unwrap_or_else(|e| e.into_inner());
                let mut evict = false;
                for slot in shard.values().flat_map(HashMap::values) {
                    let used = slot.last_used_ms.load(Ordering::Relaxed);
                    let recent = now.saturating_sub(used) < idle;
                    let expired = now >= slot.expires_ms.load(Ordering::Acquire);
                    if recent && !expired && now >= slot.refresh_ms.load(Ordering::Acquire) {
                        due.push(Arc::clone(slot));
                    }
                    evict |= !keep(slot);
                }
                evict
            };
            if evict {
                let mut shard = shard.write().unwrap_or_else(|e| e.into_inner());
                shard.retain(|_, features| {
                    features.retain(|_, slot| keep(slot));
                    !features.is_empty()
                });
            }
        }
        for slot in due {
            self.queue_refresh(&slot);
        }
    }
}

fn refresh_loop(inner: &Inner) {
    let mut last_sweep = Instant::now();
    loop {
        let next = {
            let mut q = lock(&inner.refresh_queue);
            loop {
                if q.closed {
                    return;
                }
                if let Some(s) = q.slots.pop_front() {
                    break Some(s);
                }
                let left = inner
                    .config
                    .sweep_interval
                    .saturating_sub(last_sweep.elapsed());
                if left.is_zero() {
                    break None;
                }
                q = match inner.refresh_wake.wait_timeout(q, left) {
                    Ok((g, _)) => g,
                    Err(p) => p.into_inner().0,
                };
            }
        };
        match next {
            Some(slot) => inner.refresh(&slot),
            None => {
                inner.sweep();
                last_sweep = Instant::now();
            }
        }
    }
}

/// A source that grants a fixed amount per request (tests, benches, and a
/// single-node stack with no meter role).
#[derive(Debug)]
pub struct FixedSource {
    pub grant: LeaseGrant,
    calls: AtomicU64,
}

impl FixedSource {
    pub fn new(grant: LeaseGrant) -> Self {
        Self {
            grant,
            calls: AtomicU64::new(0),
        }
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LeaseSource for FixedSource {
    fn acquire(
        &self,
        _req: &LeaseRequest<'_>,
        _deadline: Duration,
    ) -> Result<LeaseGrant, SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.grant.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::MemorySink;

    /// Answers from a script; errors when it runs out.
    #[derive(Debug, Default)]
    struct Scripted {
        answers: Mutex<VecDeque<Result<LeaseGrant, SourceError>>>,
        requests: Mutex<Vec<(Amount, Option<LeaseReport>)>>,
    }

    impl Scripted {
        fn push(&self, a: Result<LeaseGrant, SourceError>) {
            lock(&self.answers).push_back(a);
        }

        fn calls(&self) -> usize {
            lock(&self.requests).len()
        }
    }

    impl LeaseSource for Scripted {
        fn acquire(
            &self,
            req: &LeaseRequest<'_>,
            _deadline: Duration,
        ) -> Result<LeaseGrant, SourceError> {
            lock(&self.requests).push((req.want, req.held));
            lock(&self.answers)
                .pop_front()
                .unwrap_or(Err(SourceError::Unavailable("script ended".into())))
        }
    }

    fn gate(source: Arc<Scripted>) -> (LeaseGate, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::default());
        clock.set(1_000);
        let g = LeaseGate::with_clock(
            source,
            clock.clone(),
            GateConfig {
                node: "n1".into(),
                // The test drives refreshes itself.
                background: false,
                ..GateConfig::default()
            },
        )
        .unwrap();
        (g, clock)
    }

    #[test]
    fn check_many_amortises_and_matches_sequential_checks() {
        let (g, _clock) = gate(Arc::new(Scripted::default()));
        let grant = LeaseGrant {
            ttl: Duration::from_secs(3600),
            refresh_after: Duration::from_secs(3600),
            ..LeaseGrant::limit(LeaseId(7), 10, 1)
        };
        g.install("org_a", "f", &grant);
        let remaining = |g: &LeaseGate| {
            g.inner
                .lookup("org_a", "f")
                .unwrap()
                .remaining
                .load(Ordering::Acquire)
        };
        let req = |subject, cost| CheckRequest {
            subject,
            feature: "f",
            cost,
        };
        let mut out = Vec::new();
        // Four requests on one lease plus one subject with no lease (the
        // scripted owner is unreachable, so it fails closed).
        g.check_many(
            &[
                req("org_a", 2),
                req("org_b", 1),
                req("org_a", 2),
                req("org_a", 2),
                req("org_a", 2),
            ],
            &mut out,
        );
        assert_eq!(out.len(), 5);
        for k in [0, 2, 3, 4] {
            assert_eq!(out[k], Decision::Allow { lease: LeaseId(7) }, "request {k}");
        }
        assert!(matches!(
            &out[1],
            Decision::Refuse(r) if r.reason == RefusalReason::MeterUnavailable
        ));
        assert_eq!(remaining(&g), 2);
        // The sum (3) exceeds what is left (2): each request is decided on
        // its own, as sequential checks would be.
        g.check_many(
            &[req("org_a", 1), req("org_a", 1), req("org_a", 1)],
            &mut out,
        );
        assert!(out[0].is_allowed() && out[1].is_allowed());
        assert!(!out[2].is_allowed());
        assert_eq!(remaining(&g), 0);
        // An empty batch is an empty answer.
        g.check_many(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn sweep_evicts_only_idle_expired_slots() {
        let (g, clock) = gate(Arc::new(Scripted::default()));
        let grant = LeaseGrant {
            ttl: Duration::from_secs(1),
            refresh_after: Duration::from_secs(3600),
            ..LeaseGrant::limit(LeaseId(1), 100, 1)
        };
        g.install("org_idle", "f", &grant);
        g.install("org_busy", "f", &grant);
        assert!(
            g.check("org_busy", "f", 1).is_allowed(),
            "a hit under the read lock"
        );
        let slots = |g: &LeaseGate| {
            g.inner
                .shards
                .iter()
                .map(|s| s.read().unwrap().values().map(HashMap::len).sum::<usize>())
                .sum::<usize>()
        };
        g.refresh_now();
        assert_eq!(slots(&g), 2, "nothing idle yet");
        // Past the TTL and `idle_after`: only the subject still checked stays.
        clock.advance(Duration::from_secs(61));
        let _ = g.check("org_busy", "f", 1);
        g.refresh_now();
        assert_eq!(slots(&g), 1);
        assert!(g.inner.lookup("org_busy", "f").is_some());
        assert!(g.inner.lookup("org_idle", "f").is_none());
    }

    #[test]
    fn no_gate_allows_everything() {
        assert!(NoGate.check("org_1", "f", u64::MAX).is_allowed());
        let r = NoGate.reserve("org_1", "f", 10).unwrap();
        assert_eq!(r.settle(5), LeaseId::NONE);
    }

    #[test]
    fn lease_id_round_trips() {
        for s in ["l9k2m4q8x1", "a", "0", "zzzzzzzzzz"] {
            assert_eq!(LeaseId::parse(s).unwrap().to_string(), s);
        }
        assert!(LeaseId::parse("ABC").is_none());
        assert!(LeaseId::parse("abcdefghijk").is_none());
    }

    #[test]
    fn hit_spends_locally_and_refuses_at_zero() {
        let src = Arc::new(Scripted::default());
        let lease = LeaseId::parse("abc").unwrap();
        src.push(Ok(LeaseGrant::limit(lease, 10, 1)));
        // The owner has nothing more.
        src.push(Ok(LeaseGrant::refused(
            RefusalReason::EntitlementExhausted,
            1,
        )));
        let (g, _) = gate(src.clone());
        assert_eq!(
            g.check("org_1", "api_requests", 4),
            Decision::Allow { lease }
        );
        assert_eq!(
            g.check("org_1", "api_requests", 6),
            Decision::Allow { lease }
        );
        assert_eq!(src.calls(), 1, "hits do not call the owner");
        let d = g.check("org_1", "api_requests", 1);
        let Decision::Refuse(r) = &d else {
            panic!("expected a refusal, got {d:?}")
        };
        assert_eq!(r.reason, RefusalReason::EntitlementExhausted);
        assert_eq!(r.feature, "api_requests");
        // The refusal is cached: no more calls until refresh_after.
        assert!(!g.check("org_1", "api_requests", 1).is_allowed());
        assert!(!g.check("org_1", "api_requests", 1).is_allowed());
        assert_eq!(src.calls(), 2);
    }

    #[test]
    fn refused_subject_calls_owner_once_per_interval() {
        let src = Arc::new(Scripted::default());
        src.push(Ok(LeaseGrant::refused(RefusalReason::NotEntitled, 1)));
        src.push(Ok(LeaseGrant::refused(RefusalReason::NotEntitled, 1)));
        let (g, clock) = gate(src.clone());
        for _ in 0..1_000 {
            let d = g.check("org_1", "byoc", 0);
            assert!(matches!(
                d,
                Decision::Refuse(Refusal {
                    reason: RefusalReason::NotEntitled,
                    ..
                })
            ));
        }
        assert_eq!(src.calls(), 1);
        clock.advance(Duration::from_secs(11));
        g.refresh_now();
        assert_eq!(src.calls(), 2);
    }

    #[test]
    fn owner_unreachable_fails_closed_and_caches() {
        let src = Arc::new(Scripted::default());
        let (g, clock) = gate(src.clone());
        let d = g.check("org_1", "house_cpu", 1);
        let Decision::Refuse(r) = &d else {
            panic!("expected a refusal, got {d:?}")
        };
        assert_eq!(r.reason, RefusalReason::MeterUnavailable);
        assert_eq!(r.retry_after, Some(Duration::from_secs(1)));
        assert!(!g.check("org_1", "house_cpu", 1).is_allowed());
        assert_eq!(src.calls(), 1, "the refusal is cached for a second");
        clock.advance(Duration::from_millis(1_001));
        assert!(!g.check("org_1", "house_cpu", 1).is_allowed());
        assert_eq!(src.calls(), 2);
    }

    #[test]
    fn background_refresh_reports_spent_and_extends() {
        let src = Arc::new(Scripted::default());
        let l1 = LeaseId(1);
        let l2 = LeaseId(2);
        src.push(Ok(LeaseGrant::limit(l1, 10, 1)));
        src.push(Ok(LeaseGrant::limit(l2, 10, 1)));
        let (g, _) = gate(src.clone());
        assert!(g.check("s", "f", 6).is_allowed()); // half spent: queued
        g.refresh_now();
        let reqs = lock(&src.requests).clone();
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[1].1,
            Some(LeaseReport {
                lease: l1,
                granted: 10,
                spent: 6
            })
        );
        assert_eq!(g.check("s", "f", 10), Decision::Allow { lease: l2 });
    }

    #[test]
    fn stale_epoch_grant_is_ignored() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        assert!(g.install("s", "f", &LeaseGrant::limit(LeaseId(5), 100, 7)));
        assert!(!g.install("s", "f", &LeaseGrant::limit(LeaseId(6), 1_000, 6)));
        assert_eq!(g.check("s", "f", 1), Decision::Allow { lease: LeaseId(5) });
        assert_eq!(g.stats().stale_grants, 1);
    }

    #[test]
    fn counting_lease_never_refuses_and_reports_overage() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        let grant = LeaseGrant {
            counting: true,
            overage: true,
            ..LeaseGrant::limit(LeaseId(9), 5, 1)
        };
        g.install("s", "llm_output_tokens", &grant);
        for _ in 0..10 {
            assert_eq!(
                g.check("s", "llm_output_tokens", 3),
                Decision::AllowOverage { lease: LeaseId(9) }
            );
        }
    }

    #[test]
    fn reservation_returns_the_unspent_part() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        g.install(
            "s",
            "llm_output_tokens",
            &LeaseGrant::limit(LeaseId(3), 1_000, 1),
        );
        let r = g.reserve("s", "llm_output_tokens", 800).unwrap();
        assert!(g.reserve("s", "llm_output_tokens", 800).is_err());
        let sink = MemorySink::default();
        let ev = UsageEvent::new(crate::event::types::LLM_TOKENS, "req-1");
        r.settle_and_emit(100, ev, &sink);
        assert_eq!(sink.events()[0].lease.as_deref(), Some("3"));
        // 900 left: a second large hold fits.
        assert!(g.reserve("s", "llm_output_tokens", 900).is_ok());
    }

    #[test]
    fn dropped_reservation_gives_everything_back() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        g.install("s", "f", &LeaseGrant::limit(LeaseId(3), 100, 1));
        drop(g.reserve("s", "f", 100).unwrap());
        assert!(g.check("s", "f", 100).is_allowed());
    }

    #[test]
    fn settle_above_the_hold_overshoots_by_that_operation() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        g.install("s", "house_cpu", &LeaseGrant::limit(LeaseId(3), 100, 1));
        let r = g.reserve("s", "house_cpu", 1).unwrap();
        r.settle(150);
        // Nothing left locally; the next statement is refused by the owner
        // (the script is empty, so this is meter_unavailable here).
        assert!(!g.check("s", "house_cpu", 1).is_allowed());
    }

    #[test]
    fn concurrent_checks_never_exceed_the_lease() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate(src);
        let g = Arc::new(g);
        g.install("s", "f", &LeaseGrant::limit(LeaseId(1), 10_000, 1));
        let allowed = Arc::new(AtomicU64::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let g = Arc::clone(&g);
                let allowed = Arc::clone(&allowed);
                std::thread::spawn(move || {
                    for _ in 0..2_000 {
                        if g.check("s", "f", 1).is_allowed() {
                            allowed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(allowed.load(Ordering::Relaxed), 10_000);
    }

    /// A fixed fail-open budget for every subject.
    #[derive(Debug)]
    struct Budget(Amount);

    impl FailOpenLookup for Budget {
        fn fail_open_budget(&self, _subject: &str, _feature: &str) -> Amount {
            self.0
        }
    }

    fn gate_with(
        source: Arc<Scripted>,
        lookup: Arc<dyn FailOpenLookup>,
    ) -> (LeaseGate, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::default());
        clock.set(1_000);
        let g = LeaseGate::with_clock(
            source,
            clock.clone(),
            GateConfig {
                node: "n1".into(),
                background: false,
                fail_open: lookup,
                ..GateConfig::default()
            },
        )
        .unwrap();
        (g, clock)
    }

    #[test]
    fn free_fails_closed_when_owner_unreachable() {
        let src = Arc::new(Scripted::default());
        // A free subject's last grant carried no fail-open budget.
        src.push(Ok(LeaseGrant::limit(LeaseId(1), 10, 1)));
        let (g, clock) = gate_with(src.clone(), Arc::new(FailClosedLookup));
        assert!(g.check("org_free", "house_cpu", 1).is_allowed());
        clock.advance(Duration::from_secs(31)); // the lease expired
        let d = g.check("org_free", "house_cpu", 1);
        assert!(matches!(
            d,
            Decision::Refuse(Refusal {
                reason: RefusalReason::MeterUnavailable,
                ..
            })
        ));
        assert_eq!(g.stats().fail_open_allowed, 0);
    }

    #[test]
    fn paid_fail_open_bounded_by_cap() {
        let src = Arc::new(Scripted::default());
        src.push(Ok(LeaseGrant {
            fail_open: 100,
            ..LeaseGrant::limit(LeaseId(1), 10, 1)
        }));
        let (g, clock) = gate_with(src.clone(), Arc::new(FailClosedLookup));
        assert!(g.check("org_paid", "house_cpu", 1).is_allowed());
        clock.advance(Duration::from_secs(31));
        // Owner unreachable: 100 units of headroom, spent locally.
        for _ in 0..3 {
            assert_eq!(
                g.check("org_paid", "house_cpu", 30),
                Decision::Allow {
                    lease: LeaseId::NONE
                }
            );
        }
        let d = g.check("org_paid", "house_cpu", 30);
        assert!(matches!(
            d,
            Decision::Refuse(Refusal {
                reason: RefusalReason::MeterUnavailable,
                ..
            })
        ));
        assert_eq!(g.stats().fail_open_allowed, 3);
        // One owner call for the lease, one when it was found unreachable;
        // the rest were answered locally.
        assert_eq!(src.calls(), 2);
    }

    #[test]
    fn unknown_subject_uses_the_lookup_budget() {
        let src = Arc::new(Scripted::default());
        let (g, _) = gate_with(src, Arc::new(Budget(50)));
        assert!(g.check("org_new", "api_requests", 50).is_allowed());
        assert!(!g.check("org_new", "api_requests", 1).is_allowed());
    }

    #[test]
    fn shards_spread_keys() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..1_000 {
            seen.insert(shard_of(&format!("org_{i}"), "house_cpu"));
        }
        assert!(seen.len() > SHARDS / 2);
    }
}
