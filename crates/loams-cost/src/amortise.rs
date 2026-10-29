//! Hardware amortisation (design §84 §6.1, D2353): an asset's cost, less
//! its residual value, spread evenly over the days of its life, plus its
//! declared monthly costs spread over the days of each month. Each day's
//! share is exact: over the whole life the days sum to the cost, to the
//! nano-unit, with the remainder going to the earliest days.

use std::collections::BTreeSet;

use loams_meter_core::Amount;
use time::Month;

use crate::inputs::{Asset, CostInputs, DEFAULT_LIFE_MONTHS};
use crate::money::split;
use crate::{Basis, Capacity, CostError, CostSource, Day, Flag, ResourceKind, SourceRef};

/// Seconds in a UTC day (no leap seconds in Unix time).
pub const DAY_SECONDS: i128 = 86_400;

/// `day` plus `months` calendar months, the day of month clamped to the
/// target month's length.
pub fn add_months(day: Day, months: u32) -> Result<Day, CostError> {
    let d = day.date();
    let total = i64::from(d.year()) * 12 + i64::from(u8::from(d.month()) - 1) + i64::from(months);
    let year = i32::try_from(total.div_euclid(12)).map_err(|_| CostError::Overflow)?;
    let month0 = u8::try_from(total.rem_euclid(12)).map_err(|_| CostError::Overflow)?;
    let month = Month::try_from(month0 + 1).map_err(|_| CostError::Overflow)?;
    let dom = d.day().min(month.length(year));
    Day::new(year, month0 + 1, dom)
}

/// Days from `a` to `b` (`b` exclusive); negative when `b` is before `a`.
pub fn days_between(a: Day, b: Day) -> i64 {
    (b.date() - a.date()).whole_days()
}

/// Part `index` (0-based) of `total` split evenly into `parts`: the floor,
/// plus one nano-unit for the first `total mod parts` parts. Sums exactly.
fn even_part(total: Amount, parts: i64, index: i64) -> Amount {
    let parts = i128::from(parts);
    let index = i128::from(index);
    let raw = total.raw();
    let base = raw.div_euclid(parts);
    let rem = raw.rem_euclid(parts);
    Amount::from_raw(base + i128::from(index < rem))
}

/// The asset's life, `[in_service, end)`, and whether the default was used.
pub fn life(asset: &Asset) -> Result<(Day, Day, bool), CostError> {
    let months = asset.life_months.unwrap_or(DEFAULT_LIFE_MONTHS);
    let end = add_months(asset.in_service, months)?;
    Ok((asset.in_service, end, asset.life_months.is_none()))
}

/// The asset's cost sources and capacities for `day`.
///
/// - Amortisation runs over the life; the monthly costs run from
///   `in_service` until `retired` (unbounded when left out).
/// - Each source is split over the asset's `split` kinds (exactly, with
///   [`split`]); with no split it all goes to `cpu`, `estimated`, with
///   [`Flag::DefaultSplitCpuOnly`].
/// - Capacity is declared for every day from `in_service` until `retired`,
///   also after the life ends: a fully depreciated server is idle capacity at
///   zero cost.
pub fn asset_day(
    asset: &Asset,
    inputs_version: u64,
    day: Day,
) -> Result<(Vec<CostSource>, Vec<Capacity>), CostError> {
    let mut sources = Vec::new();
    let mut capacities = Vec::new();
    if day < asset.in_service || asset.retired.is_some_and(|r| day >= r) {
        return Ok((sources, capacities));
    }
    let (start, end, default_life) = life(asset)?;
    let (kinds, weights, basis, mut flags): (
        Vec<ResourceKind>,
        Vec<Amount>,
        Basis,
        BTreeSet<Flag>,
    ) = match &asset.split {
        Some(split) => (
            split.keys().copied().collect(),
            split.values().copied().collect(),
            Basis::Declared,
            BTreeSet::new(),
        ),
        None => (
            vec![ResourceKind::Cpu],
            vec![Amount::from_units(1)],
            Basis::Estimated,
            BTreeSet::from([Flag::DefaultSplitCpuOnly]),
        ),
    };

    let mut push =
        |amount: Amount, source: SourceRef, flags: &BTreeSet<Flag>| -> Result<(), CostError> {
            let parts = split(amount, &weights)?;
            for (kind, part) in kinds.iter().zip(parts) {
                sources.push(CostSource {
                    day,
                    pool: asset.pool.clone(),
                    kind: *kind,
                    amount: part,
                    basis,
                    source: source.clone(),
                    flags: flags.clone(),
                });
            }
            Ok(())
        };

    if day < end {
        let life_days = days_between(start, end);
        let index = days_between(start, day);
        let depreciable = asset.cost - asset.residual.unwrap_or(Amount::ZERO);
        let mut f = flags.clone();
        if default_life || asset.residual.is_none() {
            f.insert(Flag::DefaultInput);
        }
        push(
            even_part(depreciable, life_days, index),
            SourceRef::Asset {
                asset: asset.id.clone(),
                inputs_version,
            },
            &f,
        )?;
    }
    for (item, monthly) in &asset.monthly {
        let days = i64::from(day.days_in_month());
        let index = i64::from(day.date().day()) - 1;
        push(
            even_part(*monthly, days, index),
            SourceRef::AssetMonthly {
                asset: asset.id.clone(),
                item: item.clone(),
                inputs_version,
            },
            &flags,
        )?;
    }
    flags.remove(&Flag::DefaultSplitCpuOnly);
    for kind in &kinds {
        if let Some(per_second) = asset.capacity.per_second(*kind) {
            capacities.push(Capacity {
                day,
                pool: asset.pool.clone(),
                kind: *kind,
                quantity: per_second.mul_int(DAY_SECONDS),
                basis: Basis::Declared,
            });
        }
    }
    Ok((sources, capacities))
}

