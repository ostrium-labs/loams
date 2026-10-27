//! Who may use a tenant's SQL Gateway (D1978).
//!
//! Two seams, one trait each, the pg-control shape (design 46 §4.1), so
//! design 76's identity and OpenFGA drop in without touching the proxy:
//!
//! - [`Authenticator`]: request headers to a [`Principal`].
//!   [`KeyDigestAuthenticator`] checks `authorization: Bearer lk_…`
//!   against a keys file of SHA-256 digests; [`DevLoopbackAuthenticator`]
//!   answers one fixed principal and can only be built for a loopback
//!   listener.
//! - [`Authorizer`]: may a principal do an [`Action`] on a tenant's
//!   cluster? [`GrantsAuthorizer`] reads the principal's tenant grants.
//!
//! The keys file has one key per line,
//! `<key-id> <sha256-hex-of-the-whole-key> <tenant>[,<tenant>…] [kind] [label]`,
//! where `kind` is `user` (the default), `service` or `agent`. Blank lines
//! and `#` comments are ignored. Keys are 190 bits of randomness, so a
//! fast hash is enough; nothing here logs or keeps a key.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::TenantId;

/// The API key prefix (`lk_<26-char ULID>_<32 base62>`, the engine's).
pub const KEY_PREFIX: &str = "lk_";
const ID_LEN: usize = 26;
const SECRET_LEN: usize = 32;

/// What kind of caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// A person.
    User,
    /// A service account.
    Service,
    /// An agent (never `Admin`, design 46 §4.1).
    Agent,
}

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// The key id (or `dev`).
    pub id: String,
    /// What it is.
    pub kind: PrincipalKind,
    /// The tenants it holds grants on.
    pub tenants: Vec<TenantId>,
    /// A label for logs.
    #[serde(default)]
    pub label: Option<String>,
}

/// What a request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Read-only calls (`info`, `api_versions`, completion, result fetches).
    Read,
    /// Sessions and statements.
    Query,
    /// `deploy-script` (starts an application cluster).
    Deploy,
}

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// No or a bad credential (401).
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),
    /// Authenticated, not allowed (403).
    #[error("forbidden: {0}")]
    Forbidden(String),
}

/// Headers to a principal.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// The principal `headers` authenticate.
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>;
}

/// May a principal act on a tenant?
pub trait Authorizer: Send + Sync {
    /// Ok when `principal` may do `action` on `tenant`'s cluster.
    ///
    /// # Errors
    ///
    /// [`AuthError::Forbidden`] saying why not.
    fn authorize(
        &self,
        principal: &Principal,
        tenant: &TenantId,
        action: Action,
    ) -> Result<(), AuthError>;
}

/// The bearer token of `headers`.
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// The id of an API key, when it has the shape.
pub fn key_id(key: &str) -> Option<&str> {
    let rest = key.strip_prefix(KEY_PREFIX)?;
    if key.len() != KEY_PREFIX.len() + ID_LEN + 1 + SECRET_LEN || !key.is_ascii() {
        return None;
    }
    let (id, secret) = rest.split_at(ID_LEN);
    let secret = secret.strip_prefix('_')?;
    let id_ok = id
        .bytes()
        .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase());
    let secret_ok = secret.len() == SECRET_LEN && secret.bytes().all(|b| b.is_ascii_alphanumeric());
    (id_ok && secret_ok).then_some(id)
}

