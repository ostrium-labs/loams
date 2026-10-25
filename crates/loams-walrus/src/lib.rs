//! The Walrus fast path (MS1 Task 4; D1810, D1811).
//!
//! Walrus is a public, read-only, verifiable tier for public agent
//! artifacts, a read-only project knowledge cache and a build cache. The
//! bucket stays the only source of truth (D1): Walrus is never a `Store`
//! backend, and nothing here writes to the bucket.
//!
//! The fast path speaks HTTP only, to aggregators (reads) and publishers
//! (writes). There is no Sui SDK, no Walrus SDK and no wallet.
//!
//! - [`Walrus::read`], [`Walrus::read_quilt_patch`], [`Walrus::read_patch`]
//!   and [`Walrus::read_to_path`] are hedged across the configured
//!   aggregators and check every byte against the caller's sha256 before
//!   returning it. An aggregator is untrusted.
//! - [`Walrus::read_range`] is the one unverified read; it returns
//!   [`Unverified`].
//! - [`Walrus::store`] and [`Walrus::store_quilt`] write through a
//!   publisher, refuse bodies over `max_blob_bytes` before uploading, and
//!   read the result back verified before answering `Ok`. A `404` within
//!   `read_after_write_ms` of the write (a CDN's or a lagging aggregator's
//!   stale answer) is retried with backoff.
//! - [`VerifiedDiskCache`] keeps verified blobs on disk by sha256.
//!
//! Everything is off by default: a client built from a config with
//! `enabled = false` answers [`WalrusError::Disabled`] to every call
//! without any I/O.

mod cache;
mod config;
mod error;
mod hedge;
mod read;
mod tls;
mod types;
mod write;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use url::Url;

pub use cache::VerifiedDiskCache;
pub use config::{
    CACHE_DIR_NAME, DEFAULT_CACHE_MAX_BYTES, DEFAULT_MAX_BLOB_BYTES, DEFAULT_READ_AFTER_WRITE_MS,
    MAINNET_AGGREGATOR, Network, Secret, TESTNET_AGGREGATOR, TESTNET_PUBLISHER, WalrusConfig,
};
pub use error::{AttemptFailure, WalrusError};
pub use types::{
    BlobId, ContentDigest, PatchEntry, QuiltId, QuiltPatchId, StoreOpts, Stored, StoredQuilt,
    Unverified,
};

/// The first retry delay of a read just after a write; it doubles up to
/// `RETRY_MAX` (ruling 1 of PR #1232).
const RETRY_START: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);

/// The Walrus fast path: hedged, verified reads from aggregators, writes
/// through a publisher, and an optional [`VerifiedDiskCache`].
#[derive(Clone, Debug)]
pub struct Walrus {
    inner: Option<Arc<Inner>>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    aggregators: Vec<Url>,
    publisher: Option<Url>,
    auth: Option<Secret>,
    hedge_after: Duration,
    timeout: Duration,
    max_blob_bytes: u64,
    epochs: u32,
    /// How long after this client's write a `404` is retried
    /// (`read_after_write_ms`).
    write_window: Duration,
    cache: Option<VerifiedDiskCache>,
    /// Blob and patch ids this client wrote, and when.
    writes: Mutex<HashMap<String, Instant>>,
}

impl Walrus {
    /// A client for `cfg`. With `enabled = false` it touches nothing (no
    /// cache directory, no token file, no network) and every call answers
    /// [`WalrusError::Disabled`].
    pub async fn new(cfg: &WalrusConfig) -> Result<Self, WalrusError> {
        if !cfg.enabled {
            return Ok(Self { inner: None });
        }
        if cfg.epochs == 0 {
            return Err(WalrusError::Config("epochs must be at least 1".into()));
        }
        let auth = match &cfg.publisher_auth {
            Some(path) => {
                let token = tokio::fs::read_to_string(path).await?;
                let token = token.trim();
                if token.is_empty() {
                    return Err(WalrusError::Config(format!(
                        "publisher_auth file {} is empty",
                        path.display()
                    )));
                }
                Some(Secret::new(token))
            }
            None => None,
        };
        tls::ensure_provider();
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| WalrusError::Config(format!("HTTP client: {e}")))?;
        let cache = match &cfg.cache_dir {
            Some(dir) => Some(VerifiedDiskCache::open(dir, cfg.cache_max_bytes).await?),
            None => None,
        };
        Ok(Self {
            inner: Some(Arc::new(Inner {
                http,
                aggregators: cfg.effective_aggregators(),
                publisher: cfg.effective_publisher(),
                auth,
                hedge_after: cfg.hedge_after(),
                timeout: cfg.request_timeout(),
                max_blob_bytes: cfg.max_blob_bytes,
                epochs: cfg.epochs,
                write_window: cfg.read_after_write(),
                cache,
                writes: Mutex::new(HashMap::new()),
            })),
        })
    }

    /// Whether the fast path is on.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether writes have a publisher to go to.
    pub fn can_write(&self) -> bool {
        self.inner.as_ref().is_some_and(|i| i.publisher.is_some())
    }

    /// The disk cache, if one is configured.
    pub fn cache(&self) -> Option<&VerifiedDiskCache> {
        self.inner.as_ref().and_then(|i| i.cache.as_ref())
    }

    fn inner(&self) -> Result<&Inner, WalrusError> {
        self.inner.as_deref().ok_or(WalrusError::Disabled)
    }
}

impl Inner {
    fn note_write(&self, key: &str) {
        let now = Instant::now();
        let mut writes = self.writes.lock().unwrap_or_else(|p| p.into_inner());
        writes.retain(|_, at| now.duration_since(*at) < self.write_window);
        writes.insert(key.to_string(), now);
    }

    /// When `key` was written by this client, if within the window.
    fn written_at(&self, key: &str) -> Option<Instant> {
        let writes = self.writes.lock().unwrap_or_else(|p| p.into_inner());
        writes
            .get(key)
            .copied()
            .filter(|at| at.elapsed() < self.write_window)
    }
}
