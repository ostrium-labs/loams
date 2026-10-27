//! The egress policy: which URLs a provider is willing to ask a browser to
//! open.
//!
//! A browser fetches on our behalf, so without a policy Loams would be an open
//! proxy into whatever the browser can reach, including a cloud's metadata
//! endpoint. Two layers enforce the policy:
//!
//! 1. **In the provider**, before every navigation and on every request the
//!    engine pauses for it ([`EgressPolicy::check`]): scheme, private address,
//!    metadata endpoint, deny list, allow list.
//! 2. **In the engine**: Obscura refuses loopback, RFC1918, link-local and
//!    unique-local targets after DNS resolution unless it was started with
//!    `--allow-private-network`, which covers the DNS-rebinding case layer one
//!    cannot see (it does not resolve names).

use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

use crate::error::ConfigError;

/// The schemes a browser tool may navigate to.
const ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

/// The longest allow list a configuration may carry. Large lists belong in a
/// network policy around the browser service, not in every agent's config.
pub const MAX_ALLOWED_DOMAINS: usize = 50;

/// Hostnames that are never allowed, whatever the allow list says. The
/// instance metadata endpoints are the reason a remote browser behind a policy
/// is still a server-side request forgery waiting to happen.
const DENIED_HOSTS: [&str; 6] = [
    "localhost",
    "metadata.google.internal",
    "metadata.goog",
    "instance-data",
    "169.254.169.254",
    "100.100.100.200",
];

/// Hostname suffixes that stay inside a network.
const DENIED_SUFFIXES: [&str; 4] = [".localhost", ".internal", ".local", ".lan"];

/// One entry of an allow or deny list.
///
/// A hostname and no more (no scheme, no port, no path) with at most one `*`; `*.example.com` matches
/// subdomains but not the apex; `*example.com` matches the apex too, and so
/// matches lookalikes such as `evilexample.com`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern(String);

impl HostPattern {
    /// Parse and check a pattern.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let pattern = raw.trim().to_ascii_lowercase();
        if pattern.is_empty() {
            return Err(ConfigError::Invalid {
                field: "egress.allowed_domains",
                reason: "an entry may not be empty".to_string(),
            });
        }
        if pattern.contains("://") || pattern.contains('/') || pattern.contains(':') {
            return Err(ConfigError::Invalid {
                field: "egress.allowed_domains",
                reason: format!("{raw:?} must be a bare hostname: no scheme, port or path"),
            });
        }
        if pattern.matches('*').count() > 1 {
            return Err(ConfigError::Invalid {
                field: "egress.allowed_domains",
                reason: format!("{raw:?} has more than one wildcard"),
            });
        }
        Ok(HostPattern(pattern))
    }

    /// Whether `host` matches.
    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let host = host.strip_suffix('.').unwrap_or(&host);
        match self.0.strip_prefix("*") {
            // The two wildcard forms differ in one respect: `*.example.com`
            // covers subdomains only, while `*example.com` covers the apex
            // too, and lookalikes such as `evilexample.com`.
            Some(rest) if self.0.starts_with("*.") => {
                host.len() > rest.len() && host.ends_with(rest)
            }
            Some(rest) => host.ends_with(rest),
            None => host == self.0,
        }
    }

    /// The pattern as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which URLs a provider may open.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    allowed: Vec<HostPattern>,
    denied: Vec<HostPattern>,
    allow_private_hosts: bool,
}

impl Default for EgressPolicy {
    /// The public web, no allow list, private hosts refused.
    fn default() -> Self {
        Self {
            allowed: Vec::new(),
            denied: Vec::new(),
            allow_private_hosts: false,
        }
    }
}

impl EgressPolicy {
    /// A policy that permits every public host and refuses private ones.
    pub fn public_web() -> Self {
        Self::default()
    }

    /// A policy that permits only the listed hosts and their subdomains.
    pub fn allow_list<I, S>(hosts: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut policy = Self::default();
        for host in hosts {
            policy.allowed.push(HostPattern::parse(host.as_ref())?);
        }
        policy.check_limits()?;
        Ok(policy)
    }

