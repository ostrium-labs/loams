//! Reconciliation (design §63 §12, D1127; ME1 Task 13).
//!
//! Every [`RECONCILE_EVERY`], and at every period close, each counter owner,
//! per partition *p*:
//!
//! 1. reads the House's latest `_meter.events` snapshot and its committed
//!    pipe offset `O_p`;
//! 2. reads period-to-date usage per counter from `_meter.events FINAL`,
//!    restricted to *p* and offsets `<= O_p`, pinned to that snapshot;
//! 3. sets `counted = house_total + tail`, where the tail is the offset
//!    buckets wholly after `O_p` plus a replay of the events after `O_p` in
//!    the bucket that straddles it;
//! 4. records the drift `|counted_before - counted_after|`, and raises an
//!    alert when it is over 1% of the counter's allowance.
//!
//! Each event is on exactly one side of `O_p`, so it is counted once; a
//! duplicate that dedup layer 2 missed is removed, because the House side
//! is deduplicated. A House pipe that is behind only makes the tail longer.
//! Billing never reads counters, and enforcement never waits for this.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use loams_meter_core::Amount;

use crate::owner::buckets::bucket_range;
use crate::owner::{
    CounterKey, FeatureIndex, HouseTotals, OwnerError, PartitionCounters, PartitionReader, Periods,
};

/// How often counters are reconciled (§63 §12).
pub const RECONCILE_EVERY: Duration = Duration::from_secs(5 * 60);

/// A drift above this share of an allowance raises an alert, in basis
/// points (100 = 1%).
pub const DRIFT_ALERT_BP: i128 = 100;

/// The metric reconciliation sets per (subject, feature) on an alert.
pub const DRIFT_METRIC: &str = "loams_meter_reconcile_drift_ratio";

/// A counter's allowance for the period (the total available at its
/// start), for the drift ratio; `None` for counters with none (a soft limit
/// without grants), which never alert.
pub trait Allowances: Sync {
    fn allowance(&self, key: &CounterKey) -> Option<Amount>;
}

/// One counter's correction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    pub key: CounterKey,
    pub before: Amount,
    pub after: Amount,
    pub allowance: Option<Amount>,
}

impl Drift {
    /// `|before - after|`.
    pub fn amount(&self) -> Amount {
        let d = self.before.saturating_sub(self.after);
        if d.is_negative() { -d } else { d }
    }

    /// The drift as a share of the allowance, in basis points; `None`
    /// without an allowance (or a zero one).
    pub fn ratio_bp(&self) -> Option<i128> {
        let allowance = self.allowance?;
        if !allowance.is_positive() {
            return None;
        }
        Some(self.amount().raw().saturating_mul(10_000) / allowance.raw())
    }

    /// Over [`DRIFT_ALERT_BP`] of the allowance (strictly).
    pub fn alerts(&self) -> bool {
        self.allowance.is_some_and(|a| {
            a.is_positive()
                && self.amount().raw().saturating_mul(10_000)
                    > a.raw().saturating_mul(DRIFT_ALERT_BP)
        })
    }
}

/// What one partition's reconciliation did.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    pub partition: u32,
    /// The House snapshot and committed offset it split at; `None` when the
    /// House pipe has committed nothing for the partition (nothing changed).
    pub split: Option<(i64, u64)>,
    /// Every counter whose count changed.
    pub drifts: Vec<Drift>,
    /// The counters set, with their new counts (for the lease allocators to
    /// rebase on).
    pub counted: Vec<(CounterKey, Amount)>,
    /// Events replayed from the straddling bucket.
    pub replayed: usize,
}

impl Report {
    /// The drifts over the alert threshold.
    pub fn alerts(&self) -> impl Iterator<Item = &Drift> {
        self.drifts.iter().filter(|d| d.alerts())
    }
}

