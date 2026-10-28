//! Per-namespace quotas (HS1 Task 22, design §49 §15).
//!
//! A namespace's quotas come from a [`QuotaSource`]: the front's configuration
//! now ([`ConfigQuotas`], `[house.quotas]`), the control plane's limits API
//! later (§27 §3.5, D202: enforcement stays in the engine, the limits come from
//! configuration or a control plane). Every quota is optional; an unset one
//! does not limit.
//!
//! | Quota | Counted from | Past it |
//! |---|---|---|
//! | `concurrent_queries` | statements admitted and not yet ended | `202` |
//! | `queries_per_minute` | statements admitted in the last 60 s | `202` |
//! | `scan_bytes_per_day` | `house.query`'s bytes scanned, this UTC day | `202` for reads, `164` for writes |
//! | `cpu_seconds_per_day` | `house.query`'s CPU time, this UTC day | `202` for reads, `164` for writes |
//! | `insert_bytes_per_day` | `house.insert`'s bytes, this UTC day | `164` (writes only) |
//! | `storage_bytes` | the last `house.storage` round's bytes, plus the bytes inserted since | `164` (writes only) |
//! | `pipes` | the namespace's pipes, when a `CREATE` adds one | `164` |
//!
//! [`Quotas::admit`] runs before [`crate::admission::Admission`]'s queue: a
//! statement past a quota is refused at once, never queued. A volume quota is
//! checked against what was used before the statement starts, so the statement
//! that crosses it runs to its end and the next one is refused. Every refusal's
//! message starts with [`QUOTA_EXCEEDED`] and names the quota and its limit
//! ([`refusal_parts`] reads them back for the API's `ErrorInfo`); none uses
//! `241`, which is one query's memory.
//!
//! The counters live in the front's memory: a restarted front starts the day
//! from zero, and two fronts count apart (the control plane, which sees every
//! front's usage events, is where a shared count belongs).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use loams_meter_gate::entitle::{Entitlements, features};
use loams_meter_gate::gate::Refusal;

use crate::errors::{ChError, HouseError};
use crate::usage::UsageEvent;

/// The first words of every quota refusal (§49 §15; the API's reason).
pub const QUOTA_EXCEEDED: &str = "house_quota_exceeded";

/// The window `queries_per_minute` counts over.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);

/// One namespace's quotas; `None` does not limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamespaceQuota {
    /// Statements running at once.
    pub concurrent_queries: Option<u64>,
    /// Statements started in any 60 s.
    pub queries_per_minute: Option<u64>,
    /// Bytes statements read from storage in a UTC day.
    pub scan_bytes_per_day: Option<u64>,
    /// Workers' CPU seconds in a UTC day.
    pub cpu_seconds_per_day: Option<u64>,
    /// Bytes in the namespace's lake tables.
    pub storage_bytes: Option<u64>,
    /// Bytes inserted into lake tables in a UTC day.
    pub insert_bytes_per_day: Option<u64>,
    /// Pipes.
    pub pipes: Option<u64>,
}

impl NamespaceQuota {
    /// `self` with every quota `over` sets replaced.
    pub fn overlaid(self, over: &NamespaceQuota) -> Self {
        Self {
            concurrent_queries: over.concurrent_queries.or(self.concurrent_queries),
            queries_per_minute: over.queries_per_minute.or(self.queries_per_minute),
            scan_bytes_per_day: over.scan_bytes_per_day.or(self.scan_bytes_per_day),
            cpu_seconds_per_day: over.cpu_seconds_per_day.or(self.cpu_seconds_per_day),
            storage_bytes: over.storage_bytes.or(self.storage_bytes),
            insert_bytes_per_day: over.insert_bytes_per_day.or(self.insert_bytes_per_day),
            pipes: over.pipes.or(self.pipes),
        }
    }
}

/// Where a namespace's quotas come from. Called before every statement: it
/// must answer from memory, quickly.
pub trait QuotaSource: Send + Sync + std::fmt::Debug {
    /// The quotas of `namespace` now.
    fn quota(&self, namespace: &str) -> NamespaceQuota;
}

