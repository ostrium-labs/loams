//! An installed agent's identity in agentd (design 76 §10, D1618; plan IA1, IA4 Task 3; D1629–
//! D1633).
//!
//! Every installed agent is one Zitadel machine user (IA4 Task 1, D1607). agentd keeps that
//! user's private-key JWT key in its vault ([`machine_key_vault_key`]), outside every sandbox, and
//! vends the agent a short-lived token through the contract's `LOAMS_TOKEN_FILE` (D1453):
//!
//! - **The token** is Zitadel's own access token for the machine user (D1629). [`ZitadelIssuer`]
//!   signs an RFC 7523 assertion with the key ([`loams_zitadel::token::MachineKey`]) and trades
//!   it at `<issuer>/oauth/v2/token` with the Loams project's audience scope. Its `sub` is the
//!   machine user, so the engine verifies it as it verifies any Zitadel JWT and no Loams signing
//!   key exists for agents.
//! - **Its lifetime** is Zitadel's instance-wide access-token lifetime, which Loams' installs set
//!   to 900 s (D1630). An issuer that hands out a longer-lived token is used, with a warning; the
//!   file is still rewritten every 10 minutes ([`crate::contract::TOKEN_ROTATION`]).
//! - **The Place** is not a claim (D1631): an agent reaches only what OpenFGA grants
//!   `agent:<id>` (its project's environments, `agent_session` grants), and the token names no
//!   namespace.
//! - **Uninstalling** retires the identity (D1632, [`retire`]): agentd deactivates the machine
//!   user and removes its key at Zitadel when it holds a management credential
//!   ([`ZitadelUsers`]), and always deletes the key from its vault. A token already in a file
//!   dies within its lifetime, and the deactivated user is refused on its next request (D1624).
//! - **The supervisor** (§19 §5.2 flow 3) vends with the same [`ZitadelIssuer`] (D1633).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use loams_zitadel::token::{AccessToken, JWT_BEARER, MachineKey, project_audience_scope};
use time::OffsetDateTime;

use crate::contract::{AsyncTokenIssuer, TokenFile};
use crate::service::{Vault, secret_key};

/// The vault secret that holds an install's machine key file.
pub const MACHINE_KEY_SECRET: &str = "loams-machine-key";

/// The identity provider's issuer URL (`https://auth.loams.dev`); the SDKs' name for it.
pub const ISSUER_ENV: &str = "LOAMS_AGENT_ISSUER";
/// The Loams project's id at the identity provider: the audience the engine checks.
pub const PROJECT_ENV: &str = "LOAMS_AGENT_PROJECT";
/// Optional: the Zitadel API's base URL for deactivating machine users (default the issuer).
pub const ZITADEL_URL_ENV: &str = "LOAMS_AGENT_ZITADEL_URL";
/// Optional: a file holding a Zitadel token allowed to deactivate the org's machine users and
/// remove their keys. Without it, an uninstall leaves the machine user for the console.
pub const ZITADEL_TOKEN_FILE_ENV: &str = "LOAMS_AGENT_ZITADEL_TOKEN_FILE";

/// How long agentd waits before trying a failed rotation again.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The vault key of `listing_id`'s machine key.
#[must_use]
pub fn machine_key_vault_key(listing_id: &str) -> String {
    secret_key(listing_id, MACHINE_KEY_SECRET)
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("the identity HTTP client: {e}"))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// Where agentd trades machine keys for tokens.
#[derive(Clone)]
pub struct AgentIdentity {
    issuer: String,
    project: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for AgentIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentIdentity")
            .field("issuer", &self.issuer)
            .field("project", &self.project)
            .finish_non_exhaustive()
    }
}

