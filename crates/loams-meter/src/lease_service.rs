//! `LeaseService` (design §63 §11, D1126; ME1 Task 17): the counter owner's
//! side of leasing, over ME-core's sans-I/O [`Allocator`], and the gate's
//! [`EpochFence`].
//!
//! - Only Loams service identities (product roles) may call it.
//! - A request for a subject whose partition this instance does not own is
//!   refused (`not_owner`), and so is one from a gate that has already seen
//!   a newer owner epoch: this instance was fenced and has not noticed yet.
//! - One allocator per (subject, feature, usage period) per owner epoch. A
//!   partition gained from another owner (epoch > 1) starts its allocators
//!   in learning mode for one TTL (§11.5 step 3).
//! - The counter owner feeds usage ([`LeaseServer::on_usage`]) and
//!   reconciliation ([`LeaseServer::rebase`]) in; losing the partition
//!   drops its allocators.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use loams_meter_core::{
    Allocator, AllocatorConfig, Amount, FeatureKey, LeaseGrant, LeaseId, LeasePolicy, LeaseReport,
    LeaseRequest, Period, RefusalReason, SubjectKey, UtcNanos,
};

use crate::authz::Caller;
use crate::customers::Clock;
use crate::error::{MeterError, Result};
use crate::owner::{CounterKey, partition_of};

/// What an allocator needs for one (subject, feature) now: the role
/// computes it from the entitlement, its grants, the customer's payment
/// state and spend cap (§13, ME-gate's policy table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocatorInputs {
    /// The usage period the lease belongs to.
    pub period: Period,
    /// The period's total available (grants), in the feature's unit.
    pub allowance: Amount,
    /// The smallest useful chunk for the feature (§11.2: 1 request, 64 KiB,
    /// 1,000 tokens, 100 CPU-ms).
    pub min_chunk: Amount,
    pub policy: LeasePolicy,
    /// Usage counted so far in the period (the partition's counter).
    pub counted: Amount,
}

/// Where the allocator inputs come from.
#[async_trait]
pub trait LeaseInputs: Send + Sync + fmt::Debug {
    /// `Ok(None)`: the subject has no metered entitlement to the feature,
    /// answered as a `not_entitled` refusal.
    async fn inputs(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
        now: UtcNanos,
    ) -> Result<Option<AllocatorInputs>>;
}

/// A partition this instance owns.
#[derive(Clone, Copy, Debug)]
struct Owned {
    epoch: u64,
    gained_at: UtcNanos,
    /// Taken over from another owner: allocators start in learning mode.
    failover: bool,
}

#[derive(Debug, Default)]
struct State {
    owned: BTreeMap<u32, Owned>,
    allocators: HashMap<(u32, CounterKey), Allocator>,
}

/// A granted (or refused) lease with the partition it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Granted {
    pub partition: u32,
    pub grant: LeaseGrant,
}

/// The counter owner's lease server.
pub struct LeaseServer {
    partitions: u32,
    cfg: AllocatorConfig,
    inputs: Arc<dyn LeaseInputs>,
    /// The service identities allowed to lease (the product roles').
    services: Vec<String>,
    clock: Clock,
    state: Mutex<State>,
}

impl fmt::Debug for LeaseServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseServer")
            .field("partitions", &self.partitions)
            .field("services", &self.services)
            .finish_non_exhaustive()
    }
}

