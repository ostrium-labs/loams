//! OIDC access tokens (preview contract): a JWT signed by one of the
//! issuers in `LOAMS_OIDC_ISSUER` (a comma list: by default the hosted
//! `https://auth.loams.dev`, or the user's own Zitadel, Keycloak, Auth0,
//! Google or Cloudflare Access) for one of the audiences in
//! `LOAMS_OIDC_AUDIENCE` (the console's client id), verified against that
//! issuer's JWKS. The token's (unverified) `iss` only picks which issuer's
//! keys to check it with; the signature and every claim are then verified.
//! An issuer matches with or without its trailing `/`.
//!
//! - Only RS256 and EdDSA are accepted; the token's `alg` must match the
//!   key's family, so `none`, HMAC and algorithm confusion are refused.
//! - `iss`, `aud`, `exp` and `sub` are required; `nbf` is checked; the
//!   clock leeway is [`LEEWAY_SECS`].
//! - The JWKS comes from the issuer's discovery document (or the matching
//!   entry of `LOAMS_OIDC_JWKS_URL`; a Cloudflare Access team domain uses
//!   `/cdn-cgi/access/certs`), is cached for [`JWKS_TTL`], and is fetched again
//!   early for an unknown `kid` at most every [`JWKS_MIN_REFETCH`]. A failed
//!   refresh keeps the cached keys.
//! - A valid token is admitted only if its `sub` is in
//!   `LOAMS_OIDC_ALLOWED_SUBJECTS`, its verified `email` in
//!   `LOAMS_OIDC_ALLOWED_EMAILS`, or one of its `groups` in
//!   `LOAMS_OIDC_ALLOWED_GROUPS`. A shared issuer signs tokens for every
//!   user of the hosted console, so the signature alone says nothing about
//!   who may use *this* engine. With no allowlist the server refuses to
//!   start.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use super::{AuthError, Authenticator, Credential, Principal, PrincipalKind, reason};

/// How long a fetched JWKS is used before it is fetched again.
pub const JWKS_TTL: Duration = Duration::from_secs(600);
/// The shortest gap between two fetches (an unknown `kid` forces one).
pub const JWKS_MIN_REFETCH: Duration = Duration::from_secs(30);
/// Clock skew allowed on `exp` and `nbf`.
pub const LEEWAY_SECS: u64 = 30;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// One trusted issuer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Issuer {
    /// The `iss` its tokens carry (compared without a trailing `/`).
    pub issuer: String,
    /// The JWKS URL; `None`: the discovery document's `jwks_uri`.
    pub jwks_url: Option<String>,
}

impl Issuer {
    pub fn new(issuer: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            jwks_url: None,
        }
    }

    fn discovery_url(&self) -> String {
        format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        )
    }

    /// The JWKS URL known without discovery: the configured one, or a
    /// Cloudflare Access team domain's certificate endpoint.
    fn known_jwks_url(&self) -> Option<String> {
        if let Some(url) = &self.jwks_url {
            return Some(url.clone());
        }
        let url = url::Url::parse(&self.issuer).ok()?;
        url.host_str()?
            .ends_with(".cloudflareaccess.com")
            .then(|| format!("{}/cdn-cgi/access/certs", self.issuer.trim_end_matches('/')))
    }

    fn matches(&self, iss: &str) -> bool {
        same_issuer(&self.issuer, iss)
    }
}

/// Issuers compare without a trailing `/` (`https://auth.loams.dev` and
/// `https://auth.loams.dev/` are the same; some IdPs publish the slash).
fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// `LOAMS_OIDC_*`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OidcConfig {
    /// The trusted issuers (`LOAMS_OIDC_ISSUER`, a comma list).
    pub issuers: Vec<Issuer>,
    /// A token's `aud` must include one of these (`LOAMS_OIDC_AUDIENCE`, a
    /// comma list: the console's client id, and a self-registered one).
    pub audiences: Vec<String>,
    pub allowed_subjects: BTreeSet<String>,
    /// Lowercased; matched only with `email_verified: true`.
    pub allowed_emails: BTreeSet<String>,
    pub allowed_groups: BTreeSet<String>,
}

