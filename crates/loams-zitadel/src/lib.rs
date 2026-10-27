//! Loams' client for Zitadel's public HTTP APIs (design 76 §10, D1607; plan IA1, IA4 Task 1).
//!
//! Zitadel (AGPL-3.0-only) runs unmodified as its own service. This crate talks to it over
//! its documented v2 HTTP API (the user service, `/v2/users/…`) and its OAuth token endpoint,
//! as described by Zitadel's public documentation and its Apache-2.0 protos
//! (`proto/zitadel/user/v2`). It contains no Zitadel code.
//!
//! - [`Zitadel`]: machine users with JWT access tokens, their private-key JWT keys, and
//!   deactivation (the authoritative revocation, §10).
//! - [`agents`]: a Loams agent as a Zitadel machine user, with its principal link and its
//!   OpenFGA tuples: created with no grant, granted one environment at a time, revoked in one
//!   call (D1607, D1624, D1628).
//! - [`token`]: the RFC 7523 assertion a machine key signs, traded for an access token.
//!
//! The caller passes the `reqwest::Client`, so TLS is configured where the process configures
//! it. The management credential (a personal access token of a Zitadel admin machine user)
//! is never logged: [`Zitadel`]'s `Debug` leaves it out.

pub mod agents;
pub mod events;
pub mod token;

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use url::Url;

/// What can go wrong talking to Zitadel.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Zitadel answered with an error status.
    #[error("zitadel answered {status}: {message}")]
    Status { status: u16, message: String },
    /// Zitadel could not be reached, or the connection failed.
    #[error("zitadel is unreachable: {0}")]
    Transport(String),
    /// The answer was not what the API documents.
    #[error("zitadel's answer did not parse: {0}")]
    Decode(String),
    /// A local mistake (a bad URL, an unsigned key, an authorization write).
    #[error("{0}")]
    Invalid(String),
    /// An OpenFGA or built-in authorizer write failed.
    #[error("authorization store: {0}")]
    Authz(#[from] loams_authz::AuthzError),
}

impl Error {
    /// Whether Zitadel said the object is not there (404).
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Status { status: 404, .. })
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A user's state, as `GetUserByID` reports it (`zitadel.user.v2.UserState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum UserState {
    #[serde(rename = "USER_STATE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "USER_STATE_ACTIVE")]
    Active,
    #[serde(rename = "USER_STATE_INACTIVE")]
    Inactive,
    #[serde(rename = "USER_STATE_DELETED")]
    Deleted,
    #[serde(rename = "USER_STATE_LOCKED")]
    Locked,
    #[serde(rename = "USER_STATE_INITIAL")]
    Initial,
}

/// A key `AddKey` registered.
#[derive(Clone, PartialEq, Eq)]
pub struct AddedKey {
    pub key_id: String,
    /// The JSON key file Zitadel generated (with the private key), when the request sent no
    /// public key. Zitadel cannot hand it out again.
    pub key_file: Option<Vec<u8>>,
}

impl fmt::Debug for AddedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddedKey")
            .field("key_id", &self.key_id)
            .field("key_file", &self.key_file.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// Zitadel's v2 user API, with a management token.
#[derive(Clone)]
pub struct Zitadel {
    base: Url,
    token: String,
    http: reqwest::Client,
}

impl fmt::Debug for Zitadel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Zitadel")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUserResponse {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddKeyResponse {
    key_id: String,
    #[serde(default)]
    key_content: Option<String>,
}

#[derive(Deserialize)]
struct GetUserResponse {
    user: UserBody,
}

#[derive(Deserialize)]
struct UserBody {
    state: UserState,
}

impl Zitadel {
    /// `base` is the instance's URL (`https://auth.loams.dev`); `token` a personal access token
    /// with `user.write` in the org that will own the agents.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a URL that is not `http` or `https`.
    pub fn new(base: &str, token: impl Into<String>, http: reqwest::Client) -> Result<Self> {
        let mut base = Url::parse(base).map_err(|e| Error::Invalid(format!("{base:?}: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(Error::Invalid(format!("{base} is not an http(s) URL")));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            token: token.into(),
            http,
        })
    }

    /// The instance's URL.
    pub fn base(&self) -> &Url {
        &self.base
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path.trim_start_matches('/'))
            .map_err(|e| Error::Invalid(format!("{path}: {e}")))
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.send_in(None, method, path, body).await
    }

