//! Exports (design §84 §13, D2374, D2375): a FOCUS 1.3 Cost and Usage
//! dataset with split cost allocation columns, and OpenMeter-compatible
//! CloudEvents. Both read allocation lines and their pool-days, and both
//! conserve each pool-day exactly (ratios sum to 1; micros sum to the
//! pool-day's cost in micros).

pub mod focus;
pub mod openmeter;

use std::collections::BTreeMap;

use loams_meter_core::Amount;

use crate::money::split;
use crate::{CostError, Line, PoolDayKey};

/// Groups line indices by pool-day, in key order.
pub(crate) fn by_pool_day(lines: &[Line]) -> BTreeMap<PoolDayKey, Vec<usize>> {
    let mut out: BTreeMap<PoolDayKey, Vec<usize>> = BTreeMap::new();
    for (i, l) in lines.iter().enumerate() {
        out.entry(l.key.clone()).or_default().push(i);
    }
    out
}

/// Splits `total` over the lines `idx` by the magnitude of their amounts;
/// `None` when every amount is zero.
pub(crate) fn split_by_amount(
    total: Amount,
    lines: &[Line],
    idx: &[usize],
) -> Result<Option<Vec<Amount>>, CostError> {
    let weights: Vec<Amount> = idx
        .iter()
        .map(|i| Amount::from_raw(lines[*i].amount.raw().abs()))
        .collect();
    if weights.iter().all(|w| w.is_zero()) {
        return Ok(None);
    }
    split(total, &weights).map(Some)
}
