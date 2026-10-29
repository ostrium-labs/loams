//! One partition's counters (design §63 §11.3, §11.5, §12).
//!
//! The owner of partition *p* reads `_system/_meter`'s partition *p* as the
//! named consumer `meter-counters` and keeps, per (subject, feature,
//! period), the usage counted so far: `counted`. Each event is
//!
//! 1. skipped if its offset was already applied (a replay after a restart),
//! 2. skipped if its dedup key was counted recently (dedup layer 2),
//! 3. turned into an amount per feature by the feature's meter and filter,
//! 4. added to `counted` and to the key's offset bucket.
//!
//! On gaining a partition the owner **rebuilds**: the House's period-to-date
//! totals at the pipe's committed offset `O_p`, then a replay of the stream
//! after `O_p` through [`PartitionCounters::apply`]. Each event is on exactly
//! one side of `O_p`, so it is counted once. (An event appended twice with
//! the two copies on either side of `O_p` is counted twice until the next
//! reconciliation moves `O_p` past both, where the House dedups them.)

use std::collections::{BTreeMap, HashMap};

use loams_meter_core::{
    Amount, Event, Feature, FeatureKey, LeaseId, Meter, Period, SubjectKey, UtcNanos,
};

use super::buckets::{OffsetBuckets, Tail};
use super::recent::{DEFAULT_RECENT_KEYS, RecentKeys};

/// What one counter counts: a subject's use of a feature in one usage
/// period (named by its start).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CounterKey {
    pub subject: SubjectKey,
    pub feature: FeatureKey,
    pub period_start: UtcNanos,
}

/// Which features an event type feeds, built from the meters and features
/// (§63 §8.1, §9.2).
#[derive(Debug, Clone, Default)]
pub struct FeatureIndex {
    by_type: HashMap<String, Vec<(Meter, Feature)>>,
}

impl FeatureIndex {
    /// The index of `features` over `meters`; archived features and
    /// features without a meter feed no counter.
    pub fn new(meters: &[Meter], features: &[Feature]) -> Self {
        let mut by_type: HashMap<String, Vec<(Meter, Feature)>> = HashMap::new();
        for f in features.iter().filter(|f| !f.archived) {
            let Some(slug) = &f.meter else { continue };
            if let Some(m) = meters.iter().find(|m| &m.slug == slug) {
                by_type
                    .entry(m.event_type.clone())
                    .or_default()
                    .push((m.clone(), f.clone()));
            }
        }
        FeatureIndex { by_type }
    }

    /// What `event` adds to each feature it feeds.
    pub fn amounts(&self, event: &Event) -> Vec<(FeatureKey, Amount)> {
        let Some(candidates) = self.by_type.get(&event.r#type) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (meter, feature) in candidates {
            if !meter.matches(event) {
                continue;
            }
            if !feature.filter.is_empty() {
                let groups = meter.group_values(event);
                if !feature
                    .filter
                    .iter()
                    .all(|(k, v)| groups.get(k).is_some_and(|g| g == v))
                {
                    continue;
                }
            }
            match meter.value(event) {
                Ok(Some(amount)) => out.push((feature.key.clone(), amount)),
                Ok(None) => {}
                Err(e) => tracing::debug!(
                    meter = meter.slug.as_str(),
                    id = event.id.as_str(),
                    "event not counted: {e}"
                ),
            }
        }
        out
    }
}

/// The usage period an event falls in, from the subject's entitlement to
/// the feature; `None` when the subject has no metered entitlement to it, so
/// there is nothing to enforce (the House still bills it).
pub trait Periods: Sync {
    fn period(&self, subject: &SubjectKey, feature: &FeatureKey, at: UtcNanos) -> Option<Period>;
}

/// What [`PartitionCounters::apply`] did with an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Its offset was already applied.
    Replayed,
    /// Its dedup key was counted recently.
    Duplicate,
    /// Counted into these counters (empty: no counter is fed by it). The
    /// lease it carried, if any, retires that much of the lease (§11.3).
    Counted {
        amounts: Vec<(CounterKey, Amount)>,
        lease: Option<LeaseId>,
    },
}