impl AgentIdentity {
    pub fn new(
        issuer: impl Into<String>,
        project: impl Into<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            issuer: issuer.into().trim_end_matches('/').to_owned(),
            project: project.into(),
            http,
        }
    }

    /// From [`ISSUER_ENV`] and [`PROJECT_ENV`]; `None` when either is unset (agents then get no
    /// token file).
    ///
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn from_env() -> Result<Option<Self>, String> {
        match (env(ISSUER_ENV), env(PROJECT_ENV)) {
            (Some(issuer), Some(project)) => Ok(Some(Self::new(issuer, project, http_client()?))),
            _ => Ok(None),
        }
    }

    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
}

/// Checks a machine key file and keeps it in the vault for `listing_id`; answers the machine
/// user's id.
///
/// # Errors
///
/// A file that is not a machine user's key, or the vault's refusal.
pub fn store_machine_key(
    vault: &dyn Vault,
    listing_id: &str,
    json: &str,
) -> Result<String, String> {
    let key = MachineKey::from_json(json.as_bytes()).map_err(|e| e.to_string())?;
    zitadel_id(&key.user_id)?;
    zitadel_id(&key.key_id)?;
    vault.set(&machine_key_vault_key(listing_id), json)?;
    Ok(key.user_id)
}

/// The machine key kept for `listing_id`, if any.
///
/// # Errors
///
/// A stored value that is no longer a machine key.
pub fn machine_key(vault: &dyn Vault, listing_id: &str) -> Result<Option<MachineKey>, String> {
    vault
        .get(&machine_key_vault_key(listing_id))
        .map(|json| MachineKey::from_json(json.as_bytes()).map_err(|e| e.to_string()))
        .transpose()
}

/// A Zitadel id as a URL path segment.
fn zitadel_id(id: &str) -> Result<&str, String> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(id)
    } else {
        Err(format!("{id:?} is not a Zitadel id"))
    }
}

/// `application/x-www-form-urlencoded` (RFC 3986 unreserved characters kept).
fn form(pairs: &[(&str, &str)]) -> String {
    fn encode(s: &str, out: &mut String) {
        use std::fmt::Write as _;
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                out.push(char::from(b));
            } else {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode(k, &mut out);
        out.push('=');
        encode(v, &mut out);
    }
    out
}

/// Vends a machine user's Zitadel access token (D1629; module docs).
pub struct ZitadelIssuer {
    identity: AgentIdentity,
    key: MachineKey,
}

impl std::fmt::Debug for ZitadelIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZitadelIssuer")
            .field("identity", &self.identity)
            .field("key", &self.key)
            .finish()
    }
}

impl ZitadelIssuer {
    pub fn new(identity: AgentIdentity, key: MachineKey) -> Self {
        Self { identity, key }
    }

    /// The issuer of `listing_id`'s machine key.
    ///
    /// # Errors
    ///
    /// No key in the vault for it, or a stored value that is not one.
    pub fn from_vault(
        identity: AgentIdentity,
        vault: &dyn Vault,
        listing_id: &str,
    ) -> Result<Self, String> {
        let key = machine_key(vault, listing_id)?
            .ok_or_else(|| format!("{listing_id} has no machine key in agentd's vault"))?;
        Ok(Self::new(identity, key))
    }

    /// The machine user (the tokens' `sub`).
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.key.user_id
    }

    /// The scopes asked: OpenID, and the Loams project in `aud`.
    fn scope(&self) -> String {
        format!("openid {}", project_audience_scope(&self.identity.project))
    }
}

#[async_trait]
impl AsyncTokenIssuer for ZitadelIssuer {
    async fn issue(&self, _namespace: &str, lifetime: Duration) -> Result<String, String> {
        let issuer = self.identity.issuer.as_str();
        let assertion = self
            .key
            .assertion(issuer, OffsetDateTime::now_utc())
            .map_err(|e| e.to_string())?;
        let body = form(&[
            ("grant_type", JWT_BEARER),
            ("scope", &self.scope()),
            ("assertion", &assertion),
        ]);
        let res = self
            .identity
            .http
            .post(format!("{issuer}/oauth/v2/token"))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| format!("the token endpoint: {e}"))?;
        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| format!("the token endpoint: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "the token endpoint refused the machine user {} ({status}): {}",
                self.key.user_id,
                String::from_utf8_lossy(&bytes[..bytes.len().min(256)])
            ));
        }
        token_of(&bytes, lifetime, &self.key.user_id)
    }
}

