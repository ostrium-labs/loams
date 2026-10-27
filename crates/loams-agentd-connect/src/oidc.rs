//! OIDC access tokens for the remote listener (design §53 §6.3, D909, D921;
//! plan SD1 Task 7, ruling T0-6).
//!
//! The verifier is a port of the engine's (`crates/loams/src/auth/oidc.rs`),
//! narrowed to one issuer, one audience and one subject: a remote agentd
//! serves exactly one person (D921). Like the engine's:
//! - only RS256 and EdDSA are accepted, and the token's `alg` must match its
//!   key's family, so `none`, HMAC and algorithm confusion are refused;
//! - `iss`, `aud`, `exp` and `sub` are required and `nbf` is checked, with
//!   [`LEEWAY_SECS`] of clock skew;
//! - the JWKS comes from the issuer's discovery document, is cached for
//!   [`JWKS_TTL`], and is fetched again early for an unknown `kid` at most
//!   once per [`JWKS_MIN_REFETCH`]. A failed refresh keeps the cached keys.
//!
//! Then `sub` must equal the enrolled subject: a valid token for anyone else
//! is a 403, since the issuer signs tokens for every user it has.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use connectrpc::ErrorCode;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

/// How long a fetched JWKS is used before it is fetched again.
pub const JWKS_TTL: Duration = Duration::from_secs(600);
/// The shortest gap between two fetches: an unknown `kid` forces one at most
/// this often.
pub const JWKS_MIN_REFETCH: Duration = Duration::from_secs(60);
/// Clock skew allowed on `exp` and `nbf`.
pub const LEEWAY_SECS: u64 = 60;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the issuer's signing keys come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksSource {
    /// The `jwks_uri` of `<issuer>/.well-known/openid-configuration`
    /// (Zitadel: `/oauth/v2/keys`).
    Discovery,
    /// A local JWKS file, read on each fetch (tests:
    /// `LOAMS_AGENTD_TEST_ISSUER`).
    File(PathBuf),
}

/// What the remote listener accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcOptions {
    /// The `iss` the tokens carry (compared with or without a trailing `/`).
    pub issuer: String,
    /// `aud` must contain it (`loams-agentd`).
    pub audience: String,
    /// The one `sub` admitted.
    pub subject: String,
    pub jwks: JwksSource,
    /// [`JWKS_MIN_REFETCH`] unless a test shortens it.
    pub min_refetch: Duration,
}

