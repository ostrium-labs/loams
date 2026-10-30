//! Drivers: usage from Loams Meter's events (design §84 §7, D2359–D2362).
//!
//! The [`CATALOGUE`] says, per event type, which fields measure which
//! resource kind, which engine the event belongs to, and how it counts
//! ops. [`drivers`] turns the House's per-day rows (built by
//! `loams_meter_house::cost_sql`) into:
//!
//! - **level-one usage** per (day, pool, kind, engine), from **one source**:
//!   the node agent when it reported that engine and kind, else the
//!   product's own events (D2359: no usage counts twice);
//! - **level-two splits** of each engine's line among (tenant, project,
//!   query class), from the product events when there are any, else from
//!   the node agent's tenants (§84 §7.1);
//! - **ops counts** per fine consumer, for $ per 1M ops and $ per job.
//!
//! Usage that no pool claims is kept aside as `unpooled` (shown in meter
//! health, never allocated).

use std::collections::{BTreeMap, BTreeSet};

use loams_meter_core::Amount;
use serde::{Deserialize, Serialize};

use crate::allocate::{ConsumerUsage, Line, split_line};
use crate::import::mapping::glob;
use crate::inputs::CostInputs;
use crate::report::OpsCount;
use crate::{Basis, Consumer, ConsumerKey, CostError, Day, NONE, PoolId, ResourceKind, Usage};

/// How a field converts into the driver unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    One,
    /// Divide by 10ⁿ (µs → s is 6, ms → s is 3).
    Div10(u32),
    /// MB·ms → byte-seconds (× 2²⁰ / 10³).
    MbMsToByteSeconds,
}

/// One kind an event measures: the sum of `fields`, scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measure {
    pub kind: ResourceKind,
    pub fields: &'static [&'static str],
    pub scale: Scale,
}

/// How an event type counts ops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ops {
    None,
    /// One op per event (a connection, a job, an invocation).
    PerEvent,
    /// The sum of a field (`statements`, `records`).
    Field(&'static str),
}

/// A catalogue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub event_type: &'static str,
    /// The engine, or `None` for the node agent's events (the engine label,
    /// then the `engines` rules).
    pub engine: Option<&'static str>,
    pub measures: &'static [Measure],
    pub ops: Ops,
    /// A gauge event: its value field, read as byte-seconds by the gauge
    /// query.
    pub gauge: Option<&'static str>,
}

impl Entry {
    pub fn is_node_agent(&self) -> bool {
        self.engine.is_none()
    }
}

const fn m(kind: ResourceKind, fields: &'static [&'static str], scale: Scale) -> Measure {
    Measure {
        kind,
        fields,
        scale,
    }
}

const P: &str = "io.loams.dev.usage.";

macro_rules! ty {
    ($s:literal) => {
        concat!("io.loams.dev.usage.", $s)
    };
}

