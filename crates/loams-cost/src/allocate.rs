//! Allocation (design §84 §8, D2363, D2364): each pool-day's cost goes to
//! its consumers in proportion to their measured driver usage, with idle
//! capacity as its own line when capacity is declared. Parts are exact:
//! per pool-day, the lines sum to the cost, to the nano-unit.

use std::collections::{BTreeMap, BTreeSet};

use loams_meter_core::Amount;
use serde::{Deserialize, Serialize};

use crate::money::split;
use crate::{
    Basis, Capacity, Consumer, ConsumerKey, CostError, CostSource, Day, Flag, PoolId, ResourceKind,
    SourceRef, Usage,
};

/// A pool's kind on a day: the unit allocation works on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PoolDayKey {
    pub day: Day,
    pub pool: PoolId,
    pub kind: ResourceKind,
}

/// One source's contribution to a pool-day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAmount {
    pub source: SourceRef,
    pub amount: Amount,
    pub basis: Basis,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub flags: BTreeSet<Flag>,
}

/// A consumer's usage of a pool-day (summed when it appears more than once).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumerUsage {
    pub consumer: ConsumerKey,
    pub quantity: Amount,
    pub basis: Basis,
}

/// Everything allocation needs about one pool-day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolDay {
    pub key: PoolDayKey,
    /// Σ sources.
    pub cost: Amount,
    /// The weakest basis of the sources; `estimated` with no sources.
    pub amount_basis: Basis,
    pub sources: Vec<SourceAmount>,
    /// Declared capacity, if any (summed over the pool's assets).
    pub capacity: Option<(Amount, Basis)>,
    /// Sorted by consumer key.
    pub usage: Vec<ConsumerUsage>,
}

impl PoolDay {
    /// Σ usage.
    pub fn used(&self) -> Result<Amount, CostError> {
        self.usage.iter().try_fold(Amount::ZERO, |acc, u| {
            acc.checked_add(u.quantity).ok_or(CostError::Overflow)
        })
    }
}

/// One allocation line: part of a pool-day's cost assigned to a consumer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub key: PoolDayKey,
    pub consumer: Consumer,
    /// In the reporting currency.
    pub amount: Amount,
    /// The consumer's driver quantity (for `idle`, the unused capacity).
    pub quantity: Amount,
    /// The quantity the share was taken of: capacity, or Σ usage, or (for a
    /// second-level split) the parent's sub-usage total. Zero for
    /// `unattributed` and `shared`.
    pub denominator: Amount,
    /// How the amount was obtained (the pool-day's sources).
    pub amount_basis: Basis,
    /// How the share was obtained.
    pub share_basis: Basis,
    /// The weaker of the two (§84 §10.1).
    pub basis: Basis,
    pub flags: BTreeSet<Flag>,
}

impl Line {
    #[allow(clippy::too_many_arguments)]
    fn new(
        key: &PoolDayKey,
        consumer: Consumer,
        amount: Amount,
        quantity: Amount,
        denominator: Amount,
        amount_basis: Basis,
        share_basis: Basis,
        flags: &BTreeSet<Flag>,
    ) -> Line {
        Line {
            key: key.clone(),
            consumer,
            amount,
            quantity,
            denominator,
            amount_basis,
            share_basis,
            basis: amount_basis.and(share_basis),
            flags: flags.clone(),
        }
    }
}

