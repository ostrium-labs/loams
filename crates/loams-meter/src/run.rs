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
use crate::store::{Customer, MeterStore, PaymentState, Records};

/// How often owned partitions are read to their end.
pub const TICK: Duration = Duration::from_millis(250);

/// How often the cached records are read again.
pub const CATALOG_REFRESH: Duration = Duration::from_secs(10);

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
    /// leases the events carried. The stream is read without the counters'
    /// lock held; an event already applied meanwhile is a replay.
    async fn catch_up(&self, partition: u32) -> std::result::Result<(), crate::owner::OwnerError> {
        loop {
            let from = {
                let owners = self.counters.owners.lock().await;
                let Some(c) = owners.get(partition) else {
                    return Ok(());
                };
                c.next_offset()
            };
            let batch = self.reader.read(partition, from, READ_BATCH).await?;
            if batch.is_empty() {
                return Ok(());
            }
            let catalog = self.counters.catalog();
            let mut owners = self.counters.owners.lock().await;
            let Some(c) = owners.get_mut(partition) else {
                return Ok(());
            };
            for (offset, event) in &batch {
                if let Applied::Counted { amounts, lease } =
                    c.apply(*offset, event, catalog.index(), &*catalog)
                {
                    for (key, amount) in amounts {
                        self.leases
                            .on_usage(partition, &key, lease.as_ref(), amount);
                    }
                }
            }
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
    async fn expire(&self, now: UtcNanos) {
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
