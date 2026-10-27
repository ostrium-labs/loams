//! Signing in as a Loams Cloud admin: WorkOS CLI Auth, the OAuth 2.0 device
//! authorization flow (design 78 §25.8, D1746, D1755; R-LC2a-7: on staging
//! `POST /user_management/authorize/device` returns a device code, a user code,
//! `expires_in: 300` and `interval: 5`).
//!
//! The refresh token is kept in the OS keychain; the access token lives in memory
//! for one command. Every command refreshes into the Ostrium organisation, so the
//! token carries `org_id` and the `cloud_admin` role the control plane checks.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::Error;

/// Where the refresh token lives.
pub trait TokenStore: Send + Sync {
    /// The stored refresh token, if any.
    ///
    /// # Errors
    ///
    /// Keychain failures other than "nothing stored".
    fn load(&self, key: &str) -> Result<Option<String>, Error>;
    /// Stores or replaces it.
    ///
    /// # Errors
    ///
    /// Keychain failures.
    fn save(&self, key: &str, secret: &str) -> Result<(), Error>;
    /// Removes it; absent is not an error.
    ///
    /// # Errors
    ///
    /// Keychain failures other than "nothing stored".
    fn delete(&self, key: &str) -> Result<(), Error>;
}

/// The OS keychain: macOS Keychain, Windows Credential Manager, the Linux Secret Service.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
}

/// The keychain service name.
pub const KEYRING_SERVICE: &str = "dev.loams.cloud-admin";

impl KeyringStore {
    /// A store under `service`.
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, Error> {
        keyring::Entry::new(&self.service, key).map_err(|e| Error::Store(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        match self.entry(key)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Store(e.to_string())),
        }
    }
    fn save(&self, key: &str, secret: &str) -> Result<(), Error> {
        self.entry(key)?
            .set_password(secret)
            .map_err(|e| Error::Store(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), Error> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Store(e.to_string())),
        }
    }
}

/// An in-memory store, for tests.
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryStore {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned())
    }
    fn save(&self, key: &str, secret: &str) -> Result<(), Error> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), Error> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        Ok(())
    }
}

/// What the person sees while the device flow waits.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// WorkOS's user-management endpoints the CLI uses.
#[derive(Debug, Clone)]
pub struct WorkOs {
    http: reqwest::Client,
    base: String,
    client_id: String,
}

impl WorkOs {
    /// `base` is `https://api.workos.com` outside tests.
    #[must_use]
    pub fn new(
        http: reqwest::Client,
        base: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
            client_id: client_id.into(),
        }
    }

    /// The keychain key: one refresh token per WorkOS client (staging and production differ).
    #[must_use]
    pub fn store_key(&self) -> String {
        format!("workos:{}", self.client_id)
    }

    /// Starts the device flow.
    ///
    /// # Errors
    ///
    /// A network error or a refusal from WorkOS.
    pub async fn authorize_device(&self) -> Result<DeviceAuthorization, Error> {
        let r = self
            .http
            .post(format!("{}/user_management/authorize/device", self.base))
            .form(&[("client_id", self.client_id.as_str())])
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(Error::WorkOs(format!(
                "device authorization: {}",
                r.status()
            )));
        }
        Ok(r.json().await?)
    }

    async fn authenticate(
        &self,
        form: &[(&str, &str)],
    ) -> Result<Result<TokenResponse, OAuthError>, Error> {
        let r = self
            .http
            .post(format!("{}/user_management/authenticate", self.base))
            .form(form)
            .send()
            .await?;
        if r.status().is_success() {
            return Ok(Ok(r.json().await?));
        }
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        Ok(Err(serde_json::from_str(&body).unwrap_or(OAuthError {
            error: format!("http_{}", status.as_u16()),
            error_description: None,
        })))
    }

    /// Polls until the person approves, denies, or the code expires. Returns the refresh token.
    ///
    /// # Errors
    ///
    /// Denied, expired, or a network error.
    pub async fn poll(&self, auth: &DeviceAuthorization) -> Result<String, Error> {
        let mut interval = Duration::from_secs(auth.interval.max(1));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(auth.expires_in);
        loop {
            tokio::time::sleep(interval).await;
            let res = self
                .authenticate(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", auth.device_code.as_str()),
                    ("client_id", self.client_id.as_str()),
                ])
                .await?;
            match res {
                Ok(t) => return Ok(t.refresh_token),
                Err(e) if e.error == "authorization_pending" => {}
                Err(e) if e.error == "slow_down" => interval += Duration::from_secs(5),
                Err(e) => {
                    return Err(Error::SignIn(format!(
                        "{}{}",
                        e.error,
                        e.error_description
                            .map(|d| format!(": {d}"))
                            .unwrap_or_default()
                    )));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::SignIn(
                    "expired_token: the code expired; run login again".into(),
                ));
            }
        }
    }

    /// Exchanges the refresh token for an access token in `organization_id`.
    /// Returns the access token and the rotated refresh token.
    ///
    /// # Errors
    ///
    /// A revoked or expired session (`Error::SignIn`), or a network error.
    pub async fn refresh(
        &self,
        refresh_token: &str,
        organization_id: &str,
    ) -> Result<(String, String), Error> {
        match self
            .authenticate(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", self.client_id.as_str()),
                ("organization_id", organization_id),
            ])
            .await?
        {
            Ok(t) => Ok((t.access_token, t.refresh_token)),
            Err(e) => Err(Error::SignIn(format!(
                "{}: run `loams cloud admin login`",
                e.error
            ))),
        }
    }
}
