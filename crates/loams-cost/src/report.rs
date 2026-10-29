//! Reports, unit costs, unit economics and trends (design §84 §9,
//! D2365, D2366, D2369).
//!
//! A report groups allocation lines by the dimensions asked for and by
//! period, and computes the unit costs that apply. Each row keeps the
//! weakest basis of its lines, the union of their flags and their trace
//! ids. Unit costs below the minimum sample are withheld, never shown as
//! noise. Fully loaded figures (idle, unattributed and shared spread over
//! work in proportion to attributed cost) are separate columns, labelled
//! `estimated` (§84 §8.3).

use std::collections::{BTreeMap, BTreeSet};

use loams_meter_core::Amount;
use serde::{Deserialize, Serialize};

use crate::inputs::MEAN_MONTH_SECONDS;
use crate::money::{mul_div, split};
use crate::{Basis, Consumer, ConsumerKey, CostError, Day, Flag, Line, ResourceKind, trace_id};

/// Minimum ops for $ per 1M ops (D2369).
pub const MIN_OPS: u64 = 1_000;
/// Minimum jobs for $ per job (D2369).
pub const MIN_JOBS: u64 = 10;
/// Minimum stored byte-seconds for $ per GB-month: 1 GiB-day (D2369).
pub const MIN_STORAGE_BYTE_SECONDS: i128 = (1 << 30) * 86_400;

/// Engines whose ops counter counts jobs, so they get $ per job.
pub const JOB_ENGINES: [&str; 3] = ["jobs", "durable", "functions"];

/// A dimension a report can group by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Engine,
    Tenant,
    Project,
    QueryClass,
    Pool,
    Kind,
}

impl Dimension {
    pub fn parse(s: &str) -> Result<Dimension, CostError> {
        Ok(match s {
            "engine" => Dimension::Engine,
            "tenant" => Dimension::Tenant,
            "project" => Dimension::Project,
            "query_class" | "class" => Dimension::QueryClass,
            "pool" => Dimension::Pool,
            "kind" => Dimension::Kind,
            other => {
                return Err(CostError::invalid(
                    "group_by",
                    format!("unknown dimension {other:?}"),
                ));
            }
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Dimension::Engine => "engine",
            Dimension::Tenant => "tenant",
            Dimension::Project => "project",
            Dimension::QueryClass => "query_class",
            Dimension::Pool => "pool",
            Dimension::Kind => "kind",
        }
    }

    fn value(self, line: &Line) -> String {
        let work = line.consumer.work();
        let part = |f: fn(&ConsumerKey) -> &String| work.map_or("_".to_string(), |k| f(k).clone());
        match self {
            Dimension::Engine => line.consumer.engine().to_string(),
            Dimension::Tenant => part(|k| &k.tenant),
            Dimension::Project => part(|k| &k.project),
            Dimension::QueryClass => part(|k| &k.query_class),
            Dimension::Pool => line.key.pool.to_string(),
            Dimension::Kind => line.key.kind.to_string(),
        }
    }
}

/// How rows are bucketed in time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    #[default]
    Day,
    /// ISO weeks, starting Monday.
    Week,
    Month,
    /// One bucket for the whole range.
    Total,
}

impl Granularity {
    /// The first day of `day`'s bucket (`from` for `Total`).
    pub fn bucket(self, day: Day, from: Day) -> Result<Day, CostError> {
        let d = day.date();
        Ok(match self {
            Granularity::Day => day,
            Granularity::Week => Day::from_date(
                d - time::Duration::days(i64::from(d.weekday().number_days_from_monday())),
            ),
            Granularity::Month => Day::new(d.year(), u8::from(d.month()), 1)?,
            Granularity::Total => from,
        })
    }
}

/// An engine's ops on a day for a consumer, from the drivers (§84 §7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsCount {
    pub day: Day,
    pub consumer: ConsumerKey,
    pub ops: u64,
}