/// Reconciles one partition's counters against the House.
pub async fn reconcile(
    counters: &mut PartitionCounters,
    house: &dyn HouseTotals,
    reader: &dyn PartitionReader,
    index: &FeatureIndex,
    periods: &dyn Periods,
    allowances: &dyn Allowances,
) -> Result<Report, OwnerError> {
    let partition = counters.partition();
    let mut report = Report {
        partition,
        ..Report::default()
    };
    let Some((committed, snapshot)) = house.committed(partition).await? else {
        return Ok(report);
    };
    // Counters have not read past the House yet (a rebuild still catching
    // up): the House is ahead, and its totals plus nothing are the count.
    let read_to = counters.next_offset();
    let totals: BTreeMap<CounterKey, Amount> = house
        .totals(partition, committed, snapshot)
        .await?
        .into_iter()
        .collect();
    let replay = replay_sums(counters, reader, index, periods, committed, read_to).await?;
    report.replayed = replay.events;

    let keys: BTreeSet<CounterKey> = counters
        .counters()
        .map(|(k, _)| k.clone())
        .chain(totals.keys().cloned())
        .collect();
    for key in keys {
        let house_total = totals.get(&key).copied().unwrap_or(Amount::ZERO);
        let tail = if read_to > committed + 1 {
            counters.tail(&key, committed).whole
        } else {
            Amount::ZERO
        };
        let replayed = replay.sums.get(&key).copied().unwrap_or(Amount::ZERO);
        let after = house_total.saturating_add(tail).saturating_add(replayed);
        let before = counters.rebase(key.clone(), after);
        if before != after {
            report.drifts.push(Drift {
                allowance: allowances.allowance(&key),
                key: key.clone(),
                before,
                after,
            });
        }
        report.counted.push((key, after));
    }
    counters.trim(committed);
    report.split = Some((snapshot, committed));
    for d in report.alerts() {
        tracing::warn!(
            metric = DRIFT_METRIC,
            subject = d.key.subject.as_str(),
            feature = d.key.feature.as_str(),
            before = %d.before,
            after = %d.after,
            ratio_bp = d.ratio_bp().unwrap_or_default(),
            "meter counter drifted more than 1% of its allowance"
        );
    }
    Ok(report)
}

struct Replay {
    sums: BTreeMap<CounterKey, Amount>,
    events: usize,
}

/// The per-counter sums of the events after `committed` in its bucket, up
/// to what the counters have read (`read_to`, exclusive), deduplicated
/// among themselves.
async fn replay_sums(
    counters: &PartitionCounters,
    reader: &dyn PartitionReader,
    index: &FeatureIndex,
    periods: &dyn Periods,
    committed: u64,
    read_to: u64,
) -> Result<Replay, OwnerError> {
    let end = (*bucket_range(crate::owner::buckets::bucket_of(committed)).end())
        .min(read_to.saturating_sub(1));
    let mut sums = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut events = 0;
    let mut from = committed + 1;
    while from <= end && read_to > 0 {
        let batch = reader
            .read(counters.partition(), from, crate::owner::READ_BATCH)
            .await?;
        if batch.is_empty() {
            break;
        }
        for (offset, event) in batch {
            if offset > end {
                from = end + 1;
                break;
            }
            from = offset + 1;
            events += 1;
            if !seen.insert(event.dedup_key()) {
                continue;
            }
            for (feature, amount) in index.amounts(&event) {
                let Some(period) = periods.period(&event.subject, &feature, event.time) else {
                    continue;
                };
                let key = CounterKey {
                    subject: event.subject.clone(),
                    feature,
                    period_start: period.start,
                };
                let s = sums.entry(key).or_insert(Amount::ZERO);
                *s = s.saturating_add(amount);
            }
        }
    }
    Ok(Replay { sums, events })
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_meter_core::{FeatureKey, SubjectKey, UtcNanos};

    fn drift(before: i64, after: i64, allowance: Option<i64>) -> Drift {
        Drift {
            key: CounterKey {
                subject: SubjectKey::new("s").expect("subject"),
                feature: FeatureKey::new("f").expect("feature"),
                period_start: UtcNanos(0),
            },
            before: Amount::from_units(before),
            after: Amount::from_units(after),
            allowance: allowance.map(Amount::from_units),
        }
    }

    #[test]
    fn drift_alert_above_one_percent() {
        // 1% of 1,000 is 10.
        assert!(drift(1_020, 1_000, Some(1_000)).alerts());
        assert!(drift(980, 1_000, Some(1_000)).alerts(), "either direction");
        assert!(
            !drift(1_010, 1_000, Some(1_000)).alerts(),
            "exactly 1% does not"
        );
        assert!(!drift(1_005, 1_000, Some(1_000)).alerts());
        assert!(
            !drift(5_000, 1_000, None).alerts(),
            "no allowance, no alert"
        );
        assert!(!drift(5_000, 1_000, Some(0)).alerts());
        assert_eq!(drift(1_020, 1_000, Some(1_000)).ratio_bp(), Some(200));
    }
}