/// The driver catalogue (§84 §7.1).
pub const CATALOGUE: &[Entry] = &[
    Entry {
        event_type: ty!("node.cpu.v1"),
        engine: None,
        measures: &[m(ResourceKind::Cpu, &["cpu_usec"], Scale::Div10(6))],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("node.memory.v1"),
        engine: None,
        measures: &[m(ResourceKind::Memory, &["byte_seconds"], Scale::One)],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("node.network.v1"),
        engine: None,
        measures: &[
            m(ResourceKind::NetworkEgress, &["egress_bytes"], Scale::One),
            m(ResourceKind::NetworkIngress, &["ingress_bytes"], Scale::One),
        ],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("node.io.v1"),
        engine: None,
        measures: &[m(ResourceKind::Io, &["read_ops", "write_ops"], Scale::One)],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("house.query.v1"),
        engine: Some("house"),
        measures: &[m(ResourceKind::Cpu, &["cpu_ms"], Scale::Div10(3))],
        ops: Ops::Field("statements"),
        gauge: None,
    },
    Entry {
        event_type: ty!("house.insert.v1"),
        engine: Some("house"),
        measures: &[],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("house.compaction.v1"),
        engine: Some("house"),
        measures: &[m(ResourceKind::Cpu, &["cpu_ms"], Scale::Div10(3))],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("house.storage.v1"),
        engine: Some("house"),
        measures: &[],
        ops: Ops::None,
        gauge: Some("bytes"),
    },
    Entry {
        event_type: ty!("postgres.compute.v1"),
        engine: Some("postgres"),
        measures: &[m(ResourceKind::Cpu, &["cu_ms"], Scale::Div10(3))],
        ops: Ops::None,
        gauge: None,
    },
    Entry {
        event_type: ty!("postgres.storage.v1"),
        engine: Some("postgres"),
        measures: &[],
        ops: Ops::None,
        gauge: Some("logical_bytes"),
    },
    Entry {
        event_type: ty!("gateway.connection.v1"),
        engine: Some("gateway"),
        measures: &[
            m(ResourceKind::NetworkEgress, &["bytes_out"], Scale::One),
            m(ResourceKind::NetworkIngress, &["bytes_in"], Scale::One),
        ],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("gateway.request.v1"),
        engine: Some("gateway"),
        measures: &[
            m(ResourceKind::NetworkEgress, &["bytes_out"], Scale::One),
            m(ResourceKind::NetworkIngress, &["bytes_in"], Scale::One),
        ],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("jobs.run.v1"),
        engine: Some("jobs"),
        measures: &[m(ResourceKind::Cpu, &["cpu_ms"], Scale::Div10(3))],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("function.invocation.v1"),
        engine: Some("functions"),
        measures: &[m(ResourceKind::Cpu, &["cpu_ms"], Scale::Div10(3))],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("stream.ingest.v1"),
        engine: Some("stream"),
        measures: &[],
        ops: Ops::Field("records"),
        gauge: None,
    },
    Entry {
        event_type: ty!("storage.logical.v1"),
        engine: Some("stream"),
        measures: &[],
        ops: Ops::None,
        gauge: Some("logical_bytes"),
    },
    Entry {
        event_type: ty!("git.storage.v1"),
        engine: Some("git"),
        measures: &[],
        ops: Ops::None,
        gauge: Some("bytes"),
    },
    Entry {
        event_type: ty!("git.transfer.v1"),
        engine: Some("git"),
        measures: &[m(ResourceKind::NetworkEgress, &["bytes"], Scale::One)],
        ops: Ops::PerEvent,
        gauge: None,
    },
    Entry {
        event_type: ty!("sandbox.session.v1"),
        engine: Some("sandbox"),
        measures: &[
            m(ResourceKind::Cpu, &["cpu_ms"], Scale::Div10(3)),
            m(
                ResourceKind::Memory,
                &["memory_mb_ms"],
                Scale::MbMsToByteSeconds,
            ),
        ],
        ops: Ops::PerEvent,
        gauge: None,
    },
];

/// The catalogue entry of `event_type`.
pub fn entry(event_type: &str) -> Option<&'static Entry> {
    CATALOGUE.iter().find(|e| e.event_type == event_type)
}

/// The counter event types (for `counter_query`).
pub fn counter_types() -> Vec<String> {
    CATALOGUE
        .iter()
        .filter(|e| e.gauge.is_none())
        .map(|e| e.event_type.to_string())
        .collect()
}

/// Every field the counter query sums (measures and ops fields).
pub fn counter_fields() -> Vec<String> {
    let mut out = BTreeSet::new();
    for e in CATALOGUE.iter().filter(|e| e.gauge.is_none()) {
        for m in e.measures {
            out.extend(m.fields.iter().map(|f| f.to_string()));
        }
        if let Ops::Field(f) = e.ops {
            out.insert(f.to_string());
        }
    }
    out.into_iter().collect()
}

/// (type, value field) of every gauge (for `gauge_query`).
pub fn gauges() -> Vec<(String, String)> {
    CATALOGUE
        .iter()
        .filter_map(|e| e.gauge.map(|g| (e.event_type.to_string(), g.to_string())))
        .collect()
}

/// The event types of `engine` (for query-class rules).
pub fn engine_types(engine: &str) -> Vec<String> {
    CATALOGUE
        .iter()
        .filter(|e| e.engine == Some(engine))
        .map(|e| e.event_type.to_string())
        .collect()
}

/// Built-in query classes (§84 §7.4): (engine, class, condition), tried
/// after the operator's rules.
pub const DEFAULT_CLASSES: &[(&str, &str, &str)] = &[
    ("house", "scan", "bytes_scanned >= 1073741824"),
    ("house", "point", "rows_read <= 1000"),
];

/// One row of a driver query, parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverRow {
    pub event_type: String,
    pub subject: String,
    pub tenantid: String,
    pub node: String,
    pub engine_label: String,
    pub resource: String,
    pub class: String,
    pub events: u64,
    /// Field sums (counter rows) or `byte_seconds` (gauge rows).
    pub sums: BTreeMap<String, Amount>,
}

