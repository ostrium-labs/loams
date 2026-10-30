//! The raw store: single-key operations on TiKV's RawKV API (API v2,
//! keyspace-aware), with no transactions and no timestamp from PD
//! (coordinator ruling 2026-10-30; design note
//! `docs/design/kv-rawkv.md`).
//!
//! A [`Store`](crate::Store) runs every operation, even a one-key `get`, as a
//! Percolator transaction: it takes a timestamp from PD's TSO first, so PD is
//! on every hot path. A [`RawStore`] talks to the region leader directly:
//! `get`, `put`, `delete`, their batches, compare-and-swap and TTLs, and
//! nothing that needs two keys to change together.
//!
//! # Rules
//!
//! - **Raw and transactional data never mix.** API v2 encodes the mode in
//!   the key (a raw key and a transactional key with the same bytes are two
//!   keys), so data a [`RawStore`] writes is invisible to a
//!   [`Store`](crate::Store) on the same keyspace, and the other way round.
//!   A key range is raw or transactional for its whole life. Callers give a
//!   raw store its own root (or keyspace).
//! - **Every write is atomic-mode.** TiKV's compare-and-swap is linearizable
//!   only if every write to the key takes the same latch, so `put`,
//!   `batch_put` and `delete` go through the client's atomic mode too
//!   (`with_atomic_for_cas`). Reads use the plain client. Atomic mode refuses
//!   `batch_delete`, so [`RawStore::batch_delete`] deletes key by key,
//!   concurrently.
//! - **Batches are not atomic.** `batch_put` of keys in two regions can apply
//!   in one and fail in the other; a caller that needs all-or-nothing uses a
//!   [`Store`](crate::Store).
//! - **TTL is in seconds.** API v2 keeps a TTL per raw key when the cluster
//!   runs with `storage.enable-ttl = true`. A [`Duration`] TTL is rounded up
//!   to whole seconds (at least one); an expired key reads as absent.
//!   [`RawStore::compare_and_swap`] writes without a TTL, so it clears one the
//!   key had (the client's CAS request carries none).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Errors of a [`RawStore`].
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    /// The TiKV raw client failed; its text unchanged.
    #[error("tikv raw: {0}")]
    Tikv(String),
    /// The store's configuration cannot be used.
    #[error("raw store config: {0}")]
    Config(String),
}

/// A pair a batch or scan returns: the key (relative to the root) and its
/// value.
pub type RawPair = (Vec<u8>, Vec<u8>);

/// A key's time to live, as [`RawStore::ttl`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTtl {
    /// The key is absent (or expired).
    Absent,
    /// The key has no TTL.
    Persistent,
    /// The key expires in about this long (whole seconds on TiKV).
    Expires(Duration),
}

/// Single-key operations without transactions; see the module docs. Cheap to
/// clone.
#[derive(Clone)]
pub struct RawStore {
    root: Arc<[u8]>,
    backend: Arc<Backend>,
}

enum Backend {
    #[cfg(feature = "tikv")]
    Tikv {
        plain: loams_tikv::tikv_client::RawClient,
        atomic: loams_tikv::tikv_client::RawClient,
    },
    Memory(Mutex<BTreeMap<Vec<u8>, MemEntry>>),
}

struct MemEntry {
    value: Vec<u8>,
    expires: Option<Instant>,
}

impl MemEntry {
    fn live(&self, now: Instant) -> bool {
        self.expires.is_none_or(|at| at > now)
    }
}

impl std::fmt::Debug for RawStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let backend = match *self.backend {
            #[cfg(feature = "tikv")]
            Backend::Tikv { .. } => "tikv",
            Backend::Memory(_) => "memory",
        };
        f.debug_struct("RawStore")
            .field("backend", &backend)
            .field("root", &self.root)
            .finish()
    }
}

/// The first key after every key that starts with `prefix`; empty (no bound)
/// when there is none (an empty prefix, or all `0xff`).
fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last + 1);
            return end;
        }
    }
    Vec::new()
}

/// Whole seconds, rounded up, at least one.
fn ttl_secs(ttl: Duration) -> u64 {
    let secs = ttl.as_secs() + u64::from(ttl.subsec_nanos() > 0);
    secs.max(1)
}

#[cfg(feature = "tikv")]
fn tikv_err(e: loams_tikv::tikv_client::Error) -> RawError {
    RawError::Tikv(e.to_string())
}

impl RawStore {
    /// A raw store on the keyspace and root of `config`, creating the
    /// keyspace through PD's HTTP API when it is missing. The cluster must run
    /// API v2 (and `storage.enable-ttl = true` for TTLs).
    #[cfg(feature = "tikv")]
    pub async fn open(config: &loams_tikv::TikvConfig) -> Result<Self, RawError> {
        if config.keyspace.is_empty() {
            return Err(RawError::Config("a raw store needs a keyspace".into()));
        }
        loams_tikv::ensure_keyspace(&config.pd_http_url(), &config.keyspace)
            .await
            .map_err(|e| RawError::Config(e.to_string()))?;
        let client_config = loams_tikv::tikv_client::Config::default()
            .with_keyspace(&config.keyspace)
            .with_timeout(config.request_timeout);
        let plain =
            loams_tikv::tikv_client::RawClient::new_with_config(config.pd.clone(), client_config)
                .await
                .map_err(tikv_err)?;
        let atomic = plain.with_atomic_for_cas();
        Ok(RawStore {
            root: config.root.clone().into(),
            backend: Arc::new(Backend::Tikv { plain, atomic }),
        })
    }