    /// [`Self::send`] in the context of organization `org` (the management API's
    /// `x-zitadel-orgid` header) when given.
    async fn send_in(
        &self,
        org: Option<&str>,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut req = self
            .http
            .request(method, self.url(path)?)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(org) = org {
            req = req.header("x-zitadel-orgid", org);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
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
                message: error_message(&bytes),
            });
        }
        if bytes.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    /// Creates a machine user in `org_id` whose access tokens are JWTs (so a Loams engine
    /// verifies them offline), and answers its user id (its `sub`).
    ///
    /// # Errors
    ///
    /// Zitadel's refusal (a taken username: 409), or a transport error.
    pub async fn create_machine_user(
        &self,
        org_id: &str,
        username: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<String> {
        let mut machine = json!({
            "name": name,
            "accessTokenType": "ACCESS_TOKEN_TYPE_JWT",
        });
        if let Some(d) = description {
            machine["description"] = json!(d);
        }
        let body = json!({
            "organizationId": org_id,
            "username": username,
            "machine": machine,
        });
        let res: CreateUserResponse = decode(
            self.send(reqwest::Method::POST, "v2/users/new", Some(body))
                .await?,
        )?;
        Ok(res.id)
    }

    /// Registers a private-key JWT key for `user_id`, valid until `expires`. With `public_key`
    /// (PEM), Zitadel stores it and the private key never leaves the caller; without it,
    /// Zitadel generates the pair and answers the key file once.
    ///
    /// # Errors
    ///
    /// Zitadel's refusal (a human user, a past expiry), or a transport error.
    pub async fn add_key(
        &self,
        user_id: &str,
        expires: OffsetDateTime,
        public_key: Option<&[u8]>,
    ) -> Result<AddedKey> {
        let expires = expires
            .format(&Rfc3339)
            .map_err(|e| Error::Invalid(format!("expiry: {e}")))?;
        let mut body = json!({ "expirationDate": expires });
        if let Some(pem) = public_key {
            body["publicKey"] = json!(B64.encode(pem));
        }
        let res: AddKeyResponse = decode(
            self.send(
                reqwest::Method::POST,
                &format!("v2/users/{}/keys", seg(user_id)?),
                Some(body),
            )
            .await?,
        )?;
        let key_file = match res.key_content.filter(|c| !c.is_empty()) {
            Some(c) => Some(
                B64.decode(c)
                    .map_err(|e| Error::Decode(format!("keyContent: {e}")))?,
            ),
            None => None,
        };
        Ok(AddedKey {
            key_id: res.key_id,
            key_file,
        })
    }

    /// Removes a key; a key that is already gone is not an error.
    ///
    /// # Errors
    ///
    /// Any other refusal, or a transport error.
    pub async fn remove_key(&self, user_id: &str, key_id: &str) -> Result<()> {
        let path = format!("v2/users/{}/keys/{}", seg(user_id)?, seg(key_id)?);
        match self.send(reqwest::Method::DELETE, &path, None).await {
            Err(e) if e.is_not_found() => Ok(()),
            other => other.map(drop),
        }
    }

    /// The user's state.
    ///
    /// # Errors
    ///
    /// 404 for an unknown user, or a transport error.
    pub async fn user_state(&self, user_id: &str) -> Result<UserState> {
        let res: GetUserResponse = decode(
            self.send(
                reqwest::Method::GET,
                &format!("v2/users/{}", seg(user_id)?),
                None,
            )
            .await?,
        )?;
        Ok(res.user.state)
    }

    /// Deactivates the user: it can no longer get tokens. Idempotent: a user that is already
    /// inactive (Zitadel refuses to deactivate it twice) is success.
    ///
    /// # Errors
    ///
    /// Any other refusal, or a transport error.
    pub async fn deactivate(&self, user_id: &str) -> Result<()> {
        self.set_active(user_id, false).await
    }

    /// Reactivates a deactivated user. Idempotent, as [`Zitadel::deactivate`].
    ///
    /// # Errors
    ///
    /// As [`Zitadel::deactivate`].
    pub async fn reactivate(&self, user_id: &str) -> Result<()> {
        self.set_active(user_id, true).await
    }

    async fn set_active(&self, user_id: &str, active: bool) -> Result<()> {
        let verb = if active { "reactivate" } else { "deactivate" };
        let path = format!("v2/users/{}/{verb}", seg(user_id)?);
        match self
            .send(reqwest::Method::POST, &path, Some(json!({})))
            .await
        {
            Ok(_) => Ok(()),
            Err(e @ Error::Status { status, .. })
                if status != 404 && (400..500).contains(&status) =>
            {
                let want = if active {
                    UserState::Active
                } else {
                    UserState::Inactive
                };
                match self.user_state(user_id).await {
                    Ok(state) if state == want => Ok(()),
                    _ => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Deletes the user (an agent that is removed, not only revoked). A user that is already
    /// gone is success.
    ///
    /// # Errors
    ///
    /// Any other refusal, or a transport error.
    pub async fn delete_user(&self, user_id: &str) -> Result<()> {
        match self
            .send(
                reqwest::Method::DELETE,
                &format!("v2/users/{}", seg(user_id)?),
                None,
            )
            .await
        {
            Err(e) if e.is_not_found() => Ok(()),
            other => other.map(drop),
        }
    }
}

/// A path segment from an id: ids are alphanumeric in Zitadel, so anything else is refused
/// rather than escaped into another path.
fn seg(id: &str) -> Result<&str> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(id)
    } else {
        Err(Error::Invalid(format!("{id:?} is not a Zitadel id")))
    }
}

fn decode<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::Decode(e.to_string()))
}

/// The `message` of a gRPC-gateway error body, or the body itself, shortened.
fn error_message(body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Body {
        message: String,
    }
    match serde_json::from_slice::<Body>(body) {
        Ok(b) => b.message,
        Err(_) => String::from_utf8_lossy(&body[..body.len().min(512)]).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_path_segments_only() {
        assert!(seg("2948573459873").is_ok());
        for bad in ["", "../x", "a/b", "a?b", "a b", "%2e"] {
            assert!(seg(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn debug_leaves_out_the_token_and_the_key() {
        let z = Zitadel::new(
            "http://localhost:18080",
            "pat-secret",
            reqwest::Client::new(),
        )
        .unwrap();
        assert!(!format!("{z:?}").contains("pat-secret"));
        let k = AddedKey {
            key_id: "k1".into(),
            key_file: Some(b"private".to_vec()),
        };
        assert!(!format!("{k:?}").contains("private"));
    }

    #[test]
    fn base_keeps_its_path() {
        let z = Zitadel::new("https://example.com/zitadel", "t", reqwest::Client::new()).unwrap();
        assert_eq!(
            z.url("v2/users/new").unwrap().as_str(),
            "https://example.com/zitadel/v2/users/new"
        );
        assert!(Zitadel::new("ftp://x", "t", reqwest::Client::new()).is_err());
    }
}
