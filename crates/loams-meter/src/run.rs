//! The counter owners at work (design §63 §11–§12; ME1 role wiring, part 2).
//!
//! One `meter` instance runs one [`CounterRuntime`]:
//!
//! - every [`RENEW_EVERY`] the [`Assigner`] takes, keeps and gives up
//!   `_system/_meter` partitions; a gained partition's counters are rebuilt
//!   from the House ([`HouseTotals`]) and the stream tail before the
//!   [`LeaseServer`] grants for it, and a lost one stops granting at once;
//! - every [`TICK`] each owned partition is read to its end; every counted
//!   event retires its share of the lease it carried
//!   ([`LeaseServer::on_usage`]);
//! - every [`RECONCILE_EVERY`] each owned partition is reconciled against the
//!   House (§12) and the allocators rebase on the new counts;
//! - every [`CATALOG_REFRESH`] the meters, features, entitlements, grants and
//!   customers are read again ([`Catalog`]), which is what feeds the counters
//!   (which features an event feeds, which period it falls in) and the lease
//!   service (allowance, chunk, policy).
//!
//! The counter owner also emits the meter's own events (§15): after each
//! read it checks the touched counters against their thresholds and access
//! ([`ThresholdTracker`]) and appends what crossed to `_system/_meter`
//! ([`MeterEventSink`]), keyed by subject, so the events land in the
//! partition this instance owns. Reading that partition, it hands each
//! `io.loams.dev.meter.*` event to the notifier ([`MeterEventHandler`]):
//! that is the notifier's consumer. Failed deliveries are retried every
//! [`RETRY_EVERY`].
//!
//! [`CounterValues`] answers balances from the counters of the partitions
//! this instance owns. A subject whose partition another instance owns, or
//! one still being rebuilt, answers `meter_unavailable`: never a balance
//! without its usage.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use loams_meter_core::balance::{UsageSegment, balance};
use loams_meter_core::{
    Amount, AsOf, Balance, Entitlement, EntitlementKind, Feature, FeatureKey, Grant, LeasePolicy,
    Mode, Period, SubjectKey, Unit, UtcNanos,
};
use tokio::sync::{Mutex, Notify};

use crate::customers::Clock;
use crate::entitlements::ValueSource;
use crate::error::{MeterError, Result};
use crate::lease_service::{AllocatorInputs, LeaseInputs, LeaseServer};
use crate::owner::assign::RENEW_EVERY;
use crate::owner::{
    Applied, Assigner, CounterKey, CounterOwners, FeatureIndex, HouseTotals, OwnershipChange,
    PartitionLeases, PartitionReader, Periods, READ_BATCH, partition_of,
};
use crate::reconcile::{Allowances, RECONCILE_EVERY, reconcile};
use crate::notify::events::DEFAULT_THRESHOLDS;
use crate::notify::{MeterEvent, Notifier, ThresholdTracker};
use crate::store::{Customer, MeterStore, PaymentState, Records};

/// How often owned partitions are read to their end.
pub const TICK: Duration = Duration::from_millis(250);

/// How often the cached records are read again.
pub const CATALOG_REFRESH: Duration = Duration::from_secs(10);

/// How often failed webhook deliveries are retried.
pub const RETRY_EVERY: Duration = Duration::from_secs(30);

/// Meter events older than this, read again on a rebuild, are not handed
/// to the notifier again (their deliveries are long recorded).
pub const NOTIFY_WINDOW: Duration = Duration::from_secs(24 * 3600);

/// The type prefix of the meter's own events.
pub const METER_EVENT_PREFIX: &str = "io.loams.dev.meter.";

/// Where the counter owner appends the meter's own events: `_system/_meter`,
/// keyed by subject.
#[async_trait]
pub trait MeterEventSink: Send + Sync + fmt::Debug {
    async fn append(&self, events: Vec<MeterEvent>) -> Result<()>;
}