/// Quotas from the front's configuration: one default for every namespace,
/// and per-namespace overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigQuotas {
    /// Every namespace's.
    pub default: NamespaceQuota,
    /// Namespace id → what it sets over the default.
    pub namespaces: BTreeMap<String, NamespaceQuota>,
}

impl QuotaSource for ConfigQuotas {
    fn quota(&self, namespace: &str) -> NamespaceQuota {
        match self.namespaces.get(namespace) {
            Some(over) => self.default.overlaid(over),
            None => self.default,
        }
    }
}

/// `[house.quotas]`: every namespace's quotas, and
/// `[house.quotas.namespaces.<id>]` over them.
///
/// ```toml
/// [house.quotas]
/// queries_per_minute = 600
/// [house.quotas.namespaces.7]
/// scan_bytes_per_day = 1099511627776
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotasSection {
    /// [`NamespaceQuota::concurrent_queries`].
    pub concurrent_queries: Option<u64>,
    /// [`NamespaceQuota::queries_per_minute`].
    pub queries_per_minute: Option<u64>,
    /// [`NamespaceQuota::scan_bytes_per_day`].
    pub scan_bytes_per_day: Option<u64>,
    /// [`NamespaceQuota::cpu_seconds_per_day`].
    pub cpu_seconds_per_day: Option<u64>,
    /// [`NamespaceQuota::storage_bytes`].
    pub storage_bytes: Option<u64>,
    /// [`NamespaceQuota::insert_bytes_per_day`].
    pub insert_bytes_per_day: Option<u64>,
    /// [`NamespaceQuota::pipes`].
    pub pipes: Option<u64>,
    /// Namespace id → its quotas over these.
    #[serde(default)]
    pub namespaces: BTreeMap<String, NamespaceQuota>,
}

impl QuotasSection {
    /// The quotas, or why a namespace key is not a namespace id.
    pub fn to_quotas(&self) -> Result<ConfigQuotas, String> {
        for id in self.namespaces.keys() {
            if id.parse::<u64>().is_err() {
                return Err(format!(
                    "[house.quotas.namespaces] {id:?} is not a namespace id"
                ));
            }
        }
        Ok(ConfigQuotas {
            default: NamespaceQuota {
                concurrent_queries: self.concurrent_queries,
                queries_per_minute: self.queries_per_minute,
                scan_bytes_per_day: self.scan_bytes_per_day,
                cpu_seconds_per_day: self.cpu_seconds_per_day,
                storage_bytes: self.storage_bytes,
                insert_bytes_per_day: self.insert_bytes_per_day,
                pipes: self.pipes,
            },
            namespaces: self.namespaces.clone(),
        })
    }

    /// Whether it sets any quota.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether a statement reads or writes, for the volume quotas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// A query, `SHOW`, `DESCRIBE`, …
    Read,
    /// An `INSERT`, DDL, `OPTIMIZE`, …
    Write,
}

impl Access {
    /// A statement's access for the quotas: what reads, and what only frees
    /// storage (`DROP`, `TRUNCATE`), is a read; everything else a write.
    pub fn of(stmt: &crate::classify::Stmt) -> Self {
        use crate::classify::Stmt;
        if stmt.is_read() || matches!(stmt, Stmt::Drop(_) | Stmt::Truncate { .. }) {
            Self::Read
        } else {
            Self::Write
        }
    }
}

/// A namespace's counters.
#[derive(Debug, Default)]
struct Counters {
    running: u64,
    /// When the statements of the last [`RATE_WINDOW`] started.
    started: VecDeque<Instant>,
    /// The UTC day the `*_today` counters are for (days since the epoch).
    day: u64,
    scanned_today: u64,
    cpu_today: Duration,
    inserted_today: u64,
    /// The last storage round's bytes.
    stored: u64,
    /// Bytes inserted since that round.
    inserted_since_stored: u64,
}