/// Every asset's sources and capacities for each day in `[from, to)`.
pub fn assets_between(
    inputs: &CostInputs,
    from: Day,
    to: Day,
) -> Result<(Vec<CostSource>, Vec<Capacity>), CostError> {
    let mut sources = Vec::new();
    let mut capacities = Vec::new();
    let mut day = from;
    while day < to {
        for asset in &inputs.assets {
            let (s, c) = asset_day(asset, inputs.version, day)?;
            sources.extend(s);
            capacities.extend(c);
        }
        day = day.next().ok_or(CostError::Overflow)?;
    }
    Ok((sources, capacities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PoolId;

    fn asset(life: Option<u32>, split: bool) -> Asset {
        Asset {
            id: "n1".into(),
            pool: PoolId::new("p").expect("p"),
            cost: Amount::parse("1000").expect("a"),
            residual: Some(Amount::parse("100").expect("a")),
            in_service: Day::parse("2026-01-31").expect("d"),
            life_months: life,
            monthly: [("hosting".to_string(), Amount::parse("31").expect("a"))].into(),
            retired: None,
            capacity: crate::inputs::AssetCapacity {
                cores: Some(Amount::from_units(4)),
                ..Default::default()
            },
            split: split.then(|| {
                [
                    (ResourceKind::Cpu, Amount::parse("0.7").expect("a")),
                    (ResourceKind::Memory, Amount::parse("0.3").expect("a")),
                ]
                .into()
            }),
            nodes: vec![],
        }
    }

    #[test]
    fn add_months_clamps() {
        let d = Day::parse("2026-01-31").expect("d");
        assert_eq!(add_months(d, 1).expect("m").to_string(), "2026-02-28");
        assert_eq!(add_months(d, 13).expect("m").to_string(), "2027-02-28");
        assert_eq!(add_months(d, 60).expect("m").to_string(), "2031-01-31");
    }

    #[test]
    fn amortisation_sums_to_cost_over_life() {
        let a = asset(Some(3), true);
        let (start, end, _) = life(&a).expect("life");
        let mut total = Amount::ZERO;
        let mut day = start;
        while day < end {
            let (s, _) = asset_day(&a, 1, day).expect("day");
            total += s
                .iter()
                .filter(|s| matches!(s.source, SourceRef::Asset { .. }))
                .map(|s| s.amount)
                .sum::<Amount>();
            day = day.next().expect("next");
        }
        assert_eq!(total, Amount::parse("900").expect("a"));
        // After the life: no amortisation, capacity still declared.
        let (s, c) = asset_day(&a, 1, end).expect("day");
        assert!(
            s.iter()
                .all(|s| !matches!(s.source, SourceRef::Asset { .. }))
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].quantity, Amount::from_units(4 * 86_400));
    }

    #[test]
    fn monthly_spread_per_day() {
        let a = asset(Some(3), false);
        let mut total = Amount::ZERO;
        let mut day = Day::parse("2026-02-01").expect("d");
        while day < Day::parse("2026-03-01").expect("d") {
            let (s, _) = asset_day(&a, 1, day).expect("day");
            total += s
                .iter()
                .filter(|s| matches!(s.source, SourceRef::AssetMonthly { .. }))
                .map(|s| s.amount)
                .sum::<Amount>();
            day = day.next().expect("next");
        }
        assert_eq!(total, Amount::parse("31").expect("a"));
    }

    #[test]
    fn default_split_marks_estimated() {
        let a = asset(Some(3), false);
        let (s, _) = asset_day(&a, 1, Day::parse("2026-02-10").expect("d")).expect("day");
        assert!(s.iter().all(|s| s.kind == ResourceKind::Cpu));
        assert!(s.iter().all(|s| s.basis == Basis::Estimated));
        assert!(
            s.iter()
                .all(|s| s.flags.contains(&Flag::DefaultSplitCpuOnly))
        );
        let a = asset(Some(3), true);
        let (s, c) = asset_day(&a, 1, Day::parse("2026-02-10").expect("d")).expect("day");
        assert!(s.iter().all(|s| s.basis == Basis::Declared));
        assert_eq!(s.len(), 4); // amortisation and hosting, each over cpu and memory
        assert_eq!(c.len(), 1); // only cores declared
    }

    #[test]
    fn default_life_marked_in_trace() {
        let a = asset(None, true);
        let (s, _) = asset_day(&a, 1, Day::parse("2026-02-10").expect("d")).expect("day");
        let am: Vec<_> = s
            .iter()
            .filter(|s| matches!(s.source, SourceRef::Asset { .. }))
            .collect();
        assert!(am.iter().all(|s| s.flags.contains(&Flag::DefaultInput)));
        let (_, end, default) = life(&a).expect("life");
        assert!(default);
        assert_eq!(end.to_string(), "2031-01-31");
    }

    #[test]
    fn nothing_before_in_service_or_after_retired() {
        let mut a = asset(Some(3), true);
        let (s, c) = asset_day(&a, 1, Day::parse("2026-01-30").expect("d")).expect("day");
        assert!(s.is_empty() && c.is_empty());
        a.retired = Some(Day::parse("2026-02-01").expect("d"));
        let (s, c) = asset_day(&a, 1, Day::parse("2026-02-01").expect("d")).expect("day");
        assert!(s.is_empty() && c.is_empty());
    }
}