/// Groups sources, capacities and usage into pool-days, sorted by key.
/// A pool-day exists when any of the three names it.
pub fn pool_days(
    sources: &[CostSource],
    capacities: &[Capacity],
    usage: &[Usage],
) -> Result<Vec<PoolDay>, CostError> {
    #[derive(Default)]
    struct Acc {
        cost: Amount,
        sources: Vec<SourceAmount>,
        capacity: Option<(Amount, Basis)>,
        usage: BTreeMap<ConsumerKey, (Amount, Basis)>,
    }
    let mut by: BTreeMap<PoolDayKey, Acc> = BTreeMap::new();
    let key = |day: Day, pool: &PoolId, kind: ResourceKind| PoolDayKey {
        day,
        pool: pool.clone(),
        kind,
    };
    for s in sources {
        let acc = by.entry(key(s.day, &s.pool, s.kind)).or_default();
        acc.cost = acc.cost.checked_add(s.amount).ok_or(CostError::Overflow)?;
        acc.sources.push(SourceAmount {
            source: s.source.clone(),
            amount: s.amount,
            basis: s.basis,
            flags: s.flags.clone(),
        });
    }
    for c in capacities {
        if c.quantity.is_negative() {
            return Err(CostError::NegativeQuantity);
        }
        let acc = by.entry(key(c.day, &c.pool, c.kind)).or_default();
        acc.capacity = Some(match acc.capacity {
            None => (c.quantity, c.basis),
            Some((q, b)) => (
                q.checked_add(c.quantity).ok_or(CostError::Overflow)?,
                b.and(c.basis),
            ),
        });
    }
    for u in usage {
        if u.quantity.is_negative() {
            return Err(CostError::NegativeQuantity);
        }
        u.consumer.validate()?;
        let acc = by.entry(key(u.day, &u.pool, u.kind)).or_default();
        let slot = acc
            .usage
            .entry(u.consumer.clone())
            .or_insert((Amount::ZERO, u.basis));
        slot.0 = slot.0.checked_add(u.quantity).ok_or(CostError::Overflow)?;
        slot.1 = slot.1.and(u.basis);
    }
    Ok(by
        .into_iter()
        .map(|(key, mut acc)| {
            // Sources in a canonical order, so the trace does not depend on
            // input order.
            acc.sources.sort_by(|a, b| {
                (&a.source, a.amount, a.basis).cmp(&(&b.source, b.amount, b.basis))
            });
            let amount_basis =
                Basis::weakest(acc.sources.iter().map(|s| s.basis)).unwrap_or(Basis::Estimated);
            PoolDay {
                key,
                cost: acc.cost,
                amount_basis,
                sources: acc.sources,
                capacity: acc.capacity,
                usage: acc
                    .usage
                    .into_iter()
                    .map(|(consumer, (quantity, basis))| ConsumerUsage {
                        consumer,
                        quantity,
                        basis,
                    })
                    .collect(),
            }
        })
        .collect())
}

/// Allocates one pool-day (§84 §8.1). The lines are in consumer order, with
/// work first and pseudo-consumers after.
pub fn allocate(pd: &PoolDay) -> Result<Vec<Line>, CostError> {
    let key = &pd.key;
    // Lines carry their sources' flags (a default split, a default input).
    let mut flags: BTreeSet<Flag> = pd
        .sources
        .iter()
        .flat_map(|s| s.flags.iter().copied())
        .collect();
    if pd.sources.is_empty() {
        flags.insert(Flag::NoCostInput);
    }
    // Usage with zero quantity takes part in nothing but is still reported.
    let used = pd.used()?;
    let usage_basis = Basis::weakest(pd.usage.iter().map(|u| u.basis));

    // `fixed` pools have no driver: their cost is shared (D2364).
    if pd.key.kind == ResourceKind::Fixed {
        return Ok(vec![Line::new(
            key,
            Consumer::Shared,
            pd.cost,
            Amount::ZERO,
            Amount::ZERO,
            pd.amount_basis,
            pd.amount_basis,
            &flags,
        )]);
    }

    if used.is_zero() {
        let mut lines: Vec<Line> = pd
            .usage
            .iter()
            .map(|u| {
                Line::new(
                    key,
                    Consumer::Work(u.consumer.clone()),
                    Amount::ZERO,
                    u.quantity,
                    Amount::ZERO,
                    pd.amount_basis,
                    u.basis,
                    &flags,
                )
            })
            .collect();
        if !pd.cost.is_zero() {
            // With declared capacity and no usage, the whole pool sat idle;
            // without it, nobody can say who it was for.
            let consumer = if pd.capacity.is_some_and(|(c, _)| c.is_positive()) {
                Consumer::Idle
            } else {
                Consumer::Unattributed
            };
            let (quantity, share_basis) = match (&consumer, pd.capacity) {
                (Consumer::Idle, Some((c, b))) => (c, b),
                _ => (Amount::ZERO, pd.amount_basis),
            };
            lines.push(Line::new(
                key,
                consumer,
                pd.cost,
                quantity,
                quantity,
                pd.amount_basis,
                share_basis,
                &flags,
            ));
        }
        return Ok(lines);
    }

    let mut weights: Vec<Amount> = pd.usage.iter().map(|u| u.quantity).collect();
    let mut share_bases: Vec<Basis> = pd.usage.iter().map(|u| u.basis).collect();
    let idle;
    let denominator;
    match pd.capacity {
        Some((cap, cap_basis)) if cap >= used && cap.is_positive() => {
            let unused = cap - used;
            // Shares of capacity rest on the declared capacity too.
            for b in &mut share_bases {
                *b = b.and(cap_basis);
            }
            idle = Some((unused, cap_basis.and(usage_basis.unwrap_or(cap_basis))));
            denominator = cap;
        }
        Some(_) => {
            flags.insert(Flag::OverCapacity);
            share_bases.fill(Basis::Estimated);
            idle = None;
            denominator = used;
        }
        None => {
            flags.insert(Flag::IdleUnknown);
            idle = None;
            denominator = used;
        }
    }
    if let Some((unused, _)) = idle {
        weights.push(unused);
    }
    let parts = split(pd.cost, &weights)?;
    let mut lines = Vec::with_capacity(parts.len());
    for (i, u) in pd.usage.iter().enumerate() {
        lines.push(Line::new(
            key,
            Consumer::Work(u.consumer.clone()),
            parts[i],
            u.quantity,
            denominator,
            pd.amount_basis,
            share_bases[i],
            &flags,
        ));
    }
    if let Some((unused, basis)) = idle {
        // An idle line appears even at zero, so a full pool says so.
        lines.push(Line::new(
            key,
            Consumer::Idle,
            parts[pd.usage.len()],
            unused,
            denominator,
            pd.amount_basis,
            basis,
            &flags,
        ));
    }
    Ok(lines)
}