impl DriverRow {
    /// A row as `cost_sql::parse_rows` gives it: the key columns, `events`,
    /// and `f_<field>` sums (or `byte_seconds`).
    pub fn from_columns(cols: &BTreeMap<String, String>) -> Result<DriverRow, CostError> {
        let get = |k: &str| cols.get(k).cloned().unwrap_or_default();
        let mut sums = BTreeMap::new();
        for (k, v) in cols {
            let name = if let Some(f) = k.strip_prefix("f_") {
                f
            } else if k == "byte_seconds" {
                k.as_str()
            } else {
                continue;
            };
            let a = Amount::parse(v.trim())
                .map_err(|e| CostError::invalid("drivers", format!("{k} = {v:?}: {e}")))?;
            sums.insert(name.to_string(), a);
        }
        Ok(DriverRow {
            event_type: get("type"),
            subject: get("subject"),
            tenantid: get("tenantid"),
            node: get("node"),
            engine_label: get("engine"),
            resource: get("resource"),
            class: get("class"),
            events: get("events").trim().parse().unwrap_or(0),
            sums,
        })
    }
}

/// A name made safe for a consumer key part (`_` when empty).
pub fn sanitize(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._~/:@-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(128)
        .collect();
    if t.is_empty() { NONE.to_string() } else { t }
}

fn scaled(sum: Amount, scale: Scale) -> Amount {
    match scale {
        Scale::One => sum,
        Scale::Div10(n) => Amount::from_raw(sum.raw() / 10i128.pow(n)),
        Scale::MbMsToByteSeconds => Amount::from_raw(sum.raw().saturating_mul(1 << 20) / 1_000),
    }
}

/// Usage no pool claims (never allocated; shown in meter health).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unpooled {
    pub day: Day,
    pub node: String,
    pub event_type: String,
    pub kind: ResourceKind,
    pub quantity: Amount,
}

/// What the drivers give allocation and reports.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drivers {
    /// Level one: per (day, pool, kind, engine), engine-level consumers.
    pub usage: Vec<Usage>,
    /// Level two: how each engine's line divides, keyed by
    /// (day, pool, kind, engine).
    pub splits: BTreeMap<(Day, PoolId, ResourceKind, String), Vec<ConsumerUsage>>,
    pub ops: Vec<OpsCount>,
    pub unpooled: Vec<Unpooled>,
    /// Rows of event types the catalogue does not know.
    pub unknown_types: BTreeSet<String>,
}

/// The pool of usage on `node` from `event_type` of `kind`: an asset that
/// lists the node, then the `usage_pools` rules.
pub fn pool_of(
    inputs: &CostInputs,
    node: &str,
    event_type: &str,
    kind: ResourceKind,
) -> Option<PoolId> {
    let has_kind = |a: &crate::inputs::Asset| match &a.split {
        Some(s) => s.contains_key(&kind),
        None => kind == ResourceKind::Cpu,
    };
    if let Some(a) = inputs
        .assets
        .iter()
        .find(|a| a.nodes.iter().any(|n| n == node) && has_kind(a))
    {
        return Some(a.pool.clone());
    }
    inputs
        .usage_pools
        .iter()
        .find(|r| {
            let m = &r.matcher;
            m.node.as_deref().is_none_or(|g| glob(g, node))
                && m.event_type.as_deref().is_none_or(|g| glob(g, event_type))
                && m.kind.is_none_or(|k| k == kind)
        })
        .map(|r| r.pool.clone())
}

/// The engine of a node-agent row: its label, then the `engines` rules.
fn node_engine(inputs: &CostInputs, row: &DriverRow) -> String {
    if !row.engine_label.is_empty() {
        return sanitize(&row.engine_label);
    }
    inputs
        .engines
        .iter()
        .find(|r| glob(&r.matcher.resource, &row.resource))
        .map_or_else(|| "_unknown".to_string(), |r| r.engine.clone())
}

#[derive(Default)]
struct Side {
    node: BTreeMap<ConsumerKey, Amount>,
    product: BTreeMap<ConsumerKey, Amount>,
}

