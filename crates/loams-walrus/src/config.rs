//! The `[walrus]` config section (MS1 plan, Task 4).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

/// Mysten Labs' public Testnet aggregator (docs.wal.app, Network
/// Reference). Testnet state can be wiped at any time.
pub const TESTNET_AGGREGATOR: &str = "https://aggregator.walrus-testnet.walrus.space";
/// Mysten Labs' public, unauthenticated Testnet publisher.
pub const TESTNET_PUBLISHER: &str = "https://publisher.walrus-testnet.walrus.space";
/// Mysten Labs' public Mainnet aggregator. Mainnet has no public publisher
/// (Q1330).
pub const MAINNET_AGGREGATOR: &str = "https://aggregator.walrus-mainnet.walrus.space";

/// The public endpoints' request limit, 10 MiB.
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 10 << 20;
/// How long after a write a `404` is retried by default: 3 minutes, since
/// public Testnet aggregators were seen to lag past 60 s (ruling 1 of PR
/// #1232).
pub const DEFAULT_READ_AFTER_WRITE_MS: u64 = 180_000;
/// The disk cache's default bound, 4 GiB.
pub const DEFAULT_CACHE_MAX_BYTES: u64 = 4 << 30;
/// The cache directory's name under the data directory.
pub const CACHE_DIR_NAME: &str = "walrus-cache";

/// Which Walrus network: a label, and the source of the default endpoints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    /// Testnet (the default; testnet first, D1811).
    #[default]
    Testnet,
    /// Mainnet: reads only unless an own publisher is configured.
    Mainnet,
}

impl Network {
    /// The network's public aggregator.
    pub fn default_aggregator(self) -> Url {
        match self {
            Self::Testnet => constant_url(TESTNET_AGGREGATOR),
            Self::Mainnet => constant_url(MAINNET_AGGREGATOR),
        }
    }

    /// The network's public publisher, if it has one.
    pub fn default_publisher(self) -> Option<Url> {
        match self {
            Self::Testnet => Some(constant_url(TESTNET_PUBLISHER)),
            Self::Mainnet => None,
        }
    }
}

/// The `[walrus]` section. Off by default; nothing is contacted while
/// `enabled` is false.
///
/// ```toml
/// [walrus]
/// enabled = false
/// network = "testnet"          # "testnet" | "mainnet"
/// aggregators = ["https://aggregator.walrus-testnet.walrus.space"]
/// publisher = "https://publisher.walrus-testnet.walrus.space"
/// publisher_auth = ""          # bearer token file for an own publisher
/// epochs = 5
/// hedge_after_ms = 300
/// request_timeout_ms = 30000
/// max_blob_bytes = 10485760
/// read_after_write_ms = 180000   # retry a 404 this long after a write
/// cache_dir = ""               # default: <data dir>/walrus-cache
/// cache_max_bytes = 4294967296
/// ```
///
/// An empty `aggregators` list means the network's public aggregator; an
/// unset or empty `publisher` means the network's public publisher, which
/// Mainnet does not have (so a Mainnet config without one is read-only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WalrusConfig {
    /// Whether the fast path runs at all. `false` by default.
    pub enabled: bool,
    /// The network.
    pub network: Network,
    /// Aggregators, in order of preference: a read starts on the first.
    pub aggregators: Vec<Url>,
    /// The publisher stores go to.
    #[serde(deserialize_with = "empty_is_none")]
    pub publisher: Option<Url>,
    /// A file holding a bearer token (JWT) for an own publisher (Q1330).
    #[serde(deserialize_with = "empty_is_none")]
    pub publisher_auth: Option<PathBuf>,
    /// The default storage epochs of a write.
    pub epochs: u32,
    /// How long a read waits on one aggregator before also asking the next.
    pub hedge_after_ms: u64,
    /// The deadline of one HTTP request, body included.
    pub request_timeout_ms: u64,
    /// The largest blob read or written.
    pub max_blob_bytes: u64,
    /// How long after this client's own write a `404` is retried, with
    /// exponential backoff capped at 10 s (ruling 1 of PR #1232).
    pub read_after_write_ms: u64,
    /// Where verified blobs are cached; see [`WalrusConfig::with_data_dir`].
    /// No cache when unset.
    #[serde(deserialize_with = "empty_is_none")]
    pub cache_dir: Option<PathBuf>,
    /// The cache's size bound; least recently read blobs go first.
    pub cache_max_bytes: u64,
}

impl Default for WalrusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            network: Network::Testnet,
            aggregators: Vec::new(),
            publisher: None,
            publisher_auth: None,
            epochs: 5,
            hedge_after_ms: 300,
            request_timeout_ms: 30_000,
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
            read_after_write_ms: DEFAULT_READ_AFTER_WRITE_MS,
            cache_dir: None,
            cache_max_bytes: DEFAULT_CACHE_MAX_BYTES,
        }
    }
}

impl WalrusConfig {
    /// Testnet's public aggregator and publisher, enabled, no cache.
    pub fn testnet() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    /// Fills an unset `cache_dir` with `<data_dir>/walrus-cache`.
    pub fn with_data_dir(mut self, data_dir: &Path) -> Self {
        if self.cache_dir.is_none() {
            self.cache_dir = Some(data_dir.join(CACHE_DIR_NAME));
        }
        self
    }

    /// The aggregators reads use: the configured ones, else the network's.
    pub fn effective_aggregators(&self) -> Vec<Url> {
        if self.aggregators.is_empty() {
            vec![self.network.default_aggregator()]
        } else {
            self.aggregators.clone()
        }
    }

    /// The publisher writes use: the configured one, else the network's.
    pub fn effective_publisher(&self) -> Option<Url> {
        self.publisher
            .clone()
            .or_else(|| self.network.default_publisher())
    }

    /// `hedge_after_ms` as a duration.
    pub fn hedge_after(&self) -> Duration {
        Duration::from_millis(self.hedge_after_ms)
    }

    /// `read_after_write_ms` as a duration.
    pub fn read_after_write(&self) -> Duration {
        Duration::from_millis(self.read_after_write_ms)
    }

    /// `request_timeout_ms` as a duration.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
}

fn constant_url(s: &str) -> Url {
    // The constants above are valid URLs (checked by a unit test).
    Url::parse(s).unwrap_or_else(|e| unreachable!("constant URL {s}: {e}"))
}

/// `""` and an absent key both mean unset.
fn empty_is_none<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

/// A value that must never be printed (the publisher's token). `Debug`
/// prints `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps `value`.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that must use it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off_and_on_testnet() {
        let c = WalrusConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.network, Network::Testnet);
        assert_eq!(
            c.effective_aggregators()[0].as_str(),
            format!("{TESTNET_AGGREGATOR}/")
        );
        assert_eq!(
            c.effective_publisher().as_ref().map(Url::as_str),
            Some(format!("{TESTNET_PUBLISHER}/").as_str())
        );
        assert_eq!(c.max_blob_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn mainnet_has_no_default_publisher() {
        let c = WalrusConfig {
            network: Network::Mainnet,
            ..WalrusConfig::default()
        };
        assert_eq!(
            c.effective_aggregators()[0].as_str(),
            format!("{MAINNET_AGGREGATOR}/")
        );
        assert_eq!(c.effective_publisher(), None);
    }

    #[test]
    fn secret_is_redacted() {
        assert_eq!(format!("{:?}", Secret::new("eyJhbGciOi")), "[redacted]");
    }
}