impl Counters {
    fn roll(&mut self, day: u64) {
        if self.day != day {
            self.day = day;
            self.scanned_today = 0;
            self.cpu_today = Duration::ZERO;
            self.inserted_today = 0;
        }
    }

    fn trim(&mut self, now: Instant) {
        while self
            .started
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
        {
            self.started.pop_front();
        }
    }
}

/// What a namespace has used, as the quotas count it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuotaUsage {
    /// Statements running now.
    pub running: u64,
    /// Statements started in the last 60 s.
    pub last_minute: u64,
    /// Bytes scanned today (UTC).
    pub scanned_today: u64,
    /// CPU time today (UTC).
    pub cpu_today: Duration,
    /// Bytes inserted today (UTC).
    pub inserted_today: u64,
    /// The storage quota's count: the last round's bytes plus those inserted
    /// since.
    pub storage: u64,
}

/// The front's quota counters and their source. One per front, shared by both
/// surfaces, fed by the usage events ([`Quotas::account`]).
#[derive(Debug)]
pub struct Quotas {
    source: Arc<dyn QuotaSource>,
    namespaces: Mutex<HashMap<String, Counters>>,
    /// The meter's gate (design §63 §10.2, ME1 Task 20), asked after the
    /// protective quotas; `NoGate` by default.
    entitlements: Entitlements,
}

/// An admitted statement's place under `concurrent_queries`; dropping it ends
/// it.
#[derive(Debug)]
pub struct QuotaGuard {
    quotas: Arc<Quotas>,
    namespace: String,
}

impl Drop for QuotaGuard {
    fn drop(&mut self) {
        let mut namespaces = self.quotas.lock();
        if let Some(counters) = namespaces.get_mut(&self.namespace) {
            counters.running = counters.running.saturating_sub(1);
        }
    }
}