/// The access token of a token endpoint answer, checked (D1630: a longer life than asked is
/// used, with a warning).
fn token_of(bytes: &[u8], lifetime: Duration, user: &str) -> Result<String, String> {
    let token: AccessToken =
        serde_json::from_slice(bytes).map_err(|e| format!("the token endpoint's answer: {e}"))?;
    if !token.token_type.is_empty() && !token.token_type.eq_ignore_ascii_case("bearer") {
        return Err(format!(
            "the token endpoint answered a {:?} token, not a bearer token",
            token.token_type
        ));
    }
    if token.access_token.is_empty() {
        return Err("the token endpoint answered no access token".into());
    }
    if token.expires_in > lifetime.as_secs() {
        tracing::warn!(
            user,
            expires_in = token.expires_in,
            wanted = lifetime.as_secs(),
            "Zitadel's access tokens outlive the agent token lifetime; set the instance's \
             access-token lifetime to 900 s (D1630)"
        );
    }
    Ok(token.access_token)
}

/// Keeps `file` fresh until `stop` is notified: rotates when due, and retries a failed rotation
/// after [`RETRY_AFTER`].
pub async fn run_token_file<I: AsyncTokenIssuer>(
    mut file: TokenFile<I>,
    stop: Arc<tokio::sync::Notify>,
) {
    loop {
        let wait = match file.tick_async(SystemTime::now()).await {
            Ok(_) => file
                .next_async_rotation()
                .and_then(|next| next.duration_since(SystemTime::now()).ok())
                .unwrap_or(RETRY_AFTER),
            Err(error) => {
                tracing::warn!(%error, "rotating an agent's token file failed; retrying");
                RETRY_AFTER
            }
        };
        tokio::select! {
            () = stop.notified() => return,
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// Zitadel's user API, as far as retiring an agent goes.
#[async_trait]
pub trait MachineUsers: Send + Sync {
    /// Deactivates the user; an inactive or deleted user is success.
    async fn deactivate(&self, user_id: &str) -> Result<(), String>;
    /// Removes one of the user's keys; a key already gone is success.
    async fn remove_key(&self, user_id: &str, key_id: &str) -> Result<(), String>;
}

/// [`MachineUsers`] over Zitadel's v2 user API with a management token.
#[derive(Clone)]
pub struct ZitadelUsers {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for ZitadelUsers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZitadelUsers")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl ZitadelUsers {
    pub fn new(base: impl Into<String>, token: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            http,
        }
    }

    /// From [`ZITADEL_TOKEN_FILE_ENV`] and [`ZITADEL_URL_ENV`] (default [`ISSUER_ENV`]); `None`
    /// without a token file.
    ///
    /// # Errors
    ///
    /// An unreadable or empty token file, a token file without a URL, or no HTTP client.
    pub fn from_env() -> Result<Option<Self>, String> {
        let Some(file) = env(ZITADEL_TOKEN_FILE_ENV) else {
            return Ok(None);
        };
        let base = env(ZITADEL_URL_ENV)
            .or_else(|| env(ISSUER_ENV))
            .ok_or_else(|| {
                format!("{ZITADEL_TOKEN_FILE_ENV} needs {ZITADEL_URL_ENV} or {ISSUER_ENV}")
            })?;
        let token = std::fs::read_to_string(&file)
            .map_err(|e| format!("{ZITADEL_TOKEN_FILE_ENV}: {e}"))?
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err(format!("{ZITADEL_TOKEN_FILE_ENV} is empty"));
        }
        Ok(Some(Self::new(base, token, http_client()?)))
    }

    async fn call(&self, method: reqwest::Method, path: &str) -> Result<(u16, Vec<u8>), String> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}/{path}", self.base))
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json");
        if method == reqwest::Method::POST {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body("{}");
        }
        let res = req.send().await.map_err(|e| format!("Zitadel: {e}"))?;
        let status = res.status().as_u16();
        let bytes = res.bytes().await.map_err(|e| format!("Zitadel: {e}"))?;
        Ok((status, bytes.to_vec()))
    }
}

