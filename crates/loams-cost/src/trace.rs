//! Traces and digests (design §84 §10.2, D2368): every line has a stable
//! trace id, every allocation run has a digest over its inputs, and
//! [`explain`] gives the formula, the sources and the usage behind a line.

use loams_meter_core::Amount;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::allocate::{ConsumerUsage, Line, PoolDay, SourceAmount};
use crate::{Basis, Capacity, Consumer, CostError, CostSource, Flag, Usage};

/// Bumped whenever the arithmetic or the rules change, so a digest says
/// which code produced a result.
pub const ALLOCATION_VERSION: &str = "loams-proportional-v1";

/// The stable id of a line: the same (day, pool, kind, consumer) has the
/// same id in every run, so a link survives a recompute.
pub fn trace_id(line: &Line) -> String {
    let mut h = Sha256::new();
    h.update(b"loams-cost/trace/v1\0");
    h.update(line.key.day.to_string().as_bytes());
    h.update(b"\0");
    h.update(line.key.pool.as_str().as_bytes());
    h.update(b"\0");
    h.update(line.key.kind.as_str().as_bytes());
    h.update(b"\0");
    h.update(line.consumer.to_string().as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..16])
}

/// The digest of one allocation's inputs: the canonical JSON of every
/// source, capacity and usage row, sorted, plus [`ALLOCATION_VERSION`] and
/// `extra` (the cost-input version's bytes and the import digests, from the
/// caller). Input order does not change it.
pub fn digest(
    sources: &[CostSource],
    capacities: &[Capacity],
    usage: &[Usage],
    extra: &[&[u8]],
) -> Result<String, CostError> {
    fn rows<T: Serialize>(tag: &str, items: &[T]) -> Result<Vec<String>, CostError> {
        let mut out = items
            .iter()
            .map(|i| {
                serde_json::to_string(i)
                    .map(|j| format!("{tag}{j}"))
                    .map_err(|e| CostError::invalid("digest", e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.sort();
        Ok(out)
    }
    let mut h = Sha256::new();
    h.update(ALLOCATION_VERSION.as_bytes());
    h.update(b"\n");
    for part in [
        rows("s", sources)?,
        rows("c", capacities)?,
        rows("u", usage)?,
    ] {
        for row in part {
            h.update(row.as_bytes());
            h.update(b"\n");
        }
    }
    for e in extra {
        h.update(Sha256::digest(e));
    }
    Ok(hex::encode(h.finalize()))
}

/// What `ExplainCost` returns for one line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub trace_id: String,
    pub line: Line,
    /// The formula with its numbers, for people.
    pub formula: String,
    pub pool_cost: Amount,
    pub sources: Vec<SourceAmount>,
    pub capacity: Option<(Amount, Basis)>,
    pub usage: Vec<ConsumerUsage>,
    pub allocation_version: String,
}

/// Explains `line`, which `allocate(pd)` produced.
pub fn explain(pd: &PoolDay, line: &Line) -> Explanation {
    let kind = pd.key.kind;
    let unit = kind.driver_unit();
    let formula = match &line.consumer {
        Consumer::Shared => format!(
            "shared: {} pool cost {} has no driver; shown, not spread",
            kind, pd.cost
        ),
        Consumer::Unattributed => format!(
            "unattributed: pool cost {} had no driver usage on {}",
            pd.cost, pd.key.day
        ),
        _ if line.denominator.is_zero() => {
            format!("no usage to share by: {} {unit}", line.quantity)
        }
        consumer => {
            let what = if line.flags.contains(&Flag::OverCapacity) {
                "Σ usage (usage exceeded declared capacity)"
            } else if pd.capacity.is_some() {
                "capacity"
            } else {
                "Σ usage (no declared capacity)"
            };
            let who = if matches!(consumer, Consumer::Idle) {
                "unused capacity"
            } else {
                "usage"
            };
            format!(
                "cost × {who} / {what} = {} × {} / {} {unit} ≈ {} (exact share, largest-remainder rounded to 10⁻⁹)",
                pd.cost, line.quantity, line.denominator, line.amount
            )
        }
    };
    Explanation {
        trace_id: trace_id(line),
        line: line.clone(),
        formula,
        pool_cost: pd.cost,
        sources: pd.sources.clone(),
        capacity: pd.capacity,
        usage: pd.usage.clone(),
        allocation_version: ALLOCATION_VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConsumerKey, Day, PoolId, ResourceKind, SourceRef, allocate_all};

    fn inputs() -> (Vec<CostSource>, Vec<Capacity>, Vec<Usage>) {
        let day = Day::parse("2026-10-28").expect("day");
        let pool = PoolId::new("p").expect("pool");
        let s = |n: &str, amt: &str| CostSource {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            amount: Amount::parse(amt).expect("a"),
            basis: Basis::Billed,
            source: SourceRef::BillLine {
                import: "i1".into(),
                line: n.parse().expect("n"),
            },
            flags: Default::default(),
        };
        let u = |e: &str, q: &str| Usage {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            consumer: ConsumerKey::new(e, "org_1", "_", "_").expect("k"),
            quantity: Amount::parse(q).expect("q"),
            basis: Basis::Measured,
        };
        (
            vec![s("1", "3"), s("2", "4")],
            vec![Capacity {
                day,
                pool: pool.clone(),
                kind: ResourceKind::Cpu,
                quantity: Amount::parse("10").expect("c"),
                basis: Basis::Declared,
            }],
            vec![u("house", "2"), u("pg", "5")],
        )
    }

    #[test]
    fn report_digest_stable_under_reordering() {
        let (mut s, c, mut u) = inputs();
        let d1 = digest(&s, &c, &u, &[b"v7"]).expect("digest");
        s.reverse();
        u.reverse();
        assert_eq!(digest(&s, &c, &u, &[b"v7"]).expect("digest"), d1);
        assert_ne!(digest(&s, &c, &u, &[b"v8"]).expect("digest"), d1);
    }

    #[test]
    fn trace_ids_are_stable_and_distinct() {
        let (s, c, u) = inputs();
        let a = allocate_all(&s, &c, &u).expect("alloc");
        let ids: Vec<String> = a.lines.iter().map(trace_id).collect();
        assert_eq!(ids.len(), 3);
        assert_ne!(ids[0], ids[1]);
        let b = allocate_all(&s, &c, &u).expect("alloc");
        assert_eq!(ids, b.lines.iter().map(trace_id).collect::<Vec<_>>());
    }

    #[test]
    fn explain_names_sources_and_formula() {
        let (s, c, u) = inputs();
        let a = allocate_all(&s, &c, &u).expect("alloc");
        let e = explain(&a.pool_days[0], &a.lines[0]);
        assert_eq!(e.sources.len(), 2);
        assert_eq!(e.pool_cost, Amount::parse("7").expect("a"));
        assert!(e.formula.contains("7 × 2 / 10"), "{}", e.formula);
        assert_eq!(e.line.amount, Amount::parse("1.4").expect("a"));
        let idle = explain(&a.pool_days[0], &a.lines[2]);
        assert!(idle.formula.contains("unused capacity"), "{}", idle.formula);
    }
}