/// Turns one day's driver rows into usage, splits and ops (see the module
/// documentation).
pub fn drivers(inputs: &CostInputs, day: Day, rows: &[DriverRow]) -> Result<Drivers, CostError> {
    let mut out = Drivers::default();
    let mut sides: BTreeMap<(PoolId, ResourceKind, String), Side> = BTreeMap::new();
    let mut ops: BTreeMap<ConsumerKey, u64> = BTreeMap::new();
    let add = |m: &mut BTreeMap<ConsumerKey, Amount>, k: ConsumerKey, q: Amount| {
        let slot = m.entry(k).or_insert(Amount::ZERO);
        *slot = slot.checked_add(q).ok_or(CostError::Overflow)?;
        Ok::<(), CostError>(())
    };
    for row in rows {
        let Some(entry) = entry(&row.event_type) else {
            out.unknown_types.insert(row.event_type.clone());
            continue;
        };
        let engine = match entry.engine {
            Some(e) => e.to_string(),
            None => node_engine(inputs, row),
        };
        let tenant = sanitize(&row.subject);
        let project = sanitize(row.tenantid.split_once('/').map_or("", |(_, ns)| ns));
        let class = if entry.is_node_agent() || entry.gauge.is_some() {
            NONE.to_string()
        } else {
            sanitize(&row.class)
        };
        let fine = ConsumerKey::new(engine.clone(), tenant.clone(), project.clone(), class)?;
        let mut quantities: Vec<(ResourceKind, Amount)> = Vec::new();
        if entry.gauge.is_some()
            && let Some(bs) = row.sums.get("byte_seconds")
        {
            quantities.push((ResourceKind::Storage, *bs));
        }
        for m in entry.measures {
            let mut sum = Amount::ZERO;
            for f in m.fields {
                if let Some(v) = row.sums.get(*f) {
                    sum = sum.checked_add(*v).ok_or(CostError::Overflow)?;
                }
            }
            quantities.push((m.kind, scaled(sum, m.scale)));
        }
        for (kind, q) in quantities {
            if q.is_negative() {
                return Err(CostError::NegativeQuantity);
            }
            let Some(pool) = pool_of(inputs, &row.node, &row.event_type, kind) else {
                out.unpooled.push(Unpooled {
                    day,
                    node: row.node.clone(),
                    event_type: row.event_type.clone(),
                    kind,
                    quantity: q,
                });
                continue;
            };
            let side = sides.entry((pool, kind, engine.clone())).or_default();
            if entry.is_node_agent() {
                add(&mut side.node, fine.clone(), q)?;
            } else {
                add(&mut side.product, fine.clone(), q)?;
            }
        }
        let n = match entry.ops {
            Ops::None => None,
            Ops::PerEvent => Some(row.events),
            Ops::Field(f) => Some(
                row.sums
                    .get(f)
                    .map_or(0, |a| u64::try_from(a.trunc_units()).unwrap_or(0)),
            ),
        };
        if let Some(n) = n {
            let slot = ops.entry(fine).or_default();
            *slot = slot.saturating_add(n);
        }
    }
    for ((pool, kind, engine), side) in sides {
        // One source per (engine, kind): the node agent first (D2359).
        let level_one = if side.node.is_empty() {
            &side.product
        } else {
            &side.node
        };
        let total = level_one.values().copied().sum::<Amount>();
        out.usage.push(Usage {
            day,
            pool: pool.clone(),
            kind,
            consumer: ConsumerKey::new(engine.clone(), NONE, NONE, NONE)?,
            quantity: total,
            basis: Basis::Measured,
        });
        // The split inside the engine: product detail when present.
        let detail = if side.product.is_empty() {
            &side.node
        } else {
            &side.product
        };
        out.splits.insert(
            (day, pool, kind, engine),
            detail
                .iter()
                .map(|(k, q)| ConsumerUsage {
                    consumer: k.clone(),
                    quantity: *q,
                    basis: Basis::Measured,
                })
                .collect(),
        );
    }
    out.ops = ops
        .into_iter()
        .map(|(consumer, ops)| OpsCount { day, consumer, ops })
        .collect();
    Ok(out)
}

/// Applies the level-two splits to allocation lines: each engine-level
/// work line becomes its (tenant, project, class) lines; other lines pass
/// through.
pub fn apply_splits(
    lines: &[Line],
    splits: &BTreeMap<(Day, PoolId, ResourceKind, String), Vec<ConsumerUsage>>,
) -> Result<Vec<Line>, CostError> {
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let key = match &l.consumer {
            Consumer::Work(k) if k.tenant == NONE && k.project == NONE && k.query_class == NONE => {
                (l.key.day, l.key.pool.clone(), l.key.kind, k.engine.clone())
            }
            _ => {
                out.push(l.clone());
                continue;
            }
        };
        match splits.get(&key) {
            Some(sub) => out.extend(split_line(l, sub)?),
            None => out.push(l.clone()),
        }
    }
    Ok(out)
}

