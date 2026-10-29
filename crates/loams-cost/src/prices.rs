//! The price table (design §84 §6.4, D2356): a rate per driver unit prices
//! a pool-day that has **no** asset and no billed line behind it, as
//! `rate × Σ usage`. A `contract` rate is `declared`; a `list` or
//! `estimate` rate is `estimated`. A price never overrides a billed amount.
//!
//! The same rates also give **reference costs** (what the measured usage
//! would cost at a rate), which reports show beside the actual cost and
//! never instead of it (§84 §6.4, Task 4).

use std::collections::{BTreeMap, BTreeSet};

use loams_meter_core::Amount;

use crate::allocate::PoolDayKey;
use crate::inputs::{CostInputs, Price, PriceSource};
use crate::{Basis, CostError, CostSource, SourceRef, Usage};

impl PriceSource {
    /// The basis of an amount priced at this source.
    pub const fn basis(self) -> Basis {
        match self {
            PriceSource::Contract => Basis::Declared,
            PriceSource::List | PriceSource::Estimate => Basis::Estimated,
        }
    }
}

/// `rate × quantity / per`, rounded half away from zero to the nano-unit:
/// `quantity` is in driver units and `per` is the price unit's size in
/// driver units ([`crate::inputs::PriceUnit::size`]).
pub fn price_per(rate: Amount, quantity: Amount, per: u128) -> Result<Amount, CostError> {
    let (r, q) = (rate.raw(), quantity.raw());
    if r < 0 || q < 0 {
        return Err(CostError::NegativeQuantity);
    }
    let den = (Amount::SCALE as u128)
        .checked_mul(per)
        .ok_or(CostError::Overflow)?;
    let (q_, rem) = crate::money::mul_div(r.unsigned_abs(), q.unsigned_abs(), den)
        .ok_or(CostError::Overflow)?;
    let rounded = q_ + u128::from(rem >= den - rem);
    Ok(Amount::from_raw(
        i128::try_from(rounded).map_err(|_| CostError::Overflow)?,
    ))
}

/// `rate × quantity` (a price per one driver unit, or an FX rate).
pub fn price_of(rate: Amount, quantity: Amount) -> Result<Amount, CostError> {
    price_per(rate, quantity, 1)
}

/// `rate × amount` for an amount of either sign (a credit converted with
/// an FX rate), rounded half away from zero.
pub fn price_of_signed(rate: Amount, amount: Amount) -> Result<Amount, CostError> {
    let p = price_of(rate, Amount::from_raw(amount.raw().abs()))?;
    Ok(if amount.is_negative() { -p } else { p })
}

/// Σ usage per pool-day.
pub fn usage_totals(usage: &[Usage]) -> Result<BTreeMap<PoolDayKey, Amount>, CostError> {
    let mut out: BTreeMap<PoolDayKey, Amount> = BTreeMap::new();
    for u in usage {
        let slot = out
            .entry(PoolDayKey {
                day: u.day,
                pool: u.pool.clone(),
                kind: u.kind,
            })
            .or_insert(Amount::ZERO);
        *slot = slot.checked_add(u.quantity).ok_or(CostError::Overflow)?;
    }
    Ok(out)
}

/// Price sources for every pool-day that has usage, a price, and no other
/// source in `existing` (assets and bill lines).
pub fn priced_sources(
    inputs: &CostInputs,
    usage: &[Usage],
    existing: &[CostSource],
) -> Result<Vec<CostSource>, CostError> {
    let covered: BTreeSet<PoolDayKey> = existing
        .iter()
        .map(|s| PoolDayKey {
            day: s.day,
            pool: s.pool.clone(),
            kind: s.kind,
        })
        .collect();
    let mut out = Vec::new();
    for (key, total) in usage_totals(usage)? {
        if covered.contains(&key) {
            continue;
        }
        let Some(price) = inputs.price(&key.pool, key.kind, key.day) else {
            continue;
        };
        out.push(source_for(price, &key, total, inputs.version)?);
    }
    Ok(out)
}

fn source_for(
    price: &Price,
    key: &PoolDayKey,
    quantity: Amount,
    inputs_version: u64,
) -> Result<CostSource, CostError> {
    let per = price.unit.size(key.kind).ok_or_else(|| {
        CostError::invalid(
            "prices",
            format!("{}: unit does not fit {}", price.id, key.kind),
        )
    })?;
    Ok(CostSource {
        day: key.day,
        pool: key.pool.clone(),
        kind: key.kind,
        amount: price_per(price.rate, quantity, per)?,
        basis: price.source.basis(),
        source: SourceRef::Price {
            price: price.id.clone(),
            inputs_version,
        },
        flags: BTreeSet::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs::CostInputs;
    use crate::{ConsumerKey, Day, PoolId, ResourceKind};

    const YAML: &str = r#"
version: 1
currency: USD
prices:
  - { id: s3, kind: storage, scope: eu/bucket, rate: "0.02", unit: gb_month, source: contract, effective: 2026-10-01 }
  - { id: list-cpu, kind: cpu, scope: laptop, rate: "0.045", unit: core_hour, source: list, effective: 2026-10-01, note: "example input, not a quote" }
"#;

    fn usage(pool: &str, kind: ResourceKind, q: &str) -> Usage {
        Usage {
            day: Day::parse("2026-10-28").expect("d"),
            pool: PoolId::new(pool).expect("p"),
            kind,
            consumer: ConsumerKey::new("house", "org_1", "_", "_").expect("k"),
            quantity: Amount::parse(q).expect("q"),
            basis: crate::Basis::Measured,
        }
    }

    #[test]
    fn price_of_rounds_half_up() {
        assert_eq!(
            price_of(
                Amount::parse("0.000000001").expect("a"),
                Amount::parse("0.5").expect("a")
            )
            .expect("p"),
            Amount::parse("0.000000001").expect("a")
        );
        assert_eq!(
            price_of(
                Amount::parse("2").expect("a"),
                Amount::parse("3.25").expect("a")
            )
            .expect("p"),
            Amount::parse("6.5").expect("a")
        );
    }

    #[test]
    fn price_effective_dates() {
        let inputs = CostInputs::parse(YAML).expect("parse");
        let mut u = usage("eu/bucket", ResourceKind::Storage, "1000000000000");
        u.day = Day::parse("2026-09-30").expect("d");
        assert!(priced_sources(&inputs, &[u], &[]).expect("p").is_empty());
    }

    #[test]
    fn prices_only_uncovered_pools() {
        let inputs = CostInputs::parse(YAML).expect("parse");
        let u = vec![
            usage("eu/bucket", ResourceKind::Storage, "1314873000000000"),
            usage("eu/bucket", ResourceKind::Storage, "1314873000000000"),
            usage("laptop", ResourceKind::Cpu, "3600"),
        ];
        let s = priced_sources(&inputs, &u, &[]).expect("p");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].pool.as_str(), "eu/bucket");
        // Two halves of a GB-month at 0.02 per GB-month.
        assert_eq!(s[0].amount, Amount::parse("0.02").expect("a"));
        assert_eq!(s[0].basis, Basis::Declared);
        assert_eq!(s[1].amount, Amount::parse("0.045").expect("a"));
        assert_eq!(s[1].basis, Basis::Estimated);
        // A billed line for the bucket wins: no price source.
        let billed = CostSource {
            basis: Basis::Billed,
            source: SourceRef::BillLine {
                import: "i".into(),
                line: 1,
            },
            ..s[0].clone()
        };
        let s = priced_sources(&inputs, &u, &[billed]).expect("p");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].pool.as_str(), "laptop");
    }
}