/// What consumes the meter's own events: the notifier.
#[async_trait]
pub trait MeterEventHandler: Send + Sync + fmt::Debug {
    async fn handle(&self, event: &MeterEvent) -> Result<()>;
    /// Retries what is due; answers how many were tried.
    async fn retry_due(&self) -> Result<usize>;
}

#[async_trait]
impl MeterEventHandler for Notifier {
    async fn handle(&self, event: &MeterEvent) -> Result<()> {
        Notifier::handle(self, event).await.map(|_| ())
    }

    async fn retry_due(&self) -> Result<usize> {
        Notifier::retry_due(self).await
    }
}

/// One of the meter's own events (`io.loams.dev.meter.*`) read off the
/// stream, as an [`loams_meter_core::Event`]. They are not usage events: no
/// tenant, no dataschema, so the usage profile does not apply to them.
/// `None` for any other type, or one that is malformed.
pub fn own_event(ce: &loams_cloudevents::CloudEvent) -> Option<loams_meter_core::Event> {
    if !ce.ty().starts_with(METER_EVENT_PREFIX) {
        return None;
    }
    let subject = SubjectKey::new(ce.attr("subject")?).ok()?;
    let time = UtcNanos::from_rfc3339(ce.attr("time")?).ok()?;
    let data = match serde_json::from_slice::<serde_json::Value>(ce.data()?).ok()? {
        serde_json::Value::Object(m) => m,
        _ => return None,
    };
    Some(loams_meter_core::Event {
        id: ce.id().to_string(),
        source: ce.source().to_string(),
        r#type: ce.ty().to_string(),
        subject,
        time,
        tenant: None,
        lease: None,
        data,
    })
}

/// A meter event as the CloudEvent appended to `_system/_meter`.
pub fn to_cloudevent(e: &MeterEvent) -> Result<loams_cloudevents::CloudEvent> {
    let body = serde_json::to_vec(&e.to_cloudevent_json())
        .map_err(|err| MeterError::Unavailable(format!("a meter event: {err}")))?;
    loams_cloudevents::json::parse_event(&body)
        .map_err(|err| MeterError::Unavailable(format!("a meter event: {err}")))
}

/// A stream event of the meter's own types, as the notifier takes it.
pub fn meter_event(e: &loams_meter_core::Event) -> Option<MeterEvent> {
    e.r#type.starts_with(METER_EVENT_PREFIX).then(|| MeterEvent {
        id: e.id.clone(),
        r#type: e.r#type.clone(),
        source: e.source.clone(),
        subject: e.subject.to_string(),
        time: e.time,
        data: e.data.clone(),
    })
}

/// The service identity a gate on another node has on the cluster
/// listener. Like every route of that listener in M1 (overview §6.9), it is
/// admitted by the network it is reached on, not by a credential; mTLS
/// identities per product role replace it when the cluster listener gets
/// them.
pub const CLUSTER_SERVICE: &str = "loams-cluster";

/// The smallest useful lease chunk of a feature's unit (§63 §11.2: 1
/// request, 64 KiB, 1,000 tokens, 100 CPU-ms).
pub fn min_chunk(unit: &Unit) -> Amount {
    match unit {
        Unit::Bytes | Unit::ByteHours => Amount::from_units(64 * 1024),
        Unit::Tokens => Amount::from_units(1_000),
        Unit::Ms | Unit::CuMs => Amount::from_units(100),
        Unit::Count | Unit::Requests | Unit::Other(_) => Amount::from_units(1),
    }
}

