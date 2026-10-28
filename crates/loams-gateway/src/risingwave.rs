//! The RisingWave module (design 81, D1957, ENG81 R8): managed RisingWave
//! instances behind Loams auth.
//!
//! A client connects with TLS to `<name>.<ns>.<region>.rw.<base-domain>` on
//! the shared port, or without SNI to an instance's dedicated port (compose's
//! `engine-gateway` on 127.0.0.1:4566, R7). The gateway then:
//!
//! 1. terminates TLS with a certificate of [`RwConfig::certs`] that covers
//!    the name;
//! 2. reads the `StartupMessage` and asks for a cleartext password, which
//!    travels inside TLS;
//! 3. checks the password as a Loams API key or token for the instance's
//!    namespace ([`Verifier`]): the Loams API lists the namespaces the
//!    credential reaches (`NamespaceService/ListNamespaces`, the same
//!    `can_read` on `environment:<ns>` the console uses);
//! 4. logs in to the instance's frontend as the instance's gateway user,
//!    whose password it reads from a mounted file (MD5 or cleartext, as
//!    RisingWave asks), and splices the rest of the session.
//!
//! RisingWave v3.1 offers only cleartext and MD5 password authentication
//! (no SCRAM-SHA-256), so the upstream login is MD5 on the instance's
//! private network.
//!
//! The user's credential never reaches RisingWave (R5), and the routes file
//! carries no password. Cancel requests are not routed to RisingWave.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use md5::Md5;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::config::CertConfig;
use crate::routes::{RouteError, check_hostname, check_upstream};
use crate::sniff::pg::{MAX_STARTUP_LEN, fatal_error};

/// Postgres protocol 3.0.
const PROTOCOL_3_0: u32 = 196_608;
/// The longest password message the gateway reads (API keys and JWTs).
const MAX_PASSWORD_LEN: usize = 16 * 1024;
/// The longest message the gateway reads from RisingWave during login.
const MAX_UPSTREAM_MESSAGE: usize = 64 * 1024;

/// `[risingwave]` in `gateway.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RwConfig {
    /// Certificates for the instances' hostnames (a wildcard such as
    /// `*.dev.eu1.rw.example.com` per namespace, or the compose stack's
    /// self-signed one).
    #[serde(default)]
    pub certs: Vec<CertConfig>,
    /// Names for a certificate the gateway makes itself at start when
    /// `certs` is empty: the compose stack, whose clients connect with
    /// `sslmode=require` (encrypted, not verified). Never in production.
    #[serde(default)]
    pub self_signed_names: Vec<String>,
    /// How client passwords are checked. Without it, RisingWave routes are
    /// refused.
    #[serde(default)]
    pub auth: Option<RwAuthConfig>,
    #[serde(default = "default_upstream_connect_timeout_ms")]
    pub upstream_connect_timeout_ms: u64,
}

fn default_upstream_connect_timeout_ms() -> u64 {
    5_000
}

impl RwConfig {
    pub fn upstream_connect_timeout(&self) -> Duration {
        Duration::from_millis(self.upstream_connect_timeout_ms)
    }
}

/// How the gateway checks a client's password.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RwAuthConfig {
    /// The Loams API (the engine's Connect endpoint, e.g.
    /// `http://loams:7780`).
    Loams { api_url: String },
    /// A file of `<namespace> <sha256 hex of the key>` lines. A test seam
    /// (ENG81 R25), never the production path.
    KeysFile { path: PathBuf },
}

/// One `[[risingwave.endpoints]]` entry, as `loams-risingwave` renders it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RwEndpoint {
    /// The instance id, `eng_<26>`.
    pub name: String,
    /// The Loams namespace whose principals may connect.
    pub namespace: String,
    /// SNI names, lower-cased; the first is the canonical one.
    pub hostnames: Vec<String>,
    /// Frontend addresses, `host:port`, tried in order.
    pub upstreams: Vec<String>,
    pub upstream_user: String,
    pub upstream_password_file: PathBuf,
    /// A dedicated port that routes to this instance without SNI.
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RwRoutesFile {
    #[serde(default)]
    endpoints: Vec<RwEndpoint>,
}

