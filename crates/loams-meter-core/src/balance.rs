//! The balance engine (design §63 §9.3–§9.4, D2166): a pure replay of an
//! entitlement's grants and usage to a point in time.
//!
//! [`balance`] replays from the entitlement's first period to `at`:
//! - At each period start, every live grant keeps `rollover.apply(remaining)`
//!   (dead ones forfeit), `issue_after_reset` issues a one-period grant
//!   (priority 1, included), and for `SoftLimit` with
//!   `preserve_overage_at_reset` last period's overage burns first.
//! - A grant becomes spendable at `effective_at`; a recurring grant refills
//!   to its amount at each recurrence start; expiry and voiding stop it.
//! - Each usage segment counts wholly at its `start` and burns the live
//!   grants in burn order (priority, nearest expiry, promotional before
//!   included before prepaid, oldest, id); what no grant covers is overage.
//!
//! At one instant the order is: reset, grants becoming effective, refills,
//! carried overage, usage. Everything at or before `at` is applied.

use serde::{Deserialize, Serialize};

use crate::amount::{Amount, AmountError};
use crate::entitlement::{
    AsOf, Balance, Entitlement, EntitlementKind, Grant, GrantKind, Mode, Rollover, UsagePeriod,
};
use crate::ids::{GrantId, UtcNanos};

/// The upper bound on replay steps (resets, refills, grants and segments),
/// so a malformed history cannot make the engine run without end.
pub const MAX_STEPS: usize = 2_000_000;

/// Usage of one closed window: `amount` units in `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSegment {
    pub start: UtcNanos,
    pub end: UtcNanos,
    pub amount: Amount,
}

/// Why a balance could not be computed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BalanceError {
    #[error("the entitlement is not metered")]
    NotMetered,
    #[error("usage segment starting {0:?} is negative or ends before it starts")]
    BadSegment(UtcNanos),
    #[error("usage segment starting {0:?} straddles a usage period boundary")]
    Straddles(UtcNanos),
    #[error("the replay needs more than {MAX_STEPS} steps ({0})")]
    TooLong(usize),
    #[error("money: {0}")]
    Money(#[from] AmountError),
}

/// The synthetic grant id `issue_after_reset` uses for period `n`.
pub fn issue_grant_id(n: i64) -> GrantId {
    // "issue/" plus an integer is never empty, never over 128 bytes and has
    // no control characters, so `new` cannot fail.
    GrantId::new(format!("issue/{n}"))
        .unwrap_or_else(|_| unreachable!("issue grant ids are always valid"))
}

/// The period index replay starts from: the earliest of the entitlement's
/// creation, any grant's `effective_at` and any segment's start.
pub fn first_period_index(
    period: &UsagePeriod,
    e: &Entitlement,
    grants: &[Grant],
    usage: &[UsageSegment],
    at: UtcNanos,
) -> i64 {
    let earliest = grants
        .iter()
        .filter(|g| g.entitlement == e.id)
        .map(|g| g.effective_at)
        .chain(usage.iter().map(|s| s.start))
        .chain([e.created_at, at])
        .min()
        .unwrap_or(at);
    period.index_at(earliest)
}

#[derive(Clone, Debug)]
struct Slot {
    id: GrantId,
    priority: u8,
    expires_at: Option<UtcNanos>,
    rank: u8,
    created: UtcNanos,
    effective_at: UtcNanos,
    voided_at: Option<UtcNanos>,
    rollover: Rollover,
    amount: Amount,
    prepaid: bool,
    active: bool,
    remaining: Amount,
}

impl Slot {
    fn from_grant(g: &Grant) -> Slot {
        Slot {
            id: g.id.clone(),
            priority: g.priority,
            expires_at: g.expires_at(),
            rank: g.kind.burn_rank(),
            created: g.created_at,
            effective_at: g.effective_at,
            voided_at: g.voided_at,
            rollover: g.rollover,
            amount: g.amount,
            prepaid: matches!(g.kind, GrantKind::Prepaid { .. }),
            active: false,
            remaining: Amount::ZERO,
        }
    }

    fn issue(n: i64, start: UtcNanos, amount: Amount) -> Slot {
        Slot {
            id: issue_grant_id(n),
            priority: 1,
            expires_at: None,
            rank: GrantKind::Included.burn_rank(),
            created: start,
            effective_at: start,
            voided_at: None,
            rollover: Rollover::default(),
            amount,
            prepaid: false,
            active: true,
            remaining: amount,
        }
    }

