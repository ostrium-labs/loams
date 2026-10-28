//! The configuration surface of the web bridge: which provider runs, where it
//! finds its engine, what it is allowed to open, which secret fills it may
//! make, how much browser time it may spend and whether it records.
//!
//! One engine, Obscura, sits behind both providers (D2110, D2111):
//!
//! * `local` launches `obscura serve` on loopback on the machine the agent
//!   runs on (agentd or Desktop);
//! * `service` connects to the same engine image running as the `browser`
//!   service of a self-hosted or cloud deployment (deploy/browser).
//!
//! Everything else — the network policy, the credential boundary, the budgets
//! and recording — is one section with one set of defaults, applied the same
//! way whichever provider is selected (D2112). The defaults are the
//! conservative ones: private hosts are refused, secret fills are off and
//! recording is off.
//!
//! Cloudflare Browser Run, the remote provider this file used to configure, was
//! removed on 2026-10-27 (D2110).

use std::path::PathBuf;
use std::time::Duration;

use url::{Host, Url};

use crate::egress::{EgressPolicy, HostPattern, MAX_ALLOWED_DOMAINS};
use crate::error::ConfigError;
use crate::secret::SecretRef;

/// The provider the bridge uses (D565: configuration decides, not a fork).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// The browser runs on the machine the agent runs on: an Obscura process
    /// agentd or Desktop launches on loopback, or a host's own page driver.
    #[default]
    Local,
    /// The browser is the deployment's `browser` service: the same Obscura
    /// engine, reached over the private network with a bearer token.
    Service,
}

impl ProviderKind {
    /// The name as written in configuration.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Local => "local",
            ProviderKind::Service => "service",
        }
    }

    /// Parse a configured name.
    pub fn parse(field: &'static str, raw: &str) -> Result<Self, ConfigError> {
        match raw.trim() {
            "local" => Ok(ProviderKind::Local),
            "service" => Ok(ProviderKind::Service),
            "browser-run" => Err(ConfigError::Invalid {
                field,
                reason: "the Cloudflare Browser Run provider was removed (D2110); use local or \
                         service"
                    .to_string(),
            }),
            other => Err(ConfigError::Invalid {
                field,
                reason: format!("{other:?} is not local or service"),
            }),
        }
    }
}

/// The Obscura release every provider runs (D2113). Kept equal to
/// `loams_sidebar_browser::engine::PINNED_ENGINE_VERSION` and to the image tag
/// in deploy/browser, so local and cloud run the same build.
pub const PINNED_OBSCURA_VERSION: &str = "0.2.3";

/// Where the `service` provider finds the engine by default: the compose and
/// Kubernetes service name, on the private network.
pub const DEFAULT_SERVICE_URL: &str = "ws://browser:9222/devtools/browser";

/// The environment variable that names the Obscura binary for `local`.
pub const OBSCURA_BIN_ENV: &str = "LOAMS_OBSCURA_BIN";

/// The browser engine behind a provider (D627, D2110): Obscura, everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// Obscura (Apache-2.0), driven over the Chrome DevTools Protocol.
    #[default]
    Obscura,
}

impl Engine {
    /// The engine's name, for messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::Obscura => "obscura",
        }
    }
}

/// Which secret fills the bridge may make, and where (D567).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialPolicy {
    /// Whether `fill` may resolve a secret at all. Off by default on every
    /// provider; when on, a secret is typed only into a host listed below.
    #[serde(default)]
    pub allow_secret_fills: bool,

    /// The hosts a secret may be typed into. Empty means none.
    #[serde(default)]
    pub allowed_fill_hosts: Vec<String>,
}

impl CredentialPolicy {
    /// Whether a secret may be typed into `host`.
    pub fn allows_fill_on(&self, host: &str) -> bool {
        self.allowed_fill_hosts
            .iter()
            .filter_map(|raw| HostPattern::parse(raw).ok())
            .any(|pattern| pattern.matches(host))
    }
}

/// The local provider's settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    /// The profile a session opens by default, named `<environment>/<site>` as
    /// §18.14.4 D506 has it.
    #[serde(default)]
    pub default_profile: Option<String>,

    /// The Obscura binary. `None` reads `LOAMS_OBSCURA_BIN`, then `obscura` on
    /// `PATH`.
    #[serde(default)]
    pub obscura_bin: Option<PathBuf>,

    /// How long the launched engine may take to accept a connection.
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
}

