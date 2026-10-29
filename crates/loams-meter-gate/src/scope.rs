//! Where the meter has a policy (D2400): the (subject, feature) pairs that
//! hold a metered entitlement, kept current by the `meter` role.
//!
//! The products' gate is a [`LeaseGate`](crate::gate::LeaseGate) for every
//! pair; the scoping happens where leases come from. For a pair outside
//! [`Policies`], the lease source answers an unmetered grant (allowed,
//! counting only, no amount limit) that the gate caches like any other, so
//! a product without a plan is never refused and its hot path costs a lease
//! hit, nothing more. A new entitlement takes effect when that grant is
//! refreshed.
//!
//! Reading the set is lock-free on the common path: each thread keeps its
//! own copy and checks one shared version number before using it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

/// A fast non-cryptographic hasher for the policy lookups (the keys are
/// the meter's own subjects and features, not attacker-chosen collisions
/// worth defending against on the hot path): the multiply-rotate word hash
/// rustc uses, written out (std only).
#[derive(Debug, Default, Clone, Copy)]
pub struct WordHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for WordHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(SEED);
        }
        for &b in chunks.remainder() {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(SEED);
        }
    }
}

type Fast = BuildHasherDefault<WordHasher>;

/// One version of the policy set: feature → subjects.
#[derive(Debug, Default)]
pub struct PolicySet {
    by_feature: HashMap<String, HashSet<String, Fast>, Fast>,
}

impl PolicySet {
    /// The set of `(subject, feature)` pairs.
    pub fn new(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut by_feature: HashMap<String, HashSet<String, Fast>, Fast> = HashMap::default();
        for (subject, feature) in pairs {
            by_feature.entry(feature).or_default().insert(subject);
        }
        PolicySet { by_feature }
    }

    pub fn contains(&self, subject: &str, feature: &str) -> bool {
        self.by_feature
            .get(feature)
            .is_some_and(|s| s.contains(subject))
    }

    pub fn len(&self) -> usize {
        self.by_feature.values().map(|s| s.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Per thread: (policies id, version, set) for each [`Policies`] used.
    static CACHE: RefCell<Vec<(u64, u64, Arc<PolicySet>)>> = const { RefCell::new(Vec::new()) };
}

/// The policy set the `meter` role keeps current.
pub struct Policies {
    id: u64,
    version: AtomicU64,
    current: RwLock<Arc<PolicySet>>,
}

impl fmt::Debug for Policies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Policies")
            .field("version", &self.version.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Default for Policies {
    fn default() -> Self {
        Policies {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            version: AtomicU64::new(0),
            current: RwLock::new(Arc::new(PolicySet::default())),
        }
    }
}

impl Policies {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replaces the set.
    pub fn set(&self, set: PolicySet) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(set);
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Runs `f` on the current set. A hit touches only this thread's copy
    /// and one shared version number; a lock is taken only when the copy is
    /// stale. `f` must not use a [`Policies`] itself.
    pub fn with<R>(&self, f: impl FnOnce(&PolicySet) -> R) -> R {
        let version = self.version.load(Ordering::Acquire);
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let i = match cache.iter().position(|(id, _, _)| *id == self.id) {
                Some(i) if cache[i].1 == version => i,
                found => {
                    let fresh =
                        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner));
                    match found {
                        Some(i) => {
                            cache[i] = (self.id, version, fresh);
                            i
                        }
                        None => {
                            cache.push((self.id, version, fresh));
                            cache.len() - 1
                        }
                    }
                }
            };
            f(&cache[i].2)
        })
    }

    pub fn contains(&self, subject: &str, feature: &str) -> bool {
        self.with(|s| s.contains(subject, feature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_found_by_subject_and_feature() {
        let set = PolicySet::new([("org_a".into(), "api_requests".into())]);
        assert!(set.contains("org_a", "api_requests"));
        assert!(!set.contains("org_b", "api_requests"));
        assert!(!set.contains("org_a", "mcp_calls"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn another_thread_sees_a_replacement() {
        let policies = Policies::new();
        assert!(!policies.contains("s", "f"));
        policies.set(PolicySet::new([("s".into(), "f".into())]));
        let p = policies.clone();
        assert!(
            std::thread::spawn(move || p.contains("s", "f"))
                .join()
                .expect("thread")
        );
        policies.set(PolicySet::default());
        assert!(!policies.contains("s", "f"));
    }
}