/// What the allocator does at the end of the allowance (§63 §13, D2203):
///
/// - `HardLimit` refuses there, and `SoftLimit` only counts.
/// - `OverageAllowed` lets usage run past the allowance only for a customer
///   who can pay (a card or an invoice) and has turned overage on. The
///   headroom is the lower of the entitlement's and the customer's spend
///   caps, at the overage price. A zero price means free overage, which
///   counts like a soft limit. Anything else refuses at the allowance.
pub fn policy(mode: &Mode, customer: Option<&Customer>) -> LeasePolicy {
    match mode {
        Mode::HardLimit => LeasePolicy::HardLimit,
        Mode::SoftLimit => LeasePolicy::SoftLimit,
        Mode::OverageAllowed {
            overage_price,
            spend_cap,
        } => {
            let Some(c) = customer else {
                return LeasePolicy::HardLimit;
            };
            if c.payment_state == PaymentState::None || !c.spend_controls.overage_enabled {
                return LeasePolicy::HardLimit;
            }
            if overage_price.micros <= 0 {
                return LeasePolicy::SoftLimit;
            }
            let mut cap = *spend_cap;
            if let Some(own) = c.spend_controls.spend_cap
                && own.currency == cap.currency
                && own.micros < cap.micros
            {
                cap = own;
            }
            if cap.currency != overage_price.currency || cap.micros <= 0 {
                return LeasePolicy::HardLimit;
            }
            LeasePolicy::Overage {
                headroom: Amount::from_units(1)
                    .mul_ratio(i128::from(cap.micros), i128::from(overage_price.micros)),
            }
        }
    }
}

/// Whole units of `a` for ME-gate's integer amounts (D2180), floored, and
/// zero below zero.
pub fn gate_units(a: Amount) -> u64 {
    u64::try_from(a.raw().div_euclid(Amount::SCALE).max(0)).unwrap_or(u64::MAX)
}

/// What a gate may spend for a subject's feature when the owner cannot be
/// reached and its lease has expired: ME-gate's policy table (§13's "when
/// unreachable" row). No payment method fails closed; a card fails open up
/// to the overage headroom (else the allowance); a contract up to the
/// headroom (else without bound).
pub fn fail_open(policy: LeasePolicy, customer: Option<&Customer>, allowance: Amount) -> Amount {
    use loams_meter_gate::policy::{Funding, PolicyTable, WhenUnreachable};
    let funding = match customer.map(|c| (c.payment_state, c.spend_controls.overage_enabled)) {
        None | Some((PaymentState::None, _)) => Funding::Free,
        Some((PaymentState::Card, true)) => Funding::PaidOverage,
        Some((PaymentState::Card, false)) => Funding::Paid,
        Some((PaymentState::Invoice, _)) => Funding::Contract,
    };
    let headroom = match policy {
        LeasePolicy::Overage { headroom } => Some(gate_units(headroom)),
        _ => None,
    };
    match PolicyTable::default().when_unreachable(funding, headroom, gate_units(allowance)) {
        WhenUnreachable::FailClosed => Amount::ZERO,
        WhenUnreachable::FailOpen { budget } => {
            Amount::from_raw(i128::from(budget).saturating_mul(Amount::SCALE))
        }
    }
}

/// The records the runtime works from, read every [`CATALOG_REFRESH`].
#[derive(Debug, Default)]
pub struct Catalog {
    index: FeatureIndex,
    features: HashMap<FeatureKey, Feature>,
    entitlements: HashMap<(SubjectKey, FeatureKey), Entitlement>,
    grants: HashMap<String, Vec<Grant>>,
    customers: HashMap<SubjectKey, Customer>,
}

impl Catalog {
    pub fn new(records: Records) -> Self {
        let index = FeatureIndex::new(&records.meters, &records.features);
        let mut grants: HashMap<String, Vec<Grant>> = HashMap::new();
        for g in records.grants {
            grants.entry(g.entitlement.as_str().to_string()).or_default().push(g);
        }
        let mut customers = HashMap::new();
        for c in records.customers {
            for s in &c.subjects {
                customers.insert(s.clone(), c.clone());
            }
        }
        Catalog {
            index,
            features: records
                .features
                .into_iter()
                .map(|f| (f.key.clone(), f))
                .collect(),
            entitlements: records
                .entitlements
                .into_iter()
                .map(|e| ((e.subject.clone(), e.feature.clone()), e))
                .collect(),
            grants,
            customers,
        }
    }

