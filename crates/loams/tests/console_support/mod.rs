//! What `connect_auth.rs` and `connect_admin.rs` share: an engine to call,
//! a Connect JSON client that keeps a console session, the `ErrorInfo`
//! reason of a failed call, and a stand-in `Authenticator`.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use buffa::Message as _;
use loams::auth::{
    AuthConfig, AuthError, Authenticator, Authenticators, Credential, Principal, PrincipalKind,
};
use loams::{Server, ServerConfig};
use serde_json::{Value, json};
use tempfile::TempDir;

pub const LOOPBACK0: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// The setup token the engines with authentication on are started with.
pub const SETUP_TOKEN: &str = "test-setup-token";
/// The owner `setup` creates.
pub const OWNER_EMAIL: &str = "ada@example.test";
pub const OWNER_PASSWORD: &str = "correct horse battery";

/// A running engine.
pub struct Engine {
    pub server: Server,
    pub base: String,
    _dir: TempDir,
}

impl Engine {
    pub async fn start(auth: AuthConfig) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = LOOPBACK0;
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.auth = auth;
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            _dir: dir,
        }
    }

    /// Authentication off (loopback only): setup needs no token.
    pub async fn dev() -> Self {
        Self::start(AuthConfig::off()).await
    }

    /// Authentication on through `authenticators` alone, with
    /// [`SETUP_TOKEN`].
    pub async fn guarded(authenticators: Vec<Arc<dyn Authenticator>>) -> Self {
        Self::start(AuthConfig {
            setup_token: Some(SETUP_TOKEN.to_owned()),
            extra_authenticators: Authenticators(authenticators),
            ..AuthConfig::off()
        })
        .await
    }

    pub fn client(&self) -> Client {
        Client::new(&self.base)
    }

    pub async fn stop(self) {
        self.server.shutdown().await.expect("shutdown");
    }
}

/// One Connect call's answer.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Answer {
    /// The body of a successful call (panics with the error otherwise).
    #[track_caller]
    pub fn ok(&self) -> &Value {
        assert_eq!(self.status, 200, "expected success, got {}", self.body);
        &self.body
    }

    /// The Connect error code, `None` on success.
    pub fn code(&self) -> Option<&str> {
        (self.status != 200).then(|| self.body["code"].as_str().unwrap_or_default())
    }

    /// The `loams.errors.v1.ErrorInfo` reason, `None` on success.
    pub fn reason(&self) -> Option<String> {
        if self.status == 200 {
            return None;
        }
        reason_of(&self.body)
    }

    /// `(code, reason)` of a failed call.
    #[track_caller]
    pub fn failure(&self) -> (String, String) {
        assert_ne!(self.status, 200, "expected a failure, got {}", self.body);
        (
            self.code().unwrap_or_default().to_owned(),
            self.reason().unwrap_or_default(),
        )
    }

    /// Every `Set-Cookie` value.
    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .collect()
    }
}

/// The `ErrorInfo.reason` of a Connect error body.
pub fn reason_of(body: &Value) -> Option<String> {
    body["details"].as_array()?.iter().find_map(|detail| {
        if detail["type"] != "loams.errors.v1.ErrorInfo" {
            return None;
        }
        let value = detail["value"].as_str()?;
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(value.trim_end_matches('='))
            .ok()?;
        loams_proto::loams::errors::v1::ErrorInfo::decode_from_slice(&bytes)
            .ok()
            .map(|info| info.reason)
    })
}

/// A Connect JSON client, with an optional bearer and console session.
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
    pub bearer: Option<String>,
    /// `(cookie token, csrf token)`.
    pub session: Option<(String, String)>,
    /// Send the session's CSRF token (true unless a test leaves it out).
    pub send_csrf: bool,
}

impl Client {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.to_owned(),
            http: reqwest::Client::new(),
            bearer: None,
            session: None,
            send_csrf: true,
        }
    }

    pub fn with_bearer(&self, bearer: &str) -> Self {
        Self {
            bearer: Some(bearer.to_owned()),
            session: None,
            ..self.clone()
        }
    }

    /// Calls `/<service>/<method>` with `body` (`service` without the
    /// `loams.` prefix, as `auth.v1.AuthService`).
    pub async fn call(&self, service: &str, method: &str, body: Value) -> Answer {
        let mut request = self
            .http
            .post(format!("{}/loams.{service}/{method}", self.base))
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .body(body.to_string());
        if let Some(bearer) = &self.bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        if let Some((token, csrf)) = &self.session {
            request = request.header("cookie", format!("loams_session={token}"));
            if self.send_csrf {
                request = request.header("x-csrf-token", csrf);
            }
        }
        let response = request.send().await.expect("call");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Answer {
            status,
            headers,
            body,
        }
    }

    /// Keeps the session an answer started (its cookie and CSRF token).
    #[track_caller]
    pub fn keep_session(&mut self, answer: &Answer) {
        let token = answer
            .set_cookies()
            .iter()
            .find_map(|c| {
                c.strip_prefix("loams_session=")
                    .and_then(|rest| rest.split(';').next())
                    .map(str::to_owned)
            })
            .filter(|t| !t.is_empty())
            .expect("a session cookie");
        let csrf = answer.ok()["session"]["csrfToken"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.session = Some((token, csrf));
    }

    /// First-run setup as [`OWNER_EMAIL`], keeping the session.
    pub async fn setup(&mut self, setup_token: &str) -> Answer {
        let answer = self
            .call(
                "auth.v1.AuthService",
                "CompleteSetup",
                json!({
                    "setupToken": setup_token,
                    "orgName": "Acme",
                    "name": "Ada Admin",
                    "email": OWNER_EMAIL,
                    "password": OWNER_PASSWORD,
                }),
            )
            .await;
        self.keep_session(&answer);
        answer
    }
}

/// A stand-in for MT1's authenticator: the bearer `test.<name>.jwt` is the
/// person `<name>@example.test`.
#[derive(Debug, Default)]
pub struct TestAuthenticator;

impl TestAuthenticator {
    pub fn token(name: &str) -> String {
        format!("test.{name}.jwt")
    }
}

#[async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Option<Result<Principal, AuthError>> {
        let Credential::Jwt(token) = credential else {
            return None;
        };
        let name = token.strip_prefix("test.")?.strip_suffix(".jwt")?;
        Some(Ok(Principal {
            subject: format!("test|{name}"),
            kind: PrincipalKind::User,
            issuer: None,
            display_name: name.to_owned(),
            email: Some(format!("{name}@example.test")),
            grant: Default::default(),
        }))
    }
}
