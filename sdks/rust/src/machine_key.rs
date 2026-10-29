//! [`MachineKey`]: an agent's token from its private-key JWT key (design 76 §10, D1607; plan
//! IA1, IA4 Task 2). Feature `machine-key`.
//!
//! An agent is a machine user at the identity provider (Zitadel). Its key file is the JSON the
//! provider hands out once (`{"type": "serviceaccount", "keyId", "key", "userId"}`), named by
//! `LOAMS_AGENT_KEY_FILE`. This source:
//!
//! 1. signs an RFC 7523 assertion with the key: RS256, `kid` = `keyId`, `iss` = `sub` =
//!    `userId`, `aud` = the issuer, valid for 5 minutes;
//! 2. trades it at `<issuer>/oauth/v2/token` (grant type
//!    `urn:ietf:params:oauth:grant-type:jwt-bearer`) for an access token whose audience is the
//!    Loams project (`urn:zitadel:iam:org:project:id:<project>:aud`), which every Loams
//!    listener checks;
//! 3. caches it and trades again a minute before it expires (or at half its life, if that is
//!    sooner), and whenever the server answers `token_expired`.
//!
//! The private key never leaves the process and is never logged (`Debug` leaves it out). A
//! sandboxed agent holds no key at all: agentd vends it a 15-minute token file instead
//! (D1618), which `EnvToken` and the token-file source read.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LoamsError;
use crate::token::{FormPoster, TokenSource, form_encode};

/// How long an assertion is valid.
pub const ASSERTION_TTL: Duration = Duration::from_secs(300);

/// How long before expiry a cached token is traded again.
pub const REFRESH_EARLY: Duration = Duration::from_secs(60);

/// The environment variables [`MachineKey::from_env`] reads.
pub const KEY_FILE_ENV: &str = "LOAMS_AGENT_KEY_FILE";
/// The identity provider's issuer URL (`https://auth.loams.dev`).
pub const ISSUER_ENV: &str = "LOAMS_AGENT_ISSUER";
/// The Loams project's id at the identity provider: the audience Loams checks.
pub const PROJECT_ENV: &str = "LOAMS_AGENT_PROJECT";