    pub fn index(&self) -> &FeatureIndex {
        &self.index
    }

    pub fn entitlement(&self, subject: &SubjectKey, feature: &FeatureKey) -> Option<&Entitlement> {
        self.entitlements.get(&(subject.clone(), feature.clone()))
    }

    pub fn grants(&self, e: &Entitlement) -> &[Grant] {
        self.grants
            .get(e.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn customer(&self, subject: &SubjectKey) -> Option<&Customer> {
        self.customers.get(subject)
    }

    /// The end of the period a counter counts, if its entitlement is still
    /// metered.
    fn period_end(&self, key: &CounterKey) -> Option<UtcNanos> {
        let e = self.entitlement(&key.subject, &key.feature)?;
        Some(e.usage_period()?.period_at(key.period_start).end)
    }

    /// The period's total available before any usage (the drift ratio's
    /// denominator and the allocator's allowance).
    fn allowance_at(&self, e: &Entitlement, period: Period) -> Option<Amount> {
        match balance(e, self.grants(e), &[], period.start, AsOf::Replay) {
            Ok(b) => Some(b.total_available),
            Err(err) => {
                tracing::warn!(entitlement = e.id.as_str(), %err, "meter balance replay failed");
                None
            }
        }
    }
}

impl Periods for Catalog {
    fn period(&self, subject: &SubjectKey, feature: &FeatureKey, at: UtcNanos) -> Option<Period> {
        Some(self.entitlement(subject, feature)?.usage_period()?.period_at(at))
    }
}

impl Allowances for Catalog {
    fn allowance(&self, key: &CounterKey) -> Option<Amount> {
        let e = self.entitlement(&key.subject, &key.feature)?;
        let period = e.usage_period()?.period_at(key.period_start);
        self.allowance_at(e, period).filter(|a| a.is_positive())
    }
}

/// What the runtime, the balances and the lease service share: the
/// counters of the owned partitions and the cached records.
#[derive(Debug)]
pub struct Counters {
    partitions: u32,
    owners: Mutex<CounterOwners>,
    catalog: RwLock<Arc<Catalog>>,
}

impl Counters {
    pub fn new(partitions: u32) -> Arc<Self> {
        Arc::new(Counters {
            partitions,
            owners: Mutex::new(CounterOwners::new()),
            catalog: RwLock::new(Arc::new(Catalog::default())),
        })
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        *self.catalog.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(catalog);
    }

    /// The partitions this instance counts, with their owner epochs.
    pub async fn owned(&self) -> Vec<(u32, u64)> {
        self.owners.lock().await.owned().collect()
    }

    /// `key`'s count and the point it is as of; `None` when this instance
    /// does not count the subject's partition.
    pub async fn counted(&self, key: &CounterKey) -> Option<(Amount, AsOf)> {
        let p = partition_of(key.subject.as_str(), self.partitions);
        let owners = self.owners.lock().await;
        let c = owners.get(p)?;
        Some((
            c.counted(key),
            AsOf::Counter {
                owner_epoch: c.epoch(),
                offset: c.next_offset(),
            },
        ))
    }
}

/// Balances from the counters (§63 §9.3): the entitlement's grants replayed
/// with the period's usage so far.
///
/// Only the current period's usage is known to the counters; earlier
/// periods replay with none, which matters only to rollover of grants that
/// span periods. ME-house's closed period totals fill that in.
#[derive(Debug, Clone)]
pub struct CounterValues(pub Arc<Counters>);

#[async_trait]
impl ValueSource for CounterValues {
    async fn value(&self, e: &Entitlement, grants: &[Grant], at: UtcNanos) -> Result<Balance> {
        let Some(usage_period) = e.usage_period() else {
            return Err(MeterError::invalid("entitlement", "not metered"));
        };
        let period = usage_period.period_at(at);
        let key = CounterKey {
            subject: e.subject.clone(),
            feature: e.feature.clone(),
            period_start: period.start,
        };
        let Some((counted, as_of)) = self.0.counted(&key).await else {
            let p = partition_of(e.subject.as_str(), self.0.partitions);
            return Err(MeterError::Unavailable(format!(
                "partition {p} of the subject is not counted on this meter instance \
                 (another instance owns it, or it is being rebuilt)"
            )));
        };
        let usage = if counted.is_positive() {
            vec![UsageSegment {
                start: period.start,
                end: UtcNanos(at.0.clamp(period.start.0, period.end.0)),
                amount: counted,
            }]
        } else {
            Vec::new()
        };
        balance(e, grants, &usage, at, as_of)
            .map_err(|err| MeterError::Unavailable(format!("the balance replay failed: {err}")))
    }
}

/// The lease service's inputs from the catalog and the counters.
#[derive(Debug, Clone)]
pub struct CatalogInputs(pub Arc<Counters>);

#[async_trait]
impl LeaseInputs for CatalogInputs {
    async fn inputs(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
        now: UtcNanos,
    ) -> Result<Option<AllocatorInputs>> {
        let catalog = self.0.catalog();
        let Some(e) = catalog.entitlement(subject, feature) else {
            return Ok(None);
        };
        let EntitlementKind::Metered {
            usage_period, mode, ..
        } = &e.kind
        else {
            return Ok(None);
        };
        let Some(f) = catalog.features.get(feature).filter(|f| !f.archived) else {
            return Ok(None);
        };
        let period = usage_period.period_at(now);
        let key = CounterKey {
            subject: subject.clone(),
            feature: feature.clone(),
            period_start: period.start,
        };
        // The lease server only asks for owned partitions; one lost
        // meanwhile counts nothing, and the next acquire is `not_owner`.
        let counted = self
            .0
            .counted(&key)
            .await
            .map_or(Amount::ZERO, |(c, _)| c);
        let allowance = catalog.allowance_at(e, period).ok_or_else(|| {
            MeterError::Unavailable(format!("no allowance for entitlement {}", e.id.as_str()))
        })?;
        Ok(Some(AllocatorInputs {
            period,
            allowance,
            min_chunk: min_chunk(&f.unit),
            policy: policy(mode, catalog.customer(subject)),
            counted,
            fail_open: fail_open(
                policy(mode, catalog.customer(subject)),
                catalog.customer(subject),
                allowance,
            ),
        }))
    }
}

/// What a [`CounterRuntime`] runs against.
pub struct RuntimeParts<L> {
    pub leases: L,
    /// This instance's lease holder name (unique per process).
    pub me: String,
    pub reader: Arc<dyn PartitionReader>,
    pub house: Arc<dyn HouseTotals>,
    pub store: Arc<dyn MeterStore>,
    pub counters: Arc<Counters>,
    pub lease_server: Arc<LeaseServer>,
    pub clock: Clock,
    /// Where threshold and access events are appended (`None`: not
    /// emitted).
    pub events: Option<Arc<dyn MeterEventSink>>,
    /// The notifier (`None`: meter events are read and not delivered).
    pub notifier: Option<Arc<dyn MeterEventHandler>>,
}

impl<L> fmt::Debug for RuntimeParts<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeParts")
            .field("me", &self.me)
            .finish_non_exhaustive()
    }
}