/// SHA-256 of `key`, hex.
pub fn digest_hex(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// Equal in time independent of where they differ.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Clone)]
struct KeyEntry {
    digest: [u8; 32],
    principal: Principal,
}

/// Checks `lk_…` keys against a keys file of digests.
#[derive(Clone)]
pub struct KeyDigestAuthenticator {
    keys: Arc<HashMap<String, KeyEntry>>,
}

impl std::fmt::Debug for KeyDigestAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyDigestAuthenticator")
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl KeyDigestAuthenticator {
    /// Reads the keys file at `path`.
    ///
    /// # Errors
    ///
    /// The read error, or the first malformed line.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parses a keys file.
    ///
    /// # Errors
    ///
    /// The first malformed line, by number.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut keys = HashMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |what: &str| format!("line {}: {what}", n + 1);
            let mut parts = line.split_whitespace();
            let id = parts.next().ok_or_else(|| bad("no key id"))?;
            if id.len() != ID_LEN
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
            {
                return Err(bad("the key id is a 26-character ULID"));
            }
            let digest_hex = parts.next().ok_or_else(|| bad("no digest"))?;
            let mut digest = [0u8; 32];
            hex::decode_to_slice(digest_hex, &mut digest)
                .map_err(|_| bad("the digest is 64 hex digits"))?;
            let tenants = parts
                .next()
                .ok_or_else(|| bad("no tenants"))?
                .split(',')
                .map(TenantId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| bad(&e.to_string()))?;
            let mut kind = PrincipalKind::User;
            let mut label_parts: Vec<&str> = Vec::new();
            for (i, p) in parts.enumerate() {
                match (i, p) {
                    (0, "user") => kind = PrincipalKind::User,
                    (0, "service") => kind = PrincipalKind::Service,
                    (0, "agent") => kind = PrincipalKind::Agent,
                    _ => label_parts.push(p),
                }
            }
            let label = (!label_parts.is_empty()).then(|| label_parts.join(" "));
            if keys
                .insert(
                    id.to_owned(),
                    KeyEntry {
                        digest,
                        principal: Principal {
                            id: id.to_owned(),
                            kind,
                            tenants,
                            label,
                        },
                    },
                )
                .is_some()
            {
                return Err(bad("a key id appears twice"));
            }
        }
        Ok(Self {
            keys: Arc::new(keys),
        })
    }

    /// The principal of `key`.
    ///
    /// # Errors
    ///
    /// [`AuthError::Unauthenticated`] for a malformed, unknown or wrong key.
    pub fn verify(&self, key: &str) -> Result<Principal, AuthError> {
        let id = key_id(key).ok_or_else(|| {
            AuthError::Unauthenticated("an API key is lk_<26-char id>_<32 characters>".to_owned())
        })?;
        let invalid = || AuthError::Unauthenticated("the API key is not valid".to_owned());
        let entry = self.keys.get(id).ok_or_else(invalid)?;
        let digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        if ct_eq(&digest, &entry.digest) {
            Ok(entry.principal.clone())
        } else {
            Err(invalid())
        }
    }
}

#[async_trait]
impl Authenticator for KeyDigestAuthenticator {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let key = bearer(headers).ok_or_else(|| {
            AuthError::Unauthenticated("send authorization: Bearer <Loams API key>".to_owned())
        })?;
        self.verify(key)
    }
}

/// One fixed principal for every request; only for a loopback listener
/// (local development).
#[derive(Debug, Clone)]
pub struct DevLoopbackAuthenticator {
    principal: Principal,
}

impl DevLoopbackAuthenticator {
    /// The dev principal with grants on `tenants`, for a gateway that
    /// listens on `listen`.
    ///
    /// # Errors
    ///
    /// When `listen` is not a loopback address.
    pub fn new(listen: SocketAddr, tenants: Vec<TenantId>) -> Result<Self, String> {
        if !listen.ip().is_loopback() {
            return Err(format!(
                "the dev authenticator answers every request; {listen} is not loopback"
            ));
        }
        Ok(Self {
            principal: Principal {
                id: "dev".to_owned(),
                kind: PrincipalKind::User,
                tenants,
                label: Some("dev loopback".to_owned()),
            },
        })
    }
}

#[async_trait]
impl Authenticator for DevLoopbackAuthenticator {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<Principal, AuthError> {
        Ok(self.principal.clone())
    }
}

/// Allows a principal what its tenant grants say; agents may query but not
/// deploy application clusters.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrantsAuthorizer;