impl OidcConfig {
    /// Refuses a configuration the authenticator cannot run safely.
    pub fn validate(&self) -> Result<(), String> {
        if self.issuers.is_empty() {
            return Err("LOAMS_OIDC_ISSUER is empty".to_owned());
        }
        for issuer in &self.issuers {
            check_url("LOAMS_OIDC_ISSUER", &issuer.issuer)?;
            if let Some(jwks) = &issuer.jwks_url {
                check_url("LOAMS_OIDC_JWKS_URL", jwks)?;
            }
        }
        if self.audiences.iter().all(|a| a.trim().is_empty()) {
            return Err("LOAMS_OIDC_AUDIENCE is empty".to_owned());
        }
        if self.allowed_subjects.is_empty()
            && self.allowed_emails.is_empty()
            && self.allowed_groups.is_empty()
        {
            return Err(
                "oidc mode needs an allowlist: set LOAMS_OIDC_ALLOWED_EMAILS, \
                 LOAMS_OIDC_ALLOWED_SUBJECTS or LOAMS_OIDC_ALLOWED_GROUPS (the issuer signs \
                 tokens for every user of the console, not only yours)"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// HTTPS, or HTTP to a loopback host (tests and a local issuer).
fn check_url(name: &str, value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|e| format!("{name} {value:?}: {e}"))?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d == "localhost",
        None => false,
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(format!(
            "{name} {value:?} must be an https:// URL (http:// only on loopback)"
        )),
    }
}

#[derive(Debug)]
struct Cached {
    keys: JwkSet,
    fetched: Instant,
}

/// One issuer's signing keys.
#[derive(Debug)]
struct IssuerKeys {
    issuer: Issuer,
    cache: RwLock<Option<Cached>>,
    /// Serializes fetches; holds when the last one started.
    fetching: Mutex<Option<Instant>>,
}

/// Verifies OIDC JWTs against their issuer's JWKS.
#[derive(Debug)]
pub struct OidcAuthenticator {
    config: OidcConfig,
    http: reqwest::Client,
    issuers: Vec<IssuerKeys>,
}

/// The only claim read before the signature is checked.
#[derive(Debug, Deserialize)]
struct Unverified {
    iss: Option<String>,
}

/// The token's `iss`, unverified: it only picks the issuer's keys.
fn unverified_issuer(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Unverified>(&bytes).ok()?.iss
}

#[derive(Debug, Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    /// The expiry, Unix seconds (required by the validation).
    #[serde(default)]
    exp: Option<u64>,
    /// A Loams token's org and environment (MT1's `org` and `env`).
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    env: Option<String>,
    /// Scopes: a Loams token's `scp` (a list or a space-separated string)
    /// or the OAuth `scope` string.
    #[serde(default)]
    scp: Option<ScopeClaim>,
    #[serde(default)]
    scope: Option<String>,
}

/// A scope claim as a list or as one space-separated string.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum ScopeClaim {
    List(Vec<String>),
    Text(String),
}

impl Claims {
    /// What the token reaches (LV1 plan Task 14): the claims it carries,
    /// the self-hosted default for the ones it does not.
    fn grant(&self) -> super::Grant {
        // `scp` first; a `scp` that names no scope falls back to `scope`,
        // so an empty `scp` never widens a token to every scope.
        let scp = match &self.scp {
            Some(ScopeClaim::List(list)) => super::Grant::scopes_of(list),
            Some(ScopeClaim::Text(text)) => super::Grant::scopes_of(text.split_whitespace()),
            None => None,
        };
        let scopes = scp.or_else(|| {
            self.scope
                .as_deref()
                .and_then(|text| super::Grant::scopes_of(text.split_whitespace()))
        });
        super::Grant {
            org: self.org.clone().filter(|o| !o.is_empty()),
            environment: self.env.clone().filter(|e| !e.is_empty()),
            scopes,
            expires_at_ms: self.exp.map(|s| s.saturating_mul(1000)),
        }
    }
}