/// The prefix every Loams usage type shares.
pub const USAGE_PREFIX: &str = P;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capacity, CostSource, SourceRef, allocate_all};

    const YAML: &str = r#"
version: 1
currency: USD
assets:
  - id: n1
    pool: fra/compute
    cost: "1000"
    in_service: 2026-01-01
    nodes: [node-1]
    split: { cpu: "0.8", io: "0.2" }
engines:
  - match: { resource: "*/loams-kv*" }
    engine: kv
usage_pools:
  - match: { type: "io.loams.dev.usage.house.storage.v1" }
    pool: fra/bucket
"#;

    #[allow(clippy::too_many_arguments)]
    fn row(
        t: &str,
        subject: &str,
        tenantid: &str,
        node: &str,
        label: &str,
        resource: &str,
        class: &str,
        events: u64,
        sums: &[(&str, &str)],
    ) -> DriverRow {
        DriverRow {
            event_type: format!("{P}{t}"),
            subject: subject.into(),
            tenantid: tenantid.into(),
            node: node.into(),
            engine_label: label.into(),
            resource: resource.into(),
            class: class.into(),
            events,
            sums: sums
                .iter()
                .map(|(k, v)| (k.to_string(), Amount::parse(v).expect("amount")))
                .collect(),
        }
    }

    fn day() -> Day {
        Day::parse("2026-10-28").expect("day")
    }

    fn sample() -> Vec<DriverRow> {
        vec![
            // The node agent saw house use 100 core-s and kv 50 core-s.
            row(
                "node.cpu.v1",
                "org_1",
                "1/7",
                "node-1",
                "house",
                "/x/house",
                "",
                10,
                &[("cpu_usec", "60000000")],
            ),
            row(
                "node.cpu.v1",
                "org_2",
                "2/9",
                "node-1",
                "house",
                "/x/house",
                "",
                10,
                &[("cpu_usec", "40000000")],
            ),
            row(
                "node.cpu.v1",
                "org_2",
                "2/9",
                "node-1",
                "",
                "/y/loams-kv-3",
                "",
                10,
                &[("cpu_usec", "50000000")],
            ),
            row(
                "node.io.v1",
                "org_1",
                "1/7",
                "node-1",
                "house",
                "/x/house",
                "",
                3,
                &[("read_ops", "700"), ("write_ops", "300")],
            ),
            // House's own events split its line: 30 core-s scan for org_1,
            // 10 core-s point for org_2; 5,000 statements.
            row(
                "house.query.v1",
                "org_1",
                "1/7",
                "node-1",
                "",
                "",
                "scan",
                4,
                &[("cpu_ms", "30000"), ("statements", "3000")],
            ),
            row(
                "house.query.v1",
                "org_2",
                "2/9",
                "node-1",
                "",
                "",
                "point",
                9,
                &[("cpu_ms", "10000"), ("statements", "2000")],
            ),
            // A node no asset or rule claims.
            row(
                "node.cpu.v1",
                "org_3",
                "3/1",
                "node-9",
                "pg",
                "/z",
                "",
                1,
                &[("cpu_usec", "1000000")],
            ),
            // Storage, as byte-seconds from the gauge query.
            row(
                "house.storage.v1",
                "org_1",
                "1/7",
                "node-1",
                "",
                "",
                "_",
                1,
                &[("byte_seconds", "864000000000000")],
            ),
            // A type the catalogue does not know.
            row("llm.tokens.v1", "org_1", "1/7", "gw", "", "", "", 1, &[]),
        ]
    }

    #[test]
    fn node_agent_preferred_for_cpu() {
        let inputs = CostInputs::parse(YAML).expect("inputs");
        let d = drivers(&inputs, day(), &sample()).expect("drivers");
        let cpu = |engine: &str| {
            d.usage
                .iter()
                .find(|u| u.kind == ResourceKind::Cpu && u.consumer.engine == engine)
                .map(|u| u.quantity)
        };
        // Level one is the node agent's 100 core-s, not House's 40.
        assert_eq!(cpu("house"), Some(Amount::from_units(100)));
        assert_eq!(cpu("kv"), Some(Amount::from_units(50)));
        // Level two for house comes from House's events.
        let split = &d.splits[&(
            day(),
            PoolId::new("fra/compute").expect("p"),
            ResourceKind::Cpu,
            "house".to_string(),
        )];
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].consumer.query_class, "scan");
        assert_eq!(split[0].quantity, Amount::from_units(30));
        // kv has no product events: its split is the node agent's tenants.
        let kv = &d.splits[&(
            day(),
            PoolId::new("fra/compute").expect("p"),
            ResourceKind::Cpu,
            "kv".to_string(),
        )];
        assert_eq!(kv[0].consumer.tenant, "org_2");
        assert_eq!(kv[0].consumer.query_class, "_");
        // Ops from statements, per fine consumer.
        let total: u64 = d.ops.iter().map(|o| o.ops).sum();
        assert_eq!(total, 5_000);
        // Unpooled and unknown are kept aside.
        assert_eq!(d.unpooled.len(), 1);
        assert_eq!(d.unpooled[0].node, "node-9");
        assert!(d.unknown_types.contains(&format!("{P}llm.tokens.v1")));
        // Storage went to the bucket by the usage_pools rule.
        assert!(
            d.usage
                .iter()
                .any(|u| u.kind == ResourceKind::Storage && u.pool.as_str() == "fra/bucket")
        );
        // I/O from read + write ops.
        assert!(
            d.usage
                .iter()
                .any(|u| u.kind == ResourceKind::Io && u.quantity == Amount::from_units(1000))
        );
    }

    #[test]
    fn splits_apply_and_conserve() {
        let inputs = CostInputs::parse(YAML).expect("inputs");
        let d = drivers(&inputs, day(), &sample()).expect("drivers");
        let src = CostSource {
            day: day(),
            pool: PoolId::new("fra/compute").expect("p"),
            kind: ResourceKind::Cpu,
            amount: Amount::from_units(30),
            basis: Basis::Billed,
            source: SourceRef::Other { note: "x".into() },
            flags: BTreeSet::new(),
        };
        let cap = Capacity {
            day: day(),
            pool: src.pool.clone(),
            kind: ResourceKind::Cpu,
            quantity: Amount::from_units(300),
            basis: Basis::Declared,
        };
        let a = allocate_all(&[src], &[cap], &d.usage).expect("alloc");
        let lines = apply_splits(&a.lines, &d.splits).expect("split");
        let cpu: Vec<&Line> = lines
            .iter()
            .filter(|l| l.key.kind == ResourceKind::Cpu && l.key.pool.as_str() == "fra/compute")
            .collect();
        assert_eq!(
            cpu.iter().map(|l| l.amount).sum::<Amount>(),
            Amount::from_units(30)
        );
        // house: 100 of 300 → 10, split 30:10 → 7.5 and 2.5.
        let scan = cpu
            .iter()
            .find(|l| l.consumer.work().is_some_and(|k| k.query_class == "scan"))
            .expect("scan");
        assert_eq!(scan.amount, Amount::parse("7.5").expect("a"));
        assert!(lines.iter().all(|l| {
            l.consumer
                .work()
                .is_none_or(|k| k.tenant != NONE || k.engine == "_unknown")
        }));
    }

    #[test]
    fn rows_parse_from_columns() {
        let cols: BTreeMap<String, String> = [
            ("type", "io.loams.dev.usage.house.query.v1"),
            ("subject", "org_1"),
            ("tenantid", "1/7"),
            ("node", "n"),
            ("engine", ""),
            ("resource", ""),
            ("class", "scan"),
            ("events", "4"),
            ("f_cpu_ms", "1840"),
            ("f_statements", "1.000000000"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let r = DriverRow::from_columns(&cols).expect("row");
        assert_eq!(r.events, 4);
        assert_eq!(r.sums["cpu_ms"], Amount::from_units(1840));
        assert_eq!(r.sums["statements"], Amount::from_units(1));
    }

    #[test]
    fn catalogue_lists() {
        assert!(counter_fields().contains(&"cpu_usec".to_string()));
        assert!(counter_fields().contains(&"statements".to_string()));
        assert!(!counter_types().iter().any(|t| t.ends_with("storage.v1")));
        assert_eq!(gauges().len(), 4);
        assert!(engine_types("house").len() >= 3);
        // Every catalogue type is a Loams usage type.
        assert!(
            CATALOGUE
                .iter()
                .all(|e| e.event_type.starts_with(USAGE_PREFIX))
        );
    }
}
