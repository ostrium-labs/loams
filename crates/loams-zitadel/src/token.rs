//! A machine user's access token from its private-key JWT key (RFC 7523 §2.1, as Zitadel's
//! service-account guide documents it).
//!
//! The key file is the JSON Zitadel hands out once (`{"type": "serviceaccount", "keyId",
//! "key", "userId"}`). The assertion is an RS256 JWT with `kid` = `keyId`, `iss` = `sub` =
//! `userId`, `aud` = the issuer, and a short life ([`ASSERTION_TTL`]). The token endpoint
//! (`<issuer>/oauth/v2/token`) trades it, with grant type
//! `urn:ietf:params:oauth:grant-type:jwt-bearer`, for an access token whose audience includes
//! the Loams project ([`project_audience_scope`]).

use std::fmt;
use std::time::Duration;

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{Error, Result};

/// How long an assertion is valid: 5 minutes (plan IA4 Task 2).
pub const ASSERTION_TTL: Duration = Duration::from_secs(300);

/// The grant type of RFC 7523.
pub const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// The scope that puts a project's id in the token's `aud` (Zitadel's reserved scope).
pub fn project_audience_scope(project_id: &str) -> String {
    format!("urn:zitadel:iam:org:project:id:{project_id}:aud")
}

/// A machine key file.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineKey {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub key_id: String,
    /// The PEM private key.
    key: String,
    pub user_id: String,
}

impl fmt::Debug for MachineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineKey")
            .field("key_id", &self.key_id)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct Assertion<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

/// What the token endpoint answers.
#[derive(Clone, Deserialize)]
pub struct AccessToken {
    pub access_token: String,
    #[serde(default)]
    pub token_type: String,
    /// Seconds.
    #[serde(default)]
    pub expires_in: u64,
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

impl MachineKey {
    /// Parses a key file.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a file that is not a service-account key.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let key: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::Invalid(format!("not a machine key file: {e}")))?;
        if !key.kind.is_empty() && key.kind != "serviceaccount" {
            return Err(Error::Invalid(format!(
                "a {:?} key is not a machine user's key",
                key.kind
            )));
        }
        Ok(key)
    }

    /// The signed assertion for `issuer`, valid from `now` for [`ASSERTION_TTL`].
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the key does not sign.
    pub fn assertion(&self, issuer: &str, now: OffsetDateTime) -> Result<String> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.key_id.clone());
        let iat = now.unix_timestamp();
        let claims = Assertion {
            iss: &self.user_id,
            sub: &self.user_id,
            aud: issuer,
            iat,
            exp: iat + i64::try_from(ASSERTION_TTL.as_secs()).unwrap_or(300),
        };
        let key = EncodingKey::from_rsa_pem(self.key.as_bytes())
            .map_err(|e| Error::Invalid(format!("the machine key does not load: {e}")))?;
        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| Error::Invalid(format!("signing the assertion failed: {e}")))
    }

    /// Trades a fresh assertion for an access token with `scopes`.
    ///
    /// # Errors
    ///
    /// The token endpoint's refusal (a deactivated user, a removed key), or a transport error.
    pub async fn access_token(
        &self,
        http: &reqwest::Client,
        issuer: &str,
        scopes: &[&str],
    ) -> Result<AccessToken> {
        let assertion = self.assertion(issuer, OffsetDateTime::now_utc())?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", JWT_BEARER)
            .append_pair("scope", &scopes.join(" "))
            .append_pair("assertion", &assertion)
            .finish();
        let res = http
            .post(format!("{}/oauth/v2/token", issuer.trim_end_matches('/')))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into_owned(),
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_an_application_key() {
        let file = br#"{"type":"application","keyId":"k","key":"x","userId":"u"}"#;
        assert!(MachineKey::from_json(file).is_err());
        let file = br#"{"type":"serviceaccount","keyId":"k","key":"x","userId":"u"}"#;
        let key = MachineKey::from_json(file).unwrap();
        assert_eq!((key.key_id.as_str(), key.user_id.as_str()), ("k", "u"));
        assert!(!format!("{key:?}").contains("\"x\""));
        assert!(
            key.assertion("https://i", OffsetDateTime::now_utc())
                .is_err()
        );
    }

    #[test]
    fn project_scope() {
        assert_eq!(
            project_audience_scope("123"),
            "urn:zitadel:iam:org:project:id:123:aud"
        );
    }
}