impl OidcOptions {
    /// Options with the default refetch interval and the discovered JWKS.
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            subject: subject.into(),
            jwks: JwksSource::Discovery,
            min_refetch: JWKS_MIN_REFETCH,
        }
    }

    /// Refuses options the verifier cannot run safely.
    ///
    /// # Errors
    ///
    /// The setting at fault, by its flag name.
    pub fn validate(&self) -> Result<(), String> {
        check_url("--issuer", &self.issuer)?;
        if self.audience.trim().is_empty() {
            return Err("--audience is empty".to_owned());
        }
        if self.subject.trim().is_empty() {
            return Err("--subject is empty".to_owned());
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

/// Issuers compare without a trailing `/`.
fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Why a token was refused, as the Connect error the client sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: ErrorCode,
    /// The agentd reason (`unauthenticated`, `forbidden_role`), or `None`
    /// when the keys are unavailable.
    pub reason: Option<&'static str>,
    pub message: String,
}

impl Rejection {
    fn unauthenticated(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Unauthenticated,
            reason: Some("unauthenticated"),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
struct Cached {
    keys: JwkSet,
    fetched: Instant,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    /// Seconds since the epoch; required by the validation.
    exp: u64,
}

/// Verifies the remote listener's bearer tokens.
#[derive(Debug)]
pub struct Verifier {
    options: OidcOptions,
    http: jwks_http::Client,
    cache: RwLock<Option<Cached>>,
    /// Serializes fetches; holds when the last one started.
    fetching: Mutex<Option<Instant>>,
}

impl Verifier {
    /// A verifier for `options`. Nothing is fetched until the first token.
    ///
    /// # Errors
    ///
    /// Invalid options ([`OidcOptions::validate`]), or no HTTP client.
    pub fn new(options: OidcOptions) -> Result<Self, String> {
        options.validate()?;
        let http = jwks_http::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(jwks_http::redirect::Policy::none())
            .build()
            .map_err(|e| format!("the OIDC HTTP client: {e}"))?;
        Ok(Self {
            options,
            http,
            cache: RwLock::new(None),
            fetching: Mutex::new(None),
        })
    }

    /// The enrolled subject.
    pub fn subject(&self) -> &str {
        &self.options.subject
    }

    async fn fetch(&self) -> Result<JwkSet, String> {
        let issuer = &self.options.issuer;
        match &self.options.jwks {
            JwksSource::File(path) => {
                let bytes = tokio::fs::read(path)
                    .await
                    .map_err(|e| format!("JWKS {}: {e}", path.display()))?;
                serde_json::from_slice(&bytes).map_err(|e| format!("JWKS {}: {e}", path.display()))
            }
            JwksSource::Discovery => {
                let url = format!(
                    "{}/.well-known/openid-configuration",
                    issuer.trim_end_matches('/')
                );
                let discovery: Discovery = self
                    .http
                    .get(url)
                    .send()
                    .await
                    .and_then(jwks_http::Response::error_for_status)
                    .map_err(|e| format!("OIDC discovery: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("OIDC discovery: {e}"))?;
                if !same_issuer(&discovery.issuer, issuer) {
                    return Err(format!(
                        "OIDC discovery names the issuer {:?}, not --issuer {issuer:?}",
                        discovery.issuer
                    ));
                }
                check_url("jwks_uri", &discovery.jwks_uri)?;
                self.http
                    .get(&discovery.jwks_uri)
                    .send()
                    .await
                    .and_then(jwks_http::Response::error_for_status)
                    .map_err(|e| format!("JWKS: {e}"))?
                    .json::<JwkSet>()
                    .await
                    .map_err(|e| format!("JWKS: {e}"))
            }
        }
    }

    /// The cached keys, fetched when absent, stale, or (`force`) when a
    /// `kid` is unknown and the last fetch is old enough.
    async fn keys(&self, force: bool) -> Result<JwkSet, Rejection> {
        if !force
            && let Some(cached) = self.cache.read().await.as_ref()
            && cached.fetched.elapsed() < JWKS_TTL
        {
            return Ok(cached.keys.clone());
        }
        let mut last = self.fetching.lock().await;
        // Another request may have refreshed while this one waited.
        if let Some(cached) = self.cache.read().await.as_ref() {
            let fresh = cached.fetched.elapsed() < JWKS_TTL;
            let recent = last.is_some_and(|t| t.elapsed() < self.options.min_refetch);
            if (fresh && !force) || recent {
                return Ok(cached.keys.clone());
            }
        }
        *last = Some(Instant::now());
        match self.fetch().await {
            Ok(keys) => {
                *self.cache.write().await = Some(Cached {
                    keys: keys.clone(),
                    fetched: Instant::now(),
                });
                Ok(keys)
            }
            Err(error) => {
                tracing::warn!(%error, "fetching the OIDC signing keys failed");
                match self.cache.read().await.as_ref() {
                    Some(cached) => Ok(cached.keys.clone()),
                    None => Err(Rejection {
                        code: ErrorCode::Unavailable,
                        reason: None,
                        message: "the OIDC issuer's signing keys cannot be fetched; try again"
                            .to_owned(),
                    }),
                }
            }
        }
    }

    /// Checks `token`; `Ok` means it is the enrolled subject's.
    ///
    /// # Errors
    ///
    /// 401 (`unauthenticated`) for a token that does not verify, 403
    /// (`forbidden_role`) for a valid token of another subject, 503 when no
    /// signing keys can be had.
    pub async fn verify(&self, token: &str) -> Result<(), Rejection> {
        self.verify_until(token).await.map(|_| ())
    }

    /// As [`Verifier::verify`], answering the token's `exp` (seconds since the
    /// epoch): a preview ticket minted with it lasts no longer (SD1 Task 26).
    ///
    /// # Errors
    ///
    /// As [`Verifier::verify`].
    pub async fn verify_until(&self, token: &str) -> Result<u64, Rejection> {
        let header = token_header(token)?;
        let mut keys = self.keys(false).await?;
        let mut jwk = find_key(&keys, header.kid.as_deref(), header.alg).cloned();
        if jwk.is_none() {
            keys = self.keys(true).await?;
            jwk = find_key(&keys, header.kid.as_deref(), header.alg).cloned();
        }
        verify_with_key(&self.options, &header, jwk.as_ref(), token)
    }
}

/// The header of a bearer `token`, refused unless it is a JWT signed with
/// RS256 or EdDSA (so `none`, HMAC and the other algorithms never reach a
/// key).
///
/// # Errors
///
/// 401 (`unauthenticated`).
pub fn token_header(token: &str) -> Result<jsonwebtoken::Header, Rejection> {
    let header = jsonwebtoken::decode_header(token)
        .map_err(|_| Rejection::unauthenticated("the bearer token is not a JWT"))?;
    if !matches!(header.alg, Algorithm::RS256 | Algorithm::EdDSA) {
        return Err(Rejection::unauthenticated(format!(
            "tokens signed with {:?} are refused; use RS256 or EdDSA",
            header.alg
        )));
    }
    Ok(header)
}

/// The key of `keys` that `kid` names; without a `kid`, the one usable key
/// of `alg`'s family, if there is exactly one.
pub fn find_key<'a>(keys: &'a JwkSet, kid: Option<&str>, alg: Algorithm) -> Option<&'a Jwk> {
    match kid {
        Some(kid) => keys.find(kid),
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

/// Verifies `token`, whose header is `header`, with `jwk`, the issuer's key
/// it names, against `options`: the signature, `iss`, `aud`, `exp`, `nbf`,
/// then `sub`. Pure, so the `jwt` fuzz target runs it on a fixed key set
/// (SD1 Task 30).
///
/// # Errors
///
/// As [`Verifier::verify`], apart from unavailable keys.
pub fn verify_with_key(
    options: &OidcOptions,
    header: &jsonwebtoken::Header,
    jwk: Option<&Jwk>,
    token: &str,
) -> Result<u64, Rejection> {
    let jwk = jwk.ok_or_else(|| {
        Rejection::unauthenticated("the token is signed with a key the issuer does not publish")
    })?;
    if !family_matches(jwk, header.alg) {
        return Err(Rejection::unauthenticated(
            "the token's alg does not match its signing key",
        ));
    }
    let key = DecodingKey::from_jwk(jwk)
        .map_err(|_| Rejection::unauthenticated("the issuer's signing key is not usable"))?;
    let issuer = options.issuer.trim_end_matches('/');
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[issuer.to_owned(), format!("{issuer}/")]);
    validation.set_audience(&[&options.audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = LEEWAY_SECS;
    let data =
        jsonwebtoken::decode::<Claims>(token, &key, &validation).map_err(|e| map_error(&e))?;
    if data.claims.sub != options.subject {
        return Err(Rejection {
            code: ErrorCode::PermissionDenied,
            reason: Some("forbidden_role"),
            message: "signed in, but this agentd serves another account (--subject)".to_owned(),
        });
    }
    Ok(data.claims.exp)
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

fn map_error(err: &jsonwebtoken::errors::Error) -> Rejection {
    let message = match err.kind() {
        ErrorKind::ExpiredSignature => "the token has expired; sign in again",
        ErrorKind::ImmatureSignature => "the token is not valid yet",
        ErrorKind::InvalidSignature => "the token's signature does not verify",
        ErrorKind::InvalidAudience => "the token is not for this agentd (--audience)",
        ErrorKind::InvalidIssuer => "the token is from another issuer (--issuer)",
        ErrorKind::InvalidAlgorithm => "the token's alg does not match its signing key",
        ErrorKind::MissingRequiredClaim(_) => {
            "the token lacks a required claim (exp, iss, aud, sub)"
        }
        _ => "the token cannot be decoded",
    };
    Rejection::unauthenticated(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_need_https_an_audience_and_a_subject() {
        let ok = OidcOptions::new(
            "https://auth.example.com/application/o/loams/",
            "loams-agentd",
            "s1",
        );
        assert!(ok.validate().is_ok());
        let mut o = ok.clone();
        o.issuer = "http://auth.example.com/".into();
        assert!(o.validate().unwrap_err().contains("https"));
        o.issuer = "http://127.0.0.1:9000/application/o/loams/".into();
        assert!(o.validate().is_ok());
        let mut o = ok.clone();
        o.subject = " ".into();
        assert!(o.validate().unwrap_err().contains("--subject"));
        let mut o = ok;
        o.audience = String::new();
        assert!(o.validate().unwrap_err().contains("--audience"));
    }

    #[test]
    fn issuers_match_with_or_without_the_trailing_slash() {
        assert!(same_issuer("https://a.example/x/", "https://a.example/x"));
        assert!(!same_issuer("https://a.example/x/", "https://a.example/y/"));
    }
}