fn default_startup_timeout_ms() -> u64 {
    30_000
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            default_profile: None,
            obscura_bin: None,
            startup_timeout_ms: default_startup_timeout_ms(),
        }
    }
}

impl LocalConfig {
    /// The binary to launch.
    pub fn program(&self) -> PathBuf {
        self.obscura_bin
            .clone()
            .or_else(|| env_var(OBSCURA_BIN_ENV).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("obscura"))
    }

    /// The startup timeout.
    pub fn startup_timeout(&self) -> Duration {
        Duration::from_millis(self.startup_timeout_ms)
    }
}

/// The `service` provider's settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    /// The engine's DevTools WebSocket URL.
    #[serde(default = "default_service_url")]
    pub url: String,

    /// Where the engine's bearer token (`OBSCURA_CDP_TOKEN`) lives. Required
    /// unless the URL is loopback: the engine refuses a non-loopback bind
    /// without one.
    #[serde(default)]
    pub token_secret_ref: Option<String>,
}

fn default_service_url() -> String {
    DEFAULT_SERVICE_URL.to_string()
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            url: default_service_url(),
            token_secret_ref: None,
        }
    }
}

impl ServiceConfig {
    /// The parsed token reference.
    pub fn token_ref(&self) -> Result<Option<SecretRef>, ConfigError> {
        match &self.token_secret_ref {
            None => Ok(None),
            Some(raw) => SecretRef::parse(raw)
                .map(Some)
                .map_err(|error| ConfigError::Invalid {
                    field: "service.token_secret_ref",
                    reason: error.to_string(),
                }),
        }
    }

    /// Check the URL: `ws://` only where the token cannot cross a network the
    /// operator does not own, `wss://` anywhere else.
    pub fn check(&self) -> Result<Url, ConfigError> {
        let url = Url::parse(&self.url).map_err(|error| ConfigError::Invalid {
            field: "service.url",
            reason: format!("{error}"),
        })?;
        let host = url.host().ok_or_else(|| ConfigError::Invalid {
            field: "service.url",
            reason: "the url has no host".to_string(),
        })?;
        match url.scheme() {
            "wss" => {}
            "ws" if is_private_network_host(&host) => {}
            "ws" => {
                return Err(ConfigError::Invalid {
                    field: "service.url",
                    reason: format!(
                        "ws:// would send the engine's token in clear to {host}; use wss:// or \
                         reach the service over the private network"
                    ),
                });
            }
            other => {
                return Err(ConfigError::Invalid {
                    field: "service.url",
                    reason: format!("{other}:// is not ws:// or wss://"),
                });
            }
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ConfigError::Invalid {
                field: "service.url",
                reason: "credentials go in service.token_secret_ref, never in the url".to_string(),
            });
        }
        if self.token_secret_ref.is_none() && !is_loopback_host(&host) {
            return Err(ConfigError::Missing {
                field: "service.token_secret_ref",
            });
        }
        self.token_ref()?;
        Ok(url)
    }
}

/// Whether a host is loopback.
fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

/// Whether a host is on a network the operator owns: loopback, a private
/// address, a single-label name (a compose service), or a cluster or internal
/// DNS suffix.
fn is_private_network_host(host: &Host<&str>) -> bool {
    if is_loopback_host(host) {
        return true;
    }
    match host {
        Host::Domain(domain) => {
            let domain = domain.to_ascii_lowercase();
            !domain.contains('.')
                || [".svc", ".cluster.local", ".internal", ".local", ".lan"]
                    .iter()
                    .any(|suffix| domain.ends_with(suffix))
        }
        Host::Ipv4(address) => address.is_private() || address.is_link_local(),
        Host::Ipv6(address) => (address.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// How much browser a provider may use. One set of defaults for every
/// provider (D2112).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    /// How many pages may be open at once.
    #[serde(default = "default_max_concurrent_sessions")]
    pub max_concurrent_sessions: usize,
    /// How long one CDP call may take, in milliseconds.
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// How long one page may stay open, in milliseconds.
    #[serde(default = "default_max_session_ms")]
    pub max_session_ms: u64,
    /// The ceiling on browser time per UTC day, in milliseconds. `0` is off.
    #[serde(default)]
    pub daily_budget_ms: u64,
}

fn default_max_concurrent_sessions() -> usize {
    4
}

fn default_request_timeout_ms() -> u64 {
    60_000
}

fn default_max_session_ms() -> u64 {
    600_000
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: default_max_concurrent_sessions(),
            request_timeout_ms: default_request_timeout_ms(),
            max_session_ms: default_max_session_ms(),
            daily_budget_ms: 0,
        }
    }
}