/// The UTC day of `at`, in days since the epoch.
pub fn utc_day(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

/// A quota refusal: `202` (`TOO_MANY_SIMULTANEOUS_QUERIES`) or `164`
/// (`READONLY`), its message naming the quota and its limit.
pub fn refusal(code: i32, quota: &str, limit: u64, what: &str) -> HouseError {
    let message = format!(
        "{QUOTA_EXCEEDED}: the namespace's quota {quota} (limit {limit}) is reached: {what}"
    );
    if code == 164 {
        HouseError::from(ChError::readonly(message))
    } else {
        HouseError::from(ChError::too_many_simultaneous_queries(message))
    }
}

/// A refusal by the meter's gate (§63 §17.4): `202` for reads, `164` for
/// writes, with a message that starts with [`QUOTA_EXCEEDED`] and names the
/// entitlement and the reason (`entitlement_exhausted`,
/// `payment_method_required`, `spend_cap_reached`, `not_entitled`,
/// `meter_unavailable`). [`refusal_parts`] does not read it: it names no
/// limit.
pub fn entitlement_refusal(code: i32, r: &Refusal) -> HouseError {
    let mut message = format!(
        "{QUOTA_EXCEEDED}: entitlement {} is refused: {}",
        r.feature, r.reason
    );
    if let Some(after) = r.retry_after {
        message.push_str(&format!(" (retry after {} s)", after.as_secs().max(1)));
    }
    if code == 164 {
        HouseError::from(ChError::readonly(message))
    } else {
        HouseError::from(ChError::too_many_simultaneous_queries(message))
    }
}

/// The quota and limit a [`refusal`]'s message names, when it is one.
pub fn refusal_parts(message: &str) -> Option<(&str, u64)> {
    let rest = message.strip_prefix(QUOTA_EXCEEDED)?;
    let rest = rest.strip_prefix(": the namespace's quota ")?;
    let (quota, rest) = rest.split_once(" (limit ")?;
    let (limit, _) = rest.split_once(')')?;
    Some((quota, limit.parse().ok()?))
}

impl Quotas {
    /// Counters over `source`'s quotas.
    pub fn new(source: Arc<dyn QuotaSource>) -> Arc<Self> {
        Self::with_entitlements(source, Entitlements::default())
    }

    /// Counters over `source`'s quotas, then `entitlements` for each
    /// admitted statement (§63 §10.2).
    pub fn with_entitlements(
        source: Arc<dyn QuotaSource>,
        entitlements: Entitlements,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            namespaces: Mutex::new(HashMap::new()),
            entitlements,
        })
    }

    /// No quotas at all.
    pub fn unlimited() -> Arc<Self> {
        Self::new(Arc::new(ConfigQuotas::default()))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Counters>> {
        self.namespaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The quotas of `namespace` now.
    pub fn quota(&self, namespace: &str) -> NamespaceQuota {
        self.source.quota(namespace)
    }

    /// Admits one statement of `namespace` under its quotas, as of `now`
    /// (`at` for the UTC day): a rate or concurrency quota past is `202`; a
    /// volume quota used up is `202` for a read and `164` for a write.
    pub fn admit(
        self: &Arc<Self>,
        namespace: &str,
        access: Access,
        now: Instant,
        at: SystemTime,
    ) -> Result<QuotaGuard, HouseError> {
        let quota = self.source.quota(namespace);
        let mut namespaces = self.lock();
        let counters = namespaces.entry(namespace.to_string()).or_default();
        counters.roll(utc_day(at));
        counters.trim(now);
        if let Some(limit) = quota.concurrent_queries
            && counters.running >= limit
        {
            return Err(refusal(
                202,
                "concurrent_queries",
                limit,
                &format!("{} statements are running", counters.running),
            ));
        }
        if let Some(limit) = quota.queries_per_minute
            && counters.started.len() as u64 >= limit
        {
            return Err(refusal(
                202,
                "queries_per_minute",
                limit,
                &format!(
                    "{} statements started in the last minute",
                    counters.started.len()
                ),
            ));
        }
        let volume = if access == Access::Write { 164 } else { 202 };
        if let Some(limit) = quota.scan_bytes_per_day
            && counters.scanned_today >= limit
        {
            return Err(refusal(
                volume,
                "scan_bytes_per_day",
                limit,
                &format!("{} bytes scanned today (UTC)", counters.scanned_today),
            ));
        }
        if let Some(limit) = quota.cpu_seconds_per_day
            && counters.cpu_today >= Duration::from_secs(limit)
        {
            return Err(refusal(
                volume,
                "cpu_seconds_per_day",
                limit,
                &format!(
                    "{:.3} CPU seconds used today (UTC)",
                    counters.cpu_today.as_secs_f64()
                ),
            ));
        }
        if access == Access::Write {
            if let Some(limit) = quota.insert_bytes_per_day
                && counters.inserted_today >= limit
            {
                return Err(refusal(
                    164,
                    "insert_bytes_per_day",
                    limit,
                    &format!("{} bytes inserted today (UTC)", counters.inserted_today),
                ));
            }
            let stored = counters
                .stored
                .saturating_add(counters.inserted_since_stored);
            if let Some(limit) = quota.storage_bytes
                && stored >= limit
            {
                return Err(refusal(
                    164,
                    "storage_bytes",
                    limit,
                    &format!("the namespace's lake tables hold {stored} bytes"),
                ));
            }
        }
        counters.running += 1;
        counters.started.push_back(now);
        drop(namespaces);
        // The statement holds its place under the protective quotas while
        // the gate is asked (a lease hit is local; a cold miss waits at most
        // the gate's deadline), so the lock is not held across it.
        let guard = QuotaGuard {
            quotas: Arc::clone(self),
            namespace: namespace.to_string(),
        };
        if let Err(r) = self.entitled(namespace, access) {
            drop(guard);
            let mut namespaces = self.lock();
            if let Some(counters) = namespaces.get_mut(namespace) {
                // A refused statement does not count toward the rate.
                if let Some(i) = counters.started.iter().rposition(|t| *t == now) {
                    counters.started.remove(i);
                }
            }
            let code = if access == Access::Write { 164 } else { 202 };
            return Err(entitlement_refusal(code, &r));
        }
        Ok(guard)
    }

    /// The entitlements a statement needs: CPU always, scanned bytes for a
    /// read, each with the 1-unit floor. The statement that crosses the
    /// limit finishes; the next is refused (R22.1). Its actual cost is
    /// settled by its `house.query` usage event.
    fn entitled(&self, namespace: &str, access: Access) -> Result<(), Refusal> {
        self.entitlements
            .check(None, namespace, features::HOUSE_CPU, 1)?;
        if access == Access::Read {
            self.entitlements
                .check(None, namespace, features::HOUSE_SCAN, 1)?;
        }
        Ok(())
    }

    /// `164` when `namespace` holds `existing` pipes and its `pipes` quota
    /// leaves no room for another.
    pub fn check_pipes(&self, namespace: &str, existing: usize) -> Result<(), HouseError> {
        match self.source.quota(namespace).pipes {
            Some(limit) if existing as u64 >= limit => Err(refusal(
                164,
                "pipes",
                limit,
                &format!("the namespace holds {existing} pipes"),
            )),
            _ => Ok(()),
        }
    }

    /// Counts one usage event against its namespace's day (as of `at`).
    pub fn account(&self, event: &UsageEvent, at: SystemTime) {
        let mut namespaces = self.lock();
        let counters = namespaces.entry(event.namespace().to_string()).or_default();
        counters.roll(utc_day(at));
        match event {
            UsageEvent::Query(q) => {
                counters.scanned_today = counters.scanned_today.saturating_add(q.bytes_scanned);
                counters.cpu_today = counters.cpu_today.saturating_add(q.cpu.unwrap_or_default());
            }
            UsageEvent::Insert(i) => {
                counters.inserted_today = counters.inserted_today.saturating_add(i.bytes);
                counters.inserted_since_stored =
                    counters.inserted_since_stored.saturating_add(i.bytes);
            }
            // The storage round sets the namespace's total itself
            // ([`Quotas::stored`]); compaction is the House's own upkeep.
            UsageEvent::Storage(_) | UsageEvent::Compaction(_) => {}
        }
    }

    /// A storage round measured `bytes` in `namespace`'s lake tables.
    pub fn stored(&self, namespace: &str, bytes: u64) {
        let mut namespaces = self.lock();
        let counters = namespaces.entry(namespace.to_string()).or_default();
        counters.stored = bytes;
        counters.inserted_since_stored = 0;
    }

    /// What `namespace` has used, as of `now` and `at`.
    pub fn usage(&self, namespace: &str, now: Instant, at: SystemTime) -> QuotaUsage {
        let mut namespaces = self.lock();
        let Some(counters) = namespaces.get_mut(namespace) else {
            return QuotaUsage::default();
        };
        counters.roll(utc_day(at));
        counters.trim(now);
        QuotaUsage {
            running: counters.running,
            last_minute: counters.started.len() as u64,
            scanned_today: counters.scanned_today,
            cpu_today: counters.cpu_today,
            inserted_today: counters.inserted_today,
            storage: counters
                .stored
                .saturating_add(counters.inserted_since_stored),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{InsertUsage, QueryUsage};
    use loams_meter_gate::adapters::DeploymentOrg;
    use loams_meter_gate::entitle::DenyGate;
    use loams_meter_gate::gate::RefusalReason;

    fn gated(feature: &str, reason: RefusalReason) -> Arc<Quotas> {
        Quotas::with_entitlements(
            Arc::new(ConfigQuotas::default()),
            Entitlements::new(
                Arc::new(DenyGate::new("org_42", feature, reason)),
                Arc::new(DeploymentOrg::new("42")),
            ),
        )
    }

    #[test]
    fn house_entitlement_refusal_names_feature() {
        let q = gated(features::HOUSE_CPU, RefusalReason::EntitlementExhausted);
        let now = Instant::now();
        let at = SystemTime::now();
        let read = q.admit("7", Access::Read, now, at).expect_err("refused");
        assert_eq!(read.code(), 202);
        assert!(
            read.message().starts_with(QUOTA_EXCEEDED),
            "{}",
            read.message()
        );
        assert!(
            read.message().contains("entitlement house_cpu"),
            "{}",
            read.message()
        );
        assert!(
            read.message().contains("entitlement_exhausted"),
            "{}",
            read.message()
        );
        let write = q.admit("7", Access::Write, now, at).expect_err("refused");
        assert_eq!(write.code(), 164);
        // A refused statement holds no place under the quotas.
        assert_eq!(q.usage("7", now, at).running, 0);
    }

    #[test]
    fn scan_entitlement_refuses_reads_only() {
        let q = gated(features::HOUSE_SCAN, RefusalReason::SpendCapReached);
        let now = Instant::now();
        let at = SystemTime::now();
        let read = q.admit("7", Access::Read, now, at).expect_err("refused");
        assert!(
            read.message().contains("spend_cap_reached"),
            "{}",
            read.message()
        );
        drop(
            q.admit("7", Access::Write, now, at)
                .expect("writes scan nothing"),
        );
    }

    #[test]
    fn no_gate_admits_as_before() {
        let q = Quotas::unlimited();
        drop(
            q.admit("7", Access::Read, Instant::now(), SystemTime::now())
                .expect("no gate"),
        );
    }

    fn quotas(quota: NamespaceQuota) -> Arc<Quotas> {
        Quotas::new(Arc::new(ConfigQuotas {
            default: quota,
            namespaces: BTreeMap::new(),
        }))
    }

    fn query(namespace: &str, bytes: u64, cpu_ms: u64) -> UsageEvent {
        UsageEvent::Query(QueryUsage {
            namespace: namespace.to_string(),
            bytes_scanned: bytes,
            cpu: Some(Duration::from_millis(cpu_ms)),
            ..QueryUsage::default()
        })
    }

    #[test]
    fn rate_and_concurrency_are_202() {
        let q = quotas(NamespaceQuota {
            concurrent_queries: Some(1),
            queries_per_minute: Some(2),
            ..NamespaceQuota::default()
        });
        let t0 = Instant::now();
        let at = SystemTime::now();
        let first = q.admit("2", Access::Read, t0, at).expect("first");
        let busy = q
            .admit("2", Access::Read, t0, at)
            .expect_err("one at a time");
        assert_eq!(busy.code(), 202);
        assert_eq!(
            refusal_parts(busy.message()),
            Some(("concurrent_queries", 1))
        );
        // Another namespace has its own counters.
        drop(q.admit("3", Access::Read, t0, at).expect("namespace 3"));
        drop(first);
        drop(q.admit("2", Access::Read, t0, at).expect("second"));
        let fast = q.admit("2", Access::Write, t0, at).expect_err("rate");
        assert_eq!(fast.code(), 202, "a rate quota is 202 for writes too");
        assert_eq!(
            refusal_parts(fast.message()),
            Some(("queries_per_minute", 2))
        );
        // A minute on, the window has room again.
        drop(
            q.admit("2", Access::Read, t0 + RATE_WINDOW, at)
                .expect("a minute later"),
        );
    }

    #[test]
    fn volume_quotas_are_202_for_reads_and_164_for_writes() {
        let q = quotas(NamespaceQuota {
            scan_bytes_per_day: Some(1_000),
            ..NamespaceQuota::default()
        });
        let now = Instant::now();
        let day = UNIX_EPOCH + Duration::from_secs(20_000 * 86_400 + 3_600);
        drop(q.admit("2", Access::Read, now, day).expect("nothing used"));
        q.account(&query("2", 600, 0), day);
        drop(q.admit("2", Access::Read, now, day).expect("600 of 1000"));
        q.account(&query("2", 600, 0), day);
        let read = q.admit("2", Access::Read, now, day).expect_err("used up");
        assert_eq!(read.code(), 202);
        assert!(
            read.message().starts_with(QUOTA_EXCEEDED),
            "{}",
            read.message()
        );
        let write = q.admit("2", Access::Write, now, day).expect_err("used up");
        assert_eq!(write.code(), 164);
        assert_eq!(
            refusal_parts(write.message()),
            Some(("scan_bytes_per_day", 1_000))
        );
        // The next UTC day starts from zero.
        let tomorrow = day + Duration::from_secs(86_400);
        drop(q.admit("2", Access::Read, now, tomorrow).expect("tomorrow"));
        assert_eq!(q.usage("2", now, tomorrow).scanned_today, 0);
    }

    #[test]
    fn cpu_insert_and_storage_quotas() {
        let q = quotas(NamespaceQuota {
            cpu_seconds_per_day: Some(1),
            insert_bytes_per_day: Some(100),
            storage_bytes: Some(1_000),
            ..NamespaceQuota::default()
        });
        let now = Instant::now();
        let at = SystemTime::now();
        q.account(&query("2", 0, 1_500), at);
        assert_eq!(
            q.admit("2", Access::Read, now, at).expect_err("cpu").code(),
            202
        );
        let q = quotas(NamespaceQuota {
            insert_bytes_per_day: Some(100),
            storage_bytes: Some(1_000),
            ..NamespaceQuota::default()
        });
        let insert = |bytes| {
            UsageEvent::Insert(InsertUsage {
                namespace: "2".to_string(),
                bytes,
                ..InsertUsage::default()
            })
        };
        q.account(&insert(150), at);
        let err = q.admit("2", Access::Write, now, at).expect_err("inserted");
        assert_eq!(err.code(), 164);
        assert_eq!(
            refusal_parts(err.message()),
            Some(("insert_bytes_per_day", 100))
        );
        // Reads are not held to the write quotas.
        drop(q.admit("2", Access::Read, now, at).expect("reads go on"));
        // Storage: the round's total plus what was inserted since.
        let q = quotas(NamespaceQuota {
            storage_bytes: Some(1_000),
            ..NamespaceQuota::default()
        });
        q.stored("2", 900);
        drop(q.admit("2", Access::Write, now, at).expect("900 of 1000"));
        q.account(&insert(150), at);
        assert_eq!(q.usage("2", now, at).storage, 1_050);
        let err = q.admit("2", Access::Write, now, at).expect_err("full");
        assert_eq!(refusal_parts(err.message()), Some(("storage_bytes", 1_000)));
        q.stored("2", 500);
        drop(q.admit("2", Access::Write, now, at).expect("compacted"));
    }

    #[test]
    fn pipes_quota_is_164() {
        let q = quotas(NamespaceQuota {
            pipes: Some(2),
            ..NamespaceQuota::default()
        });
        q.check_pipes("2", 1).expect("room");
        let err = q.check_pipes("2", 2).expect_err("full");
        assert_eq!(err.code(), 164);
        assert_eq!(refusal_parts(err.message()), Some(("pipes", 2)));
        Quotas::unlimited()
            .check_pipes("2", 1_000)
            .expect("unlimited");
    }

    #[test]
    fn config_quotas_overlay_per_namespace() {
        let section: QuotasSection = toml::from_str(
            r#"
            queries_per_minute = 600
            scan_bytes_per_day = 1000
            [namespaces.7]
            scan_bytes_per_day = 5
            pipes = 1
            "#,
        )
        .expect("parses");
        let parsed = section.to_quotas().expect("ids");
        assert_eq!(
            parsed.quota("7"),
            NamespaceQuota {
                queries_per_minute: Some(600),
                scan_bytes_per_day: Some(5),
                pipes: Some(1),
                ..NamespaceQuota::default()
            }
        );
        assert_eq!(parsed.quota("8").scan_bytes_per_day, Some(1000));
        assert!(toml::from_str::<QuotasSection>("nope = 1").is_err());
        assert!(toml::from_str::<QuotasSection>("[namespaces.7]\nnope = 1").is_err());
        let bad: QuotasSection = toml::from_str("[namespaces.x]\npipes = 1").expect("parses");
        assert!(bad.to_quotas().is_err());
        assert!(QuotasSection::default().is_empty());
    }
}
