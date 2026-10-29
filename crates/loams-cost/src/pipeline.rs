//! One computation from inputs to allocation lines (design §84 §8, §11.2):
//! asset days and capacity, bill sources, priced pool-days, then
//! allocation, with the digest of everything that went in. The `meter`
//! role's cost job and `loams cost report --offline` both call this, so the
//! two give the same numbers.

use crate::amortise::assets_between;
use crate::inputs::CostInputs;
use crate::prices::priced_sources;
use crate::{Allocation, CostError, CostSource, Day, Usage, allocate_all, digest};

/// The allocation and the digest of its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Computed {
    pub allocation: Allocation,
    /// Every source that was allocated (assets, bills, prices), for
    /// explanations.
    pub sources: Vec<CostSource>,
    pub digest: String,
}

/// Computes `[from, to)`.
///
/// - `inputs_yaml` is the applied document's bytes (hashed into the digest).
/// - `bill_sources` are the mapped and shared sources of the bill imports in
///   force (superseded imports already left out); `import_digests` are their
///   file digests.
/// - `usage` is the driver usage (§84 §7).
pub fn compute(
    inputs: &CostInputs,
    inputs_yaml: &str,
    bill_sources: &[CostSource],
    import_digests: &[String],
    usage: &[Usage],
    from: Day,
    to: Day,
) -> Result<Computed, CostError> {
    if from >= to {
        return Err(CostError::invalid("from", "from must be before to"));
    }
    let in_range = |d: Day| d >= from && d < to;
    let (mut sources, capacities) = assets_between(inputs, from, to)?;
    sources.extend(bill_sources.iter().filter(|s| in_range(s.day)).cloned());
    let usage: Vec<Usage> = usage.iter().filter(|u| in_range(u.day)).cloned().collect();
    let priced = priced_sources(inputs, &usage, &sources)?;
    sources.extend(priced);
    let mut extra: Vec<&[u8]> = vec![inputs_yaml.as_bytes()];
    let mut digests = import_digests.to_vec();
    digests.sort();
    extra.extend(digests.iter().map(|d| d.as_bytes()));
    let digest = digest(&sources, &capacities, &usage, &extra)?;
    let allocation = allocate_all(&sources, &capacities, &usage)?;
    Ok(Computed {
        allocation,
        sources,
        digest,
    })
}