impl Authorizer for GrantsAuthorizer {
    fn authorize(
        &self,
        principal: &Principal,
        tenant: &TenantId,
        action: Action,
    ) -> Result<(), AuthError> {
        if !principal.tenants.contains(tenant) {
            return Err(AuthError::Forbidden(format!("no grant on tenant {tenant}")));
        }
        if action == Action::Deploy && principal.kind == PrincipalKind::Agent {
            return Err(AuthError::Forbidden(
                "agents may not deploy application clusters".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "lk_01JABCDEFGHJKMNPQRSTVWXYZ0_abcdefghijklmnopqrstuvwxyz012345";

    fn file() -> String {
        format!(
            "# keys\n\n01JABCDEFGHJKMNPQRSTVWXYZ0 {} t1,t2 agent ci bot\n",
            digest_hex(KEY)
        )
    }

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(http::header::AUTHORIZATION, value.parse().unwrap());
        h
    }

    #[test]
    fn key_shapes() {
        assert_eq!(key_id(KEY), Some("01JABCDEFGHJKMNPQRSTVWXYZ0"));
        assert_eq!(key_id("lk_short"), None);
        assert_eq!(
            key_id("xx_01JABCDEFGHJKMNPQRSTVWXYZ0_abcdefghijklmnopqrstuvwxyz012345"),
            None
        );
    }

    #[tokio::test]
    async fn keys_file_authenticates_by_digest() {
        let auth = KeyDigestAuthenticator::parse(&file()).unwrap();
        let p = auth
            .authenticate(&headers(&format!("Bearer {KEY}")))
            .await
            .unwrap();
        assert_eq!(p.kind, PrincipalKind::Agent);
        assert_eq!(p.label.as_deref(), Some("ci bot"));
        assert_eq!(p.tenants.len(), 2);
        let wrong = KEY.replace("abc", "abd");
        assert!(
            auth.authenticate(&headers(&format!("Bearer {wrong}")))
                .await
                .is_err()
        );
        assert!(auth.authenticate(&HeaderMap::new()).await.is_err());
        assert!(auth.authenticate(&headers("Basic xyz")).await.is_err());
    }

    #[test]
    fn malformed_keys_files_are_refused() {
        assert!(KeyDigestAuthenticator::parse("short abc t1").is_err());
        assert!(KeyDigestAuthenticator::parse("01JABCDEFGHJKMNPQRSTVWXYZ0 zz t1").is_err());
        let d = digest_hex(KEY);
        assert!(
            KeyDigestAuthenticator::parse(&format!("01JABCDEFGHJKMNPQRSTVWXYZ0 {d} Bad_Tenant"))
                .is_err()
        );
        let twice =
            format!("01JABCDEFGHJKMNPQRSTVWXYZ0 {d} t1\n01JABCDEFGHJKMNPQRSTVWXYZ0 {d} t1\n");
        assert!(KeyDigestAuthenticator::parse(&twice).is_err());
    }

    #[test]
    fn grants_scope_tenants_and_keep_agents_from_deploying() {
        let p = Principal {
            id: "x".into(),
            kind: PrincipalKind::Agent,
            tenants: vec![TenantId::new("t1").unwrap()],
            label: None,
        };
        let t1 = TenantId::new("t1").unwrap();
        let t2 = TenantId::new("t2").unwrap();
        assert!(GrantsAuthorizer.authorize(&p, &t1, Action::Query).is_ok());
        assert!(GrantsAuthorizer.authorize(&p, &t2, Action::Read).is_err());
        assert!(GrantsAuthorizer.authorize(&p, &t1, Action::Deploy).is_err());
    }

    #[test]
    fn dev_authenticator_is_loopback_only() {
        assert!(DevLoopbackAuthenticator::new("127.0.0.1:8090".parse().unwrap(), vec![]).is_ok());
        assert!(DevLoopbackAuthenticator::new("0.0.0.0:8090".parse().unwrap(), vec![]).is_err());
    }
}