/// The RFC 7523 grant type.
pub const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyFile {
    #[serde(rename = "type", default)]
    kind: String,
    key_id: String,
    key: String,
    user_id: String,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

struct Cached {
    token: Arc<str>,
    refresh_at: Instant,
}

/// An agent's token source from its machine key (module docs).
pub struct MachineKey<P> {
    key_id: String,
    user_id: String,
    signing: jsonwebtoken::EncodingKey,
    issuer: String,
    scope: String,
    poster: P,
    cache: tokio::sync::Mutex<Option<Cached>>,
}

impl<P> std::fmt::Debug for MachineKey<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineKey")
            .field("key_id", &self.key_id)
            .field("user_id", &self.user_id)
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

fn invalid(message: impl Into<String>) -> LoamsError {
    LoamsError::new(
        crate::error::ErrorKind::InvalidArgument,
        connectrpc::ErrorCode::InvalidArgument,
        message,
    )
}

impl<P: FormPoster> MachineKey<P> {
    /// A source from a key file's bytes, for `issuer` and the Loams project `project`.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when the file is not a machine user's key, or its key does not load.
    pub fn new(
        key_file: &[u8],
        issuer: impl Into<String>,
        project: &str,
        poster: P,
    ) -> Result<Self, LoamsError> {
        let file: KeyFile = serde_json::from_slice(key_file)
            .map_err(|e| invalid(format!("not a machine key file: {e}")))?;
        if !file.kind.is_empty() && file.kind != "serviceaccount" {
            return Err(invalid(format!(
                "a {:?} key is not a machine user's key",
                file.kind
            )));
        }
        if project.is_empty() {
            return Err(invalid("the Loams project id (the audience) is empty"));
        }
        let signing = jsonwebtoken::EncodingKey::from_rsa_pem(file.key.as_bytes())
            .map_err(|e| invalid(format!("the machine key does not load: {e}")))?;
        let issuer = issuer.into().trim_end_matches('/').to_owned();
        if !(issuer.starts_with("https://") || issuer.starts_with("http://")) {
            return Err(invalid(format!("the issuer {issuer:?} is not a URL")));
        }
        Ok(Self {
            key_id: file.key_id,
            user_id: file.user_id,
            signing,
            issuer,
            scope: format!("openid urn:zitadel:iam:org:project:id:{project}:aud"),
            poster,
            cache: tokio::sync::Mutex::new(None),
        })
    }

    /// A source from the key file at `path`.
    ///
    /// # Errors
    ///
    /// As [`MachineKey::new`], and `invalid_argument` when the file cannot be read.
    pub fn from_file(
        path: impl AsRef<Path>,
        issuer: impl Into<String>,
        project: &str,
        poster: P,
    ) -> Result<Self, LoamsError> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| invalid(format!("reading {}: {e}", path.display())))?;
        Self::new(&bytes, issuer, project, poster)
    }

    /// A source from `LOAMS_AGENT_KEY_FILE`, `LOAMS_AGENT_ISSUER` and `LOAMS_AGENT_PROJECT`.
    ///
    /// # Errors
    ///
    /// `invalid_argument` naming the variable that is missing, or as [`MachineKey::from_file`].
    pub fn from_env(poster: P) -> Result<Self, LoamsError> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| invalid(format!("{name} is not set")))
        };
        Self::from_file(
            var(KEY_FILE_ENV)?,
            var(ISSUER_ENV)?,
            &var(PROJECT_ENV)?,
            poster,
        )
    }

    /// The machine user's id: the tokens' `sub`.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The signed assertion, issued at `now`.
    fn assertion(&self, now: SystemTime) -> Result<String, String> {
        let iat = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "the clock is before 1970".to_owned())?
            .as_secs();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(self.key_id.clone());
        let claims = Claims {
            iss: &self.user_id,
            sub: &self.user_id,
            aud: &self.issuer,
            iat,
            exp: iat + ASSERTION_TTL.as_secs(),
        };
        jsonwebtoken::encode(&header, &claims, &self.signing)
            .map_err(|e| format!("signing the assertion failed: {e}"))
    }

    fn form(&self, assertion: &str) -> String {
        form_encode(&[
            ("grant_type", JWT_BEARER),
            ("scope", self.scope.as_str()),
            ("assertion", assertion),
        ])
    }

    async fn exchange(&self) -> Result<Cached, String> {
        let assertion = self.assertion(SystemTime::now())?;
        let started = Instant::now();
        let body = self
            .poster
            .post_form(
                &format!("{}/oauth/v2/token", self.issuer),
                &self.form(&assertion),
            )
            .await?;
        let answer: TokenAnswer = serde_json::from_str(&body)
            .map_err(|_| "the token endpoint answered no access_token".to_owned())?;
        if answer.access_token.is_empty() {
            return Err("the token endpoint answered an empty access_token".to_owned());
        }
        // Without `expires_in`, assume the shortest life Loams issues agents (15 minutes).
        let life = Duration::from_secs(answer.expires_in.unwrap_or(900));
        let early = REFRESH_EARLY.min(life / 2);
        Ok(Cached {
            token: Arc::from(answer.access_token),
            refresh_at: started + life.saturating_sub(early),
        })
    }
}

#[async_trait]
impl<P: FormPoster> TokenSource for MachineKey<P> {
    async fn token(&self) -> Option<String> {
        // Held across the exchange: concurrent callers wait for the one trade.
        let mut cache = self.cache.lock().await;
        if let Some(c) = cache.as_ref()
            && Instant::now() < c.refresh_at
        {
            return Some(c.token.to_string());
        }
        match self.exchange().await {
            Ok(c) => {
                let token = c.token.to_string();
                *cache = Some(c);
                Some(token)
            }
            // A failed trade sends no credential; the server's `unauthenticated` names it,
            // and `refresh` reports the reason.
            Err(_) => {
                *cache = None;
                None
            }
        }
    }

    async fn refresh(&self) -> Result<(), LoamsError> {
        let mut cache = self.cache.lock().await;
        match self.exchange().await {
            Ok(c) => {
                *cache = Some(c);
                Ok(())
            }
            Err(err) => {
                *cache = None;
                Err(LoamsError::internal(format!(
                    "trading the machine key for a token failed: {err}"
                )))
            }
        }
    }

    fn can_refresh(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    const KEY: &str = include_str!("../tests/data/test-only-rsa-key.pem");
    const PUBLIC: &str = include_str!("../tests/data/test-only-rsa-key.pub");

    fn key_file(kind: &str) -> Vec<u8> {
        serde_json::json!({"type": kind, "keyId": "k-1", "key": KEY, "userId": "agent-7"})
            .to_string()
            .into_bytes()
    }

    #[derive(Debug, Default)]
    struct Endpoint {
        calls: AtomicU32,
        last: Mutex<Option<(String, String)>>,
        expires_in: u64,
        fail: bool,
    }

    #[async_trait]
    impl FormPoster for Arc<Endpoint> {
        async fn post_form(&self, url: &str, form: &str) -> Result<String, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            *self
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((url.to_owned(), form.to_owned()));
            if self.fail {
                return Err("400: invalid_grant".to_owned());
            }
            Ok(format!(
                r#"{{"access_token":"at-{n}","token_type":"Bearer","expires_in":{}}}"#,
                self.expires_in
            ))
        }
    }