impl LeaseServer {
    pub fn new(
        partitions: u32,
        cfg: AllocatorConfig,
        inputs: Arc<dyn LeaseInputs>,
        services: Vec<String>,
        clock: Clock,
    ) -> Self {
        LeaseServer {
            partitions,
            cfg,
            inputs,
            services,
            clock,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This instance now owns `partition` at `epoch` (the assigner's
    /// `Gained`). An epoch above 1 means another owner held it before.
    pub fn gained(&self, partition: u32, epoch: u64) {
        let now = (self.clock)();
        let mut s = self.lock();
        s.allocators.retain(|(p, _), _| *p != partition);
        s.owned.insert(
            partition,
            Owned {
                epoch,
                gained_at: now,
                failover: epoch > 1,
            },
        );
    }

    /// This instance lost `partition` at `epoch`: stop granting for it.
    pub fn lost(&self, partition: u32, epoch: u64) {
        let mut s = self.lock();
        if s.owned.get(&partition).is_some_and(|o| o.epoch == epoch) {
            s.owned.remove(&partition);
            s.allocators.retain(|(p, _), _| *p != partition);
        }
    }

    /// The owned partitions and their epochs.
    pub fn owned(&self) -> Vec<(u32, u64)> {
        self.lock()
            .owned
            .iter()
            .map(|(p, o)| (*p, o.epoch))
            .collect()
    }

    fn require_service(&self, caller: &Caller) -> Result<()> {
        match caller {
            Caller::Service(id) if self.services.iter().any(|s| s == id) => Ok(()),
            _ => Err(MeterError::denied(
                "service_identity_required",
                "only Loams product roles may lease meter capacity",
            )),
        }
    }

    fn not_owner(partition: u32) -> MeterError {
        MeterError::precondition(
            "not_owner",
            format!("this meter instance does not own partition {partition}; ask its owner"),
        )
    }

    /// `AcquireLease`.
    #[allow(clippy::too_many_arguments)]
    pub async fn acquire(
        &self,
        caller: &Caller,
        subject: SubjectKey,
        feature: FeatureKey,
        node: String,
        want: Amount,
        held: Vec<LeaseReport>,
        known_epoch: u64,
    ) -> Result<Granted> {
        self.require_service(caller)?;
        if node.is_empty() {
            return Err(MeterError::invalid("node", "the gate's node is required"));
        }
        if want.is_negative() {
            return Err(MeterError::invalid("want", "must not be negative"));
        }
        let partition = partition_of(subject.as_str(), self.partitions);
        let owned = {
            let s = self.lock();
            s.owned.get(&partition).copied()
        };
        let Some(owned) = owned else {
            return Err(Self::not_owner(partition));
        };
        if known_epoch > owned.epoch {
            return Err(Self::not_owner(partition));
        }
        let now = (self.clock)();
        let Some(inputs) = self.inputs.inputs(&subject, &feature, now).await? else {
            return Ok(Granted {
                partition,
                grant: LeaseGrant {
                    id: None,
                    amount: Amount::ZERO,
                    has_access: false,
                    expires_at: now.add_nanos(self.cfg.ttl),
                    refresh_after: now.add_nanos(self.cfg.refresh_after),
                    owner_epoch: owned.epoch,
                    balance_hint: Amount::ZERO,
                    exact: false,
                    refusal: Some(RefusalReason::NotEntitled),
                },
            });
        };
        let key = CounterKey {
            subject: subject.clone(),
            feature: feature.clone(),
            period_start: inputs.period.start,
        };
        let mut s = self.lock();
        // The partition may have been lost while the inputs were read.
        if s.owned.get(&partition).map(|o| o.epoch) != Some(owned.epoch) {
            return Err(Self::not_owner(partition));
        }
        // A new period starts a new allocator; the old one is dropped.
        s.allocators.retain(|(p, k), _| {
            !(*p == partition
                && k.subject == key.subject
                && k.feature == key.feature
                && k.period_start != key.period_start)
        });
        let cfg = self.cfg;
        let alloc = s.allocators.entry((partition, key)).or_insert_with(|| {
            if owned.failover && now.0 < owned.gained_at.0.saturating_add(cfg.ttl) {
                Allocator::after_failover(
                    cfg,
                    owned.epoch,
                    inputs.allowance,
                    inputs.min_chunk,
                    inputs.policy,
                    inputs.counted,
                    std::iter::empty::<String>(),
                    owned.gained_at,
                )
            } else {
                Allocator::new(
                    cfg,
                    owned.epoch,
                    inputs.allowance,
                    inputs.min_chunk,
                    inputs.policy,
                    inputs.counted,
                )
            }
        });
        // Grants and payment state change between calls.
        alloc.set_allowance(inputs.allowance);
        alloc.set_policy(inputs.policy);
        alloc.tick(now);
        let grant = alloc.acquire(&LeaseRequest {
            subject,
            feature,
            node,
            want,
            held,
            now,
        });
        Ok(Granted { partition, grant })
    }

    /// `ReturnLease`: a return at another epoch than the current one is
    /// ignored (the lease belongs to an owner that is gone).
    pub fn return_lease(
        &self,
        caller: &Caller,
        subject: &SubjectKey,
        feature: &FeatureKey,
        lease: &LeaseId,
        spent: Amount,
        epoch: u64,
    ) -> Result<()> {
        self.require_service(caller)?;
        let partition = partition_of(subject.as_str(), self.partitions);
        let mut s = self.lock();
        let Some(owned) = s.owned.get(&partition).copied() else {
            return Err(Self::not_owner(partition));
        };
        if owned.epoch != epoch {
            return Ok(());
        }
        for ((p, k), a) in s.allocators.iter_mut() {
            if *p == partition && &k.subject == subject && &k.feature == feature {
                a.return_lease(lease, spent);
            }
        }
        Ok(())
    }

    /// Usage counted by the partition's owner (§11.3): retires that much of
    /// the lease the event carried.
    pub fn on_usage(
        &self,
        partition: u32,
        key: &CounterKey,
        lease: Option<&LeaseId>,
        amount: Amount,
    ) {
        let mut s = self.lock();
        if let Some(a) = s.allocators.get_mut(&(partition, key.clone())) {
            a.on_usage(lease, amount);
        }
    }

    /// Reconciliation's new count for a counter (§12).
    pub fn rebase(&self, partition: u32, key: &CounterKey, counted: Amount) {
        let mut s = self.lock();
        if let Some(a) = s.allocators.get_mut(&(partition, key.clone())) {
            a.rebase(counted);
        }
    }
}

/// The gate's side of fencing (§11.1): the newest owner epoch seen per
/// partition. A grant from an older epoch comes from an owner that was
/// fenced and has not noticed; the gate drops it.
#[derive(Debug, Default)]
pub struct EpochFence {
    newest: Mutex<HashMap<u32, u64>>,
}

impl EpochFence {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `epoch` for `partition` and answers whether a grant at it may
    /// be used (it is not older than one seen before).
    pub fn admit(&self, partition: u32, epoch: u64) -> bool {
        let mut m = self.newest.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = m.entry(partition).or_insert(0);
        if epoch < *seen {
            return false;
        }
        *seen = epoch;
        true
    }

    /// The newest epoch seen for `partition` (0: none), sent as
    /// `known_epoch` so a stale owner refuses instead of granting.
    pub fn known(&self, partition: u32) -> u64 {
        self.newest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&partition)
            .copied()
            .unwrap_or(0)
    }
}
