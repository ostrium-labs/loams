//! Allocation properties (design §84 §8.2), checked over random inputs.

use std::collections::BTreeMap;

use loams_cost::{
    Amount, Basis, Capacity, Consumer, ConsumerKey, CostSource, Day, PoolDayKey, PoolId,
    ResourceKind, SourceRef, Usage, allocate_all, digest,
};
use proptest::prelude::*;

const ENGINES: [&str; 4] = ["house", "pg", "kv", "gateway"];
const POOLS: [&str; 3] = ["eu/compute", "eu/bucket", "us/compute"];
const KINDS: [ResourceKind; 3] = [ResourceKind::Cpu, ResourceKind::Storage, ResourceKind::Io];

fn day(i: u8) -> Day {
    Day::new(2026, 10, 1 + i).expect("day")
}

#[derive(Clone, Debug)]
struct Inputs {
    sources: Vec<CostSource>,
    capacities: Vec<Capacity>,
    usage: Vec<Usage>,
}

fn arb_inputs() -> impl Strategy<Value = Inputs> {
    let source = (
        0u8..3,
        0usize..3,
        0usize..3,
        // Up to 10^12 currency units in nano-units, with credits.
        -1_000_000_000_000_000i128..1_000_000_000_000_000_000_000i128,
        0u64..1000,
    )
        .prop_map(|(d, p, k, raw, line)| CostSource {
            day: day(d),
            pool: PoolId::new(POOLS[p]).expect("pool"),
            kind: KINDS[k],
            amount: Amount::from_raw(raw),
            basis: Basis::Billed,
            source: SourceRef::BillLine {
                import: "imp".into(),
                line,
            },
            flags: Default::default(),
        });
    let capacity = (
        0u8..3,
        0usize..3,
        0usize..3,
        // Up to 10^21 driver units (petabyte-months of byte-seconds).
        0i128..1_000_000_000_000_000_000_000_000_000_000i128,
    )
        .prop_map(|(d, p, k, raw)| Capacity {
            day: day(d),
            pool: PoolId::new(POOLS[p]).expect("pool"),
            kind: KINDS[k],
            quantity: Amount::from_raw(raw),
            basis: Basis::Declared,
        });
    let usage = (
        0u8..3,
        0usize..3,
        0usize..3,
        0usize..4,
        0u8..3,
        0i128..1_000_000_000_000_000_000_000_000_000i128,
    )
        .prop_map(|(d, p, k, e, t, raw)| Usage {
            day: day(d),
            pool: PoolId::new(POOLS[p]).expect("pool"),
            kind: KINDS[k],
            consumer: ConsumerKey::new(ENGINES[e], format!("org_{t}"), "_", "_").expect("key"),
            quantity: Amount::from_raw(raw),
            basis: Basis::Measured,
        });
    (
        prop::collection::vec(source, 0..12),
        prop::collection::vec(capacity, 0..4),
        prop::collection::vec(usage, 0..24),
    )
        .prop_map(|(sources, capacities, usage)| Inputs {
            sources,
            capacities,
            usage,
        })
}

fn sum_by_pool_day<T>(
    items: &[T],
    key: impl Fn(&T) -> PoolDayKey,
    amount: impl Fn(&T) -> Amount,
) -> BTreeMap<PoolDayKey, Amount> {
    let mut out = BTreeMap::new();
    for i in items {
        *out.entry(key(i)).or_insert(Amount::ZERO) += amount(i);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn allocation_conserves_every_pool(inp in arb_inputs()) {
        let a = allocate_all(&inp.sources, &inp.capacities, &inp.usage).expect("alloc");
        let want = sum_by_pool_day(
            &inp.sources,
            |s| PoolDayKey { day: s.day, pool: s.pool.clone(), kind: s.kind },
            |s| s.amount,
        );
        let got = sum_by_pool_day(&a.lines, |l| l.key.clone(), |l| l.amount);
        for (k, v) in &want {
            prop_assert_eq!(got.get(k).copied().unwrap_or(Amount::ZERO), *v, "pool-day {:?}", k);
        }
        for (k, v) in &got {
            prop_assert_eq!(want.get(k).copied().unwrap_or(Amount::ZERO), *v, "pool-day {:?}", k);
        }
    }

    #[test]
    fn allocation_order_independent(inp in arb_inputs()) {
        let a = allocate_all(&inp.sources, &inp.capacities, &inp.usage).expect("alloc");
        let mut s = inp.sources.clone();
        let mut c = inp.capacities.clone();
        let mut u = inp.usage.clone();
        s.reverse();
        c.reverse();
        let mid = u.len() / 2;
        u.rotate_left(mid);
        let b = allocate_all(&s, &c, &u).expect("alloc");
        prop_assert_eq!(&a.lines, &b.lines);
        prop_assert_eq!(
            digest(&inp.sources, &inp.capacities, &inp.usage, &[]).expect("d"),
            digest(&s, &c, &u, &[]).expect("d")
        );
    }

    #[test]
    fn no_negative_line_from_positive_pool(inp in arb_inputs()) {
        let a = allocate_all(&inp.sources, &inp.capacities, &inp.usage).expect("alloc");
        for pd in &a.pool_days {
            if pd.cost.is_negative() {
                continue;
            }
            for l in a.lines.iter().filter(|l| l.key == pd.key) {
                prop_assert!(!l.amount.is_negative(), "{:?}", l);
                if matches!(l.consumer, Consumer::Idle) {
                    prop_assert!(!l.quantity.is_negative());
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Doubling one consumer's usage, with capacity fixed and room to spare,
    /// doubles its line to within one nano-unit per rounding (two roundings).
    #[test]
    fn allocation_proportional(
        cost in 0i128..1_000_000_000_000_000i128,
        q1 in 1i128..1_000_000_000_000i128,
        q2 in 1i128..1_000_000_000_000i128,
    ) {
        let d = day(0);
        let pool = PoolId::new("p").expect("pool");
        let src = vec![CostSource {
            day: d, pool: pool.clone(), kind: ResourceKind::Cpu,
            amount: Amount::from_raw(cost), basis: Basis::Billed,
            source: SourceRef::Other { note: "x".into() },
            flags: Default::default(),
        }];
        let cap = vec![Capacity {
            day: d, pool: pool.clone(), kind: ResourceKind::Cpu,
            quantity: Amount::from_raw(4_000_000_000_000), basis: Basis::Declared,
        }];
        let u = |e: &str, q: i128| Usage {
            day: d, pool: pool.clone(), kind: ResourceKind::Cpu,
            consumer: ConsumerKey::new(e, "t", "_", "_").expect("k"),
            quantity: Amount::from_raw(q), basis: Basis::Measured,
        };
        let one = allocate_all(&src, &cap, &[u("a", q1), u("b", q2)]).expect("alloc");
        let two = allocate_all(&src, &cap, &[u("a", 2 * q1), u("b", q2)]).expect("alloc");
        let (x, y) = (one.lines[0].amount.raw(), two.lines[0].amount.raw());
        prop_assert!((y - 2 * x).abs() <= 2, "x={} y={}", x, y);
    }
}