/// The House side when the House cannot say where its pipe stands: the
/// House pipe keeps its cursor (the stream offsets it has committed) in its
/// Iceberg table's properties, which no SQL reads (ME-house ruling T5-1,
/// D2203). Without the cursor the counters are rebuilt from the stream alone
/// (its 14 days of retention) and reconciliation has no split to rebase on.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamOnly;

#[async_trait]
impl HouseTotals for StreamOnly {
    async fn committed(
        &self,
        _partition: u32,
    ) -> std::result::Result<Option<(u64, i64)>, crate::owner::OwnerError> {
        Ok(None)
    }

    async fn totals(
        &self,
        _partition: u32,
        _committed: u64,
        _snapshot: i64,
    ) -> std::result::Result<Vec<(CounterKey, Amount)>, crate::owner::OwnerError> {
        Ok(Vec::new())
    }
}

/// One instance's counter owners, lease grants and reconciliation.
pub struct CounterRuntime<L> {
    assigner: Assigner<L>,
    reader: Arc<dyn PartitionReader>,
    house: Arc<dyn HouseTotals>,
    store: Arc<dyn MeterStore>,
    counters: Arc<Counters>,
    leases: Arc<LeaseServer>,
    clock: Clock,
    events: Option<Arc<dyn MeterEventSink>>,
    notifier: Option<Arc<dyn MeterEventHandler>>,
    tracker: ThresholdTracker,
    next_retry: Option<UtcNanos>,
    /// Gained partitions whose rebuild has not succeeded yet, by epoch.
    rebuilding: std::collections::BTreeMap<u32, u64>,
    next_round: Option<UtcNanos>,
    next_catalog: Option<UtcNanos>,
    next_reconcile: Option<UtcNanos>,
}