    /// Add a host to the allow list.
    pub fn allow(&mut self, host: &str) -> Result<(), ConfigError> {
        self.allowed.push(HostPattern::parse(host)?);
        self.check_limits()
    }

    /// Add a host that is refused even if an allow list would match it.
    pub fn deny(&mut self, host: &str) -> Result<(), ConfigError> {
        self.denied.push(HostPattern::parse(host)?);
        Ok(())
    }

    /// Permit private, loopback, link-local and metadata addresses. Off by
    /// default, and only sensible for a loopback fixture site.
    pub fn set_allow_private_hosts(&mut self, allow: bool) {
        self.allow_private_hosts = allow;
    }

    /// Whether an allow list is configured.
    pub fn is_allow_listed(&self) -> bool {
        !self.allowed.is_empty()
    }

    /// Whether private, loopback and link-local addresses are permitted (the
    /// local launcher passes the engine's matching flag only then).
    pub fn allows_private_hosts(&self) -> bool {
        self.allow_private_hosts
    }

    /// Check a URL, in the order a URL is parsed: scheme, host, private
    /// address, deny list, allow list.
    pub fn check(&self, url: &Url) -> Result<(), EgressError> {
        if !ALLOWED_SCHEMES.contains(&url.scheme()) {
            return Err(EgressError::Scheme {
                scheme: url.scheme().to_string(),
            });
        }
        let host = match url.host() {
            Some(host) => host,
            None => return Err(EgressError::NoHost),
        };
        if !self.allow_private_hosts {
            check_private(&host)?;
        }
        if let Host::Domain(domain) = &host {
            let domain = domain.to_ascii_lowercase();
            if DENIED_HOSTS.contains(&domain.as_str())
                || DENIED_SUFFIXES
                    .iter()
                    .any(|suffix| domain.ends_with(suffix))
            {
                return Err(EgressError::DeniedHost { host: domain });
            }
        }
        if self
            .denied
            .iter()
            .any(|pattern| matches_host(pattern, &host))
        {
            return Err(EgressError::DeniedHost {
                host: host.to_string(),
            });
        }
        if self.is_allow_listed()
            && !self
                .allowed
                .iter()
                .any(|pattern| matches_host(pattern, &host))
        {
            return Err(EgressError::NotAllowed {
                host: host.to_string(),
            });
        }
        Ok(())
    }

    /// Check a request the page itself makes (a sub-resource, a redirect, a
    /// fetch) while the engine holds it paused.
    ///
    /// The same rules as [`EgressPolicy::check`], except that the schemes a
    /// page builds in memory (`data:`, `blob:`, `about:`) never leave the
    /// engine and so are not network egress.
    pub fn check_request(&self, url: &Url) -> Result<(), EgressError> {
        if matches!(url.scheme(), "data" | "blob" | "about") {
            return Ok(());
        }
        self.check(url)
    }

    /// Whether the allow list covers a hostname (always, without one).
    pub fn covers(&self, host: &str) -> bool {
        !self.is_allow_listed() || self.allowed.iter().any(|pattern| pattern.matches(host))
    }

    fn check_limits(&self) -> Result<(), ConfigError> {
        if self.allowed.len() > MAX_ALLOWED_DOMAINS {
            return Err(ConfigError::Invalid {
                field: "egress.allowed_domains",
                reason: format!("at most {MAX_ALLOWED_DOMAINS} are accepted"),
            });
        }
        Ok(())
    }
}

fn matches_host(pattern: &HostPattern, host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => pattern.matches(domain),
        Host::Ipv4(address) => pattern.matches(&address.to_string()),
        Host::Ipv6(address) => pattern.matches(&address.to_string()),
    }
}