/// A declared revenue amount for a tenant over a period (§84 §9.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revenue {
    pub tenant: String,
    pub from: Day,
    /// Exclusive.
    pub to: Day,
    pub amount: Amount,
    pub basis: Basis,
}

/// A date when an input changed, marked on trends (§84 §9.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputChange {
    pub day: Day,
    pub what: String,
}

/// What a report asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRequest {
    pub from: Day,
    /// Exclusive.
    pub to: Day,
    pub group_by: Vec<Dimension>,
    pub granularity: Granularity,
}

/// Unit costs of one row; `None` where it does not apply or is withheld
/// (the row's flags say why).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitCosts {
    /// Non-storage cost per 1M ops.
    pub per_million_ops: Option<Amount>,
    /// Storage cost per GB-month (10⁹ bytes, mean month).
    pub per_gb_month: Option<Amount>,
    /// Non-storage cost per job (job engines only).
    pub per_job: Option<Amount>,
    /// CPU cost per core-hour used.
    pub per_core_hour: Option<Amount>,
}

/// One report row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRow {
    pub period: Day,
    /// The dimension values, in the request's `group_by` order.
    pub group: Vec<(Dimension, String)>,
    pub cost: Amount,
    /// Cost with idle, unattributed and shared cost spread over work
    /// (`None` for pseudo-consumer rows).
    pub fully_loaded_cost: Option<Amount>,
    /// Driver quantity per kind.
    pub quantities: BTreeMap<ResourceKind, Amount>,
    /// Cost per kind.
    pub cost_by_kind: BTreeMap<ResourceKind, Amount>,
    pub ops: u64,
    pub unit: UnitCosts,
    /// The same unit costs on the fully loaded cost.
    pub unit_fully_loaded: UnitCosts,
    pub basis: Basis,
    pub flags: BTreeSet<Flag>,
    /// The traces of the lines in the row, sorted.
    pub traces: Vec<String>,
}

/// Totals that make a report reconcile (§84 §9.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    /// Σ every line = Σ cost inputs allocated.
    pub input: Amount,
    pub attributed: Amount,
    pub idle: Amount,
    pub unattributed: Amount,
    pub shared: Amount,
}

/// A report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub request: ReportRequest,
    pub totals: Totals,
    pub rows: Vec<ReportRow>,
    /// Input changes inside the range, for trend markers.
    pub markers: Vec<InputChange>,
}

/// `amount × scale / count`, floored; `None` for a zero count.
fn per(amount: Amount, scale: u128, count: u128) -> Option<Amount> {
    if count == 0 {
        return None;
    }
    let neg = amount.is_negative();
    let (q, _) = mul_div(amount.raw().unsigned_abs(), scale, count)?;
    let q = i128::try_from(q).ok()?;
    Some(Amount::from_raw(if neg { -q } else { q }))
}