impl<L> fmt::Debug for CounterRuntime<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CounterRuntime")
            .field("rebuilding", &self.rebuilding)
            .finish_non_exhaustive()
    }
}

fn nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

fn due(next: Option<UtcNanos>, now: UtcNanos) -> bool {
    next.is_none_or(|n| now.0 >= n.0)
}

impl<L: PartitionLeases> CounterRuntime<L> {
    pub fn new(p: RuntimeParts<L>) -> Self {
        let partitions = p.counters.partitions;
        CounterRuntime {
            tracker: ThresholdTracker::new(format!("loams://meter/{}", p.me), DEFAULT_THRESHOLDS),
            events: p.events,
            notifier: p.notifier,
            next_retry: None,
            assigner: Assigner::new(p.leases, p.me, partitions),
            reader: p.reader,
            house: p.house,
            store: p.store,
            counters: p.counters,
            leases: p.lease_server,
            clock: p.clock,
            rebuilding: std::collections::BTreeMap::new(),
            next_round: None,
            next_catalog: None,
            next_reconcile: None,
        }
    }

    /// One pass of everything that is due. Errors are logged, never fatal:
    /// the next pass retries.
    pub async fn step(&mut self) {
        let now = (self.clock)();
        if due(self.next_catalog, now) {
            match self.store.records().await {
                Ok(records) => {
                    self.counters.set_catalog(Catalog::new(records));
                    self.next_catalog = Some(now.add_nanos(nanos(CATALOG_REFRESH)));
                }
                Err(err) => tracing::warn!(%err, "the meter could not read its records"),
            }
        }
        if due(self.next_round, now) {
            self.next_round = Some(now.add_nanos(nanos(RENEW_EVERY)));
            match self.assigner.round().await {
                Ok(changes) => self.on_changes(changes).await,
                Err(err) => tracing::warn!(%err, "meter partition assignment failed"),
            }
        }
        self.rebuild_pending().await;
        self.catch_up_all().await;
        if due(self.next_reconcile, now) {
            // The first reconciliation is one period after start: a rebuild
            // has just read the House.
            if self.next_reconcile.is_some() {
                self.reconcile_all().await;
            }
            self.next_reconcile = Some(now.add_nanos(nanos(RECONCILE_EVERY)));
        }
        if due(self.next_retry, now) {
            self.next_retry = Some(now.add_nanos(nanos(RETRY_EVERY)));
            if let Some(n) = &self.notifier
                && let Err(err) = n.retry_due().await
            {
                tracing::warn!(%err, "retrying meter webhook deliveries failed");
            }
        }
        self.expire(now).await;
    }