fn refusal(what: &str, status: u16, body: &[u8]) -> String {
    format!(
        "Zitadel refused to {what} ({status}): {}",
        String::from_utf8_lossy(&body[..body.len().min(256)])
    )
}

#[async_trait]
impl MachineUsers for ZitadelUsers {
    async fn deactivate(&self, user_id: &str) -> Result<(), String> {
        let id = zitadel_id(user_id)?;
        let (status, body) = self
            .call(reqwest::Method::POST, &format!("v2/users/{id}/deactivate"))
            .await?;
        match status {
            200..=299 | 404 => Ok(()),
            // Zitadel refuses to deactivate an inactive user: success when it is inactive.
            400..=499 => {
                let (s, user) = self
                    .call(reqwest::Method::GET, &format!("v2/users/{id}"))
                    .await?;
                let inactive = (200..=299).contains(&s)
                    && serde_json::from_slice::<serde_json::Value>(&user)
                        .ok()
                        .and_then(|v| {
                            v.pointer("/user/state")
                                .and_then(|s| s.as_str())
                                .map(str::to_owned)
                        })
                        .is_some_and(|state| state == "USER_STATE_INACTIVE");
                if inactive {
                    Ok(())
                } else {
                    Err(refusal("deactivate the machine user", status, &body))
                }
            }
            _ => Err(refusal("deactivate the machine user", status, &body)),
        }
    }

    async fn remove_key(&self, user_id: &str, key_id: &str) -> Result<(), String> {
        let path = format!(
            "v2/users/{}/keys/{}",
            zitadel_id(user_id)?,
            zitadel_id(key_id)?
        );
        let (status, body) = self.call(reqwest::Method::DELETE, &path).await?;
        match status {
            200..=299 | 404 => Ok(()),
            _ => Err(refusal("remove the machine key", status, &body)),
        }
    }
}

/// What [`retire`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retired {
    /// The install had no machine key.
    NoIdentity,
    /// The machine user was deactivated, its key removed, and the vault entry deleted.
    Deactivated { user_id: String },
    /// agentd holds no Zitadel management credential: the vault entry was deleted and the
    /// machine user stays active until someone deactivates it in the console.
    KeyForgotten { user_id: String },
}

