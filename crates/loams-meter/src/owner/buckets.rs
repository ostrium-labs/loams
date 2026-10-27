//! Offset buckets (design §63 §12): per-key usage sums, one bucket per
//! [`BUCKET_SPAN`] stream offsets, so reconciliation can split the counters
//! at the House pipe's committed offset `O` without keeping every event.
//!
//! The tail after `O` is every bucket wholly after `O` plus the events of the
//! one bucket that straddles it, which the owner replays from the stream.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use loams_meter_core::Amount;

/// Stream offsets per bucket (§63 §12 step 3).
pub const BUCKET_SPAN: u64 = 65_536;

/// The bucket an offset falls in.
pub const fn bucket_of(offset: u64) -> u64 {
    offset / BUCKET_SPAN
}

/// The offsets bucket `b` covers.
pub const fn bucket_range(b: u64) -> RangeInclusive<u64> {
    let start = b * BUCKET_SPAN;
    start..=start + (BUCKET_SPAN - 1)
}

/// What the tail after a committed offset is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail {
    /// The sum of the buckets wholly after the committed offset.
    pub whole: Amount,
    /// The offsets after the committed offset that share its bucket: replay
    /// them from the stream and add what they count. Empty when the
    /// committed offset is the last of its bucket.
    pub replay: Option<RangeInclusive<u64>>,
}

/// Per-key sums by bucket.
#[derive(Clone, Debug, Default)]
pub struct OffsetBuckets<K: Ord> {
    sums: BTreeMap<K, BTreeMap<u64, Amount>>,
}

impl<K: Ord + Clone> OffsetBuckets<K> {
    pub fn new() -> Self {
        OffsetBuckets {
            sums: BTreeMap::new(),
        }
    }

    /// Adds `amount` for `key` from the event at `offset`.
    pub fn add(&mut self, key: K, offset: u64, amount: Amount) {
        let b = self
            .sums
            .entry(key)
            .or_default()
            .entry(bucket_of(offset))
            .or_insert(Amount::ZERO);
        *b = b.saturating_add(amount);
    }

    /// The tail of `key` after `committed` (exclusive).
    pub fn tail(&self, key: &K, committed: u64) -> Tail {
        let straddle = bucket_of(committed);
        let whole = self.sums.get(key).map_or(Amount::ZERO, |b| {
            b.range(straddle + 1..).map(|(_, a)| *a).sum()
        });
        let last = *bucket_range(straddle).end();
        Tail {
            whole,
            replay: (committed < last).then(|| committed + 1..=last),
        }
    }

    /// The keys with any bucket.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.sums.keys()
    }

    /// Drops every bucket before `committed`'s own: a later reconcile
    /// never splits before `committed`, so they are never read again.
    pub fn trim_before(&mut self, committed: u64) {
        let keep_from = bucket_of(committed);
        self.sums.retain(|_, b| {
            *b = b.split_off(&keep_from);
            !b.is_empty()
        });
    }

    /// Forgets `key` (its period ended).
    pub fn remove(&mut self, key: &K) {
        self.sums.remove(key);
    }

    /// How many (key, bucket) cells are held (a metric).
    pub fn cells(&self) -> usize {
        self.sums.values().map(BTreeMap::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: i64) -> Amount {
        Amount::from_units(n)
    }

    #[test]
    fn the_tail_is_whole_buckets_plus_a_replay_of_the_straddling_one() {
        let mut b = OffsetBuckets::new();
        b.add("k", 10, u(1)); // bucket 0
        b.add("k", BUCKET_SPAN + 5, u(2)); // bucket 1
        b.add("k", 2 * BUCKET_SPAN + 1, u(4)); // bucket 2
        let t = b.tail(&"k", BUCKET_SPAN + 3);
        assert_eq!(t.whole, u(4));
        assert_eq!(
            t.replay,
            Some(BUCKET_SPAN + 4..=2 * BUCKET_SPAN - 1),
            "bucket 1 after the committed offset is replayed"
        );
        // Committed at the end of a bucket: nothing to replay.
        let t = b.tail(&"k", BUCKET_SPAN - 1);
        assert_eq!(t.whole, u(6));
        assert_eq!(t.replay, None);
        assert_eq!(b.tail(&"other", 0).whole, Amount::ZERO);
    }

    #[test]
    fn trimming_keeps_the_straddling_bucket() {
        let mut b = OffsetBuckets::new();
        b.add("k", 1, u(1));
        b.add("k", BUCKET_SPAN + 1, u(2));
        b.trim_before(BUCKET_SPAN + 10);
        assert_eq!(b.cells(), 1);
        assert_eq!(b.tail(&"k", BUCKET_SPAN - 1).whole, u(2));
        b.trim_before(5 * BUCKET_SPAN);
        assert_eq!(b.cells(), 0);
    }
}
