//! The static config (design §57 §9.3): what changes only with a restart.
//! Routes, which change at runtime, are [`crate::routes`].

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// `gateway.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    #[serde(default)]
    pub admin: AdminConfig,
    pub routes: RoutesConfig,
    #[serde(default)]
    pub pg: PgConfig,
    /// The RisingWave module (design 81, D1957).
    #[serde(default)]
    pub risingwave: crate::risingwave::RwConfig,
}

impl GatewayConfig {
    /// Parses and checks a config file's text.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: GatewayConfig =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.pg.check()?;
        for cert in &config.risingwave.certs {
            if cert.names.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "risingwave.certs entry {} names no hostname",
                    cert.cert.display()
                )));
            }
        }
        Ok(config)
    }
}

/// The admin listener (`/healthz`, `/readyz`, `/v1/routes`). Never exposed
/// by the gateway's Service.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    #[serde(default = "default_admin_listen")]
    pub listen: SocketAddr,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            listen: default_admin_listen(),
        }
    }
}

/// Where the routes file is, and how often it is read (D1018).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutesConfig {
    pub path: PathBuf,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// Refuse a routes file without its `generation` line (the default;
    /// `pg-control` always writes it). Off only for hand-written local files.
    #[serde(default = "default_true")]
    pub require_generation: bool,
}

impl RoutesConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms.max(10))
    }
}

/// How the Postgres module treats TLS (D1014, §57 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Splice `SSLRequest` clients, terminate direct-TLS clients.
    #[default]
    Auto,
    /// Terminate both.
    Terminate,
    /// Splice both; direct-TLS clients are refused (PgDog negotiates no
    /// ALPN, so they could not finish).
    Passthrough,
}

/// The Postgres module.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PgConfig {
    /// The shared port, routed by SNI.
    #[serde(default = "default_pg_listen")]
    pub listen: SocketAddr,
    /// Where dedicated namespace ports are bound (D1016).
    #[serde(default = "default_dedicated_bind")]
    pub dedicated_bind: IpAddr,
    /// The range a route's `port` must fall in, as `"16000-17999"`.
    #[serde(default)]
    pub dedicated_ports: PortRange,
    #[serde(default)]
    pub tls_mode: TlsMode,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    #[serde(default = "default_upstream_connect_timeout_ms")]
    pub upstream_connect_timeout_ms: u64,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
    /// Certificates for the terminate path: copies of PgDog's own
    /// certificates (SCRAM channel binding hashes the one the client sees),
    /// with the names each covers.
    #[serde(default)]
    pub certs: Vec<CertConfig>,
    /// Trust for PgDog's certificate on the terminate path.
    #[serde(default)]
    pub upstream_tls: Option<UpstreamTlsConfig>,
}

impl Default for PgConfig {
    fn default() -> Self {
        Self {
            listen: default_pg_listen(),
            dedicated_bind: default_dedicated_bind(),
            dedicated_ports: PortRange::default(),
            tls_mode: TlsMode::default(),
            handshake_timeout_ms: default_handshake_timeout_ms(),
            upstream_connect_timeout_ms: default_upstream_connect_timeout_ms(),
            max_connections: default_max_connections(),
            drain_timeout_ms: default_drain_timeout_ms(),
            certs: Vec::new(),
            upstream_tls: None,
        }
    }
}

impl PgConfig {
    pub fn handshake_timeout(&self) -> Duration {
        Duration::from_millis(self.handshake_timeout_ms)
    }

    pub fn upstream_connect_timeout(&self) -> Duration {
        Duration::from_millis(self.upstream_connect_timeout_ms)
    }

    pub fn drain_timeout(&self) -> Duration {
        Duration::from_millis(self.drain_timeout_ms)
    }

    /// The terminate path needs a certificate and upstream trust; the
    /// splice-only mode needs neither.
    fn check(&self) -> Result<(), ConfigError> {
        if self.max_connections == 0 {
            return Err(ConfigError::Invalid(
                "pg.max_connections must be > 0".into(),
            ));
        }
        if self.tls_mode != TlsMode::Passthrough {
            if self.certs.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "pg.tls_mode = {:?} terminates TLS and needs at least one [[pg.certs]]",
                    self.tls_mode
                )));
            }
            if self.upstream_tls.is_none() {
                return Err(ConfigError::Invalid(format!(
                    "pg.tls_mode = {:?} re-encrypts to PgDog and needs [pg.upstream_tls]",
                    self.tls_mode
                )));
            }
        }
        for cert in &self.certs {
            if cert.names.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "pg.certs entry {} names no hostname",
                    cert.cert.display()
                )));
            }
        }
        Ok(())
    }
}

/// One certificate and the names it serves (exact, or one wildcard label).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertConfig {
    pub names: Vec<String>,
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The CA that PgDog's certificates chain to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTlsConfig {
    pub ca: PathBuf,
}

/// An inclusive port range written `"<first>-<last>"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct PortRange(pub RangeInclusive<u16>);

impl Default for PortRange {
    fn default() -> Self {
        Self(16000..=17999)
    }
}

impl TryFrom<String> for PortRange {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| format!("port range {s:?} is not <first>-<last>"))?;
        let first: u16 = a.trim().parse().map_err(|_| format!("bad port {a:?}"))?;
        let last: u16 = b.trim().parse().map_err(|_| format!("bad port {b:?}"))?;
        if first == 0 || first > last {
            return Err(format!("port range {s:?} is empty or starts at 0"));
        }
        Ok(Self(first..=last))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config: {0}")]
    Parse(String),
    #[error("config: {0}")]
    Invalid(String),
}

fn default_true() -> bool {
    true
}
fn default_admin_listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9187)
}
fn default_pg_listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 5432)
}
fn default_dedicated_bind() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}
fn default_poll_interval_ms() -> u64 {
    1000
}
fn default_handshake_timeout_ms() -> u64 {
    10_000
}
fn default_upstream_connect_timeout_ms() -> u64 {
    2_000
}
fn default_max_connections() -> usize {
    20_000
}
fn default_drain_timeout_ms() -> u64 {
    60_000
}