/// Every pool-day and its lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub pool_days: Vec<PoolDay>,
    pub lines: Vec<Line>,
}

/// Groups and allocates everything.
pub fn allocate_all(
    sources: &[CostSource],
    capacities: &[Capacity],
    usage: &[Usage],
) -> Result<Allocation, CostError> {
    let pool_days = pool_days(sources, capacities, usage)?;
    let mut lines = Vec::new();
    for pd in &pool_days {
        lines.extend(allocate(pd)?);
    }
    Ok(Allocation { pool_days, lines })
}

/// The second level (§84 §7.1): a work line's amount divided among finer
/// consumers (tenant, project, class inside one engine) in proportion to
/// `sub` usage, with no capacity. The new lines keep the parent's amount
/// basis and denominator chain: their `denominator` is Σ `sub`, and their
/// share basis is the weaker of the parent's and their own.
///
/// With no `sub` usage the line is returned unchanged.
pub fn split_line(line: &Line, sub: &[ConsumerUsage]) -> Result<Vec<Line>, CostError> {
    let mut merged: BTreeMap<&ConsumerKey, (Amount, Basis)> = BTreeMap::new();
    for u in sub {
        if u.quantity.is_negative() {
            return Err(CostError::NegativeQuantity);
        }
        u.consumer.validate()?;
        let slot = merged.entry(&u.consumer).or_insert((Amount::ZERO, u.basis));
        slot.0 = slot.0.checked_add(u.quantity).ok_or(CostError::Overflow)?;
        slot.1 = slot.1.and(u.basis);
    }
    let total = merged.values().try_fold(Amount::ZERO, |acc, (q, _)| {
        acc.checked_add(*q).ok_or(CostError::Overflow)
    })?;
    if total.is_zero() {
        return Ok(vec![line.clone()]);
    }
    let weights: Vec<Amount> = merged.values().map(|(q, _)| *q).collect();
    let parts = split(line.amount, &weights)?;
    Ok(merged
        .into_iter()
        .zip(parts)
        .map(|((consumer, (quantity, basis)), amount)| {
            let share_basis = line.share_basis.and(basis);
            Line {
                key: line.key.clone(),
                consumer: Consumer::Work(consumer.clone()),
                amount,
                quantity,
                denominator: total,
                amount_basis: line.amount_basis,
                share_basis,
                basis: line.amount_basis.and(share_basis),
                flags: line.flags.clone(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Amount {
        Amount::parse(s).expect("amount")
    }
    fn day() -> Day {
        Day::parse("2026-10-28").expect("day")
    }
    fn pool() -> PoolId {
        PoolId::new("prod/compute").expect("pool")
    }
    fn src(amount: &str, basis: Basis) -> CostSource {
        CostSource {
            day: day(),
            pool: pool(),
            kind: ResourceKind::Cpu,
            amount: a(amount),
            basis,
            source: SourceRef::Other {
                note: amount.into(),
            },
            flags: Default::default(),
        }
    }
    fn usage(engine: &str, q: &str) -> Usage {
        Usage {
            day: day(),
            pool: pool(),
            kind: ResourceKind::Cpu,
            consumer: ConsumerKey::new(engine, "org_1", "7", "_").expect("key"),
            quantity: a(q),
            basis: Basis::Measured,
        }
    }
    fn cap(q: &str) -> Capacity {
        Capacity {
            day: day(),
            pool: pool(),
            kind: ResourceKind::Cpu,
            quantity: a(q),
            basis: Basis::Declared,
        }
    }

    #[test]
    fn idle_line_from_capacity() {
        let out = allocate_all(
            &[src("100", Basis::Billed)],
            &[cap("1000")],
            &[usage("house", "300"), usage("pg", "200")],
        )
        .expect("alloc");
        let amounts: Vec<String> = out.lines.iter().map(|l| l.amount.to_string()).collect();
        assert_eq!(amounts, vec!["30", "20", "50"]);
        assert_eq!(out.lines[2].consumer, Consumer::Idle);
        assert_eq!(out.lines[2].quantity, a("500"));
        // Billed amount, measured usage over declared capacity → declared.
        assert_eq!(out.lines[0].basis, Basis::Declared);
        assert!(out.lines.iter().all(|l| l.flags.is_empty()));
    }

    #[test]
    fn no_capacity_is_fully_loaded_and_flagged() {
        let out = allocate_all(
            &[src("90", Basis::Billed)],
            &[],
            &[usage("house", "1"), usage("pg", "2")],
        )
        .expect("alloc");
        assert_eq!(out.lines[0].amount, a("30"));
        assert_eq!(out.lines[1].amount, a("60"));
        assert_eq!(out.lines.len(), 2);
        assert!(out.lines[0].flags.contains(&Flag::IdleUnknown));
        // Billed amount and a measured share: the strongest line.
        assert_eq!(out.lines[0].basis, Basis::Measured);
    }

    #[test]
    fn zero_usage_goes_unattributed() {
        let out = allocate_all(&[src("12.5", Basis::Billed)], &[], &[]).expect("alloc");
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].consumer, Consumer::Unattributed);
        assert_eq!(out.lines[0].amount, a("12.5"));
        let out = allocate_all(&[src("12.5", Basis::Declared)], &[cap("10")], &[]).expect("alloc");
        assert_eq!(out.lines[0].consumer, Consumer::Idle);
    }

    #[test]
    fn over_capacity_flagged_not_negative_idle() {
        let out = allocate_all(
            &[src("10", Basis::Declared)],
            &[cap("1")],
            &[usage("house", "3"), usage("pg", "1")],
        )
        .expect("alloc");
        assert_eq!(out.lines.len(), 2);
        assert!(out.lines.iter().all(|l| !l.amount.is_negative()));
        assert!(out.lines[0].flags.contains(&Flag::OverCapacity));
        assert_eq!(out.lines[0].basis, Basis::Estimated);
        assert_eq!(out.lines[0].amount, a("7.5"));
    }

    #[test]
    fn no_cost_input_still_reports_usage() {
        let out = allocate_all(&[], &[], &[usage("house", "3")]).expect("alloc");
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].amount, Amount::ZERO);
        assert!(out.lines[0].flags.contains(&Flag::NoCostInput));
    }

    #[test]
    fn fixed_pool_is_shared() {
        let mut s = src("40", Basis::Billed);
        s.kind = ResourceKind::Fixed;
        let out = allocate_all(&[s], &[], &[]).expect("alloc");
        assert_eq!(out.lines[0].consumer, Consumer::Shared);
        assert_eq!(out.lines[0].amount, a("40"));
    }

    #[test]
    fn credits_allocate_negative() {
        let out = allocate_all(
            &[src("10", Basis::Billed), src("-4", Basis::Billed)],
            &[],
            &[usage("house", "1"), usage("pg", "1")],
        )
        .expect("alloc");
        assert_eq!(out.pool_days[0].cost, a("6"));
        assert_eq!(out.lines[0].amount + out.lines[1].amount, a("6"));
    }

    #[test]
    fn two_level_split_conserves() {
        let out = allocate_all(
            &[src("1", Basis::Billed)],
            &[cap("3")],
            &[usage("house", "1")],
        )
        .expect("alloc");
        let house = &out.lines[0];
        let sub = vec![
            ConsumerUsage {
                consumer: ConsumerKey::new("house", "org_1", "7", "scan").expect("k"),
                quantity: a("2"),
                basis: Basis::Measured,
            },
            ConsumerUsage {
                consumer: ConsumerKey::new("house", "org_2", "9", "point").expect("k"),
                quantity: a("1"),
                basis: Basis::Estimated,
            },
        ];
        let lines = split_line(house, &sub).expect("split");
        assert_eq!(lines.iter().map(|l| l.amount).sum::<Amount>(), house.amount);
        assert_eq!(lines[0].basis, Basis::Declared);
        assert_eq!(lines[1].basis, Basis::Estimated);
        assert_eq!(lines[0].denominator, a("3"));
    }

    #[test]
    fn duplicate_usage_rows_are_summed() {
        let out = allocate_all(
            &[src("9", Basis::Billed)],
            &[],
            &[usage("house", "1"), usage("house", "1"), usage("pg", "1")],
        )
        .expect("alloc");
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.lines[0].amount, a("6"));
    }
}