fn unit_costs(
    engine: &str,
    cost_by_kind: &BTreeMap<ResourceKind, Amount>,
    quantities: &BTreeMap<ResourceKind, Amount>,
    ops: Option<u64>,
    flags: &mut BTreeSet<Flag>,
) -> UnitCosts {
    let mut u = UnitCosts::default();
    let non_storage: Amount = cost_by_kind
        .iter()
        .filter(|(k, _)| **k != ResourceKind::Storage)
        .map(|(_, a)| *a)
        .sum();
    let has_non_storage = cost_by_kind.keys().any(|k| *k != ResourceKind::Storage);
    let is_job = JOB_ENGINES.contains(&engine);
    match ops {
        None if has_non_storage => {
            flags.insert(Flag::OpsUnavailable);
        }
        None => {}
        Some(n) if n < MIN_OPS && !is_job => {
            flags.insert(Flag::InsufficientSample);
        }
        Some(n) => {
            if is_job {
                if n < MIN_JOBS {
                    flags.insert(Flag::InsufficientSample);
                } else {
                    u.per_job = per(non_storage, 1, u128::from(n));
                }
            }
            if n >= MIN_OPS {
                u.per_million_ops = per(non_storage, 1_000_000, u128::from(n));
            }
        }
    }
    if let Some(stored) = quantities.get(&ResourceKind::Storage) {
        let cost = cost_by_kind
            .get(&ResourceKind::Storage)
            .copied()
            .unwrap_or_default();
        if stored.raw() < MIN_STORAGE_BYTE_SECONDS * Amount::SCALE {
            if stored.is_positive() {
                flags.insert(Flag::InsufficientSample);
            }
        } else {
            // cost / (byte-seconds / (10⁹ × month)) with byte-seconds in
            // nano-units: cost × 10⁹ × month × SCALE / raw.
            u.per_gb_month = per(
                cost,
                1_000_000_000u128 * MEAN_MONTH_SECONDS * Amount::SCALE as u128,
                stored.raw().unsigned_abs(),
            );
        }
    }
    if let Some(cpu) = quantities
        .get(&ResourceKind::Cpu)
        .filter(|q| q.is_positive())
    {
        let cost = cost_by_kind
            .get(&ResourceKind::Cpu)
            .copied()
            .unwrap_or_default();
        u.per_core_hour = per(
            cost,
            3_600 * Amount::SCALE as u128,
            cpu.raw().unsigned_abs(),
        );
    }
    u
}

/// Spreads each day's idle, unattributed and shared cost over its work
/// lines in proportion to their (positive) attributed amounts. Returns the
/// fully loaded amount per line index; pseudo-consumer lines get `None`.
/// A day with no positive work keeps its pseudo cost unspread.
pub fn fully_loaded(lines: &[Line]) -> Result<Vec<Option<Amount>>, CostError> {
    let mut out: Vec<Option<Amount>> = lines
        .iter()
        .map(|l| l.consumer.work().map(|_| l.amount))
        .collect();
    let mut by_day: BTreeMap<Day, (Amount, Vec<usize>)> = BTreeMap::new();
    for (i, l) in lines.iter().enumerate() {
        let e = by_day
            .entry(l.key.day)
            .or_insert((Amount::ZERO, Vec::new()));
        match l.consumer {
            Consumer::Work(_) => {
                if l.amount.is_positive() {
                    e.1.push(i);
                }
            }
            _ => e.0 = e.0.checked_add(l.amount).ok_or(CostError::Overflow)?,
        }
    }
    for (_, (pseudo, work)) in by_day {
        if pseudo.is_zero() || work.is_empty() {
            continue;
        }
        let weights: Vec<Amount> = work.iter().map(|i| lines[*i].amount).collect();
        for (i, part) in work.iter().zip(split(pseudo, &weights)?) {
            out[*i] = Some(
                lines[*i]
                    .amount
                    .checked_add(part)
                    .ok_or(CostError::Overflow)?,
            );
        }
    }
    Ok(out)
}

