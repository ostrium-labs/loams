//! Tailscale identities (`LOAMS_AUTH_MODE=tailscale`, owner ruling
//! 2026-10-12).
//!
//! `tailscale serve` proxies a tailnet request to a local port and names the
//! caller in `Tailscale-User-Login` and `Tailscale-User-Name` (it drops any
//! such header the caller sent, and sends none for a tagged node). Those
//! headers are only as trustworthy as the connection they arrive on, so
//! they are honoured on one listener only: `LOAMS_TAILSCALE_LISTEN`
//! (default [`DEFAULT_LISTEN`]), loopback, the `tailscale serve` target
//! and nothing else. Every other listener strips them before anything reads
//! them ([`strip`]), so a request that reaches the main port with a forged
//! login is a request without a credential: `401`.
//!
//! The tailnet identity is ambient: `tailscale serve` adds it to whatever
//! the user's browser sends, as a cookie would be. So the serve target
//! takes no request from a browser origin outside `LOAMS_CORS_ORIGINS`
//! (none by default) or its own host (the console the engine serves at
//! `https://loams.<tailnet>/ui/`), and answers CORS for those origins only;
//! any other site gets `403`.
//!
//! `LOAMS_TAILSCALE_ALLOWED_USERS` (logins, case-insensitive) narrows who is
//! admitted; without it, every user the tailnet's ACLs let reach the node.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use http::HeaderMap;

use super::{AuthError, Principal, PrincipalKind, reason};

/// The default `tailscale serve` target.
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7781);

/// The headers `tailscale serve` sets.
pub const LOGIN_HEADER: &str = "tailscale-user-login";
pub const NAME_HEADER: &str = "tailscale-user-name";
const IDENTITY_HEADERS: &[&str] = &[LOGIN_HEADER, NAME_HEADER, "tailscale-user-profile-pic"];

/// `LOAMS_TAILSCALE_*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscaleConfig {
    /// The listener whose requests carry trusted identity headers.
    pub listen: SocketAddr,
    /// Lowercased logins; empty: everyone the tailnet lets through.
    pub allowed_users: BTreeSet<String>,
}

impl Default for TailscaleConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN,
            allowed_users: BTreeSet::new(),
        }
    }
}

/// Turns a trusted login into a principal.
#[derive(Clone, Debug)]
pub struct TailscaleAuth {
    allowed: BTreeSet<String>,
}

impl TailscaleAuth {
    pub fn new(config: &TailscaleConfig) -> Self {
        Self {
            allowed: config.allowed_users.clone(),
        }
    }

    /// The principal for `login` (403 when an allowlist leaves it out).
    pub fn admit(&self, login: &str, name: Option<&str>) -> Result<Principal, AuthError> {
        let login = login.trim().to_ascii_lowercase();
        if !self.allowed.is_empty() && !self.allowed.contains(&login) {
            return Err(AuthError::forbidden(
                reason::PRINCIPAL_NOT_ALLOWED,
                "this tailnet user is not allowed on this engine (LOAMS_TAILSCALE_ALLOWED_USERS)",
            ));
        }
        Ok(Principal {
            subject: format!("tailscale:{login}"),
            kind: PrincipalKind::Tailscale,
            issuer: None,
            display_name: name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(&login)
                .to_owned(),
            email: login.contains('@').then(|| login.clone()),
            grant: super::Grant::default(),
        })
    }
}

/// The login and name `tailscale serve` set, when it set a login.
pub(crate) fn identity(headers: &HeaderMap) -> Option<(String, Option<String>)> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    Some((text(LOGIN_HEADER)?, text(NAME_HEADER)))
}

/// Removes every Tailscale identity header.
pub(crate) fn strip(headers: &mut HeaderMap) {
    for name in IDENTITY_HEADERS {
        headers.remove(*name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn the_allowlist_is_case_insensitive_and_enforced() {
        let open = TailscaleAuth::new(&TailscaleConfig::default());
        let p = open.admit("Me@Example.com", Some("Me")).unwrap();
        assert_eq!(p.subject, "tailscale:me@example.com");
        assert_eq!(p.kind, PrincipalKind::Tailscale);
        assert_eq!(p.display_name, "Me");
        assert_eq!(p.email.as_deref(), Some("me@example.com"));
        let closed = TailscaleAuth::new(&TailscaleConfig {
            allowed_users: ["me@example.com".to_owned()].into(),
            ..TailscaleConfig::default()
        });
        assert!(closed.admit("ME@example.com", None).is_ok());
        let err = closed.admit("you@example.com", None).unwrap_err();
        assert_eq!(err.reason, reason::PRINCIPAL_NOT_ALLOWED);
    }

    #[test]
    fn headers_are_read_and_stripped() {
        let mut headers = HeaderMap::new();
        assert_eq!(identity(&headers), None);
        headers.insert(LOGIN_HEADER, HeaderValue::from_static("me@example.com"));
        headers.insert("tailscale-user-profile-pic", HeaderValue::from_static("x"));
        assert_eq!(
            identity(&headers),
            Some(("me@example.com".to_owned(), None))
        );
        strip(&mut headers);
        assert!(headers.is_empty());
    }
}
