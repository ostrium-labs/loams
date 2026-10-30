//! The two metastore primitives the journal needs: leases with epochs, and a
//! namespace-scoped pointer swapped by compare-and-swap under a lease's fence
//! (§85 §5.1, §8).
//!
//! [`MetaCheckpoints`] adapts any [`MetaStore`]. [`MemCheckpoints`] is an
//! in-memory implementation with the same contract and a settable clock, for
//! tests and the standalone dev server.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use loams_common::NamespaceId;
use loams_common::meta::{
    ApplyError, Consistency, Fence, Lease, LeaseGrant, MetaError, MetaStore, Pointer, PointerCas,
};

/// Why a checkpoint operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// Another owner holds the lease.
    #[error("lease held by {owner} until {deadline_ms}")]
    LeaseHeld { owner: String, deadline_ms: u64 },
    /// The caller does not hold the lease at that epoch.
    #[error("lease lost")]
    LeaseLost,
    /// The swap's fence no longer holds.
    #[error("fenced")]
    Fenced,
    /// The pointer is not at the expected version.
    #[error("pointer version mismatch")]
    VersionMismatch { current: Option<Pointer> },
    /// The namespace does not exist.
    #[error("namespace not found")]
    NamespaceNotFound,
    /// Anything else: the metastore is unavailable, or rejected the command
    /// for a reason this crate does not map.
    #[error("{0}")]
    Other(String),
}

/// Leases and fenced pointers, the metastore's commit primitives.
#[async_trait]
pub trait Checkpoints: Send + Sync + fmt::Debug + 'static {
    /// The metastore clock, in ms since the epoch.
    fn now_ms(&self) -> u64;

    /// Takes a free or expired lease, bumping its epoch, or extends one
    /// `owner` already holds at the same epoch.
    async fn acquire_lease(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, CheckpointError>;

    /// The lease on `key`, if it was ever taken.
    async fn lease(&self, key: &str) -> Result<Option<Lease>, CheckpointError>;

    /// The pointer `key` in `ns`.
    async fn pointer(&self, ns: NamespaceId, key: &str)
    -> Result<Option<Pointer>, CheckpointError>;

    /// Sets `key` to `value` if it is at version `expected` (`None`: absent)
    /// and the fence holds; returns the new version. A retry whose first
    /// attempt landed returns that version, because values are unique.
    async fn cas_pointer(
        &self,
        ns: NamespaceId,
        key: &str,
        expected: Option<u64>,
        value: String,
        fence: Option<Fence>,
    ) -> Result<u64, CheckpointError>;

    /// Every pointer in `ns` whose key starts with `prefix`, in key order.
    async fn pointers_with_prefix(
        &self,
        ns: NamespaceId,
        prefix: &str,
    ) -> Result<Vec<(String, Pointer)>, CheckpointError>;
}

// ---------------------------------------------------------------- metastore

/// [`Checkpoints`] over the engine's metastore.
#[derive(Debug, Clone)]
pub struct MetaCheckpoints {
    meta: Arc<dyn MetaStore>,
}

impl MetaCheckpoints {
    pub fn new(meta: Arc<dyn MetaStore>) -> Self {
        Self { meta }
    }
}

fn map_meta(e: MetaError) -> CheckpointError {
    match e {
        MetaError::Rejected(ApplyError::LeaseHeld { owner, deadline_ms }) => {
            CheckpointError::LeaseHeld { owner, deadline_ms }
        }
        MetaError::Rejected(ApplyError::LeaseLost { .. }) => CheckpointError::LeaseLost,
        MetaError::Rejected(ApplyError::Fenced { .. }) => CheckpointError::Fenced,
        MetaError::Rejected(ApplyError::VersionMismatch { current }) => {
            CheckpointError::VersionMismatch { current }
        }
        MetaError::Rejected(ApplyError::NamespaceNotFound(_)) => CheckpointError::NamespaceNotFound,
        other => CheckpointError::Other(other.to_string()),
    }
}

#[async_trait]
impl Checkpoints for MetaCheckpoints {
    fn now_ms(&self) -> u64 {
        self.meta.now_ms()
    }