/// Builds a report from allocation lines (after any second-level split),
/// the ops counters and the input changes. Lines outside `[from, to)` are
/// ignored.
pub fn build(
    request: &ReportRequest,
    lines: &[Line],
    ops: &[OpsCount],
    changes: &[InputChange],
) -> Result<Report, CostError> {
    if request.from >= request.to {
        return Err(CostError::invalid("from", "from must be before to"));
    }
    let lines: Vec<Line> = lines
        .iter()
        .filter(|l| l.key.day >= request.from && l.key.day < request.to)
        .cloned()
        .collect();
    let loaded = fully_loaded(&lines)?;

    let mut totals = Totals::default();
    #[derive(Default)]
    struct Acc {
        cost: Amount,
        loaded: Option<Amount>,
        quantities: BTreeMap<ResourceKind, Amount>,
        cost_by_kind: BTreeMap<ResourceKind, Amount>,
        loaded_by_kind: BTreeMap<ResourceKind, Amount>,
        basis: Option<Basis>,
        flags: BTreeSet<Flag>,
        traces: BTreeSet<String>,
        consumers: BTreeSet<(Day, ConsumerKey)>,
        engine: String,
        work: bool,
    }
    let mut rows: BTreeMap<(Day, Vec<(Dimension, String)>), Acc> = BTreeMap::new();
    let add = |a: Amount, b: Amount| a.checked_add(b).ok_or(CostError::Overflow);
    for (line, loaded) in lines.iter().zip(&loaded) {
        match line.consumer {
            Consumer::Work(_) => totals.attributed = add(totals.attributed, line.amount)?,
            Consumer::Idle => totals.idle = add(totals.idle, line.amount)?,
            Consumer::Unattributed => totals.unattributed = add(totals.unattributed, line.amount)?,
            Consumer::Shared => totals.shared = add(totals.shared, line.amount)?,
        }
        totals.input = add(totals.input, line.amount)?;
        let period = request.granularity.bucket(line.key.day, request.from)?;
        let group: Vec<(Dimension, String)> = request
            .group_by
            .iter()
            .map(|d| (*d, d.value(line)))
            .collect();
        let acc = rows.entry((period, group)).or_default();
        acc.cost = add(acc.cost, line.amount)?;
        let kind = line.key.kind;
        let q = acc.quantities.entry(kind).or_default();
        *q = add(*q, line.quantity)?;
        let c = acc.cost_by_kind.entry(kind).or_default();
        *c = add(*c, line.amount)?;
        if let Some(l) = loaded {
            acc.loaded = Some(add(acc.loaded.unwrap_or_default(), *l)?);
            let c = acc.loaded_by_kind.entry(kind).or_default();
            *c = add(*c, *l)?;
            if l != &line.amount {
                acc.flags.insert(Flag::Spread);
            }
        }
        acc.basis = Some(acc.basis.map_or(line.basis, |b| b.and(line.basis)));
        acc.flags.extend(line.flags.iter().copied());
        acc.traces.insert(trace_id(line));
        acc.engine = line.consumer.engine().to_string();
        if let Consumer::Work(k) = &line.consumer {
            acc.work = true;
            acc.consumers.insert((line.key.day, k.clone()));
        }
    }

    // Ops per (day, consumer); a consumer with no entry has no counter.
    let mut ops_by: BTreeMap<(Day, &ConsumerKey), u64> = BTreeMap::new();
    for o in ops {
        let slot = ops_by.entry((o.day, &o.consumer)).or_default();
        *slot = slot.saturating_add(o.ops);
    }

    let rows = rows
        .into_iter()
        .map(|((period, group), mut acc)| {
            let mut ops_total: Option<u64> = None;
            for (day, consumer) in &acc.consumers {
                if let Some(n) = ops_by.get(&(*day, consumer)) {
                    ops_total = Some(ops_total.unwrap_or(0).saturating_add(*n));
                }
            }
            // A row mixing engines with and without counters: the counted
            // ones only would understate the denominator, so withhold.
            let partial = acc
                .consumers
                .iter()
                .any(|(d, c)| !ops_by.contains_key(&(*d, c)));
            let ops_for_unit = if partial { None } else { ops_total };
            let unit = if acc.work {
                unit_costs(
                    &acc.engine,
                    &acc.cost_by_kind,
                    &acc.quantities,
                    ops_for_unit,
                    &mut acc.flags,
                )
            } else {
                UnitCosts::default()
            };
            let unit_fully_loaded = if acc.work {
                let mut ignore = BTreeSet::new();
                unit_costs(
                    &acc.engine,
                    &acc.loaded_by_kind,
                    &acc.quantities,
                    ops_for_unit,
                    &mut ignore,
                )
            } else {
                UnitCosts::default()
            };
            ReportRow {
                period,
                group,
                cost: acc.cost,
                fully_loaded_cost: acc.loaded,
                quantities: acc.quantities,
                cost_by_kind: acc.cost_by_kind,
                ops: ops_total.unwrap_or(0),
                unit,
                unit_fully_loaded,
                basis: acc.basis.unwrap_or(Basis::Estimated),
                flags: acc.flags,
                traces: acc.traces.into_iter().collect(),
            }
        })
        .collect();
    let markers = changes
        .iter()
        .filter(|c| c.day >= request.from && c.day < request.to)
        .cloned()
        .collect();
    Ok(Report {
        request: request.clone(),
        totals,
        rows,
        markers,
    })
}