impl BudgetConfig {
    /// The timeout for one CDP call.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }

    /// How long one page may stay open.
    pub fn max_session(&self) -> Duration {
        Duration::from_millis(self.max_session_ms)
    }

    /// The daily ceiling, if there is one.
    pub fn daily_budget(&self) -> Option<Duration> {
        (self.daily_budget_ms > 0).then(|| Duration::from_millis(self.daily_budget_ms))
    }
}

/// Whether a provider records what its pages did. Off by default on every
/// provider (D2112).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingConfig {
    /// Write an action log, one JSON line per tool call, beside the page's
    /// artifacts. Values typed into a page are never recorded.
    #[serde(default)]
    pub enabled: bool,
}

/// The whole `[web_bridge]` section.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebBridgeConfig {
    /// Which provider to build.
    #[serde(default)]
    pub provider: ProviderKind,

    /// The local provider's settings.
    #[serde(default)]
    pub local: LocalConfig,

    /// The service provider's settings.
    #[serde(default)]
    pub service: ServiceConfig,

    /// How much browser time may be spent.
    #[serde(default)]
    pub budgets: BudgetConfig,

    /// Whether pages are recorded.
    #[serde(default)]
    pub recording: RecordingConfig,

    /// Where screenshots and action logs are written.
    #[serde(default)]
    pub artifact_dir: Option<PathBuf>,

    /// Hostnames to allow or refuse.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Hostnames to refuse even if allowed above.
    #[serde(default)]
    pub denied_domains: Vec<String>,
    /// Permit private and loopback addresses. Development only.
    #[serde(default)]
    pub allow_private_hosts: bool,

    /// Which secret fills are permitted.
    #[serde(default)]
    pub credentials: CredentialPolicy,
}

impl WebBridgeConfig {
    /// The local provider with the default policies: what a fresh install gets.
    pub fn local_default() -> Self {
        Self::default()
    }

