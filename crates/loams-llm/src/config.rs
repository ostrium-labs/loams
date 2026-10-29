//! Model references, secrets, base URL rules and `local_only` (§52 §10.1–§10.2).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use url::{Host, Url};
use zeroize::Zeroize;

use crate::error::LlmError;

/// Which wire protocol a model speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-compatible HTTP: OpenAI, Azure OpenAI, DeepSeek, vLLM,
    /// LM Studio and Ollama's `/v1` endpoint.
    OpenAiCompatible,
    /// Anthropic Messages.
    Anthropic,
    /// The deterministic fakes, for tests.
    Fake,
}

/// A model: provider, model name, base URL and a reference to its key. The
/// model name is configuration; the crate has no default model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    /// The wire protocol.
    pub provider: ProviderKind,
    /// The provider's model name.
    pub model: String,
    /// For OpenAI-compatible servers, the API root including its version
    /// path (`https://api.openai.com/v1`, `http://127.0.0.1:11434/v1`). For
    /// Anthropic, the host root (`https://api.anthropic.com`); the crate
    /// adds `/v1/messages` (ruling R2.6).
    pub base_url: Url,
    /// The key, if the provider needs one.
    pub key: Option<SecretRef>,
}

impl ModelRef {
    /// Ollama's OpenAI-compatible endpoint on loopback, with no key.
    #[must_use]
    pub fn ollama(model: &str) -> Self {
        Self {
            provider: ProviderKind::OpenAiCompatible,
            model: model.to_string(),
            base_url: fixed_url("http://127.0.0.1:11434/v1"),
            key: None,
        }
    }

    /// OpenAI's API.
    #[must_use]
    pub fn openai(model: &str, key: SecretRef) -> Self {
        Self {
            provider: ProviderKind::OpenAiCompatible,
            model: model.to_string(),
            base_url: fixed_url("https://api.openai.com/v1"),
            key: Some(key),
        }
    }

    /// Anthropic's API.
    #[must_use]
    pub fn anthropic(model: &str, key: SecretRef) -> Self {
        Self {
            provider: ProviderKind::Anthropic,
            model: model.to_string(),
            base_url: fixed_url("https://api.anthropic.com"),
            key: Some(key),
        }
    }

    /// A fake model (no network).
    #[must_use]
    pub fn fake(model: &str) -> Self {
        Self {
            provider: ProviderKind::Fake,
            model: model.to_string(),
            base_url: fixed_url("fake://local/"),
            key: None,
        }
    }

    /// The base URL rules and, when `policy.local_only`, the local check.
    pub fn check(&self, policy: &LocalPolicy) -> Result<(), LlmError> {
        if self.model.is_empty() {
            return Err(LlmError::NotConfigured);
        }
        if self.provider == ProviderKind::Fake {
            return Ok(());
        }
        check_base_url(&self.base_url)?;
        policy.check(&self.base_url)
    }
}

/// A URL literal known to parse.
fn fixed_url(s: &str) -> Url {
    match Url::parse(s) {
        Ok(u) => u,
        // The literals above are valid URLs.
        Err(_) => unreachable!("fixed URL literal"),
    }
}

/// `https`, or `http` on loopback; no userinfo (§50 §10.2's rule).
pub fn check_base_url(url: &Url) -> Result<(), LlmError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(LlmError::InvalidConfig(
            "base URL must not carry userinfo".to_string(),
        ));
    }
    if url.host().is_none() {
        return Err(LlmError::InvalidConfig("base URL has no host".to_string()));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(url) => Ok(()),
        "http" => Err(LlmError::InvalidConfig(
            "plain http is allowed only on loopback".to_string(),
        )),
        _ => Err(LlmError::InvalidConfig(
            "base URL scheme must be https or http".to_string(),
        )),
    }
}

