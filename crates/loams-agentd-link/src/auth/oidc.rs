//! The OpenID Connect pieces of desktop sign-in against Authentik
//! (design 37 section 6.5): discovery, the authorization URL, and the code and
//! refresh grants at the token endpoint. Public client, PKCE, no secret.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::AuthError;
use super::pkce::Pkce;

/// Scopes requested: identity plus a refresh token.
pub const SCOPES: &str = "openid profile email offline_access";

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// The endpoints of an OIDC provider that the desktop uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Endpoints {
    /// Where the system browser is sent.
    pub authorization_endpoint: Url,
    /// Where the code and refresh grants are posted.
    pub token_endpoint: Url,
}

/// An OIDC public client.
#[derive(Debug, Clone)]
pub struct OidcClient {
    client_id: String,
    endpoints: Endpoints,
    http: reqwest::Client,
}

/// Tokens from the token endpoint. `Debug` redacts every secret.
#[derive(Clone, Deserialize)]
pub struct Tokens {
    /// The bearer for the Loams gateway exchange (or the API, before the
    /// auth plan).
    pub access_token: String,
    /// Present when `offline_access` was granted; goes to the OS keychain.
    pub refresh_token: Option<String>,
    /// The identity token; its claims are the identity, not an authorization.
    pub id_token: Option<String>,
    /// Lifetime of the access token, in seconds.
    pub expires_in: Option<u64>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

impl OidcClient {
    /// A client for already-known endpoints.
    #[must_use]
    pub fn new(client_id: impl Into<String>, endpoints: Endpoints) -> Self {
        Self {
            client_id: client_id.into(),
            endpoints,
            http: http_client(),
        }
    }

    /// Reads `{issuer}/.well-known/openid-configuration`.
    ///
    /// # Errors
    ///
    /// Network failures and malformed discovery documents.
    pub async fn discover(client_id: impl Into<String>, issuer: &str) -> Result<Self, AuthError> {
        let http = http_client();
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let endpoints: Endpoints = http
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| AuthError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Protocol(format!("discovery document: {e}")))?;
        Ok(Self {
            client_id: client_id.into(),
            endpoints,
            http,
        })
    }

    /// The URL to open in the system browser.
    #[must_use]
    pub fn authorize_url(&self, redirect_uri: &str, state: &str, nonce: &str, pkce: &Pkce) -> Url {
        let mut url = self.endpoints.authorization_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", SCOPES)
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256");
        url
    }

    /// The authorization-code grant.
    ///
    /// # Errors
    ///
    /// Network failures and error responses from the token endpoint.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<Tokens, AuthError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ])
        .await
    }

    /// The refresh-token grant (the refresh token rotates: store the new one).
    ///
    /// # Errors
    ///
    /// Network failures and error responses from the token endpoint.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens, AuthError> {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .await
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<Tokens, AuthError> {
        let mut params = vec![("client_id", self.client_id.as_str())];
        params.extend_from_slice(form);
        let response = self
            .http
            .post(self.endpoints.token_endpoint.clone())
            .form(&params)
            .send()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if !status.is_success() {
            // RFC 6749 section 5.2: {"error": "...", "error_description": "..."}.
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_owned)))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(AuthError::TokenEndpoint(detail));
        }
        serde_json::from_str(&body).map_err(|e| AuthError::Protocol(format!("token response: {e}")))
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static reqwest configuration is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> OidcClient {
        OidcClient::new(
            "loams-desktop",
            Endpoints {
                authorization_endpoint: "https://auth.example/application/o/authorize/"
                    .parse()
                    .unwrap(),
                token_endpoint: "https://auth.example/application/o/token/".parse().unwrap(),
            },
        )
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_a_loopback_redirect() {
        let pkce = Pkce::from_verifier("v".repeat(43));
        let url = client().authorize_url("http://127.0.0.1:5555/callback", "st", "no", &pkce);
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "loams-desktop");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555/callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], pkce.challenge);
        assert_eq!(q["state"], "st");
        assert_eq!(q["nonce"], "no");
        assert!(q["scope"].contains("offline_access"));
        assert!(
            !url.as_str().contains(&pkce.verifier),
            "the verifier never leaves the client"
        );
    }

    #[test]
    fn tokens_debug_redacts_secrets() {
        let t = Tokens {
            access_token: "AAA".into(),
            refresh_token: Some("RRR".into()),
            id_token: None,
            expires_in: Some(60),
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("AAA") && !shown.contains("RRR"));
    }
}