impl OidcAuthenticator {
    pub fn new(config: OidcConfig) -> Result<Self, String> {
        config.validate()?;
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("the OIDC HTTP client: {e}"))?;
        let issuers = config
            .issuers
            .iter()
            .map(|issuer| IssuerKeys {
                issuer: issuer.clone(),
                cache: RwLock::new(None),
                fetching: Mutex::new(None),
            })
            .collect();
        Ok(Self {
            config,
            http,
            issuers,
        })
    }

    async fn fetch(&self, issuer: &Issuer) -> Result<JwkSet, String> {
        let jwks_url = match issuer.known_jwks_url() {
            Some(url) => url,
            None => {
                let discovery: Discovery = self
                    .http
                    .get(issuer.discovery_url())
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|e| format!("OIDC discovery: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("OIDC discovery: {e}"))?;
                if !same_issuer(&discovery.issuer, &issuer.issuer) {
                    return Err(format!(
                        "OIDC discovery names the issuer {:?}, not LOAMS_OIDC_ISSUER {:?}",
                        discovery.issuer, issuer.issuer
                    ));
                }
                check_url("jwks_uri", &discovery.jwks_uri)?;
                discovery.jwks_uri
            }
        };
        self.http
            .get(&jwks_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("JWKS: {e}"))?
            .json::<JwkSet>()
            .await
            .map_err(|e| format!("JWKS: {e}"))
    }

    /// The cached keys, fetched when absent, stale, or (`force`) when a
    /// `kid` is unknown and the last fetch is old enough.
    async fn keys(&self, at: &IssuerKeys, force: bool) -> Result<JwkSet, AuthError> {
        if !force
            && let Some(cached) = at.cache.read().await.as_ref()
            && cached.fetched.elapsed() < JWKS_TTL
        {
            return Ok(cached.keys.clone());
        }
        let mut last = at.fetching.lock().await;
        // Another request may have refreshed while this one waited.
        if let Some(cached) = at.cache.read().await.as_ref() {
            let fresh = cached.fetched.elapsed() < JWKS_TTL;
            let recent = last.is_some_and(|t| t.elapsed() < JWKS_MIN_REFETCH);
            if (fresh && !force) || recent {
                return Ok(cached.keys.clone());
            }
        }
        *last = Some(Instant::now());
        match self.fetch(&at.issuer).await {
            Ok(keys) => {
                *at.cache.write().await = Some(Cached {
                    keys: keys.clone(),
                    fetched: Instant::now(),
                });
                Ok(keys)
            }
            Err(err) => {
                tracing::warn!(%err, issuer = at.issuer.issuer, "fetching the OIDC signing keys failed");
                match at.cache.read().await.as_ref() {
                    Some(cached) => Ok(cached.keys.clone()),
                    None => Err(AuthError::unavailable(
                        reason::AUTH_UNAVAILABLE,
                        "the OIDC issuer's signing keys cannot be fetched; try again",
                    )),
                }
            }
        }
    }

    fn find<'a>(keys: &'a JwkSet, kid: Option<&str>, alg: Algorithm) -> Option<&'a Jwk> {
        match kid {
            Some(kid) => keys.find(kid),
            // No kid: only an unambiguous key of the right family.
            None => {
                let mut candidates = keys
                    .keys
                    .iter()
                    .filter(|k| DecodingKey::from_jwk(k).is_ok_and(|_| family_matches(k, alg)));
                let first = candidates.next();
                candidates.next().is_none().then_some(first).flatten()
            }
        }
    }

    /// The principal for `token`.
    pub async fn verify(&self, token: &str) -> Result<Principal, AuthError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| {
            AuthError::unauthenticated(reason::TOKEN_MALFORMED, "the bearer token is not a JWT")
        })?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::EdDSA) {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_ALGORITHM,
                format!(
                    "tokens signed with {:?} are refused; use RS256 or EdDSA",
                    header.alg
                ),
            ));
        }
        let iss = unverified_issuer(token).ok_or_else(|| {
            AuthError::unauthenticated(reason::TOKEN_MALFORMED, "the token has no iss claim")
        })?;
        let at = self
            .issuers
            .iter()
            .find(|keys| keys.issuer.matches(&iss))
            .ok_or_else(|| {
                AuthError::unauthenticated(
                    reason::TOKEN_WRONG_ISSUER,
                    "the token is from an issuer this engine does not trust (LOAMS_OIDC_ISSUER)",
                )
            })?;
        let mut keys = self.keys(at, false).await?;
        let mut jwk = Self::find(&keys, header.kid.as_deref(), header.alg).cloned();
        if jwk.is_none() {
            keys = self.keys(at, true).await?;
            jwk = Self::find(&keys, header.kid.as_deref(), header.alg).cloned();
        }
        let jwk = jwk.ok_or_else(|| {
            AuthError::unauthenticated(
                reason::TOKEN_UNKNOWN_KEY,
                "the token is signed with a key the issuer does not publish",
            )
        })?;
        if !family_matches(&jwk, header.alg) {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_ALGORITHM,
                "the token's alg does not match its signing key",
            ));
        }
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| {
            AuthError::unauthenticated(
                reason::TOKEN_UNKNOWN_KEY,
                "the issuer's signing key is not usable",
            )
        })?;
        let mut validation = Validation::new(header.alg);
        // The exact `iss` of the token, which matched a trusted issuer.
        validation.set_issuer(&[&iss]);
        validation.set_audience(&self.config.audiences);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = LEEWAY_SECS;
        let data =
            jsonwebtoken::decode::<Claims>(token, &key, &validation).map_err(|e| map_error(&e))?;
        self.admit(data.claims, iss)
    }

    /// The allowlist (403 when the token is valid but not admitted).
    fn admit(&self, claims: Claims, iss: String) -> Result<Principal, AuthError> {
        let c = &self.config;
        let verified_email = claims
            .email
            .as_deref()
            .filter(|_| claims.email_verified == Some(true))
            .map(str::to_ascii_lowercase);
        let allowed = c.allowed_subjects.contains(&claims.sub)
            || verified_email
                .as_ref()
                .is_some_and(|e| c.allowed_emails.contains(e))
            || claims.groups.iter().any(|g| c.allowed_groups.contains(g));
        if !allowed {
            return Err(AuthError::forbidden(
                reason::PRINCIPAL_NOT_ALLOWED,
                "signed in, but this account is not allowed on this engine \
                 (LOAMS_OIDC_ALLOWED_EMAILS, _SUBJECTS or _GROUPS)",
            ));
        }
        let display_name = claims
            .name
            .clone()
            .or(claims.preferred_username.clone())
            .or(verified_email.clone())
            .unwrap_or_else(|| claims.sub.clone());
        let grant = claims.grant();
        Ok(Principal {
            subject: claims.sub,
            kind: PrincipalKind::User,
            issuer: Some(iss),
            display_name,
            email: verified_email,
            grant,
        })
    }
}