    fn source(endpoint: &Arc<Endpoint>) -> MachineKey<Arc<Endpoint>> {
        MachineKey::new(
            &key_file("serviceaccount"),
            "https://auth.example.com/",
            "3141",
            Arc::clone(endpoint),
        )
        .expect("a key file")
    }

    fn decode_form(form: &str) -> std::collections::HashMap<String, String> {
        form.split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_owned(), percent_decode(v)))
            .collect()
    }

    fn percent_decode(v: &str) -> String {
        let bytes = v.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap_or_default()
    }

    #[tokio::test]
    async fn the_assertion_is_rfc_7523_signed_with_the_key() {
        let endpoint = Arc::new(Endpoint {
            expires_in: 900,
            ..Endpoint::default()
        });
        let s = source(&endpoint);
        assert_eq!(s.token().await.as_deref(), Some("at-1"));
        let (url, form) = endpoint
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("one call");
        assert_eq!(url, "https://auth.example.com/oauth/v2/token");
        let form = decode_form(&form);
        assert_eq!(form["grant_type"], JWT_BEARER);
        assert_eq!(
            form["scope"],
            "openid urn:zitadel:iam:org:project:id:3141:aud"
        );
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_audience(&["https://auth.example.com"]);
        validation.set_issuer(&["agent-7"]);
        let public = jsonwebtoken::DecodingKey::from_rsa_pem(PUBLIC.as_bytes()).expect("public");
        let data =
            jsonwebtoken::decode::<serde_json::Value>(&form["assertion"], &public, &validation)
                .expect("signed by the key, for the issuer");
        assert_eq!(data.header.kid.as_deref(), Some("k-1"));
        assert_eq!(data.claims["sub"], "agent-7");
        let life =
            data.claims["exp"].as_u64().unwrap_or(0) - data.claims["iat"].as_u64().unwrap_or(0);
        assert_eq!(life, ASSERTION_TTL.as_secs());
    }

    #[tokio::test]
    async fn caches_until_shortly_before_expiry_and_refreshes_on_demand() {
        let endpoint = Arc::new(Endpoint {
            expires_in: 900,
            ..Endpoint::default()
        });
        let s = source(&endpoint);
        assert_eq!(s.token().await.as_deref(), Some("at-1"));
        assert_eq!(s.token().await.as_deref(), Some("at-1"));
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
        s.refresh().await.expect("a fresh trade");
        assert_eq!(s.token().await.as_deref(), Some("at-2"));
        // A token that expires at once is traded again on the next call.
        let short = Arc::new(Endpoint {
            expires_in: 0,
            ..Endpoint::default()
        });
        let s = source(&short);
        assert_eq!(s.token().await.as_deref(), Some("at-1"));
        assert_eq!(s.token().await.as_deref(), Some("at-2"));
        assert!(s.can_refresh());
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_trade() {
        let endpoint = Arc::new(Endpoint {
            expires_in: 900,
            ..Endpoint::default()
        });
        let s = Arc::new(source(&endpoint));
        let calls: Vec<_> = (0..8)
            .map(|_| {
                let s = Arc::clone(&s);
                tokio::spawn(async move { s.token().await })
            })
            .collect();
        for c in calls {
            assert_eq!(c.await.ok().flatten().as_deref(), Some("at-1"));
        }
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refused_trade_sends_no_credential_and_reports() {
        let endpoint = Arc::new(Endpoint {
            fail: true,
            ..Endpoint::default()
        });
        let s = source(&endpoint);
        assert_eq!(s.token().await, None);
        let err = s.refresh().await.expect_err("reported");
        assert!(err.to_string().contains("invalid_grant"), "{err}");
    }

    #[test]
    fn refuses_what_is_not_a_machine_key() {
        let endpoint = Arc::new(Endpoint::default());
        let p = || Arc::clone(&endpoint);
        assert!(MachineKey::new(&key_file("application"), "https://i", "p", p()).is_err());
        assert!(MachineKey::new(b"{}", "https://i", "p", p()).is_err());
        assert!(MachineKey::new(&key_file("serviceaccount"), "https://i", "", p()).is_err());
        assert!(MachineKey::new(&key_file("serviceaccount"), "auth.example", "p", p()).is_err());
        let bad = serde_json::json!({"keyId": "k", "key": "not a pem", "userId": "u"}).to_string();
        assert!(MachineKey::new(bad.as_bytes(), "https://i", "p", p()).is_err());
        let s = MachineKey::new(&key_file("serviceaccount"), "https://i", "p", p()).expect("ok");
        let shown = format!("{s:?}");
        assert!(
            !shown.contains("PRIVATE KEY") && shown.contains("agent-7"),
            "{shown}"
        );
    }
}