/// The counters of one partition at one owner epoch.
#[derive(Debug, Clone)]
pub struct PartitionCounters {
    partition: u32,
    epoch: u64,
    counted: BTreeMap<CounterKey, Amount>,
    buckets: OffsetBuckets<CounterKey>,
    recent: RecentKeys,
    next_offset: u64,
    duplicates: u64,
}

impl PartitionCounters {
    /// Rebuilt from the House (§11.5 step 2): `house` holds the
    /// period-to-date totals at the pipe's committed offset `committed`
    /// (`None` when the pipe has committed nothing). Replay the stream from
    /// [`PartitionCounters::next_offset`] next.
    pub fn rebuild(
        partition: u32,
        epoch: u64,
        house: impl IntoIterator<Item = (CounterKey, Amount)>,
        committed: Option<u64>,
    ) -> Self {
        PartitionCounters {
            partition,
            epoch,
            counted: house.into_iter().collect(),
            buckets: OffsetBuckets::new(),
            recent: RecentKeys::new(DEFAULT_RECENT_KEYS),
            next_offset: committed.map_or(0, |o| o + 1),
            duplicates: 0,
        }
    }

    pub fn partition(&self) -> u32 {
        self.partition
    }

    /// The owner epoch these counters belong to.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The next stream offset to read.
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }

    /// Events dropped by dedup layer 2 so far.
    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }

    /// Counts the event at `offset`.
    pub fn apply(
        &mut self,
        offset: u64,
        event: &Event,
        index: &FeatureIndex,
        periods: &dyn Periods,
    ) -> Applied {
        if offset < self.next_offset {
            return Applied::Replayed;
        }
        self.next_offset = offset + 1;
        if !self.recent.insert(event.dedup_key()) {
            self.duplicates += 1;
            return Applied::Duplicate;
        }
        let mut amounts = Vec::new();
        for (feature, amount) in index.amounts(event) {
            let Some(period) = periods.period(&event.subject, &feature, event.time) else {
                continue;
            };
            let key = CounterKey {
                subject: event.subject.clone(),
                feature,
                period_start: period.start,
            };
            let c = self.counted.entry(key.clone()).or_insert(Amount::ZERO);
            *c = c.saturating_add(amount);
            self.buckets.add(key.clone(), offset, amount);
            amounts.push((key, amount));
        }
        Applied::Counted {
            amounts,
            lease: event.lease.clone(),
        }
    }

    /// The usage counted for `key`.
    pub fn counted(&self, key: &CounterKey) -> Amount {
        self.counted.get(key).copied().unwrap_or(Amount::ZERO)
    }

    /// Every counter.
    pub fn counters(&self) -> impl Iterator<Item = (&CounterKey, &Amount)> {
        self.counted.iter()
    }

    /// The tail of `key` after the House's committed offset (§12 step 3).
    pub fn tail(&self, key: &CounterKey, committed: u64) -> Tail {
        self.buckets.tail(key, committed)
    }

    /// Sets `key`'s count (reconciliation, §12) and answers the old one.
    pub fn rebase(&mut self, key: CounterKey, counted: Amount) -> Amount {
        self.counted.insert(key, counted).unwrap_or(Amount::ZERO)
    }

    /// Drops the buckets reconciliation will not read again.
    pub fn trim(&mut self, committed: u64) {
        self.buckets.trim_before(committed);
    }

    /// Forgets the counters of periods that ended before `now`.
    pub fn expire(&mut self, now: UtcNanos, period_end: impl Fn(&CounterKey) -> UtcNanos) {
        let ended: Vec<CounterKey> = self
            .counted
            .keys()
            .filter(|k| period_end(k) <= now)
            .cloned()
            .collect();
        for k in ended {
            self.counted.remove(&k);
            self.buckets.remove(&k);
        }
    }
}
