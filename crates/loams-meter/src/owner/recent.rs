//! Dedup layer 2 (design §63 §7.3): the dedup keys of the events a counter
//! owner counted recently, so an event appended twice (a producer retry
//! after `CommitUnknown`) is counted once. Bounded: the oldest keys go
//! first, and whatever escapes this layer is removed at the next
//! reconciliation (§12), which re-reads the House's deduplicated totals.

use std::collections::{HashSet, VecDeque};

/// How many keys a partition remembers by default: at the §63 §7.4
/// estimate of a few thousand events a second per partition, about a
/// minute, longer than any producer retry window inside one process.
pub const DEFAULT_RECENT_KEYS: usize = 262_144;

/// A bounded set of dedup keys (SHA-256 of `source ‖ 0 ‖ id`).
#[derive(Debug, Clone)]
pub struct RecentKeys {
    capacity: usize,
    order: VecDeque<[u8; 32]>,
    set: HashSet<[u8; 32]>,
}

impl RecentKeys {
    pub fn new(capacity: usize) -> Self {
        RecentKeys {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    /// Remembers `key`; `false` if it was already there (a duplicate).
    pub fn insert(&mut self, key: [u8; 32]) -> bool {
        if !self.set.insert(key) {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }

    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.set.contains(key)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_are_refused_and_the_oldest_key_is_forgotten() {
        let mut r = RecentKeys::new(2);
        assert!(r.insert([1; 32]));
        assert!(!r.insert([1; 32]));
        assert!(r.insert([2; 32]));
        assert!(r.insert([3; 32]));
        assert!(!r.contains(&[1; 32]), "evicted");
        assert!(r.insert([1; 32]), "an evicted key counts again");
        assert_eq!(r.len(), 2);
    }
}