/// The validated RisingWave routes of a route table.
#[derive(Debug, Default)]
pub struct RwRoutes {
    pub endpoints: Vec<Arc<RwEndpoint>>,
    by_host: HashMap<String, Arc<RwEndpoint>>,
    by_port: HashMap<u16, Arc<RwEndpoint>>,
}

impl RwRoutes {
    pub(crate) fn build(
        file: RwRoutesFile,
        dedicated: &RangeInclusive<u16>,
    ) -> Result<Self, RouteError> {
        let invalid = |m: String| RouteError::Invalid(m);
        let mut routes = RwRoutes::default();
        let mut names = HashMap::new();
        for mut ep in file.endpoints {
            let name_ok = !ep.name.is_empty()
                && ep.name.len() <= 63
                && ep.name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                });
            if !name_ok {
                return Err(invalid(format!(
                    "risingwave endpoint {:?} is not [a-z0-9_-]{{1,63}}",
                    ep.name
                )));
            }
            if names.insert(ep.name.clone(), ()).is_some() {
                return Err(invalid(format!(
                    "risingwave endpoint {:?} is listed twice",
                    ep.name
                )));
            }
            if ep.namespace.is_empty() {
                return Err(invalid(format!(
                    "risingwave endpoint {:?} has no namespace",
                    ep.name
                )));
            }
            if ep.hostnames.is_empty() && ep.port.is_none() {
                return Err(invalid(format!(
                    "risingwave endpoint {:?} has neither a hostname nor a port",
                    ep.name
                )));
            }
            for h in &mut ep.hostnames {
                *h = h.to_ascii_lowercase();
                check_hostname(h)?;
            }
            if ep.upstreams.is_empty() {
                return Err(invalid(format!(
                    "risingwave endpoint {:?} has no upstream",
                    ep.name
                )));
            }
            for u in &ep.upstreams {
                check_upstream(u)?;
            }
            let user_ok = !ep.upstream_user.is_empty()
                && ep
                    .upstream_user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if !user_ok {
                return Err(invalid(format!(
                    "risingwave endpoint {:?}: upstream_user is not [A-Za-z0-9_]+",
                    ep.name
                )));
            }
            if !ep.upstream_password_file.is_absolute() {
                return Err(invalid(format!(
                    "risingwave endpoint {:?}: upstream_password_file must be absolute",
                    ep.name
                )));
            }
            let ep = Arc::new(ep);
            for h in &ep.hostnames {
                if routes.by_host.insert(h.clone(), ep.clone()).is_some() {
                    return Err(invalid(format!("hostname {h:?} is listed twice")));
                }
            }
            if let Some(port) = ep.port {
                if !dedicated.contains(&port) {
                    return Err(invalid(format!(
                        "risingwave endpoint {:?}: port {port} is outside the dedicated range {}-{}",
                        ep.name,
                        dedicated.start(),
                        dedicated.end()
                    )));
                }
                if routes.by_port.insert(port, ep.clone()).is_some() {
                    return Err(invalid(format!("port {port} is listed twice")));
                }
            }
            routes.endpoints.push(ep);
        }
        Ok(routes)
    }

    /// The endpoint whose hostnames include `sni` (case-insensitive).
    pub fn by_hostname(&self, sni: &str) -> Option<&Arc<RwEndpoint>> {
        self.by_host.get(&sni.to_ascii_lowercase())
    }

    pub fn by_port(&self, port: u16) -> Option<&Arc<RwEndpoint>> {
        self.by_port.get(&port)
    }

    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.by_port.keys().copied()
    }

    pub(crate) fn hostnames(&self) -> impl Iterator<Item = &str> + '_ {
        self.by_host.keys().map(String::as_str)
    }
}

