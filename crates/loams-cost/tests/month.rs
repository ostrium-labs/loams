//! A synthetic period end to end (CA1 Task 4 gate, design §84 §8.2,
//! §9.2): inputs + a FOCUS bill + usage → allocation → report, and the
//! report's input total equals the sources' sum exactly.

use loams_cost::import::import_bill;
use loams_cost::pipeline::compute;
use loams_cost::report::{Dimension, Granularity, OpsCount, ReportRequest, build};
use loams_cost::{Amount, Basis, ConsumerKey, CostInputs, Day, PoolId, ResourceKind, Usage};

const YAML: &str = r#"
version: 3
currency: USD
assets:
  - id: rack-node1
    pool: fra/compute
    cost: "3650"
    in_service: 2026-01-01
    life_months: 36
    capacity: { cores: 8 }
mappings:
  - match: { service: "Amazon Simple Storage Service" }
    pool: eu/bucket
    kind: storage
prices:
  - { id: laptop, kind: cpu, scope: laptop, rate: "0.05", unit: core_hour, source: estimate, effective: 2026-01-01, note: "example input, not a quote" }
"#;

fn usage(day: Day, pool: &str, kind: ResourceKind, engine: &str, q: &str) -> Usage {
    Usage {
        day,
        pool: PoolId::new(pool).expect("pool"),
        kind,
        consumer: ConsumerKey::new(engine, "org_1", "7", "_").expect("key"),
        quantity: Amount::parse(q).expect("q"),
        basis: Basis::Measured,
    }
}

#[test]
fn month_reconciles_and_is_stable() {
    let inputs = CostInputs::parse(YAML).expect("inputs");
    let bill = std::fs::read(format!(
        "{}/tests/data/focus.csv",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("bill");
    let mut bill_sources = Vec::new();
    let summary = import_bill(&inputs, "imp1", &bill[..], None, |l| {
        bill_sources.extend(l.sources);
        Ok(())
    })
    .expect("import");
    let from = Day::parse("2026-10-01").expect("d");
    let to = Day::parse("2026-11-01").expect("d");
    let mut u = Vec::new();
    let mut day = from;
    while day < to {
        u.push(usage(
            day,
            "fra/compute",
            ResourceKind::Cpu,
            "house",
            "345600",
        ));
        u.push(usage(day, "fra/compute", ResourceKind::Cpu, "pg", "100000"));
        u.push(usage(day, "eu/compute", ResourceKind::Cpu, "kv", "5000"));
        u.push(usage(
            day,
            "eu/bucket",
            ResourceKind::Storage,
            "house",
            "86400000000000",
        ));
        u.push(usage(day, "laptop", ResourceKind::Cpu, "graph", "3600"));
        day = day.next().expect("next");
    }
    let c = compute(
        &inputs,
        YAML,
        &bill_sources,
        std::slice::from_ref(&summary.digest),
        &u,
        from,
        to,
    )
    .expect("compute");
    let sources: Amount = c.sources.iter().map(|s| s.amount).sum();
    let ops: Vec<OpsCount> = Vec::new();
    let r = build(
        &ReportRequest {
            from,
            to,
            group_by: vec![Dimension::Engine],
            granularity: Granularity::Total,
        },
        &c.allocation.lines,
        &ops,
        &[],
    )
    .expect("report");
    assert_eq!(r.totals.input, sources);
    // The asset: 3650 over 36 months from 2026-01-01 (1,096 days, 2028 is
    // a leap year) is 3.330… a day; 31 days ≈ 103.239. The mapped and shared bill (117.5)
    // and the laptop price (31 × 0.05) make up the rest.
    let bill_in: Amount = bill_sources.iter().map(|s| s.amount).sum();
    assert_eq!(bill_in, summary.mapped + summary.shared);
    let laptop = Amount::parse("1.55").expect("a");
    let asset = sources - bill_in - laptop;
    assert!(
        asset > Amount::parse("103.23").expect("a") && asset < Amount::parse("103.24").expect("a"),
        "{asset}"
    );
    // Idle on the asset: 8 cores × 86,400 s = 691,200 core-s, 445,600 used.
    assert!(r.totals.idle.is_positive());
    // The same inputs reversed give the same digest and lines.
    let mut bs = bill_sources.clone();
    bs.reverse();
    let mut ur = u.clone();
    ur.reverse();
    let c2 = compute(
        &inputs,
        YAML,
        &bs,
        std::slice::from_ref(&summary.digest),
        &ur,
        from,
        to,
    )
    .expect("compute");
    assert_eq!(c.digest, c2.digest);
    assert_eq!(c.allocation.lines, c2.allocation.lines);
}
