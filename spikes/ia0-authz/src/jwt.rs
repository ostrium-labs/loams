//! Zitadel JWT verification (design 76 §9.2 step 1): JWKS from the issuer's discovery document,
//! cached for five minutes and refetched once on an unknown `kid`; `iss`, `aud`, `exp` with 60 s
//! of skew, and RS256 only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::RwLock;

const JWKS_TTL: Duration = Duration::from_secs(300);
const LEEWAY_SECS: u64 = 60;

/// The claims the auth layer reads. Zitadel puts the org in
/// `urn:zitadel:iam:user:resourceowner:id` when the token was requested with that scope.
#[derive(Clone, Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    #[serde(default)]
    pub aud: serde_json::Value,
    pub exp: u64,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(rename = "urn:zitadel:iam:user:resourceowner:id", default)]
    pub org: Option<String>,
}

#[derive(Clone)]
pub struct Verifier {
    issuer: String,
    /// The `iss` a token must carry; the issuer itself unless set by [`Verifier::expect_issuer`].
    expected_iss: String,
    audience: String,
    /// Where to fetch discovery and keys from, when the issuer URL is not reachable from here
    /// (inside the compose network the issuer `http://localhost:18080` is `http://zitadel:8080`).
    fetch_base: Option<String>,
    http: reqwest::Client,
    jwks: Arc<RwLock<Option<(Instant, JwkSet)>>>,
}

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
}

impl Verifier {
    pub fn new(issuer: &str, audience: &str, fetch_base: Option<&str>) -> Self {
        Self {
            issuer: issuer.trim_end_matches('/').to_owned(),
            expected_iss: issuer.trim_end_matches('/').to_owned(),
            audience: audience.to_owned(),
            fetch_base: fetch_base.map(|b| b.trim_end_matches('/').to_owned()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
            jwks: Arc::new(RwLock::new(None)),
        }
    }

    /// Requires a different `iss` than the issuer the keys come from (tests only).
    #[must_use]
    pub fn expect_issuer(mut self, iss: &str) -> Self {
        self.expected_iss = iss.to_owned();
        self
    }

    /// Maps an issuer URL onto `fetch_base` and sets the Host header Zitadel needs to find the
    /// instance.
    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        match &self.fetch_base {
            Some(base) if url.starts_with(&self.issuer) => {
                let host = self
                    .issuer
                    .split("://")
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                self.http
                    .get(format!("{base}{}", &url[self.issuer.len()..]))
                    .header("Host", host)
            }
            _ => self.http.get(url),
        }
    }

    async fn fetch_jwks(&self) -> Result<JwkSet> {
        let disco: Discovery = self
            .get(&format!("{}/.well-known/openid-configuration", self.issuer))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("discovery document")?;
        let set: JwkSet = self
            .get(&disco.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(set)
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey> {
        {
            let guard = self.jwks.read().await;
            if let Some((at, set)) = guard.as_ref()
                && at.elapsed() < JWKS_TTL
                && let Some(jwk) = set.find(kid)
            {
                return Ok(DecodingKey::from_jwk(jwk)?);
            }
        }
        // Unknown kid or stale set: refetch once (key rotation).
        let set = self.fetch_jwks().await?;
        let key = set.find(kid).map(DecodingKey::from_jwk).transpose()?;
        *self.jwks.write().await = Some((Instant::now(), set));
        key.ok_or_else(|| anyhow!("no JWKS key with kid {kid}"))
    }

    pub async fn verify(&self, token: &str) -> Result<Claims> {
        let header = decode_header(token).context("not a JWT")?;
        if header.alg != Algorithm::RS256 {
            bail!("algorithm {:?} is not allowed", header.alg);
        }
        let kid = header.kid.context("JWT has no kid")?;
        let key = self.key(&kid).await?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&[&self.expected_iss]);
        v.set_audience(&[&self.audience]);
        v.leeway = LEEWAY_SECS;
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data = decode::<Claims>(token, &key, &v)?;
        Ok(data.claims)
    }
}
