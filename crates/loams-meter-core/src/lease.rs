//! The lease allocator (design §63 §11, D1126, D2167): how a counter owner
//! hands out quota chunks to gates, sans-I/O.
//!
//! One [`Allocator`] serves one (subject, feature) for one owner epoch. It
//! keeps `counted` (usage read off the stream, rebased by reconciliation)
//! and, per lease, `granted − retired`, where `retired` is the usage of
//! events that carry the lease id. So a unit is never counted both as usage
//! and as an outstanding lease (§11.3):
//!
//! `available = limit − counted − Σ outstanding − learning reserve`
//!
//! Chunks follow D1126's formula, ordered as D2167 rules:
//! `min(max(min(allowance × 1%, remaining / (2 × holders)), min_chunk), available)`.
//! Below `min_chunk × holders` the allocator is in **exact mode**: one
//! holder at a time gets only what it asked for. After a failover
//! ([`Allocator::after_failover`]) it runs **learning mode** for one TTL:
//! every node that has not re-reported counts as holding one full chunk.
//!
//! Time is passed in (`now`); nothing here reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::amount::Amount;
use crate::ids::{FeatureKey, LeaseId, SubjectKey, UtcNanos};
use crate::refusal::RefusalReason;

/// Timing and sizing of leases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocatorConfig {
    /// Lease lifetime, ns (30 s).
    pub ttl: i64,
    /// When the gate should refresh, ns after the grant (10 s).
    pub refresh_after: i64,
    /// How long after expiry a lease still counts as outstanding, for its
    /// last events to arrive (5 s).
    pub stream_lag: i64,
    /// The chunk as a share of the allowance, in basis points (100 = 1%).
    pub chunk_ratio_bp: u32,
    /// How soon a requester refused only because another holds the exact
    /// lease should ask again, ns (10 ms).
    pub exact_retry: i64,
}

impl Default for AllocatorConfig {
    fn default() -> Self {
        AllocatorConfig {
            ttl: 30 * UtcNanos::SECOND,
            refresh_after: 10 * UtcNanos::SECOND,
            stream_lag: 5 * UtcNanos::SECOND,
            chunk_ratio_bp: 100,
            exact_retry: 10_000_000,
        }
    }
}

/// What the allocator does at the end of the allowance (§13; the meter
/// role picks it from the entitlement's mode, payment state and spend cap).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum LeasePolicy {
    /// Refuse at the allowance (`HardLimit`, prepaid-only funding).
    HardLimit,
    /// Count only; never refuse (`SoftLimit`, contracts).
    SoftLimit,
    /// Allow `headroom` units past the allowance (the spend cap converted
    /// to units at the overage price), then refuse with `spend_cap_reached`.
    Overage { headroom: Amount },
}

impl LeasePolicy {
    fn refusal(self) -> RefusalReason {
        match self {
            LeasePolicy::Overage { .. } => RefusalReason::SpendCapReached,
            _ => RefusalReason::EntitlementExhausted,
        }
    }
}

/// What a node reports about a lease it holds (`LeaseReport` in §11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseReport {
    pub lease: LeaseId,
    pub granted: Amount,
    pub spent: Amount,
}

/// `AcquireLease` (§11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub subject: SubjectKey,
    pub feature: FeatureKey,
    pub node: String,
    /// What the next operation needs (exact mode grants only this).
    pub want: Amount,
    pub held: Vec<LeaseReport>,
    pub now: UtcNanos,
}

/// The answer to [`LeaseRequest`] (`Lease` in §11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseGrant {
    /// `None` when nothing was granted.
    pub id: Option<LeaseId>,
    pub amount: Amount,
    /// False is a refusal the gate caches until `refresh_after`.
    pub has_access: bool,
    pub expires_at: UtcNanos,
    pub refresh_after: UtcNanos,
    pub owner_epoch: u64,
    /// What the owner believes is left after this grant.
    pub balance_hint: Amount,
    /// True when the allocator is in exact mode.
    pub exact: bool,
    pub refusal: Option<RefusalReason>,
}