    fn live(&self, t: UtcNanos) -> bool {
        self.active
            && self.effective_at <= t
            && self.expires_at.is_none_or(|e| t < e)
            && self.voided_at.is_none_or(|v| t < v)
    }

    fn key(&self) -> (u8, i64, u8, i64, &str) {
        (
            self.priority,
            self.expires_at.map_or(i64::MAX, |e| e.0),
            self.rank,
            self.created.0,
            self.id.as_str(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Reset(i64),
    Effective(usize),
    Refill(usize),
    Carry,
    Usage(usize),
}

impl Step {
    fn order(self) -> u8 {
        match self {
            Step::Reset(_) => 0,
            Step::Effective(_) => 1,
            Step::Refill(_) => 2,
            Step::Carry => 3,
            Step::Usage(_) => 4,
        }
    }
}

#[derive(Debug, Default)]
struct PeriodState {
    usage: Amount,
    overage: Amount,
    total: Amount,
    funded: usize,
    funded_prepaid: usize,
    carry: Amount,
}

impl PeriodState {
    fn fund(&mut self, amount: Amount, prepaid: bool) {
        self.total += amount;
        if amount.is_positive() {
            self.funded += 1;
            if prepaid {
                self.funded_prepaid += 1;
            }
        }
    }
}

/// Burns `amount` from the live slots in burn order; returns what no slot
/// covered.
fn burn(slots: &mut [Slot], t: UtcNanos, mut amount: Amount) -> Amount {
    if !amount.is_positive() {
        return Amount::ZERO;
    }
    let mut order: Vec<usize> = (0..slots.len())
        .filter(|&i| slots[i].live(t) && slots[i].remaining.is_positive())
        .collect();
    order.sort_by(|&a, &b| slots[a].key().cmp(&slots[b].key()));
    for i in order {
        let take = amount.min(slots[i].remaining);
        slots[i].remaining -= take;
        amount -= take;
        if amount.is_zero() {
            break;
        }
    }
    amount
}

/// The balance of a metered entitlement at `at`.
///
/// Grants of other entitlements are ignored. Usage segments must not
/// straddle a period boundary (closed minute, hour and day windows never
/// do).
pub fn balance(
    e: &Entitlement,
    grants: &[Grant],
    usage: &[UsageSegment],
    at: UtcNanos,
    as_of: AsOf,
) -> Result<Balance, BalanceError> {
    let EntitlementKind::Metered {
        usage_period,
        mode,
        issue_after_reset,
        preserve_overage_at_reset,
    } = &e.kind
    else {
        return Err(BalanceError::NotMetered);
    };
    let period = *usage_period;
    let carry_overage = *preserve_overage_at_reset && matches!(mode, Mode::SoftLimit);

    for s in usage {
        if s.amount.is_negative() || s.end < s.start {
            return Err(BalanceError::BadSegment(s.start));
        }
        if s.end > period.period_at(s.start).end {
            return Err(BalanceError::Straddles(s.start));
        }
    }

    let own: Vec<&Grant> = grants.iter().filter(|g| g.entitlement == e.id).collect();
    let mut slots: Vec<Slot> = own.iter().map(|g| Slot::from_grant(g)).collect();

    let n0 = first_period_index(&period, e, grants, usage, at);
    let n_at = period.index_at(at);
    let mut steps: Vec<(UtcNanos, Step)> = Vec::new();
    let budget = |len: usize| {
        if len > MAX_STEPS {
            Err(BalanceError::TooLong(len))
        } else {
            Ok(())
        }
    };
    for n in n0..=n_at {
        steps.push((period.start_of(n), Step::Reset(n)));
        if carry_overage && n > n0 {
            steps.push((period.start_of(n), Step::Carry));
        }
        budget(steps.len())?;
    }
    for (i, g) in own.iter().enumerate() {
        if g.effective_at <= at {
            steps.push((g.effective_at, Step::Effective(i)));
        }
        if let Some(r) = g.recurrence {
            let mut k = r.index_at(g.effective_at) + 1;
            loop {
                let t = r.start_of(k);
                if t > at {
                    break;
                }
                if t > g.effective_at {
                    steps.push((t, Step::Refill(i)));
                }
                k += 1;
                budget(steps.len())?;
            }
        }
    }
    for (i, s) in usage.iter().enumerate() {
        if s.start <= at {
            steps.push((s.start, Step::Usage(i)));
        }
    }
    budget(steps.len())?;
    steps.sort_by_key(|a| (a.0, a.1.order(), a.1));

    let mut state = PeriodState::default();
    for (t, step) in steps {
        match step {
            Step::Reset(n) => {
                let carry = state.overage;
                state = PeriodState {
                    carry,
                    ..PeriodState::default()
                };
                for slot in slots.iter_mut() {
                    if !slot.active {
                        continue;
                    }
                    slot.remaining = if slot.live(t) {
                        slot.rollover.apply(slot.remaining)
                    } else {
                        Amount::ZERO
                    };
                    if slot.live(t) {
                        state.fund(slot.remaining, slot.prepaid);
                    }
                }
                if let Some(amount) = issue_after_reset {
                    let slot = Slot::issue(n, t, *amount);
                    state.fund(slot.remaining, false);
                    slots.push(slot);
                }
            }
            Step::Effective(i) => {
                let slot = &mut slots[i];
                slot.active = true;
                if slot.live(t) {
                    slot.remaining = slot.amount;
                    let (amount, prepaid) = (slot.amount, slot.prepaid);
                    state.fund(amount, prepaid);
                }
            }
            Step::Refill(i) => {
                let slot = &mut slots[i];
                if slot.live(t) {
                    let added = slot.amount - slot.remaining;
                    slot.remaining = slot.amount;
                    let prepaid = slot.prepaid;
                    state.fund(added, prepaid);
                }
            }
            Step::Carry => {
                let carry = std::mem::take(&mut state.carry);
                let uncovered = burn(&mut slots, t, carry);
                state.overage += uncovered;
            }
            Step::Usage(i) => {
                let amount = usage[i].amount;
                state.usage += amount;
                let uncovered = burn(&mut slots, t, amount);
                state.overage += uncovered;
            }
        }
    }

    let mut live: Vec<&Slot> = slots
        .iter()
        .filter(|s| s.live(at) && s.remaining.is_positive())
        .collect();
    live.sort_by(|a, b| a.key().cmp(&b.key()));
    let remaining: Amount = live.iter().map(|s| s.remaining).sum();
    let prepaid_only = state.funded > 0 && state.funded == state.funded_prepaid;
    let has_access = access(mode, remaining, state.overage, prepaid_only)?;

    Ok(Balance {
        entitlement: e.id.clone(),
        period: period.period(n_at),
        has_access,
        balance: remaining,
        usage: state.usage,
        overage: state.overage,
        total_available: state.total,
        grants: live.iter().map(|s| (s.id.clone(), s.remaining)).collect(),
        prepaid_only,
        as_of,
    })
}

/// §13's rule for one entitlement: hard limits stop at zero, prepaid-only
/// funding stops at zero whatever the mode (§9.4), overage stops at the
/// spend cap.
pub fn access(
    mode: &Mode,
    balance: Amount,
    overage: Amount,
    prepaid_only: bool,
) -> Result<bool, AmountError> {
    let exhausted = !balance.is_positive();
    if exhausted && prepaid_only {
        return Ok(false);
    }
    Ok(match mode {
        Mode::HardLimit => !exhausted,
        Mode::SoftLimit => true,
        Mode::OverageAllowed {
            overage_price,
            spend_cap,
        } => {
            let spent = overage_price.times(overage)?;
            if spent.currency != spend_cap.currency {
                return Err(AmountError::CurrencyMismatch(
                    spent.currency,
                    spend_cap.currency,
                ));
            }
            spent.micros < spend_cap.micros
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::{Currency, Money};
    use crate::entitlement::{EntitlementSource, Expiry, ExpiryUnit, Interval};
    use crate::ids::{EntitlementId, FeatureKey, SubjectKey};

    const H: i64 = UtcNanos::HOUR;
    const D: i64 = UtcNanos::DAY;

    fn u(n: i64) -> Amount {
        Amount::from_units(n)
    }

    fn ent(mode: Mode, issue: Option<i64>, preserve: bool) -> Entitlement {
        Entitlement {
            id: EntitlementId::new("e1").expect("id"),
            subject: SubjectKey::new("org_1").expect("subject"),
            feature: FeatureKey::new("house_cpu").expect("feature"),
            kind: EntitlementKind::Metered {
                usage_period: UsagePeriod {
                    interval: Interval::Day,
                    anchor: UtcNanos(0),
                },
                mode,
                issue_after_reset: issue.map(u),
                preserve_overage_at_reset: preserve,
            },
            created_at: UtcNanos(0),
            source: EntitlementSource::Manual { by: "test".into() },
        }
    }

    fn grant(id: &str, amount: i64, priority: u8, at: i64) -> Grant {
        Grant {
            id: GrantId::new(id).expect("id"),
            entitlement: EntitlementId::new("e1").expect("id"),
            amount: u(amount),
            priority,
            effective_at: UtcNanos(at),
            expires: None,
            rollover: Rollover::default(),
            recurrence: None,
            kind: GrantKind::Included,
            voided_at: None,
            created_at: UtcNanos(at),
        }
    }

    fn seg(start: i64, amount: i64) -> UsageSegment {
        UsageSegment {
            start: UtcNanos(start),
            end: UtcNanos(start + UtcNanos::MINUTE),
            amount: u(amount),
        }
    }

    fn at(t: i64) -> UtcNanos {
        UtcNanos(t)
    }

    /// OpenMeter's documented burn-down: the lower priority number burns
    /// first; within a priority, the grant expiring sooner.
    #[test]
    fn burns_by_priority_then_expiry() {
        let e = ent(Mode::HardLimit, None, false);
        let mut soon = grant("soon", 10, 5, 0);
        soon.expires = Some(Expiry {
            duration: ExpiryUnit::Hour,
            count: 12,
        });
        let gs = [grant("p1", 10, 1, 0), grant("late", 10, 5, 0), soon];
        let b = balance(&e, &gs, &[seg(H, 15)], at(2 * H), AsOf::Replay).expect("balance");
        assert_eq!(b.balance, u(15));
        assert_eq!(b.usage, u(15));
        let left: Vec<(&str, Amount)> = b.grants.iter().map(|(g, a)| (g.as_str(), *a)).collect();
        assert_eq!(left, [("soon", u(5)), ("late", u(10))]);
        // After `soon` expires its remainder is forfeited.
        let later = balance(&e, &gs, &[seg(H, 15)], at(13 * H), AsOf::Replay).expect("balance");
        assert_eq!(later.balance, u(10));
    }

    /// Rollover: at a reset a grant keeps `min(max, max(balance, min))`.
    #[test]
    fn rollover_at_reset() {
        let e = ent(Mode::HardLimit, None, false);
        let mut g = grant("g", 100, 1, 0);
        g.rollover = Rollover {
            max: u(50),
            min: u(10),
        };
        let used_30 =
            balance(&e, &[g.clone()], &[seg(H, 30)], at(D + H), AsOf::Replay).expect("balance");
        assert_eq!(used_30.balance, u(50), "70 left, capped at 50");
        assert_eq!(used_30.total_available, u(50));
        let used_all =
            balance(&e, &[g.clone()], &[seg(H, 100)], at(D + H), AsOf::Replay).expect("balance");
        assert_eq!(used_all.balance, u(10), "floored at min");
        let no_rollover = grant("n", 100, 1, 0);
        let gone = balance(&e, &[no_rollover], &[], at(D), AsOf::Replay).expect("balance");
        assert_eq!(
            gone.balance,
            u(0),
            "a grant without rollover ends at the reset"
        );
    }

    #[test]
    fn issue_after_reset_each_period() {
        let e = ent(Mode::HardLimit, Some(100), false);
        let usage = [seg(H, 60), seg(D + H, 30)];
        let b = balance(&e, &[], &usage, at(D + 2 * H), AsOf::Replay).expect("balance");
        assert_eq!(b.balance, u(70));
        assert_eq!(b.usage, u(30));
        assert_eq!(b.total_available, u(100));
        assert_eq!(b.grants[0].0, issue_grant_id(1));
        let out = balance(&e, &[], &[seg(H, 130)], at(2 * H), AsOf::Replay).expect("balance");
        assert!(!out.has_access);
        assert_eq!(out.overage, u(30));
    }

    #[test]
    fn soft_limit_preserves_overage_into_next_period() {
        let e = ent(Mode::SoftLimit, Some(100), true);
        let b = balance(&e, &[], &[seg(H, 130)], at(D + H), AsOf::Replay).expect("balance");
        assert!(b.has_access);
        assert_eq!(b.balance, u(70), "last period's 30 overage burns first");
        assert_eq!(b.usage, u(0));
        // Hard limits never carry overage.
        let hard = ent(Mode::HardLimit, Some(100), true);
        let h = balance(&hard, &[], &[seg(H, 130)], at(D + H), AsOf::Replay).expect("balance");
        assert_eq!(h.balance, u(100));
    }

    #[test]
    fn recurring_grant_refills() {
        let e = ent(Mode::HardLimit, None, false);
        let mut g = grant("monthly", 40, 1, 0);
        g.recurrence = Some(UsagePeriod {
            interval: Interval::Hour,
            anchor: UtcNanos(0),
        });
        g.rollover = Rollover {
            max: u(1000),
            min: u(0),
        };
        let b = balance(&e, &[g], &[seg(10, 25)], at(H + 1), AsOf::Replay).expect("balance");
        assert_eq!(b.balance, u(40), "refilled at the hour");
        // 40 issued, then the refill added back the 25 used.
        assert_eq!(b.total_available, u(65));
    }

    #[test]
    fn prepaid_only_stops_at_zero_whatever_the_mode() {
        let e = ent(Mode::SoftLimit, None, false);
        let mut g = grant("credit", 10, 1, 0);
        g.kind = GrantKind::Prepaid {
            paid: Money::new(10_000_000, Currency::USD),
        };
        let b = balance(&e, &[g.clone()], &[seg(H, 10)], at(2 * H), AsOf::Replay).expect("b");
        assert!(b.prepaid_only);
        assert!(!b.has_access);
        let mixed = [g, grant("incl", 5, 1, 0)];
        let m = balance(&e, &mixed, &[seg(H, 15)], at(2 * H), AsOf::Replay).expect("b");
        assert!(!m.prepaid_only);
        assert!(m.has_access, "soft limit with included funding continues");
    }

    #[test]
    fn overage_stops_at_the_spend_cap() {
        let mode = Mode::OverageAllowed {
            overage_price: Money::new(1_000_000, Currency::USD),
            spend_cap: Money::new(5_000_000, Currency::USD),
        };
        let e = ent(mode, Some(10), false);
        let under = balance(&e, &[], &[seg(H, 14)], at(2 * H), AsOf::Replay).expect("b");
        assert!(under.has_access, "4 units over = $4 < $5");
        let at_cap = balance(&e, &[], &[seg(H, 15)], at(2 * H), AsOf::Replay).expect("b");
        assert!(!at_cap.has_access, "$5 reaches the cap");
    }

    #[test]
    fn voided_grant_forfeits() {
        let e = ent(Mode::HardLimit, None, false);
        let mut g = grant("g", 10, 1, 0);
        g.voided_at = Some(UtcNanos(H));
        let b = balance(&e, &[g], &[], at(2 * H), AsOf::Replay).expect("b");
        assert_eq!(b.balance, u(0));
        assert!(!b.has_access);
    }

    #[test]
    fn grant_effective_later_does_not_cover_earlier_usage() {
        let e = ent(Mode::SoftLimit, None, false);
        let b = balance(
            &e,
            &[grant("g", 10, 1, 2 * H)],
            &[seg(H, 4)],
            at(3 * H),
            AsOf::Replay,
        )
        .expect("b");
        assert_eq!(b.overage, u(4));
        assert_eq!(b.balance, u(10));
    }

    #[test]
    fn rejects_bad_input() {
        let e = ent(Mode::HardLimit, None, false);
        let straddle = UsageSegment {
            start: UtcNanos(D - 10),
            end: UtcNanos(D + 10),
            amount: u(1),
        };
        assert_eq!(
            balance(&e, &[], &[straddle], at(2 * D), AsOf::Replay),
            Err(BalanceError::Straddles(UtcNanos(D - 10)))
        );
        let negative = UsageSegment {
            start: UtcNanos(0),
            end: UtcNanos(1),
            amount: u(-1),
        };
        assert!(matches!(
            balance(&e, &[], &[negative], at(D), AsOf::Replay),
            Err(BalanceError::BadSegment(_))
        ));
        let mut boolean = e.clone();
        boolean.kind = EntitlementKind::Boolean { enabled: true };
        assert_eq!(
            balance(&boolean, &[], &[], at(0), AsOf::Replay),
            Err(BalanceError::NotMetered)
        );
    }

    #[test]
    fn other_entitlements_grants_are_ignored() {
        let e = ent(Mode::HardLimit, None, false);
        let mut other = grant("x", 10, 1, 0);
        other.entitlement = EntitlementId::new("e2").expect("id");
        let b = balance(&e, &[other], &[], at(H), AsOf::Replay).expect("b");
        assert_eq!(b.balance, u(0));
    }
}