fn family_matches(jwk: &Jwk, alg: Algorithm) -> bool {
    use jsonwebtoken::jwk::AlgorithmParameters as P;
    match (&jwk.algorithm, alg) {
        (P::RSA(_), Algorithm::RS256) => true,
        (P::OctetKeyPair(p), Algorithm::EdDSA) => {
            matches!(p.curve, jsonwebtoken::jwk::EllipticCurve::Ed25519)
        }
        _ => false,
    }
}

fn map_error(err: &jsonwebtoken::errors::Error) -> AuthError {
    let (why, message) = match err.kind() {
        ErrorKind::ExpiredSignature => (
            reason::TOKEN_EXPIRED,
            "the token has expired; sign in again",
        ),
        ErrorKind::ImmatureSignature => (reason::TOKEN_NOT_YET_VALID, "the token is not valid yet"),
        ErrorKind::InvalidSignature => (
            reason::TOKEN_BAD_SIGNATURE,
            "the token's signature does not verify",
        ),
        ErrorKind::InvalidAudience => (
            reason::TOKEN_WRONG_AUDIENCE,
            "the token is not for this engine (LOAMS_OIDC_AUDIENCE)",
        ),
        ErrorKind::InvalidIssuer => (
            reason::TOKEN_WRONG_ISSUER,
            "the token is from another issuer (LOAMS_OIDC_ISSUER)",
        ),
        ErrorKind::InvalidAlgorithm => (
            reason::TOKEN_ALGORITHM,
            "the token's alg does not match its signing key",
        ),
        ErrorKind::MissingRequiredClaim(_) => (
            reason::TOKEN_MALFORMED,
            "the token lacks a required claim (exp, iss, aud, sub)",
        ),
        _ => (reason::TOKEN_MALFORMED, "the token cannot be decoded"),
    };
    AuthError::unauthenticated(why, message)
}