/// `localhost`, `127.0.0.0/8` or `::1`.
#[must_use]
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// An address block in a store's private allow-list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parses `a.b.c.d/n` or `x::y/n`. The block must lie inside a private
    /// range (RFC 1918, `100.64.0.0/10`, `fc00::/7`, `fe80::/10`) or
    /// loopback: a public block is refused, since `local_only` promises
    /// that nothing leaves the operator's network.
    pub fn parse(s: &str) -> Result<Self, LlmError> {
        let bad = || LlmError::InvalidConfig("allow-list entry is not a private CIDR".to_string());
        let (a, p) = s.split_once('/').ok_or_else(bad)?;
        let addr: IpAddr = a.parse().map_err(|_| bad())?;
        let prefix: u8 = p.parse().map_err(|_| bad())?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(bad());
        }
        let cidr = Self { addr, prefix };
        if !PRIVATE.iter().any(|(net, len)| {
            let outer = Cidr {
                addr: *net,
                prefix: *len,
            };
            prefix >= *len && outer.contains(addr)
        }) {
            return Err(bad());
        }
        Ok(cidr)
    }

    /// Whether `ip` is inside the block.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

const PRIVATE: [(IpAddr, u8); 8] = [
    (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8),
    (IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)), 8),
    (IpAddr::V4(Ipv4Addr::new(172, 16, 0, 0)), 12),
    (IpAddr::V4(Ipv4Addr::new(192, 168, 0, 0)), 16),
    (IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)), 10),
    (IpAddr::V6(Ipv6Addr::LOCALHOST), 128),
    (IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)), 7),
    (IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10),
];

/// A store's `local_only` setting and its private allow-list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalPolicy {
    /// Refuse providers that are not loopback or allow-listed.
    pub local_only: bool,
    /// Private address blocks allowed besides loopback.
    pub allow: Vec<Cidr>,
}

impl LocalPolicy {
    /// `local_only: false`: every provider passes this check.
    #[must_use]
    pub fn open() -> Self {
        Self::default()
    }

    /// `local_only: true` with the given allow-list.
    #[must_use]
    pub fn local_only(allow: Vec<Cidr>) -> Self {
        Self {
            local_only: true,
            allow,
        }
    }

    /// Refuses, with [`LlmError::NotLocal`], a URL whose host is not
    /// loopback or an IP literal inside the allow-list. Host names other
    /// than `localhost` are refused under `local_only`: a name can resolve
    /// anywhere (ruling R2.4).
    pub fn check(&self, url: &Url) -> Result<(), LlmError> {
        if !self.local_only || is_loopback(url) {
            return Ok(());
        }
        let ip = match url.host() {
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
            _ => return Err(LlmError::NotLocal),
        };
        if self.allow.iter().any(|c| c.contains(ip)) {
            Ok(())
        } else {
            Err(LlmError::NotLocal)
        }
    }
}

/// A reference to a key: `env/<NAME>` on a server, `keyring/<name>` through
/// the desktop daemon. A reference is not secret; the value it names is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecretRef(String);

impl SecretRef {
    /// `<scheme>/<name>`: ASCII letters, digits, `_`, `-`, `.` and `/`, at
    /// most 256 bytes, with a non-empty scheme and name.
    pub fn parse(s: &str) -> Result<Self, LlmError> {
        let bad = || LlmError::InvalidConfig("malformed secret reference".to_string());
        let (scheme, name) = s.split_once('/').ok_or_else(bad)?;
        let ok = |p: &str| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
        };
        if s.len() > 256 || !ok(scheme) || scheme.contains('/') || !ok(name) {
            return Err(bad());
        }
        Ok(Self(s.to_string()))
    }

    /// The part before the first `/` (`env`).
    #[must_use]
    pub fn scheme(&self) -> &str {
        self.0.split_once('/').map_or("", |(s, _)| s)
    }

    /// The part after the first `/`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.split_once('/').map_or("", |(_, n)| n)
    }

    /// The whole reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecretRef {
    type Error = LlmError;
    fn try_from(s: String) -> Result<Self, LlmError> {
        Self::parse(&s)
    }
}