/// Unit economics for one tenant over a period (§84 §9.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economics {
    pub tenant: String,
    pub cost: Amount,
    /// Absent when no revenue is known, never zero by default.
    pub revenue: Option<Amount>,
    pub margin: Option<Amount>,
    /// Margin as a fraction of revenue (9 decimals), when revenue > 0.
    pub margin_ratio: Option<Amount>,
    pub basis: Basis,
}

/// Cost against revenue per tenant over `[from, to)`. Revenue counts only
/// when its period lies inside the range.
pub fn economics(
    lines: &[Line],
    revenue: &[Revenue],
    from: Day,
    to: Day,
) -> Result<Vec<Economics>, CostError> {
    let mut cost: BTreeMap<String, (Amount, Basis)> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.key.day >= from && l.key.day < to) {
        if let Consumer::Work(k) = &l.consumer {
            let e = cost
                .entry(k.tenant.clone())
                .or_insert((Amount::ZERO, l.basis));
            e.0 = e.0.checked_add(l.amount).ok_or(CostError::Overflow)?;
            e.1 = e.1.and(l.basis);
        }
    }
    let mut rev: BTreeMap<String, (Amount, Basis)> = BTreeMap::new();
    for r in revenue.iter().filter(|r| r.from >= from && r.to <= to) {
        let e = rev
            .entry(r.tenant.clone())
            .or_insert((Amount::ZERO, r.basis));
        e.0 = e.0.checked_add(r.amount).ok_or(CostError::Overflow)?;
        e.1 = e.1.and(r.basis);
    }
    Ok(cost
        .into_iter()
        .map(|(tenant, (c, cb))| {
            let r = rev.get(&tenant).copied();
            let margin = r.map(|(r, _)| r - c);
            let margin_ratio = match (margin, r) {
                (Some(m), Some((r, _))) if r.is_positive() => {
                    per(m, Amount::SCALE as u128, r.raw().unsigned_abs())
                }
                _ => None,
            };
            Economics {
                tenant,
                cost: c,
                revenue: r.map(|(r, _)| r),
                margin,
                margin_ratio,
                basis: r.map_or(cb, |(_, rb)| cb.and(rb)),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capacity, CostSource, PoolId, SourceRef, Usage, allocate_all};

    fn a(s: &str) -> Amount {
        Amount::parse(s).expect("amount")
    }
    fn day(d: u8) -> Day {
        Day::new(2026, 10, d).expect("day")
    }
    fn key(engine: &str, tenant: &str) -> ConsumerKey {
        ConsumerKey::new(engine, tenant, "_", "_").expect("key")
    }

    /// Two days of one compute pool (100 per day, 1,000 core-seconds of
    /// capacity) and a bucket (31 per day, priced storage).
    fn lines() -> Vec<Line> {
        let pool = PoolId::new("eu/compute").expect("p");
        let bucket = PoolId::new("eu/bucket").expect("p");
        let mut sources = Vec::new();
        let mut caps = Vec::new();
        let mut usage = Vec::new();
        for d in [day(1), day(2)] {
            sources.push(CostSource {
                day: d,
                pool: pool.clone(),
                kind: ResourceKind::Cpu,
                amount: a("100"),
                basis: Basis::Billed,
                source: SourceRef::Other { note: "c".into() },
                flags: BTreeSet::new(),
            });
            sources.push(CostSource {
                day: d,
                pool: bucket.clone(),
                kind: ResourceKind::Storage,
                amount: a("31"),
                basis: Basis::Billed,
                source: SourceRef::Other { note: "b".into() },
                flags: BTreeSet::new(),
            });
            caps.push(Capacity {
                day: d,
                pool: pool.clone(),
                kind: ResourceKind::Cpu,
                quantity: a("1000"),
                basis: Basis::Declared,
            });
            for (e, t, q) in [("house", "org_1", "600"), ("pg", "org_2", "200")] {
                usage.push(Usage {
                    day: d,
                    pool: pool.clone(),
                    kind: ResourceKind::Cpu,
                    consumer: key(e, t),
                    quantity: a(q),
                    basis: Basis::Measured,
                });
            }
            // 10 GB stored all day by house.
            usage.push(Usage {
                day: d,
                pool: bucket.clone(),
                kind: ResourceKind::Storage,
                consumer: key("house", "org_1"),
                quantity: a("864000000000000"),
                basis: Basis::Measured,
            });
        }
        allocate_all(&sources, &caps, &usage).expect("alloc").lines
    }

    fn ops(engine: &str, tenant: &str, n: u64) -> Vec<OpsCount> {
        [day(1), day(2)]
            .into_iter()
            .map(|d| OpsCount {
                day: d,
                consumer: key(engine, tenant),
                ops: n,
            })
            .collect()
    }

    fn request(group_by: Vec<Dimension>, granularity: Granularity) -> ReportRequest {
        ReportRequest {
            from: day(1),
            to: day(3),
            group_by,
            granularity,
        }
    }

    #[test]
    fn report_totals_reconcile_to_inputs() {
        let r = build(
            &request(vec![Dimension::Engine], Granularity::Total),
            &lines(),
            &[],
            &[],
        )
        .expect("report");
        assert_eq!(r.totals.input, a("262"));
        assert_eq!(
            r.totals.attributed + r.totals.idle + r.totals.unattributed + r.totals.shared,
            r.totals.input
        );
        assert_eq!(r.totals.idle, a("40"));
        let sum: Amount = r.rows.iter().map(|row| row.cost).sum();
        assert_eq!(sum, r.totals.input);
    }

    #[test]
    fn unit_cost_per_million_ops() {
        let mut o = ops("house", "org_1", 500_000);
        o.extend(ops("pg", "org_2", 10));
        let r = build(
            &request(vec![Dimension::Engine], Granularity::Total),
            &lines(),
            &o,
            &[],
        )
        .expect("report");
        let house = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "house")
            .expect("house");
        // CPU 60 per day × 2 = 120 over 1M ops; storage excluded.
        assert_eq!(house.cost_by_kind[&ResourceKind::Cpu], a("120"));
        assert_eq!(house.unit.per_million_ops, Some(a("120")));
        assert_eq!(house.ops, 1_000_000);
        // 120 for 1,200 core-seconds = 360 per core-hour.
        assert_eq!(house.unit.per_core_hour, Some(a("360")));
        let pg = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "pg")
            .expect("pg");
        assert_eq!(pg.unit.per_million_ops, None);
        assert!(pg.flags.contains(&Flag::InsufficientSample));
    }

    #[test]
    fn ops_unavailable_flagged() {
        let r = build(
            &request(vec![Dimension::Engine], Granularity::Total),
            &lines(),
            &[],
            &[],
        )
        .expect("report");
        let pg = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "pg")
            .expect("pg");
        assert!(pg.flags.contains(&Flag::OpsUnavailable));
        assert_eq!(pg.unit.per_million_ops, None);
    }

    #[test]
    fn gb_month_uses_mean_month() {
        let r = build(
            &request(vec![Dimension::Engine, Dimension::Kind], Granularity::Total),
            &lines(),
            &[],
            &[],
        )
        .expect("report");
        let storage = r
            .rows
            .iter()
            .find(|row| row.group[1].1 == "storage")
            .expect("storage");
        // 62 for 2 × 864e12 byte-seconds: 62 × 10⁹ × 2,629,746 / 1.728e15
        // = 94.3543125 per GB-month.
        let got = storage.unit.per_gb_month.expect("per gb-month");
        assert_eq!(got.to_string(), "94.3543125");
    }

    #[test]
    fn insufficient_sample_withheld() {
        // 200 a day each over two days: 800 ops in all.
        let mut o = ops("house", "org_1", 200);
        o.extend(ops("pg", "org_2", 200));
        let r = build(&request(vec![], Granularity::Total), &lines(), &o, &[]).expect("report");
        // One row of everything, 800 ops in total (< 1,000).
        let work = r
            .rows
            .iter()
            .find(|row| row.unit.per_core_hour.is_some())
            .expect("row");
        assert_eq!(work.unit.per_million_ops, None);
        assert!(work.flags.contains(&Flag::InsufficientSample));
    }

    #[test]
    fn fully_loaded_spreads_idle_labelled() {
        let o = ops("house", "org_1", 500_000);
        let r = build(
            &request(vec![Dimension::Engine], Granularity::Total),
            &lines(),
            &o,
            &[],
        )
        .expect("report");
        let loaded: Amount = r.rows.iter().filter_map(|row| row.fully_loaded_cost).sum();
        // Work gets everything, idle included.
        assert_eq!(loaded, r.totals.input);
        let house = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "house")
            .expect("house");
        assert!(house.flags.contains(&Flag::Spread));
        assert!(house.fully_loaded_cost.expect("loaded") > house.cost);
        assert!(house.unit_fully_loaded.per_million_ops > house.unit.per_million_ops);
        let idle = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "idle")
            .expect("idle");
        assert_eq!(idle.fully_loaded_cost, None);
    }

    #[test]
    fn granularity_buckets() {
        let r = build(&request(vec![], Granularity::Day), &lines(), &[], &[]).expect("report");
        let periods: BTreeSet<Day> = r.rows.iter().map(|row| row.period).collect();
        assert_eq!(periods.len(), 2);
        let w = Granularity::Week
            .bucket(Day::parse("2026-10-29").expect("d"), day(1))
            .expect("w");
        assert_eq!(w.to_string(), "2026-10-26");
        let m = Granularity::Month
            .bucket(Day::parse("2026-10-29").expect("d"), day(1))
            .expect("m");
        assert_eq!(m.to_string(), "2026-10-01");
    }

    #[test]
    fn trend_marks_input_changes() {
        let changes = vec![
            InputChange {
                day: day(2),
                what: "costs.yaml v8".into(),
            },
            InputChange {
                day: day(9),
                what: "outside".into(),
            },
        ];
        let r = build(&request(vec![], Granularity::Day), &lines(), &[], &changes).expect("report");
        assert_eq!(r.markers.len(), 1);
        assert_eq!(r.markers[0].what, "costs.yaml v8");
    }

    #[test]
    fn revenue_absent_not_zero() {
        let rev = vec![Revenue {
            tenant: "org_1".into(),
            from: day(1),
            to: day(3),
            amount: a("400"),
            basis: Basis::Declared,
        }];
        let e = economics(&lines(), &rev, day(1), day(3)).expect("economics");
        let org1 = e.iter().find(|x| x.tenant == "org_1").expect("org_1");
        // 120 CPU + 62 storage.
        assert_eq!(org1.cost, a("182"));
        assert_eq!(org1.margin, Some(a("218")));
        assert_eq!(org1.margin_ratio, Some(a("0.545")));
        let org2 = e.iter().find(|x| x.tenant == "org_2").expect("org_2");
        assert_eq!(org2.revenue, None);
        assert_eq!(org2.margin, None);
    }

    #[test]
    fn rows_carry_basis_and_traces() {
        let r = build(
            &request(vec![Dimension::Engine], Granularity::Total),
            &lines(),
            &[],
            &[],
        )
        .expect("report");
        for row in &r.rows {
            assert!(!row.traces.is_empty());
        }
        let house = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "house")
            .expect("house");
        // Billed amounts, measured usage over declared capacity → declared.
        assert_eq!(house.basis, Basis::Declared);
    }
}