    /// An in-memory raw store under `root`, for tests: the same semantics,
    /// TTLs included, gone with the last handle.
    pub fn memory(root: impl Into<Vec<u8>>) -> Self {
        RawStore {
            root: root.into().into(),
            backend: Arc::new(Backend::Memory(Mutex::new(BTreeMap::new()))),
        }
    }

    /// The root every key lives under.
    pub fn root(&self) -> &[u8] {
        &self.root
    }

    /// The same store under another root.
    pub fn with_root(&self, root: impl Into<Vec<u8>>) -> Self {
        RawStore {
            root: root.into().into(),
            backend: Arc::clone(&self.backend),
        }
    }

    fn full(&self, key: &[u8]) -> Vec<u8> {
        let mut k = Vec::with_capacity(self.root.len() + key.len());
        k.extend_from_slice(&self.root);
        k.extend_from_slice(key);
        k
    }

    fn strip(&self, key: &[u8]) -> Vec<u8> {
        key.strip_prefix(&*self.root).unwrap_or(key).to_vec()
    }

    fn mem(&self) -> Option<std::sync::MutexGuard<'_, BTreeMap<Vec<u8>, MemEntry>>> {
        match &*self.backend {
            Backend::Memory(m) => Some(m.lock().unwrap_or_else(|e| e.into_inner())),
            #[cfg(feature = "tikv")]
            Backend::Tikv { .. } => None,
        }
    }

    /// Reads `key`.
    pub async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, RawError> {
        let k = self.full(key);
        if let Some(m) = self.mem() {
            let now = Instant::now();
            return Ok(m.get(&k).filter(|e| e.live(now)).map(|e| e.value.clone()));
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { plain, .. } = &*self.backend {
            return plain.get(k).await.map_err(tikv_err);
        }
        unreachable!("every backend is handled above")
    }

    /// Reads `keys`; absent keys are left out. Not a snapshot: each key is
    /// read at its own moment.
    pub async fn batch_get<K: AsRef<[u8]>>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Vec<RawPair>, RawError> {
        let full: Vec<Vec<u8>> = keys.into_iter().map(|k| self.full(k.as_ref())).collect();
        if let Some(m) = self.mem() {
            let now = Instant::now();
            return Ok(full
                .iter()
                .filter_map(|k| {
                    m.get(k)
                        .filter(|e| e.live(now))
                        .map(|e| (self.strip(k), e.value.clone()))
                })
                .collect());
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { plain, .. } = &*self.backend {
            let pairs = plain.batch_get(full).await.map_err(tikv_err)?;
            return Ok(pairs
                .into_iter()
                .map(|p| {
                    let (k, v): (Vec<u8>, Vec<u8>) = (p.0.into(), p.1);
                    (self.strip(&k), v)
                })
                .collect());
        }
        unreachable!("every backend is handled above")
    }

    /// Writes `value` at `key`, with a TTL or none.
    pub async fn put(
        &self,
        key: &[u8],
        value: impl Into<Vec<u8>>,
        ttl: Option<Duration>,
    ) -> Result<(), RawError> {
        let (k, value) = (self.full(key), value.into());
        if let Some(mut m) = self.mem() {
            let expires = ttl.map(|t| Instant::now() + t);
            m.insert(k, MemEntry { value, expires });
            return Ok(());
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { atomic, .. } = &*self.backend {
            return match ttl {
                Some(t) => atomic.put_with_ttl(k, value, ttl_secs(t)).await,
                None => atomic.put(k, value).await,
            }
            .map_err(tikv_err);
        }
        unreachable!("every backend is handled above")
    }

    /// Writes every pair, each with the same TTL or none. Not atomic across
    /// regions (see the module docs).
    pub async fn batch_put(
        &self,
        pairs: impl IntoIterator<Item = RawPair>,
        ttl: Option<Duration>,
    ) -> Result<(), RawError> {
        let pairs: Vec<RawPair> = pairs.into_iter().map(|(k, v)| (self.full(&k), v)).collect();
        if let Some(mut m) = self.mem() {
            let expires = ttl.map(|t| Instant::now() + t);
            for (k, value) in pairs {
                m.insert(k, MemEntry { value, expires });
            }
            return Ok(());
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { atomic, .. } = &*self.backend {
            return match ttl {
                Some(t) => {
                    let secs = ttl_secs(t);
                    let ttls = vec![secs; pairs.len()];
                    atomic.batch_put_with_ttl(pairs, ttls).await
                }
                None => atomic.batch_put(pairs).await,
            }
            .map_err(tikv_err);
        }
        unreachable!("every backend is handled above")
    }

    /// Deletes `key` (nothing when it is absent).
    pub async fn delete(&self, key: &[u8]) -> Result<(), RawError> {
        let k = self.full(key);
        if let Some(mut m) = self.mem() {
            m.remove(&k);
            return Ok(());
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { atomic, .. } = &*self.backend {
            return atomic.delete(k).await.map_err(tikv_err);
        }
        unreachable!("every backend is handled above")
    }

    /// Deletes `keys`, one atomic-mode delete each, concurrently (atomic mode
    /// refuses TiKV's batch delete). The first error is returned after every
    /// delete ran.
    pub async fn batch_delete<K: AsRef<[u8]>>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<(), RawError> {
        let keys: Vec<Vec<u8>> = keys.into_iter().map(|k| k.as_ref().to_vec()).collect();
        let results = futures::future::join_all(keys.iter().map(|k| self.delete(k))).await;
        results.into_iter().collect()
    }

    /// Writes `new` at `key` if its value is `expected` (`None`: absent), in
    /// one step. Returns the value seen and whether it swapped. The write has
    /// no TTL, so it clears one the key had.
    ///
    /// An outcome TiKV cannot determine (a lost reply) is an error, not a
    /// retry: a retried CAS that had applied would report a failure.
    pub async fn compare_and_swap(
        &self,
        key: &[u8],
        expected: Option<&[u8]>,
        new: impl Into<Vec<u8>>,
    ) -> Result<(Option<Vec<u8>>, bool), RawError> {
        let (k, new) = (self.full(key), new.into());
        if let Some(mut m) = self.mem() {
            let now = Instant::now();
            let seen = m.get(&k).filter(|e| e.live(now)).map(|e| e.value.clone());
            if seen.as_deref() == expected {
                m.insert(
                    k,
                    MemEntry {
                        value: new,
                        expires: None,
                    },
                );
                return Ok((seen, true));
            }
            return Ok((seen, false));
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { atomic, .. } = &*self.backend {
            return atomic
                .compare_and_swap(k, expected.map(<[u8]>::to_vec), new)
                .await
                .map_err(tikv_err);
        }
        unreachable!("every backend is handled above")
    }

    /// The key's time to live.
    pub async fn ttl(&self, key: &[u8]) -> Result<RawTtl, RawError> {
        let k = self.full(key);
        if let Some(m) = self.mem() {
            let now = Instant::now();
            return Ok(match m.get(&k).filter(|e| e.live(now)) {
                None => RawTtl::Absent,
                Some(MemEntry { expires: None, .. }) => RawTtl::Persistent,
                Some(MemEntry {
                    expires: Some(at), ..
                }) => RawTtl::Expires(at.saturating_duration_since(now)),
            });
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { plain, .. } = &*self.backend {
            return Ok(match plain.get_key_ttl_secs(k).await.map_err(tikv_err)? {
                None => RawTtl::Absent,
                Some(0) => RawTtl::Persistent,
                Some(secs) => RawTtl::Expires(Duration::from_secs(secs)),
            });
        }
        unreachable!("every backend is handled above")
    }

    /// Up to `limit` pairs with keys in `start..end` (`end` `None`: to the end
    /// of the root), in key order.
    pub async fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: u32,
    ) -> Result<Vec<RawPair>, RawError> {
        let from = self.full(start);
        let to = match end {
            Some(e) => self.full(e),
            None => prefix_end(&self.root),
        };
        if let Some(m) = self.mem() {
            let now = Instant::now();
            let range = m.range(from..);
            return Ok(range
                .take_while(|(k, _)| to.is_empty() || **k < to)
                .filter(|(_, e)| e.live(now))
                .take(limit as usize)
                .map(|(k, e)| (self.strip(k), e.value.clone()))
                .collect());
        }
        #[cfg(feature = "tikv")]
        if let Backend::Tikv { plain, .. } = &*self.backend {
            let range: loams_tikv::tikv_client::BoundRange = if to.is_empty() {
                (from..).into()
            } else {
                (from..to).into()
            };
            let pairs = plain.scan(range, limit).await.map_err(tikv_err)?;
            return Ok(pairs
                .into_iter()
                .map(|p| {
                    let (k, v): (Vec<u8>, Vec<u8>) = (p.0.into(), p.1);
                    (self.strip(&k), v)
                })
                .collect());
        }
        unreachable!("every backend is handled above")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_end_is_the_next_prefix() {
        assert_eq!(prefix_end(b"ab"), b"ac".to_vec());
        assert_eq!(prefix_end(b"a\xff"), b"b".to_vec());
        assert_eq!(prefix_end(b"\xff\xff"), Vec::<u8>::new());
        assert_eq!(prefix_end(b""), Vec::<u8>::new());
    }

    #[test]
    fn ttl_rounds_up_to_whole_seconds() {
        assert_eq!(ttl_secs(Duration::from_millis(1)), 1);
        assert_eq!(ttl_secs(Duration::from_millis(1000)), 1);
        assert_eq!(ttl_secs(Duration::from_millis(1001)), 2);
        assert_eq!(ttl_secs(Duration::ZERO), 1);
    }
}