impl From<SecretRef> for String {
    fn from(r: SecretRef) -> String {
        r.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A secret value. `Debug` and `Display` print `[redacted]`; the value is
/// zeroed on drop. It has no `Serialize`.
pub struct Secret<T: Zeroize>(T);

impl<T: Zeroize> Secret<T> {
    /// Wraps a value.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// The value, for the one place that sends it.
    #[must_use]
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret<String> {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Where key references resolve: `env/` references on a server, the OS
/// keyring through the daemon on the desktop.
#[async_trait::async_trait]
pub trait SecretSource: Send + Sync {
    /// The value `r` names, or [`LlmError::NotConfigured`].
    async fn resolve(&self, r: &SecretRef) -> Result<Secret<String>, LlmError>;
}

/// The server's source: `env/<NAME>` reads the process environment variable
/// `<prefix><NAME>` (default prefix `LOAMS_LLM_KEY_`). The prefix keeps a
/// store's configuration from naming any other variable of the server's
/// environment (ruling R2.3). Until `loams-env` lands this is the only `env/`
/// backend.
#[derive(Debug, Clone)]
pub struct EnvSecretSource {
    prefix: String,
}

/// The default prefix of [`EnvSecretSource`].
pub const ENV_KEY_PREFIX: &str = "LOAMS_LLM_KEY_";

impl Default for EnvSecretSource {
    fn default() -> Self {
        Self {
            prefix: ENV_KEY_PREFIX.to_string(),
        }
    }
}

impl EnvSecretSource {
    /// Resolves `env/<NAME>` from `<prefix><NAME>`.
    #[must_use]
    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }

    /// The variable `r` names, if it is an `env/` reference with a plain
    /// name (no `/` or `.`).
    #[must_use]
    pub fn variable(&self, r: &SecretRef) -> Option<String> {
        let name = r.name();
        (r.scheme() == "env" && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .then(|| format!("{}{}", self.prefix, name))
    }
}

#[async_trait::async_trait]
impl SecretSource for EnvSecretSource {
    async fn resolve(&self, r: &SecretRef) -> Result<Secret<String>, LlmError> {
        let var = self.variable(r).ok_or(LlmError::NotConfigured)?;
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => Ok(Secret::new(v)),
            _ => Err(LlmError::NotConfigured),
        }
    }
}

/// A fixed map of references to values, for tests and embedders that
/// resolve keys themselves.
#[derive(Default)]
pub struct StaticSecrets(Vec<(SecretRef, Secret<String>)>);

impl fmt::Debug for StaticSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(r, _)| r))
            .finish()
    }
}

impl StaticSecrets {
    /// Adds a reference and its value.
    #[must_use]
    pub fn with(mut self, r: SecretRef, value: Secret<String>) -> Self {
        self.0.push((r, value));
        self
    }
}

#[async_trait::async_trait]
impl SecretSource for StaticSecrets {
    async fn resolve(&self, r: &SecretRef) -> Result<Secret<String>, LlmError> {
        self.0
            .iter()
            .find(|(k, _)| k == r)
            .map(|(_, v)| v.clone())
            .ok_or(LlmError::NotConfigured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_redacted() {
        let s = Secret::new("sk-canary".to_string());
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(s.expose(), "sk-canary");
    }

    #[test]
    fn secret_refs() {
        let r = SecretRef::parse("env/OPENAI").expect("valid");
        assert_eq!((r.scheme(), r.name()), ("env", "OPENAI"));
        assert!(SecretRef::parse("OPENAI").is_err());
        assert!(SecretRef::parse("env/").is_err());
        assert!(SecretRef::parse("env/a b").is_err());
        let env = EnvSecretSource::default();
        assert_eq!(env.variable(&r).as_deref(), Some("LOAMS_LLM_KEY_OPENAI"));
        let nested = SecretRef::parse("env/dev.env/KEY").expect("valid");
        assert_eq!(env.variable(&nested), None);
    }

    #[test]
    fn cidrs() {
        let c = Cidr::parse("192.168.1.0/24").expect("private");
        assert!(c.contains("192.168.1.7".parse().expect("ip")));
        assert!(!c.contains("192.168.2.7".parse().expect("ip")));
        assert!(Cidr::parse("8.8.8.0/24").is_err());
        assert!(Cidr::parse("10.0.0.0/4").is_err());
        assert!(Cidr::parse("fd00::/8").is_ok());
        assert!(Cidr::parse("10.1.2.3").is_err());
    }
}