/// Why a client was not let in.
#[derive(Debug, thiserror::Error)]
pub enum RwError {
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("authentication failed")]
    Denied,
    #[error("the credential check is unavailable: {0}")]
    Unavailable(String),
    #[error("no RisingWave frontend could be reached: {0}")]
    Upstream(String),
    #[error("RisingWave refused the gateway's login: {0}")]
    UpstreamAuth(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Checks a client's password for a namespace.
#[async_trait]
pub trait Verifier: Send + Sync + std::fmt::Debug {
    async fn verify(&self, namespace: &str, secret: &str) -> Result<(), RwError>;
}

/// The Loams API: a credential reaches a namespace when
/// `NamespaceService/ListNamespaces` lists it.
#[derive(Debug, Clone)]
pub struct LoamsApiVerifier {
    api_url: String,
    http: reqwest::Client,
}

const LIST_NAMESPACES: &str = "/loams.collection.v1.NamespaceService/ListNamespaces";
/// The most pages of namespaces read for one check.
const MAX_PAGES: usize = 20;

impl LoamsApiVerifier {
    pub fn new(api_url: &str, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        Self {
            api_url: api_url.trim_end_matches('/').to_owned(),
            http,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListNamespacesResponse {
    #[serde(default)]
    namespaces: Vec<NamespaceInfo>,
    #[serde(default)]
    next_page_token: String,
}

#[derive(Debug, Deserialize)]
struct NamespaceInfo {
    #[serde(default)]
    namespace: String,
}

#[async_trait]
impl Verifier for LoamsApiVerifier {
    async fn verify(&self, namespace: &str, secret: &str) -> Result<(), RwError> {
        if secret.is_empty() || secret.contains(['\r', '\n']) {
            return Err(RwError::Denied);
        }
        let mut token = String::new();
        for _ in 0..MAX_PAGES {
            let body = if token.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "pageToken": token })
            };
            let resp = self
                .http
                .post(format!("{}{LIST_NAMESPACES}", self.api_url))
                .header("Content-Type", "application/json")
                .header("Connect-Protocol-Version", "1")
                .bearer_auth(secret)
                .json(&body)
                .send()
                .await
                .map_err(|e| RwError::Unavailable(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(RwError::Denied);
            }
            if !status.is_success() {
                return Err(RwError::Unavailable(format!("ListNamespaces: {status}")));
            }
            let page: ListNamespacesResponse = resp
                .json()
                .await
                .map_err(|e| RwError::Unavailable(e.to_string()))?;
            if page.namespaces.iter().any(|n| n.namespace == namespace) {
                return Ok(());
            }
            if page.next_page_token.is_empty() {
                return Err(RwError::Denied);
            }
            token = page.next_page_token;
        }
        Err(RwError::Denied)
    }
}

/// `<namespace> <sha256 hex>` lines (a test seam, R25).
#[derive(Debug, Clone, Default)]
pub struct KeysFileVerifier {
    keys: Vec<(String, String)>,
}

impl KeysFileVerifier {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut keys = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(ns), Some(hash), None) = (parts.next(), parts.next(), parts.next()) else {
                return Err(format!(
                    "line {}: expected `<namespace> <sha256 hex>`",
                    i + 1
                ));
            };
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("line {}: not a sha256 hex digest", i + 1));
            }
            keys.push((ns.to_owned(), hash.to_ascii_lowercase()));
        }
        Ok(Self { keys })
    }

    pub async fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }
}

#[async_trait]
impl Verifier for KeysFileVerifier {
    async fn verify(&self, namespace: &str, secret: &str) -> Result<(), RwError> {
        let digest = hex::encode(Sha256::digest(secret.as_bytes()));
        if self
            .keys
            .iter()
            .any(|(ns, hash)| ns == namespace && *hash == digest)
        {
            Ok(())
        } else {
            Err(RwError::Denied)
        }
    }
}

/// A client's `StartupMessage` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Startup {
    pub params: Vec<(String, String)>,
}