    async fn on_changes(&mut self, changes: Vec<OwnershipChange>) {
        for change in changes {
            match change {
                OwnershipChange::Lost { partition, epoch } => {
                    self.leases.lost(partition, epoch);
                    self.rebuilding.remove(&partition);
                    let _ = self
                        .counters
                        .owners
                        .lock()
                        .await
                        .on_change(change, &*self.house)
                        .await;
                    tracing::info!(partition, epoch, "meter partition lost");
                }
                OwnershipChange::Gained { partition, epoch } => {
                    self.rebuilding.insert(partition, epoch);
                    tracing::info!(partition, epoch, "meter partition gained");
                }
            }
        }
    }

    /// Rebuilds gained partitions; the lease server grants for one only
    /// once its counters are rebuilt and caught up.
    async fn rebuild_pending(&mut self) {
        let pending: Vec<(u32, u64)> = self.rebuilding.iter().map(|(p, e)| (*p, *e)).collect();
        for (partition, epoch) in pending {
            if self.assigner.epoch_of(partition) != Some(epoch) {
                self.rebuilding.remove(&partition);
                continue;
            }
            let rebuilt = self
                .counters
                .owners
                .lock()
                .await
                .on_change(OwnershipChange::Gained { partition, epoch }, &*self.house)
                .await;
            match rebuilt {
                Ok(()) => match self.catch_up(partition).await {
                    Ok(()) => {
                        self.rebuilding.remove(&partition);
                        self.leases.gained(partition, epoch);
                    }
                    Err(err) => {
                        tracing::warn!(partition, %err, "meter partition replay failed");
                    }
                },
                Err(err) => tracing::warn!(partition, %err, "meter partition rebuild failed"),
            }
        }
    }