/// Refuse an address that is not routable on the public internet, plus the
/// instance metadata endpoints.
fn check_private(host: &Host<&str>) -> Result<(), EgressError> {
    match host {
        Host::Domain(domain) => {
            // A bare IPv4 in a URL is parsed as a domain by `url` only when it
            // is not a valid address, so nothing to do here.
            let _ = domain;
            Ok(())
        }
        Host::Ipv4(address) => check_private_v4(*address),
        Host::Ipv6(address) => check_private_v6(*address),
    }
}

fn check_private_v4(address: Ipv4Addr) -> Result<(), EgressError> {
    let blocked = address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_documentation()
        || address.octets()[0] == 0
        // Carrier-grade NAT, and the two cloud metadata addresses.
        || (address.octets()[0] == 100 && (64..128).contains(&address.octets()[1]))
        || address == Ipv4Addr::new(168, 254, 169, 254)
        || address == Ipv4Addr::new(100, 100, 100, 200)
        || address == Ipv4Addr::new(192, 0, 0, 192);
    if blocked {
        return Err(EgressError::PrivateHost {
            host: address.to_string(),
        });
    }
    Ok(())
}

fn check_private_v6(address: Ipv6Addr) -> Result<(), EgressError> {
    let segments = address.segments();
    let blocked = address.is_loopback()
        || address.is_unspecified()
        // Unique local (fc00::/7) and link local (fe80::/10).
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        // IPv4-mapped and IPv4-compatible forms carry a v4 address inside.
        || address.to_ipv4_mapped().is_some_and(|inner| {
            check_private_v4(inner)
                .map(|()| false)
                .unwrap_or(true)
        });
    if blocked {
        return Err(EgressError::PrivateHost {
            host: address.to_string(),
        });
    }
    Ok(())
}

/// Why a URL was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EgressError {
    /// Not `http` or `https`.
    #[error("the {scheme} scheme is not navigable; use http or https")]
    Scheme {
        /// The scheme that was asked for.
        scheme: String,
    },
    /// The URL has no host.
    #[error("the url has no host")]
    NoHost,

    /// A private, loopback, link-local or metadata address.
    #[error("{host} is not a public address")]
    PrivateHost {
        /// The address or name.
        host: String,
    },

    /// A denied hostname.
    #[error("{host} is on the deny list")]
    DeniedHost {
        /// The hostname.
        host: String,
    },

    /// Off the allow list.
    #[error("{host} is not on the egress allow list")]
    NotAllowed {
        /// The hostname.
        host: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap_or_else(|_| panic!("{raw} parses"))
    }

    #[test]
    fn private_and_metadata_addresses_are_refused() {
        let policy = EgressPolicy::public_web();
        for raw in [
            "http://127.0.0.1/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://localhost:8080/",
            "http://metadata.google.internal/",
        ] {
            assert!(policy.check(&url(raw)).is_err(), "{raw} must be refused");
        }
    }

    #[test]
    fn a_public_url_is_allowed_without_an_allow_list() {
        EgressPolicy::public_web()
            .check(&url("https://example.com/pricing"))
            .unwrap_or_else(|error| panic!("a public url passes: {error}"));
    }

    #[test]
    fn in_memory_sub_resources_are_not_egress_but_private_ones_are() {
        let policy = EgressPolicy::public_web();
        policy
            .check_request(&url("data:image/png;base64,AAAA"))
            .unwrap_or_else(|error| panic!("data: stays in the engine: {error}"));
        assert!(
            policy.check(&url("data:text/html,hi")).is_err(),
            "not navigable"
        );
        assert!(
            policy
                .check_request(&url("http://169.254.169.254/latest/meta-data/"))
                .is_err()
        );
    }

    #[test]
    fn an_allow_list_closes_the_door() {
        let policy = EgressPolicy::allow_list(["*.example.com"])
            .unwrap_or_else(|_| panic!("a valid allow list parses"));
        policy
            .check(&url("https://docs.example.com/a"))
            .unwrap_or_else(|error| panic!("a subdomain passes: {error}"));
        assert!(policy.check(&url("https://example.com/")).is_err());
        assert!(policy.check(&url("https://elsewhere.test/")).is_err());
    }
}