/// Retires `listing_id`'s identity on uninstall (D1632). The vault entry is deleted whatever
/// Zitadel answers, so this agentd never vends the agent another token.
///
/// # Errors
///
/// Zitadel's refusal to deactivate the user or remove its key (the vault entry is gone anyway).
pub async fn retire(
    vault: &dyn Vault,
    users: Option<&dyn MachineUsers>,
    listing_id: &str,
) -> Result<Retired, String> {
    let key = machine_key(vault, listing_id);
    vault.delete(&machine_key_vault_key(listing_id));
    let Some(key) = key? else {
        return Ok(Retired::NoIdentity);
    };
    let Some(users) = users else {
        tracing::warn!(
            listing = listing_id,
            user = %key.user_id,
            "no Zitadel management credential ({ZITADEL_TOKEN_FILE_ENV}): the agent's machine \
             user stays active; deactivate it in the console"
        );
        return Ok(Retired::KeyForgotten {
            user_id: key.user_id,
        });
    };
    users.deactivate(&key.user_id).await?;
    users.remove_key(&key.user_id, &key.key_id).await?;
    Ok(Retired::Deactivated {
        user_id: key.user_id,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::service::MemoryVault;

    const KEY: &str = r#"{"type":"serviceaccount","keyId":"k1","key":"-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n","userId":"u1"}"#;

    #[test]
    fn a_machine_key_is_kept_per_install() {
        let vault = MemoryVault::default();
        assert_eq!(
            store_machine_key(&vault, "acme/notes", KEY),
            Ok("u1".into())
        );
        let key = machine_key(&vault, "acme/notes").unwrap().unwrap();
        assert_eq!((key.user_id.as_str(), key.key_id.as_str()), ("u1", "k1"));
        assert!(machine_key(&vault, "acme/other").unwrap().is_none());
        let app = r#"{"type":"application","keyId":"k","key":"x","userId":"u"}"#;
        assert!(store_machine_key(&vault, "acme/app", app).is_err());
        let bad_id = r#"{"type":"serviceaccount","keyId":"k/../x","key":"x","userId":"u"}"#;
        assert!(store_machine_key(&vault, "acme/bad", bad_id).is_err());
    }

    #[test]
    fn the_token_answer_is_checked() {
        let life = Duration::from_secs(900);
        let ok = br#"{"access_token":"eyJ","token_type":"Bearer","expires_in":899}"#;
        assert_eq!(token_of(ok, life, "u1"), Ok("eyJ".into()));
        let long = br#"{"access_token":"eyJ","token_type":"Bearer","expires_in":43200}"#;
        assert_eq!(
            token_of(long, life, "u1"),
            Ok("eyJ".into()),
            "used, with a warning"
        );
        let mac = br#"{"access_token":"eyJ","token_type":"MAC","expires_in":60}"#;
        assert!(token_of(mac, life, "u1").is_err());
        let empty = br#"{"access_token":"","token_type":"Bearer"}"#;
        assert!(token_of(empty, life, "u1").is_err());
    }

    #[test]
    fn form_encoding() {
        assert_eq!(
            form(&[("scope", "openid urn:a:b"), ("x", "a.b-c_d~")]),
            "scope=openid%20urn%3Aa%3Ab&x=a.b-c_d~"
        );
    }

    #[derive(Default)]
    struct Users(Mutex<Vec<String>>);

    #[async_trait]
    impl MachineUsers for Users {
        async fn deactivate(&self, user_id: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("deactivate {user_id}"));
            Ok(())
        }
        async fn remove_key(&self, user_id: &str, key_id: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("remove {user_id}/{key_id}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn uninstall_deactivates_and_forgets() {
        let vault = MemoryVault::default();
        store_machine_key(&vault, "acme/notes", KEY).unwrap();
        let users = Users::default();
        assert_eq!(
            retire(&vault, Some(&users), "acme/notes").await,
            Ok(Retired::Deactivated {
                user_id: "u1".into()
            })
        );
        assert_eq!(
            *users.0.lock().unwrap(),
            vec!["deactivate u1".to_owned(), "remove u1/k1".to_owned()]
        );
        assert!(machine_key(&vault, "acme/notes").unwrap().is_none());
        assert_eq!(
            retire(&vault, Some(&users), "acme/notes").await,
            Ok(Retired::NoIdentity)
        );

        store_machine_key(&vault, "acme/notes", KEY).unwrap();
        assert_eq!(
            retire(&vault, None, "acme/notes").await,
            Ok(Retired::KeyForgotten {
                user_id: "u1".into()
            })
        );
        assert!(machine_key(&vault, "acme/notes").unwrap().is_none());
    }

    struct Fixed;

    #[async_trait]
    impl AsyncTokenIssuer for Fixed {
        async fn issue(&self, namespace: &str, _lifetime: Duration) -> Result<String, String> {
            Ok(format!("token-for-{namespace}"))
        }
    }

    #[tokio::test]
    async fn the_token_file_is_written_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        let file = TokenFile::new_async(path.clone(), "shop-dev".into(), Fixed);
        let stop = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn(run_token_file(file, stop.clone()));
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "token-for-shop-dev"
        );
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("stops")
            .unwrap();
    }
}