    async fn catch_up_all(&mut self) {
        let owned: Vec<u32> = self
            .counters
            .owned()
            .await
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| !self.rebuilding.contains_key(p))
            .collect();
        for partition in owned {
            if let Err(err) = self.catch_up(partition).await {
                tracing::warn!(partition, %err, "reading the meter stream failed");
            }
        }
    }

    /// Reads `partition` to its end, counting each event and retiring the
    /// leases the events carried; then hands the meter's own events to the
    /// notifier and emits the thresholds the touched counters crossed. The
    /// stream is read without the counters' lock held; an event already
    /// applied meanwhile is a replay.
    async fn catch_up(
        &mut self,
        partition: u32,
    ) -> std::result::Result<(), crate::owner::OwnerError> {
        let mut touched: BTreeSet<CounterKey> = BTreeSet::new();
        let mut notes: Vec<MeterEvent> = Vec::new();
        let now = (self.clock)();
        let oldest = now.add_nanos(-nanos(NOTIFY_WINDOW));
        loop {
            let from = {
                let owners = self.counters.owners.lock().await;
                let Some(c) = owners.get(partition) else {
                    break;
                };
                c.next_offset()
            };
            let batch = self.reader.read(partition, from, READ_BATCH).await?;
            if batch.is_empty() {
                break;
            }
            let catalog = self.counters.catalog();
            let mut owners = self.counters.owners.lock().await;
            let Some(c) = owners.get_mut(partition) else {
                break;
            };
            for (offset, event) in &batch {
                let applied = c.apply(*offset, event, catalog.index(), &*catalog);
                match applied {
                    Applied::Counted { amounts, lease } => {
                        for (key, amount) in amounts {
                            self.leases
                                .on_usage(partition, &key, lease.as_ref(), amount);
                            touched.insert(key);
                        }
                        if let Some(note) = meter_event(event)
                            && note.time.0 >= oldest.0
                        {
                            notes.push(note);
                        }
                    }
                    Applied::Replayed | Applied::Duplicate => {}
                }
            }
        }
        if let Some(n) = &self.notifier {
            for note in &notes {
                if let Err(err) = n.handle(note).await {
                    tracing::warn!(id = note.id.as_str(), %err, "a meter event was not delivered");
                }
            }
        }
        if !touched.is_empty() {
            self.emit(&touched, now).await;
        }
        Ok(())
    }

    /// Threshold and access events for the touched counters, appended to
    /// the stream (the notifier gets them when they are read back).
    async fn emit(&mut self, touched: &BTreeSet<CounterKey>, now: UtcNanos) {
        let Some(sink) = self.events.clone() else {
            return;
        };
        let catalog = self.counters.catalog();
        let mut out = Vec::new();
        for key in touched {
            let Some(e) = catalog.entitlement(&key.subject, &key.feature) else {
                continue;
            };
            let Some(usage_period) = e.usage_period() else {
                continue;
            };
            let period = usage_period.period_at(key.period_start);
            if period.start != key.period_start {
                continue;
            }
            let counted = self
                .counters
                .counted(key)
                .await
                .map_or(Amount::ZERO, |(c, _)| c);
            let usage = if counted.is_positive() {
                vec![UsageSegment {
                    start: period.start,
                    end: UtcNanos(now.0.clamp(period.start.0, period.end.0)),
                    amount: counted,
                }]
            } else {
                Vec::new()
            };
            let b = match balance(e, catalog.grants(e), &usage, now, AsOf::Replay) {
                Ok(b) => b,
                Err(err) => {
                    tracing::debug!(%err, "no threshold check for a counter");
                    continue;
                }
            };
            out.extend(self.tracker.observe(key, period, b.usage, b.total_available, now));
            let reason = (!b.has_access).then_some("entitlement_exhausted");
            out.extend(self.tracker.access(key, b.has_access, reason, now));
        }
        if !out.is_empty()
            && let Err(err) = sink.append(out).await
        {
            tracing::warn!(%err, "appending meter events failed; they are emitted again later");
        }
    }

    async fn reconcile_all(&mut self) {
        let catalog = self.counters.catalog();
        let mut owners = self.counters.owners.lock().await;
        let owned: Vec<u32> = owners
            .owned()
            .map(|(p, _)| p)
            .filter(|p| !self.rebuilding.contains_key(p))
            .collect();
        for partition in owned {
            let Some(c) = owners.get_mut(partition) else {
                continue;
            };
            match reconcile(
                c,
                &*self.house,
                &*self.reader,
                catalog.index(),
                &*catalog,
                &*catalog,
            )
            .await
            {
                Ok(report) => {
                    for (key, counted) in &report.counted {
                        self.leases.rebase(partition, key, *counted);
                    }
                }
                Err(err) => tracing::warn!(partition, %err, "meter reconciliation failed"),
            }
        }
    }

    /// Forgets counters of periods that ended, and of entitlements that are
    /// gone.
    async fn expire(&mut self, now: UtcNanos) {
        // Threshold memory for periods that started over 40 days ago.
        self.tracker
            .expire(now.add_nanos(-40 * UtcNanos::DAY));
        let catalog = self.counters.catalog();
        let mut owners = self.counters.owners.lock().await;
        let owned: BTreeSet<u32> = owners.owned().map(|(p, _)| p).collect();
        for partition in owned {
            if let Some(c) = owners.get_mut(partition) {
                c.expire(now, |k| catalog.period_end(k).unwrap_or(UtcNanos(i64::MIN)));
            }
        }
    }

    /// Gives every partition back (shutdown), so another instance takes
    /// them without waiting for the leases to lapse.
    pub async fn release(&mut self) {
        for change in self.assigner.release_all().await {
            if let OwnershipChange::Lost { partition, epoch } = change {
                self.leases.lost(partition, epoch);
            }
        }
    }

    /// Runs until `stop` is notified, then releases its partitions.
    pub async fn run(mut self, stop: Arc<Notify>) {
        loop {
            self.step().await;
            tokio::select! {
                () = stop.notified() => break,
                () = tokio::time::sleep(TICK) => {}
            }
        }
        self.release().await;
    }
}