impl Startup {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Reads a `StartupMessage` (protocol 3.0).
pub async fn read_startup<R: AsyncRead + Unpin>(r: &mut R) -> Result<Startup, RwError> {
    let len = r.read_u32().await? as usize;
    if !(8..=MAX_STARTUP_LEN).contains(&len) {
        return Err(RwError::Protocol(format!("startup length {len}")));
    }
    let mut body = vec![0u8; len - 4];
    r.read_exact(&mut body).await?;
    let code = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    if code != PROTOCOL_3_0 {
        return Err(RwError::Protocol(format!(
            "unsupported protocol code {code} inside TLS"
        )));
    }
    parse_startup_params(&body[4..])
}

fn parse_startup_params(mut rest: &[u8]) -> Result<Startup, RwError> {
    let mut params = Vec::new();
    loop {
        let (key, after) = cstring(rest)?;
        if key.is_empty() {
            break;
        }
        let (value, after) = cstring(after)?;
        params.push((key, value));
        rest = after;
    }
    Ok(Startup { params })
}

fn cstring(b: &[u8]) -> Result<(String, &[u8]), RwError> {
    let end = b
        .iter()
        .position(|&c| c == 0)
        .ok_or_else(|| RwError::Protocol("unterminated string".into()))?;
    let s = std::str::from_utf8(&b[..end])
        .map_err(|_| RwError::Protocol("a startup parameter is not UTF-8".into()))?;
    Ok((s.to_owned(), &b[end + 1..]))
}

/// A `StartupMessage` with `params`.
pub fn startup_message(params: &[(&str, &str)]) -> Vec<u8> {
    let mut body = PROTOCOL_3_0.to_be_bytes().to_vec();
    for (k, v) in params {
        body.extend_from_slice(k.as_bytes());
        body.push(0);
        body.extend_from_slice(v.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut out = (u32::try_from(body.len() + 4).unwrap_or(u32::MAX))
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(&body);
    out
}

/// One tagged message: its type and body.
async fn read_message<R: AsyncRead + Unpin>(
    r: &mut R,
    max: usize,
) -> Result<(u8, Vec<u8>), RwError> {
    let tag = r.read_u8().await?;
    let len = r.read_u32().await? as usize;
    if len < 4 || len - 4 > max {
        return Err(RwError::Protocol(format!(
            "message {:?} length {len}",
            tag as char
        )));
    }
    let mut body = vec![0u8; len - 4];
    r.read_exact(&mut body).await?;
    Ok((tag, body))
}

fn message(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(tag);
    out.extend_from_slice(&(u32::try_from(body.len() + 4).unwrap_or(u32::MAX)).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// `AuthenticationCleartextPassword`.
pub const AUTH_CLEARTEXT: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 3];

/// A `PasswordMessage`.
fn password_message(password: &str) -> Vec<u8> {
    let mut body = password.as_bytes().to_vec();
    body.push(0);
    message(b'p', &body)
}

/// Postgres's MD5 password answer: `md5` + md5(md5(password + user) + salt).
pub fn md5_password(user: &str, password: &str, salt: &[u8; 4]) -> String {
    let inner = hex::encode(Md5::digest(format!("{password}{user}").as_bytes()));
    let mut h = Md5::new();
    h.update(inner.as_bytes());
    h.update(salt);
    format!("md5{}", hex::encode(h.finalize()))
}

/// The first field of an `ErrorResponse` body that is its message.
fn error_text(body: &[u8]) -> String {
    let mut rest = body;
    while let Some((&field, after)) = rest.split_first() {
        if field == 0 {
            break;
        }
        let end = after.iter().position(|&c| c == 0).unwrap_or(after.len());
        if field == b'M' {
            return String::from_utf8_lossy(&after[..end]).into_owned();
        }
        rest = after.get(end + 1..).unwrap_or_default();
    }
    "error".into()
}

/// Steps 2 and 3 of the module docs on a terminated client stream: the
/// client's startup, the cleartext password, the check. Returns the
/// client's startup parameters. Sends the client an error when refused.
pub async fn authenticate_client<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    ep: &RwEndpoint,
    verifier: &dyn Verifier,
) -> Result<Startup, RwError> {
    let startup = read_startup(client).await?;
    client.write_all(&AUTH_CLEARTEXT).await?;
    client.flush().await?;
    let (tag, body) = read_message(client, MAX_PASSWORD_LEN).await?;
    if tag != b'p' {
        return Err(RwError::Protocol(format!(
            "expected a password message, got {:?}",
            tag as char
        )));
    }
    let end = body.iter().position(|&c| c == 0).unwrap_or(body.len());
    let secret = String::from_utf8_lossy(&body[..end]).into_owned();
    match verifier.verify(&ep.namespace, &secret).await {
        Ok(()) => Ok(startup),
        Err(RwError::Denied) => {
            let _ = client
                .write_all(&fatal_error(
                    "28P01",
                    "password authentication failed: use a Loams API key or token that reaches this namespace",
                ))
                .await;
            let _ = client.shutdown().await;
            Err(RwError::Denied)
        }
        Err(e) => {
            let _ = client
                .write_all(&fatal_error(
                    "08006",
                    "the credential check is unavailable; retry",
                ))
                .await;
            let _ = client.shutdown().await;
            Err(e)
        }
    }
}

/// Step 4: logs in to RisingWave on `upstream` as the gateway user. On
/// success returns the `AuthenticationOk` message to hand the client; the
/// rest of RisingWave's answer (parameters, `ReadyForQuery`) follows on the
/// spliced stream.
pub async fn login_upstream<S: AsyncRead + AsyncWrite + Unpin>(
    upstream: &mut S,
    startup: &Startup,
    user: &str,
    password: &str,
) -> Result<Vec<u8>, RwError> {
    let database = startup.get("database").unwrap_or("dev");
    let mut params = vec![("user", user), ("database", database)];
    if let Some(app) = startup.get("application_name") {
        params.push(("application_name", app));
    }
    upstream.write_all(&startup_message(&params)).await?;
    upstream.flush().await?;
    loop {
        let (tag, body) = read_message(upstream, MAX_UPSTREAM_MESSAGE).await?;
        match tag {
            b'R' if body.len() >= 4 => {
                let code = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                match code {
                    0 => return Ok(message(b'R', &body)),
                    3 => {
                        upstream.write_all(&password_message(password)).await?;
                        upstream.flush().await?;
                    }
                    5 if body.len() >= 8 => {
                        let salt = [body[4], body[5], body[6], body[7]];
                        upstream
                            .write_all(&password_message(&md5_password(user, password, &salt)))
                            .await?;
                        upstream.flush().await?;
                    }
                    other => {
                        return Err(RwError::UpstreamAuth(format!(
                            "unsupported authentication request {other}"
                        )));
                    }
                }
            }
            b'E' => return Err(RwError::UpstreamAuth(error_text(&body))),
            // A notice before authentication: ignored.
            b'N' => {}
            other => {
                return Err(RwError::Protocol(format!(
                    "unexpected message {:?} during login",
                    other as char
                )));
            }
        }
    }
}

/// Connects to the first frontend that answers.
pub async fn connect_upstream(ep: &RwEndpoint, timeout: Duration) -> Result<TcpStream, RwError> {
    let mut last = String::from("no upstream");
    for addr in &ep.upstreams {
        match tokio::time::timeout(timeout, TcpStream::connect(addr.as_str())).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = format!("{addr}: {e}"),
            Err(_) => last = format!("{addr}: timed out"),
        }
    }
    Err(RwError::Upstream(last))
}

/// The gateway user's password, read for each connection so a rotated
/// Secret takes effect without a reload.
pub async fn read_password(ep: &RwEndpoint) -> Result<String, RwError> {
    let text = tokio::fs::read_to_string(&ep.upstream_password_file)
        .await
        .map_err(|e| {
            RwError::UpstreamAuth(format!("{}: {e}", ep.upstream_password_file.display()))
        })?;
    Ok(text.trim().to_owned())
}

/// A self-signed certificate and key (PEM) for `names`.
pub fn self_signed(names: &[String]) -> Result<(String, String), String> {
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = rcgen::CertificateParams::new(names.to_vec())
        .map_err(|e| e.to_string())?
        .self_signed(&key)
        .map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// The module's per-process state.
#[derive(Debug)]
pub struct RwFrontDoor {
    pub server_certs: crate::tls::ServerCerts,
    pub verifier: Option<Arc<dyn Verifier>>,
    pub connect_timeout: Duration,
}

impl RwFrontDoor {
    /// Builds the module from its config.
    pub async fn from_config(config: &RwConfig) -> Result<Self, String> {
        let mut server_certs =
            crate::tls::ServerCerts::load(&config.certs).map_err(|e| e.to_string())?;
        if server_certs.is_empty() && !config.self_signed_names.is_empty() {
            let (cert, key) = self_signed(&config.self_signed_names)?;
            server_certs
                .add(&config.self_signed_names, cert.as_bytes(), key.as_bytes())
                .map_err(|e| e.to_string())?;
            tracing::warn!(
                names = ?config.self_signed_names,
                "risingwave: serving a self-signed certificate (local stacks only)"
            );
        }
        let verifier: Option<Arc<dyn Verifier>> = match &config.auth {
            None => None,
            Some(RwAuthConfig::Loams { api_url }) => Some(Arc::new(LoamsApiVerifier::new(
                api_url,
                Duration::from_secs(5),
            ))),
            Some(RwAuthConfig::KeysFile { path }) => {
                Some(Arc::new(KeysFileVerifier::load(path).await?))
            }
        };
        Ok(Self {
            server_certs,
            verifier,
            connect_timeout: config.upstream_connect_timeout(),
        })
    }

    /// Steps 2 to 4 on a client whose TLS is terminated: returns the
    /// upstream, logged in, with the client told `AuthenticationOk`.
    pub async fn login<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        client: &mut S,
        ep: &RwEndpoint,
    ) -> Result<TcpStream, RwError> {
        let Some(verifier) = self.verifier.as_deref() else {
            let _ = client
                .write_all(&fatal_error(
                    "28000",
                    "this gateway has no credential check configured for RisingWave",
                ))
                .await;
            return Err(RwError::Denied);
        };
        let startup = authenticate_client(client, ep, verifier).await?;
        let result = async {
            let password = read_password(ep).await?;
            let mut upstream = connect_upstream(ep, self.connect_timeout).await?;
            let ok = login_upstream(&mut upstream, &startup, &ep.upstream_user, &password).await?;
            Ok::<_, RwError>((upstream, ok))
        }
        .await;
        match result {
            Ok((upstream, ok)) => {
                client.write_all(&ok).await?;
                client.flush().await?;
                Ok(upstream)
            }
            Err(e) => {
                let _ = client
                    .write_all(&fatal_error(
                        "08006",
                        "the RisingWave instance is unavailable",
                    ))
                    .await;
                let _ = client.shutdown().await;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn ep(port: Option<u16>) -> RwEndpoint {
        RwEndpoint {
            name: "eng_abcdefghijklmnopqrstuvwxyz".into(),
            namespace: "dev".into(),
            hostnames: vec!["Clicks.dev.eu1.rw.loams.dev".into()],
            upstreams: vec!["rw-abc-frontend-0:4566".into()],
            upstream_user: "loams_gateway".into(),
            upstream_password_file: "/var/run/loams/risingwave/abc/password".into(),
            port,
        }
    }

    #[test]
    fn routes_validate() {
        let range = 4000..=4999;
        let routes = RwRoutes::build(
            RwRoutesFile {
                endpoints: vec![ep(Some(4566))],
            },
            &range,
        )
        .unwrap();
        assert!(routes.by_hostname("clicks.DEV.eu1.rw.loams.dev").is_some());
        assert!(routes.by_port(4566).is_some());
        assert_eq!(routes.ports().collect::<Vec<_>>(), vec![4566]);

        let bad = |f: fn(&mut RwEndpoint)| {
            let mut e = ep(None);
            f(&mut e);
            RwRoutes::build(RwRoutesFile { endpoints: vec![e] }, &range).is_err()
        };
        assert!(bad(|e| e.name = "Bad Name".into()));
        assert!(bad(|e| e.namespace.clear()));
        assert!(bad(|e| e.upstreams.clear()));
        assert!(bad(|e| e.upstreams = vec!["nohost".into()]));
        assert!(bad(|e| e.upstream_user = "a;b".into()));
        assert!(bad(|e| e.upstream_password_file = "relative".into()));
        assert!(bad(|e| e.hostnames = vec!["-x.example".into()]));
        assert!(bad(|e| e.hostnames.clear()));
        assert!(bad(|e| e.port = Some(80)));
        let twice = RwRoutes::build(
            RwRoutesFile {
                endpoints: vec![ep(None), ep(None)],
            },
            &range,
        );
        assert!(twice.is_err());
    }

    #[test]
    fn endpoints_parse_as_loams_risingwave_renders_them() {
        let text = r#"
[[endpoints]]
name = "eng_abcdefghijklmnopqrstuvwxyz"
namespace = "dev"
hostnames = ["clicks.dev.eu1.rw.loams.dev"]
upstreams = ["rw-abcdefghijklmnopqrstuvwxyz-frontend-0:4566"]
upstream_user = "loams_gateway"
upstream_password_file = "/var/run/loams/risingwave/abcdefghijklmnopqrstuvwxyz/password"
port = 4566
"#;
        let file: RwRoutesFile = toml::from_str(text).unwrap();
        assert_eq!(file.endpoints.len(), 1);
        assert_eq!(file.endpoints[0].port, Some(4566));
        assert!(
            toml::from_str::<RwRoutesFile>("[[endpoints]]\nname = \"x\"\npassword = \"p\"\n")
                .is_err()
        );
    }

    #[test]
    fn md5_matches_postgres() {
        // md5(md5("secret" + "alice") + salt), as libpq computes it.
        let inner = hex::encode(Md5::digest(b"secretalice"));
        let mut h = Md5::new();
        h.update(inner.as_bytes());
        h.update([1, 2, 3, 4]);
        let want = format!("md5{}", hex::encode(h.finalize()));
        assert_eq!(md5_password("alice", "secret", &[1, 2, 3, 4]), want);
        assert_eq!(
            hex::encode(Md5::digest(b"")),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert!(want.starts_with("md5") && want.len() == 35);
    }

    #[tokio::test]
    async fn keys_file_verifier() {
        let digest = hex::encode(Sha256::digest(b"lk_test"));
        let v = KeysFileVerifier::parse(&format!("# test\ndev {digest}\n")).unwrap();
        assert!(v.verify("dev", "lk_test").await.is_ok());
        assert!(matches!(
            v.verify("prod", "lk_test").await,
            Err(RwError::Denied)
        ));
        assert!(matches!(
            v.verify("dev", "wrong").await,
            Err(RwError::Denied)
        ));
        assert!(KeysFileVerifier::parse("dev nothex").is_err());
        assert!(KeysFileVerifier::parse("dev").is_err());
    }

    #[tokio::test]
    async fn a_self_signed_certificate_serves_its_names() {
        let config = RwConfig {
            self_signed_names: vec!["localhost".into()],
            ..RwConfig::default()
        };
        let door = RwFrontDoor::from_config(&config).await.unwrap();
        assert!(door.server_certs.config_for_name("localhost").is_some());
        assert!(door.server_certs.config_for_name("").is_some());
        assert!(door.server_certs.config_for_name("other.example").is_none());
        assert!(door.verifier.is_none());
    }

    #[test]
    fn startup_round_trip() {
        let msg = startup_message(&[("user", "alice"), ("database", "dev")]);
        let len = u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
        assert_eq!(len, msg.len());
        let s = parse_startup_params(&msg[8..]).unwrap();
        assert_eq!(s.get("user"), Some("alice"));
        assert_eq!(s.get("database"), Some("dev"));
        assert_eq!(s.get("options"), None);
    }

    #[derive(Debug)]
    struct Fixed(bool);

    #[async_trait]
    impl Verifier for Fixed {
        async fn verify(&self, _ns: &str, secret: &str) -> Result<(), RwError> {
            if self.0 && secret == "lk_good" {
                Ok(())
            } else {
                Err(RwError::Denied)
            }
        }
    }

    /// A RisingWave frontend that asks for MD5 and checks the answer.
    async fn fake_frontend<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, password: &str) -> bool {
        let startup = read_startup(&mut s).await.unwrap();
        let user = startup.get("user").unwrap().to_owned();
        assert_eq!(startup.get("database"), Some("dev"));
        let salt = [9, 8, 7, 6];
        let mut body = 5u32.to_be_bytes().to_vec();
        body.extend_from_slice(&salt);
        s.write_all(&message(b'R', &body)).await.unwrap();
        let (tag, body) = read_message(&mut s, 1024).await.unwrap();
        assert_eq!(tag, b'p');
        let got = String::from_utf8_lossy(&body[..body.len() - 1]).into_owned();
        let ok = got == md5_password(&user, password, &salt);
        if ok {
            s.write_all(&message(b'R', &0u32.to_be_bytes()))
                .await
                .unwrap();
        } else {
            s.write_all(&fatal_error("28P01", "bad password"))
                .await
                .unwrap();
        }
        ok
    }

    async fn client_hello<S: AsyncRead + AsyncWrite + Unpin>(
        s: &mut S,
        password: &str,
    ) -> (u8, Vec<u8>) {
        s.write_all(&startup_message(&[
            ("user", "alice@example.com"),
            ("database", "dev"),
        ]))
        .await
        .unwrap();
        let (tag, body) = read_message(s, 1024).await.unwrap();
        assert_eq!((tag, body.as_slice()), (b'R', &[0u8, 0, 0, 3][..]));
        s.write_all(&password_message(password)).await.unwrap();
        read_message(s, 1024).await.unwrap()
    }

    #[tokio::test]
    async fn a_good_key_logs_in_upstream_with_md5() {
        let e = ep(None);
        let (mut gw_side, mut client_side) = tokio::io::duplex(4096);
        let (mut up_gw, up_rw) = tokio::io::duplex(4096);
        let frontend = tokio::spawn(fake_frontend(up_rw, "gwpass"));
        let client = tokio::spawn(async move { client_hello(&mut client_side, "lk_good").await });
        let startup = authenticate_client(&mut gw_side, &e, &Fixed(true))
            .await
            .unwrap();
        assert_eq!(startup.get("user"), Some("alice@example.com"));
        let ok = login_upstream(&mut up_gw, &startup, "loams_gateway", "gwpass")
            .await
            .unwrap();
        gw_side.write_all(&ok).await.unwrap();
        assert!(frontend.await.unwrap());
        let (tag, body) = client.await.unwrap();
        assert_eq!((tag, body), (b'R', 0u32.to_be_bytes().to_vec()));
    }

    #[tokio::test]
    async fn a_bad_key_is_refused_before_any_upstream() {
        let e = ep(None);
        let (mut gw_side, mut client_side) = tokio::io::duplex(4096);
        let client = tokio::spawn(async move { client_hello(&mut client_side, "lk_bad").await });
        let r = authenticate_client(&mut gw_side, &e, &Fixed(true)).await;
        assert!(matches!(r, Err(RwError::Denied)));
        let (tag, body) = client.await.unwrap();
        assert_eq!(tag, b'E');
        assert!(String::from_utf8_lossy(&body).contains("28P01"));
    }

    #[tokio::test]
    async fn a_wrong_gateway_password_is_an_upstream_auth_error() {
        let (mut up_gw, up_rw) = tokio::io::duplex(4096);
        let frontend = tokio::spawn(fake_frontend(up_rw, "right"));
        let startup = Startup {
            params: vec![("user".into(), "x".into())],
        };
        let r = login_upstream(&mut up_gw, &startup, "loams_gateway", "wrong").await;
        assert!(matches!(r, Err(RwError::UpstreamAuth(m)) if m == "bad password"));
        assert!(!frontend.await.unwrap());
    }
}