#[async_trait]
impl Authenticator for OidcAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Option<Result<Principal, AuthError>> {
        match credential {
            Credential::Jwt(token) => Some(self.verify(token).await),
            Credential::ApiKey(_) | Credential::Session(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(json: serde_json::Value) -> Claims {
        serde_json::from_value(json).expect("claims")
    }

    /// LV1 plan Task 14: a token's `org`, `env`, `scp` (or `scope`) and
    /// `exp` become its grant; a token without them gets the self-hosted
    /// default.
    #[test]
    fn claims_fill_the_grant() {
        let full = claims(serde_json::json!({
            "sub": "u", "exp": 1_800_000_000u64, "org": "acme", "env": "shop/prod",
            "scp": ["live:call", "live:read"],
        }))
        .grant();
        assert_eq!(full.org.as_deref(), Some("acme"));
        assert_eq!(full.environment.as_deref(), Some("shop/prod"));
        assert!(full.has_scope("live:call") && !full.has_scope("live:deploy"));
        assert_eq!(full.expires_at_ms, Some(1_800_000_000_000));
        let spaced = claims(serde_json::json!({"sub": "u", "scope": "openid live:read"})).grant();
        assert!(spaced.has_scope("live:read") && !spaced.has_scope("live:write"));
        let scp_text = claims(serde_json::json!({"sub": "u", "scp": "live:admin"})).grant();
        assert!(scp_text.has_scope("live:admin"));
        let empty_scp =
            claims(serde_json::json!({"sub": "u", "scp": [], "scope": "live:read"})).grant();
        assert!(empty_scp.has_scope("live:read") && !empty_scp.has_scope("live:admin"));
        let bare = claims(serde_json::json!({"sub": "u"})).grant();
        assert_eq!(bare, crate::auth::Grant::default());
        assert!(
            bare.has_scope("live:deploy"),
            "no scopes named: every scope"
        );
    }

    fn config() -> OidcConfig {
        OidcConfig {
            issuers: vec![Issuer::new("https://auth.loams.dev")],
            audiences: vec!["loams-console".to_owned()],
            allowed_emails: ["me@example.com".to_owned()].into(),
            ..OidcConfig::default()
        }
    }

    #[test]
    fn config_needs_https_and_an_allowlist() {
        assert!(config().validate().is_ok());
        let mut c = config();
        c.issuers.push(Issuer::new("http://auth.example.com/"));
        assert!(c.validate().unwrap_err().contains("https"));
        c.issuers[1] = Issuer::new("http://127.0.0.1:9000/");
        assert!(c.validate().is_ok());
        c.issuers.clear();
        assert!(c.validate().unwrap_err().contains("LOAMS_OIDC_ISSUER"));
        let mut c = config();
        c.allowed_emails.clear();
        assert!(c.validate().unwrap_err().contains("allowlist"));
    }

    #[test]
    fn issuers_match_with_or_without_the_trailing_slash() {
        let issuer = Issuer::new("https://auth.loams.dev/");
        assert_eq!(
            issuer.discovery_url(),
            "https://auth.loams.dev/.well-known/openid-configuration"
        );
        assert!(issuer.matches("https://auth.loams.dev"));
        assert!(issuer.matches("https://auth.loams.dev/"));
        assert!(!issuer.matches("https://auth.loams.dev.evil.example/"));
        assert!(!issuer.matches("https://auth.loams.dev/application/o/x/"));
        assert_eq!(issuer.known_jwks_url(), None);
        assert_eq!(
            Issuer::new("https://acme.cloudflareaccess.com").known_jwks_url(),
            Some("https://acme.cloudflareaccess.com/cdn-cgi/access/certs".to_owned())
        );
    }

    #[test]
    fn the_unverified_issuer_is_read_from_the_payload() {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"iss":"https://auth.loams.dev/","sub":"x"}"#);
        assert_eq!(
            unverified_issuer(&format!("e30.{payload}.sig")).as_deref(),
            Some("https://auth.loams.dev/")
        );
        assert_eq!(unverified_issuer("e30.e30.sig"), None);
        assert_eq!(unverified_issuer("garbage"), None);
    }
}