    async fn acquire_lease(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, CheckpointError> {
        self.meta
            .acquire_lease(key, owner, ttl)
            .await
            .map_err(map_meta)
    }

    async fn lease(&self, key: &str) -> Result<Option<Lease>, CheckpointError> {
        self.meta
            .lease(Consistency::Linearizable, key)
            .await
            .map_err(map_meta)
    }

    async fn pointer(
        &self,
        ns: NamespaceId,
        key: &str,
    ) -> Result<Option<Pointer>, CheckpointError> {
        self.meta
            .pointer(Consistency::Linearizable, ns, key)
            .await
            .map_err(map_meta)
    }

    async fn cas_pointer(
        &self,
        ns: NamespaceId,
        key: &str,
        expected: Option<u64>,
        value: String,
        fence: Option<Fence>,
    ) -> Result<u64, CheckpointError> {
        let tracked = self
            .meta
            .cas_pointer(PointerCas {
                namespace: ns,
                key: key.to_string(),
                expected,
                value: value.clone(),
                fence,
                fresh: None,
            })
            .await;
        match tracked.result.map_err(map_meta) {
            // A lost acknowledgement: the pointer holds our unique value one
            // version on, so the first attempt landed.
            Err(CheckpointError::VersionMismatch { current: Some(cur) })
                if cur.value == value
                    && Some(cur.version) == expected.map(|v| v + 1).or(Some(1)) =>
            {
                Ok(cur.version)
            }
            other => other,
        }
    }

    async fn pointers_with_prefix(
        &self,
        ns: NamespaceId,
        prefix: &str,
    ) -> Result<Vec<(String, Pointer)>, CheckpointError> {
        self.meta
            .pointers_with_prefix(Consistency::Linearizable, ns, prefix)
            .await
            .map_err(map_meta)
    }
}

// ---------------------------------------------------------------- in memory

#[derive(Debug, Default)]
struct MemState {
    leases: HashMap<String, Lease>,
    pointers: BTreeMap<(u64, String), Pointer>,
}

/// An in-memory [`Checkpoints`] with the metastore's contract. Every
/// namespace exists. The clock is wall time plus a settable offset, so tests
/// can expire leases without sleeping.
#[derive(Debug, Clone, Default)]
pub struct MemCheckpoints {
    state: Arc<Mutex<MemState>>,
    skew_ms: Arc<AtomicU64>,
}

impl MemCheckpoints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves this instance's clock forward.
    pub fn advance(&self, by: Duration) {
        let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.skew_ms.fetch_add(ms, Ordering::SeqCst);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MemState> {
        // A poisoned lock only means a test panicked mid-operation; the map
        // itself is always consistent between statements.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[async_trait]
impl Checkpoints for MemCheckpoints {
    fn now_ms(&self) -> u64 {
        wall_ms().saturating_add(self.skew_ms.load(Ordering::SeqCst))
    }

    async fn acquire_lease(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, CheckpointError> {
        let now = self.now_ms();
        let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
        let mut st = self.state();
        let lease = st.leases.entry(key.to_string()).or_insert(Lease {
            epoch: 0,
            owner: None,
            deadline_ms: 0,
        });
        if lease.owner.as_deref() == Some(owner) && lease.is_held_at(now) {
            lease.deadline_ms = now.saturating_add(ttl_ms);
        } else if lease.is_held_at(now) {
            return Err(CheckpointError::LeaseHeld {
                owner: lease.owner.clone().unwrap_or_default(),
                deadline_ms: lease.deadline_ms,
            });
        } else {
            lease.epoch += 1;
            lease.owner = Some(owner.to_string());
            lease.deadline_ms = now.saturating_add(ttl_ms);
        }
        Ok(LeaseGrant {
            epoch: lease.epoch,
            deadline_ms: lease.deadline_ms,
        })
    }

    async fn lease(&self, key: &str) -> Result<Option<Lease>, CheckpointError> {
        Ok(self.state().leases.get(key).cloned())
    }

    async fn pointer(
        &self,
        ns: NamespaceId,
        key: &str,
    ) -> Result<Option<Pointer>, CheckpointError> {
        Ok(self.state().pointers.get(&(ns.0, key.to_string())).cloned())
    }

    async fn cas_pointer(
        &self,
        ns: NamespaceId,
        key: &str,
        expected: Option<u64>,
        value: String,
        fence: Option<Fence>,
    ) -> Result<u64, CheckpointError> {
        let mut st = self.state();
        if let Some(f) = &fence {
            // As in the metastore: expiry alone does not break a fence, only
            // a takeover (a newer epoch) or a release does.
            let ok = st
                .leases
                .get(&f.lease)
                .is_some_and(|l| l.epoch == f.epoch && l.owner.is_some());
            if !ok {
                return Err(CheckpointError::Fenced);
            }
        }
        let slot = (ns.0, key.to_string());
        let current = st.pointers.get(&slot).cloned();
        if current.as_ref().map(|p| p.version) != expected {
            return Err(CheckpointError::VersionMismatch { current });
        }
        let version = expected.map_or(1, |v| v + 1);
        st.pointers.insert(slot, Pointer { version, value });
        Ok(version)
    }

    async fn pointers_with_prefix(
        &self,
        ns: NamespaceId,
        prefix: &str,
    ) -> Result<Vec<(String, Pointer)>, CheckpointError> {
        Ok(self
            .state()
            .pointers
            .range((ns.0, prefix.to_string())..)
            .take_while(|((n, k), _)| *n == ns.0 && k.starts_with(prefix))
            .map(|((_, k), p)| (k.clone(), p.clone()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: NamespaceId = NamespaceId(1);

    #[tokio::test]
    async fn lease_extends_for_owner_and_refuses_others() {
        let ck = MemCheckpoints::new();
        let ttl = Duration::from_secs(30);
        let a = ck.acquire_lease("l", "a", ttl).await.expect("a");
        let again = ck.acquire_lease("l", "a", ttl).await.expect("a again");
        assert_eq!(a.epoch, again.epoch);
        assert!(matches!(
            ck.acquire_lease("l", "b", ttl).await,
            Err(CheckpointError::LeaseHeld { .. })
        ));
        ck.advance(Duration::from_secs(31));
        let b = ck
            .acquire_lease("l", "b", ttl)
            .await
            .expect("b after expiry");
        assert_eq!(b.epoch, a.epoch + 1);
    }

    #[tokio::test]
    async fn fenced_cas_refuses_old_epoch() {
        let ck = MemCheckpoints::new();
        let ttl = Duration::from_secs(1);
        let a = ck.acquire_lease("l", "a", ttl).await.expect("a");
        let fence_a = Fence {
            lease: "l".into(),
            epoch: a.epoch,
        };
        assert_eq!(
            ck.cas_pointer(NS, "p", None, "1".into(), Some(fence_a.clone()))
                .await,
            Ok(1)
        );
        ck.advance(Duration::from_secs(2));
        // Expired but not taken over: the fence still holds.
        assert_eq!(
            ck.cas_pointer(NS, "p", Some(1), "2".into(), Some(fence_a.clone()))
                .await,
            Ok(2)
        );
        ck.acquire_lease("l", "b", ttl).await.expect("b");
        assert_eq!(
            ck.cas_pointer(NS, "p", Some(2), "3".into(), Some(fence_a))
                .await,
            Err(CheckpointError::Fenced)
        );
    }

    #[tokio::test]
    async fn prefix_listing_is_scoped_and_ordered() {
        let ck = MemCheckpoints::new();
        for k in ["run/b", "run/a", "transfer/x"] {
            ck.cas_pointer(NS, k, None, k.into(), None)
                .await
                .expect("cas");
        }
        ck.cas_pointer(NamespaceId(2), "run/c", None, "c".into(), None)
            .await
            .expect("cas");
        let keys: Vec<_> = ck
            .pointers_with_prefix(NS, "run/")
            .await
            .expect("list")
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["run/a", "run/b"]);
    }
}