    /// Where artifacts go when nothing says otherwise.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.artifact_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("loams-web-bridge"))
    }

    /// The egress policy the configuration describes.
    pub fn egress(&self) -> Result<EgressPolicy, ConfigError> {
        let mut policy = EgressPolicy::public_web();
        policy.set_allow_private_hosts(self.allow_private_hosts);
        for host in &self.allowed_domains {
            policy.allow(host)?;
        }
        for host in &self.denied_domains {
            policy.deny(host)?;
        }
        Ok(policy)
    }

    /// Read a configuration from TOML.
    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        toml::from_str(raw).map_err(|error| ConfigError::Toml(error.to_string()))
    }

    /// Overlay `LOAMS_BROWSER_*` environment variables (and the older
    /// `LOAMS_WEB_BRIDGE_*` names, which still work).
    pub fn apply_env(&mut self) -> Result<(), ConfigError> {
        self.apply_env_from(&|name| env_var(name))
    }

    /// Overlay environment variables from an arbitrary lookup.
    ///
    /// The environment names the provider, the service and the token's
    /// *reference*; it never carries a token, so an environment dump of
    /// agentd cannot leak one.
    pub fn apply_env_from(
        &mut self,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<(), ConfigError> {
        let either = |new: &str, old: &str| lookup(new).or_else(|| lookup(old));
        if let Some(raw) = lookup("LOAMS_BROWSER_PROVIDER") {
            self.provider = ProviderKind::parse("LOAMS_BROWSER_PROVIDER", &raw)?;
        } else if let Some(raw) = lookup("LOAMS_WEB_BRIDGE_PROVIDER") {
            self.provider = ProviderKind::parse("LOAMS_WEB_BRIDGE_PROVIDER", &raw)?;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_SERVICE_URL") {
            self.service.url = raw;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_SERVICE_TOKEN_REF") {
            self.service.token_secret_ref = Some(raw);
        }
        if let Some(raw) = lookup(OBSCURA_BIN_ENV) {
            self.local.obscura_bin = Some(PathBuf::from(raw));
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_MAX_SESSIONS") {
            self.budgets.max_concurrent_sessions =
                usize::try_from(parse_u64("LOAMS_BROWSER_MAX_SESSIONS", &raw)?)
                    .unwrap_or(usize::MAX);
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_REQUEST_TIMEOUT_MS") {
            self.budgets.request_timeout_ms = parse_u64("LOAMS_BROWSER_REQUEST_TIMEOUT_MS", &raw)?;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_MAX_SESSION_MS") {
            self.budgets.max_session_ms = parse_u64("LOAMS_BROWSER_MAX_SESSION_MS", &raw)?;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_DAILY_BUDGET_MS") {
            self.budgets.daily_budget_ms = parse_u64("LOAMS_BROWSER_DAILY_BUDGET_MS", &raw)?;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_RECORD") {
            self.recording.enabled = parse_bool("LOAMS_BROWSER_RECORD", &raw)?;
        }
        if let Some(raw) = lookup("LOAMS_BROWSER_ALLOW_PRIVATE_HOSTS") {
            self.allow_private_hosts = parse_bool("LOAMS_BROWSER_ALLOW_PRIVATE_HOSTS", &raw)?;
        }
        if let Some(raw) = either(
            "LOAMS_BROWSER_ARTIFACT_DIR",
            "LOAMS_WEB_BRIDGE_ARTIFACT_DIR",
        ) {
            self.artifact_dir = Some(PathBuf::from(raw));
        }
        if let Some(raw) = either(
            "LOAMS_BROWSER_ALLOWED_DOMAINS",
            "LOAMS_WEB_BRIDGE_ALLOWED_DOMAINS",
        ) {
            self.allowed_domains = split_list(&raw);
        }
        if let Some(raw) = either(
            "LOAMS_BROWSER_DENIED_DOMAINS",
            "LOAMS_WEB_BRIDGE_DENIED_DOMAINS",
        ) {
            self.denied_domains = split_list(&raw);
        }
        Ok(())
    }

    /// Read the configuration from TOML, overlay the environment and check it.
    pub fn load(raw_toml: &str) -> Result<Self, ConfigError> {
        let mut config = Self::from_toml_str(raw_toml)?;
        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    /// Check everything the provider will need, so a mistake is a startup
    /// error rather than a failed session.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.egress()?;
        if self.allowed_domains.len() > MAX_ALLOWED_DOMAINS {
            return Err(ConfigError::Invalid {
                field: "allowed_domains",
                reason: format!("at most {MAX_ALLOWED_DOMAINS} are accepted"),
            });
        }
        for host in &self.credentials.allowed_fill_hosts {
            HostPattern::parse(host).map_err(|error| ConfigError::Invalid {
                field: "credentials.allowed_fill_hosts",
                reason: error.to_string(),
            })?;
        }
        if self.credentials.allow_secret_fills && self.credentials.allowed_fill_hosts.is_empty() {
            return Err(ConfigError::Invalid {
                field: "credentials.allowed_fill_hosts",
                reason: "listing no host means no host may be typed into; \
                         allow_secret_fills has nothing to allow"
                    .to_string(),
            });
        }
        let budgets = &self.budgets;
        if budgets.max_concurrent_sessions == 0 {
            return Err(ConfigError::Invalid {
                field: "budgets.max_concurrent_sessions",
                reason: "must be at least 1".to_string(),
            });
        }
        if budgets.request_timeout_ms < 1_000 {
            return Err(ConfigError::Invalid {
                field: "budgets.request_timeout_ms",
                reason: "must be at least 1000".to_string(),
            });
        }
        if budgets.max_session_ms < budgets.request_timeout_ms {
            return Err(ConfigError::Invalid {
                field: "budgets.max_session_ms",
                reason: "must be at least budgets.request_timeout_ms".to_string(),
            });
        }
        if self.local.startup_timeout_ms == 0 {
            return Err(ConfigError::Invalid {
                field: "local.startup_timeout_ms",
                reason: "must be more than 0".to_string(),
            });
        }
        if self.provider == ProviderKind::Service {
            self.service.check()?;
        }
        Ok(())
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn parse_u64(field: &'static str, raw: &str) -> Result<u64, ConfigError> {
    raw.trim().parse::<u64>().map_err(|_| ConfigError::Invalid {
        field,
        reason: format!("{raw:?} is not a number"),
    })
}

fn parse_bool(field: &'static str, raw: &str) -> Result<bool, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(ConfigError::Invalid {
            field,
            reason: format!("{other:?} is not true or false"),
        }),
    }
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn the_default_is_the_local_provider_with_conservative_policies() {
        let config = WebBridgeConfig::default();
        assert_eq!(config.provider, ProviderKind::Local);
        assert!(!config.allow_private_hosts);
        assert!(!config.credentials.allow_secret_fills);
        assert!(!config.recording.enabled);
        assert_eq!(config.budgets.max_concurrent_sessions, 4);
        assert_eq!(config.budgets.daily_budget(), None);
        assert_eq!(config.service.url, DEFAULT_SERVICE_URL);
        config.validate().unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn the_removed_remote_provider_is_named_when_asked_for() {
        let mut config = WebBridgeConfig::default();
        let error = config
            .apply_env_from(&env(&[("LOAMS_BROWSER_PROVIDER", "browser-run")]))
            .err()
            .unwrap_or_else(|| panic!("browser-run is refused"));
        assert!(error.to_string().contains("D2110"), "{error}");
    }

    #[test]
    fn the_environment_selects_the_service_and_names_only_a_token_reference() {
        let mut config = WebBridgeConfig::default();
        config
            .apply_env_from(&env(&[
                ("LOAMS_BROWSER_PROVIDER", "service"),
                (
                    "LOAMS_BROWSER_SERVICE_URL",
                    "ws://browser:9222/devtools/browser",
                ),
                ("LOAMS_BROWSER_SERVICE_TOKEN_REF", "env:browser-token"),
                ("LOAMS_BROWSER_RECORD", "1"),
            ]))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.provider, ProviderKind::Service);
        assert!(config.recording.enabled);
        config.validate().unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn the_older_variable_still_selects_a_provider() {
        let mut config = WebBridgeConfig::default();
        config
            .apply_env_from(&env(&[("LOAMS_WEB_BRIDGE_PROVIDER", "service")]))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.provider, ProviderKind::Service);
    }

    #[test]
    fn a_service_off_the_private_network_needs_tls_and_a_token() {
        let mut service = ServiceConfig {
            url: "ws://browser.example.com:9222/devtools/browser".to_string(),
            token_secret_ref: Some("env:browser-token".to_string()),
        };
        assert!(service.check().is_err(), "ws:// to a public name");
        service.url = "wss://browser.example.com/devtools/browser".to_string();
        service.check().unwrap_or_else(|error| panic!("{error}"));
        service.token_secret_ref = None;
        assert!(matches!(
            service.check(),
            Err(ConfigError::Missing {
                field: "service.token_secret_ref"
            })
        ));
        service.url = "ws://127.0.0.1:9222/devtools/browser".to_string();
        service
            .check()
            .unwrap_or_else(|error| panic!("loopback needs no token: {error}"));
    }

    #[test]
    fn a_cluster_name_may_use_plain_websocket() {
        for url in [
            "ws://browser:9222/devtools/browser",
            "ws://browser.loams.svc.cluster.local:9222/devtools/browser",
            "ws://10.0.3.7:9222/devtools/browser",
        ] {
            let service = ServiceConfig {
                url: url.to_string(),
                token_secret_ref: Some("env:browser-token".to_string()),
            };
            service
                .check()
                .unwrap_or_else(|error| panic!("{url}: {error}"));
        }
    }

    #[test]
    fn credentials_never_ride_in_the_url() {
        let service = ServiceConfig {
            url: "ws://user:pass@browser:9222/devtools/browser".to_string(),
            token_secret_ref: Some("env:browser-token".to_string()),
        };
        assert!(service.check().is_err());
    }

    #[test]
    fn budgets_are_checked() {
        let mut config = WebBridgeConfig::default();
        config.budgets.max_concurrent_sessions = 0;
        assert!(config.validate().is_err());
        config.budgets.max_concurrent_sessions = 1;
        config.budgets.max_session_ms = 10;
        assert!(config.validate().is_err());
    }
}