/// The chunk for one grant (D1126, D2167):
/// `min(max(min(allowance × ratio, remaining / (2 × holders)), min_chunk), remaining)`,
/// floored to a multiple of `min_chunk` when at least `min_chunk`, never
/// negative.
pub fn chunk_size(
    allowance: Amount,
    min_chunk: Amount,
    remaining: Amount,
    holders: u32,
    ratio_bp: u32,
) -> Amount {
    if !remaining.is_positive() {
        return Amount::ZERO;
    }
    let pct = allowance.mul_ratio(i128::from(ratio_bp), 10_000);
    let share = remaining.mul_ratio(1, 2 * i128::from(holders.max(1)));
    let chunk = pct.min(share).max(min_chunk).min(remaining);
    // Whole multiples of `min_chunk`, so a gate that spends in `min_chunk`
    // steps strands nothing; a tail smaller than `min_chunk` is granted as is.
    let step = min_chunk.raw();
    if step > 0 && chunk >= min_chunk {
        Amount::from_raw(chunk.raw() - chunk.raw().rem_euclid(step))
    } else {
        chunk
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LeaseState {
    node: String,
    granted: Amount,
    retired: Amount,
    expires_at: UtcNanos,
    exact: bool,
}

impl LeaseState {
    fn outstanding(&self) -> Amount {
        (self.granted - self.retired).clamp_non_negative()
    }
}

/// The lease allocator for one (subject, feature) in one owner epoch.
#[derive(Clone, Debug)]
pub struct Allocator {
    cfg: AllocatorConfig,
    epoch: u64,
    allowance: Amount,
    min_chunk: Amount,
    policy: LeasePolicy,
    counted: Amount,
    leases: BTreeMap<LeaseId, LeaseState>,
    learning_until: Option<UtcNanos>,
    unreported: BTreeSet<String>,
    next_seq: u64,
}

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn base36(mut n: u64, width: usize, out: &mut String) {
    let mut digits = vec![b'0'; width];
    for d in digits.iter_mut().rev() {
        *d = BASE36[(n % 36) as usize];
        n /= 36;
    }
    out.extend(digits.into_iter().map(char::from));
}

impl Allocator {
    /// A fresh allocator: the first owner of a partition, or a new period.
    pub fn new(
        cfg: AllocatorConfig,
        epoch: u64,
        allowance: Amount,
        min_chunk: Amount,
        policy: LeasePolicy,
        counted: Amount,
    ) -> Allocator {
        Allocator {
            cfg,
            epoch,
            allowance,
            min_chunk: min_chunk.max(Amount::from_raw(1)),
            policy,
            counted,
            leases: BTreeMap::new(),
            learning_until: None,
            unreported: BTreeSet::new(),
            next_seq: 0,
        }
    }

    /// The allocator of a new owner after a failover (§11.5): `counted` is
    /// the rebuilt total (House plus tail), and every node in `known_nodes`
    /// is assumed to hold one full chunk until it re-reports or one TTL has
    /// passed.
    #[allow(clippy::too_many_arguments)]
    pub fn after_failover(
        cfg: AllocatorConfig,
        epoch: u64,
        allowance: Amount,
        min_chunk: Amount,
        policy: LeasePolicy,
        counted: Amount,
        known_nodes: impl IntoIterator<Item = String>,
        now: UtcNanos,
    ) -> Allocator {
        let mut a = Allocator::new(cfg, epoch, allowance, min_chunk, policy, counted);
        a.learning_until = Some(now.add_nanos(cfg.ttl));
        a.unreported = known_nodes.into_iter().collect();
        a
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn counted(&self) -> Amount {
        self.counted
    }

    pub fn allowance(&self) -> Amount {
        self.allowance
    }

    /// True while learning mode lasts.
    pub fn learning(&self, now: UtcNanos) -> bool {
        self.learning_until.is_some_and(|u| now < u)
    }

    /// The allowance changed (a grant was added or voided).
    pub fn set_allowance(&mut self, allowance: Amount) {
        self.allowance = allowance;
    }

    pub fn set_policy(&mut self, policy: LeasePolicy) {
        self.policy = policy;
    }

    /// Reconciliation replaced `counted` (§12).
    pub fn rebase(&mut self, counted: Amount) {
        self.counted = counted;
    }

    /// One usage event read off the stream: counts it and retires it from
    /// the lease that admitted it, if that lease is known here.
    pub fn on_usage(&mut self, lease: Option<&LeaseId>, amount: Amount) {
        self.counted += amount;
        if let Some(state) = lease.and_then(|l| self.leases.get_mut(l)) {
            state.retired = (state.retired + amount).min(state.granted);
        }
    }

    /// `ReturnLease`: the node is done with `lease` after spending `spent`;
    /// the rest stops being outstanding once the spent part is retired.
    pub fn return_lease(&mut self, lease: &LeaseId, spent: Amount) {
        if let Some(state) = self.leases.get_mut(lease) {
            state.granted = state.granted.min(spent.max(state.retired));
            state.exact = false;
        }
    }

    /// Drops leases past expiry plus the stream lag, and ends learning mode
    /// after its TTL.
    pub fn tick(&mut self, now: UtcNanos) {
        let lag = self.cfg.stream_lag;
        self.leases.retain(|_, l| now < l.expires_at.add_nanos(lag));
        if self.learning_until.is_some_and(|u| now >= u) {
            self.learning_until = None;
            self.unreported.clear();
        }
    }

    /// Σ outstanding over the live leases.
    pub fn outstanding(&self) -> Amount {
        self.leases.values().map(LeaseState::outstanding).sum()
    }

    fn learning_reserve(&self, now: UtcNanos) -> Amount {
        if !self.learning(now) {
            return Amount::ZERO;
        }
        let chunk = self.full_chunk();
        chunk.mul_int(self.unreported.len() as i128)
    }

    /// The largest chunk a node can hold: 1% of the allowance or `min_chunk`.
    pub fn full_chunk(&self) -> Amount {
        self.allowance
            .mul_ratio(i128::from(self.cfg.chunk_ratio_bp), 10_000)
            .max(self.min_chunk)
    }

    /// The limit leases may not pass, or `None` when nothing is refused.
    fn limit(&self) -> Option<Amount> {
        match self.policy {
            LeasePolicy::HardLimit => Some(self.allowance),
            LeasePolicy::SoftLimit => None,
            LeasePolicy::Overage { headroom } => Some(self.allowance + headroom),
        }
    }

    /// What may still be granted at `now`; `None` when unbounded.
    pub fn available(&self, now: UtcNanos) -> Option<Amount> {
        self.limit().map(|limit| {
            (limit - self.counted - self.outstanding() - self.learning_reserve(now))
                .clamp_non_negative()
        })
    }

    fn holders(&self, requester: &str) -> u32 {
        let mut nodes: BTreeSet<&str> = self
            .leases
            .values()
            .filter(|l| l.outstanding().is_positive())
            .map(|l| l.node.as_str())
            .collect();
        nodes.insert(requester);
        u32::try_from(nodes.len()).unwrap_or(u32::MAX)
    }

    fn next_id(&mut self) -> LeaseId {
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut s = String::from("l");
        base36(self.epoch % 36u64.pow(3), 3, &mut s);
        base36(seq % 36u64.pow(6), 6, &mut s);
        // Ten lower-case letters and digits: always a valid `LeaseId`.
        LeaseId::new(s).unwrap_or_else(|_| unreachable!("generated lease ids are valid"))
    }

    fn adopt(&mut self, node: &str, report: &LeaseReport, now: UtcNanos) {
        // A lease granted by an earlier owner: what it spent is already in
        // the rebuilt `counted`, so only the unspent part is outstanding.
        if !self.leases.contains_key(&report.lease) {
            self.leases.insert(
                report.lease.clone(),
                LeaseState {
                    node: node.to_owned(),
                    granted: report.granted,
                    retired: report.spent.min(report.granted),
                    expires_at: now.add_nanos(self.cfg.ttl),
                    exact: false,
                },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn answer(
        &self,
        id: Option<LeaseId>,
        amount: Amount,
        has_access: bool,
        exact: bool,
        refusal: Option<RefusalReason>,
        retry_in: i64,
        now: UtcNanos,
    ) -> LeaseGrant {
        LeaseGrant {
            id,
            amount,
            has_access,
            expires_at: now.add_nanos(self.cfg.ttl),
            refresh_after: now.add_nanos(retry_in),
            owner_epoch: self.epoch,
            balance_hint: self
                .available(now)
                .unwrap_or_else(|| (self.allowance - self.counted).clamp_non_negative()),
            exact,
            refusal,
        }
    }

    /// `AcquireLease`.
    pub fn acquire(&mut self, req: &LeaseRequest) -> LeaseGrant {
        let now = req.now;
        self.tick(now);
        for report in &req.held {
            if self.leases.contains_key(&report.lease) {
                continue;
            }
            if self.learning(now) {
                self.adopt(&req.node, report, now);
            }
        }
        self.unreported.remove(&req.node);

        let holders = self.holders(&req.node);
        let Some(available) = self.available(now) else {
            // Unbounded: count only, chunks of the full size.
            let amount = self.full_chunk().max(req.want);
            return self.grant(req, amount, false, now);
        };
        if !available.is_positive() {
            let reason = self.policy.refusal();
            return self.answer(
                None,
                Amount::ZERO,
                false,
                false,
                Some(reason),
                self.cfg.refresh_after,
                now,
            );
        }
        let exact = available < self.min_chunk.mul_int(i128::from(holders));
        if exact {
            let busy = self
                .leases
                .values()
                .any(|l| l.exact && l.node != req.node && l.outstanding().is_positive());
            if busy {
                return self.answer(
                    None,
                    Amount::ZERO,
                    true,
                    true,
                    None,
                    self.cfg.exact_retry,
                    now,
                );
            }
            let amount = req.want.max(Amount::from_raw(1)).min(available);
            return self.grant(req, amount, true, now);
        }
        let amount = chunk_size(
            self.allowance,
            self.min_chunk,
            available,
            holders,
            self.cfg.chunk_ratio_bp,
        )
        .max(req.want.min(available));
        self.grant(req, amount, false, now)
    }

    fn grant(
        &mut self,
        req: &LeaseRequest,
        amount: Amount,
        exact: bool,
        now: UtcNanos,
    ) -> LeaseGrant {
        let id = self.next_id();
        self.leases.insert(
            id.clone(),
            LeaseState {
                node: req.node.clone(),
                granted: amount,
                retired: Amount::ZERO,
                expires_at: now.add_nanos(self.cfg.ttl),
                exact,
            },
        );
        self.answer(
            Some(id),
            amount,
            true,
            exact,
            None,
            self.cfg.refresh_after,
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: i64) -> Amount {
        Amount::from_units(n)
    }

    fn req(node: &str, want: i64, now: i64) -> LeaseRequest {
        LeaseRequest {
            subject: SubjectKey::new("org_1").expect("subject"),
            feature: FeatureKey::new("api_requests").expect("feature"),
            node: node.into(),
            want: u(want),
            held: Vec::new(),
            now: UtcNanos(now),
        }
    }

    #[test]
    fn chunk_formula() {
        // Early in the period: 1% of the allowance.
        assert_eq!(chunk_size(u(10_000), u(1), u(10_000), 3, 100), u(100));
        // Near the end: remaining / (2 × holders).
        assert_eq!(chunk_size(u(10_000), u(1), u(600), 10, 100), u(30));
        // Never below min_chunk, never above remaining.
        assert_eq!(chunk_size(u(10_000), u(64), u(600), 10, 100), u(64));
        assert_eq!(chunk_size(u(10_000), u(64), u(40), 10, 100), u(40));
        assert_eq!(chunk_size(u(10_000), u(1), u(0), 1, 100), u(0));
        // Floored to whole min_chunks: 150 / 20 = 7.5 → 7; 130 / 2 = 65 → 64.
        assert_eq!(chunk_size(u(10_000), u(1), u(150), 10, 100), u(7));
        assert_eq!(chunk_size(u(10_000), u(32), u(130), 1, 100), u(64));
    }

    #[test]
    fn grants_never_exceed_available() {
        let mut a = Allocator::new(
            AllocatorConfig::default(),
            1,
            u(1_000),
            u(1),
            LeasePolicy::HardLimit,
            u(0),
        );
        let mut total = Amount::ZERO;
        for i in 0..500 {
            let g = a.acquire(&req(&format!("n{}", i % 7), 1, i));
            total += g.amount;
            // The node spends the whole grant and its events arrive.
            a.on_usage(g.id.as_ref(), g.amount);
        }
        assert!(total <= u(1_000), "granted {total}");
        let refused = a.acquire(&req("n0", 1, 501));
        assert!(!refused.has_access);
        assert_eq!(refused.refusal, Some(RefusalReason::EntitlementExhausted));
    }

    #[test]
    fn exact_mode_near_zero() {
        let mut a = Allocator::new(
            AllocatorConfig::default(),
            1,
            u(100),
            u(20),
            LeasePolicy::HardLimit,
            u(85),
        );
        // 15 left, below min_chunk × holders (20 × 1): exact mode.
        let first = a.acquire(&req("a", 2, 0));
        assert!(first.exact);
        assert_eq!(first.amount, u(2));
        let second = a.acquire(&req("b", 2, 1));
        assert!(second.exact && second.has_access);
        assert_eq!(second.amount, u(0), "one exact holder at a time");
        // The first holder's event retires its lease; the next may go.
        a.on_usage(first.id.as_ref(), u(2));
        let third = a.acquire(&req("b", 2, 2));
        assert_eq!(third.amount, u(2));
    }

    #[test]
    fn retired_usage_is_not_counted_twice() {
        let mut a = Allocator::new(
            AllocatorConfig::default(),
            1,
            u(1_000),
            u(1),
            LeasePolicy::HardLimit,
            u(0),
        );
        let g = a.acquire(&req("a", 1, 0));
        assert_eq!(g.amount, u(10));
        assert_eq!(a.available(UtcNanos(0)), Some(u(990)));
        a.on_usage(g.id.as_ref(), u(4));
        assert_eq!(a.counted(), u(4));
        assert_eq!(a.available(UtcNanos(0)), Some(u(990)));
        // Usage of an unknown lease only counts.
        a.on_usage(None, u(5));
        assert_eq!(a.available(UtcNanos(0)), Some(u(985)));
    }

    #[test]
    fn expired_lease_releases_after_stream_lag() {
        let cfg = AllocatorConfig::default();
        let mut a = Allocator::new(cfg, 1, u(1_000), u(1), LeasePolicy::HardLimit, u(0));
        let g = a.acquire(&req("a", 1, 0));
        a.tick(g.expires_at);
        assert_eq!(a.outstanding(), u(10), "still within the lag");
        a.tick(g.expires_at.add_nanos(cfg.stream_lag));
        assert_eq!(a.outstanding(), u(0));
    }

    #[test]
    fn owner_failover_learning_mode() {
        let cfg = AllocatorConfig::default();
        let nodes = ["a", "b", "c"].map(String::from);
        let mut a = Allocator::after_failover(
            cfg,
            2,
            u(10_000),
            u(1),
            LeasePolicy::HardLimit,
            u(5_000),
            nodes.clone(),
            UtcNanos(0),
        );
        // Each unreported node is assumed to hold a full chunk (100).
        assert_eq!(a.available(UtcNanos(0)), Some(u(4_700)));
        let mut r = req("a", 1, 1);
        r.held.push(LeaseReport {
            lease: LeaseId::new("l001000000").expect("id"),
            granted: u(100),
            spent: u(30),
        });
        let g = a.acquire(&r);
        assert_eq!(g.owner_epoch, 2);
        // "a" re-reported: its reserve becomes its real 70 outstanding plus
        // the new grant; "b" and "c" are still reserved.
        assert_eq!(
            a.available(UtcNanos(1)),
            Some(u(5_000) - u(70) - g.amount - u(200))
        );
        a.tick(UtcNanos(cfg.ttl));
        assert!(!a.learning(UtcNanos(cfg.ttl)));
    }

    #[test]
    fn soft_limit_never_refuses_and_overage_stops_at_headroom() {
        let mut soft = Allocator::new(
            AllocatorConfig::default(),
            1,
            u(10),
            u(1),
            LeasePolicy::SoftLimit,
            u(50),
        );
        let g = soft.acquire(&req("a", 1, 0));
        assert!(g.has_access && g.amount.is_positive());

        let mut over = Allocator::new(
            AllocatorConfig::default(),
            1,
            u(10),
            u(1),
            LeasePolicy::Overage { headroom: u(5) },
            u(15),
        );
        let refused = over.acquire(&req("a", 1, 0));
        assert_eq!(refused.refusal, Some(RefusalReason::SpendCapReached));
    }

    #[test]
    fn lease_ids_are_valid_and_unique() {
        let mut a = Allocator::new(
            AllocatorConfig::default(),
            46_655,
            u(1_000_000),
            u(1),
            LeasePolicy::SoftLimit,
            u(0),
        );
        let mut seen = BTreeSet::new();
        for i in 0..1_000 {
            let g = a.acquire(&req("a", 1, i));
            let id = g.id.expect("granted");
            assert_eq!(id.as_str().len(), 10);
            assert!(seen.insert(id));
        }
    }
}
