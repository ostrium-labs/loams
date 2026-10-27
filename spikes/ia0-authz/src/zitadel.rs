//! Client-side helpers for the drivers and tests: an agent's private-key JWT login, a person's
//! code + PKCE login done through Zitadel's session API (what Zitadel Login does for a browser),
//! and the management calls the drivers make. Zitadel is reached only over its public HTTP APIs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
pub struct MachineKey {
    #[serde(rename = "keyId")]
    pub key_id: String,
    pub key: String,
    #[serde(rename = "userId")]
    pub user_id: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub id_token: Option<String>,
}

#[derive(Clone)]
pub struct Zitadel {
    pub issuer: String,
    pub http: reqwest::Client,
}

fn unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Zitadel {
    pub fn new(issuer: &str) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        Self {
            issuer: issuer.trim_end_matches('/').to_owned(),
            http,
        }
    }

    /// The agent flow outside a sandbox (design 76 §10): sign a JWT with the machine key, trade it
    /// at the token endpoint (RFC 7523 JWT bearer) for a short-lived JWT access token whose `aud`
    /// carries the project id.
    pub async fn agent_token(&self, key: &MachineKey, project: &str) -> Result<TokenResponse> {
        self.agent_token_scoped(key, &format!("openid urn:zitadel:iam:org:project:id:{project}:aud urn:zitadel:iam:user:resourceowner")).await
    }

    pub async fn agent_token_scoped(&self, key: &MachineKey, scope: &str) -> Result<TokenResponse> {
        #[derive(Serialize)]
        struct Assertion<'a> {
            iss: &'a str,
            sub: &'a str,
            aud: &'a str,
            iat: u64,
            exp: u64,
        }
        let now = unix();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(key.key_id.clone());
        let assertion = encode(
            &header,
            &Assertion {
                iss: &key.user_id,
                sub: &key.user_id,
                aud: &self.issuer,
                iat: now,
                exp: now + 300,
            },
            &EncodingKey::from_rsa_pem(key.key.as_bytes())?,
        )?;
        let res = self
            .http
            .post(format!("{}/oauth/v2/token", self.issuer))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
                ("scope", scope),
            ])
            .send()
            .await?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            bail!("token endpoint {status}: {body}");
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// A person's login without a browser: authorize (code + PKCE) -> Zitadel redirects to the
    /// login UI with an auth request id -> create a session with a password check through the
    /// login client -> finalize the auth request -> exchange the code.
    pub async fn human_login(
        &self,
        login_client_pat: &str,
        client_id: &str,
        project: &str,
        login_name: &str,
        password: &str,
    ) -> Result<TokenResponse> {
        let verifier =
            URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{login_name}{}", unix()).as_bytes()));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let redirect = "http://127.0.0.1:18090/callback";
        let scope = format!(
            "openid profile urn:zitadel:iam:org:project:id:{project}:aud urn:zitadel:iam:user:resourceowner"
        );
        let res = self
            .http
            .get(format!("{}/oauth/v2/authorize", self.issuer))
            .query(&[
                ("client_id", client_id),
                ("redirect_uri", redirect),
                ("response_type", "code"),
                ("scope", scope.as_str()),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await?;
        let location = res
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let auth_request = location
            .split("authRequest=")
            .nth(1)
            .map(|s| s.split('&').next().unwrap_or(s).to_owned())
            .ok_or_else(|| {
                anyhow!(
                    "authorize did not redirect to the login UI ({}): {location}",
                    res.status()
                )
            })?;

        let session: Value = self
            .api(login_client_pat, reqwest::Method::POST, "/v2/sessions", Some(json!({
                "checks": { "user": { "loginName": login_name }, "password": { "password": password } }
            })))
            .await?;
        let finalize: Value = self
            .api(login_client_pat, reqwest::Method::POST, &format!("/v2/oidc/auth_requests/{auth_request}"), Some(json!({
                "session": { "sessionId": session["sessionId"], "sessionToken": session["sessionToken"] }
            })))
            .await?;
        let callback = finalize["callbackUrl"].as_str().context("no callbackUrl")?;
        let code = callback
            .split("code=")
            .nth(1)
            .map(|s| s.split('&').next().unwrap_or(s))
            .context("no code in callback")?;
        let res = self
            .http
            .post(format!("{}/oauth/v2/token", self.issuer))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect),
                ("client_id", client_id),
                ("code_verifier", verifier.as_str()),
            ])
            .send()
            .await?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            bail!("code exchange {status}: {body}");
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// One authenticated management call. `org` sets `x-zitadel-orgid`.
    pub async fn api_org(
        &self,
        pat: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        org: Option<&str>,
    ) -> Result<Value> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.issuer))
            .bearer_auth(pat);
        if let Some(org) = org {
            req = req.header("x-zitadel-orgid", org);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            bail!("{method} {path}: {status}: {text}");
        }
        Ok(if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        })
    }

    pub async fn api(
        &self,
        pat: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.api_org(pat, method, path, body, None).await
    }
}

/// Reads the payload of a JWT without verifying it (for reports only).
pub fn peek(jwt: &str) -> Value {
    jwt.split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}
