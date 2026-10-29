//! The `browser` module (D2118): agents reach the deployment's headless
//! browser (Obscura, deploy/browser) through the gateway instead of holding
//! the engine's own token.
//!
//! `loams-gateway --module browser --config browser.toml` listens for the CDP
//! WebSocket handshake on `GET /devtools/browser` and:
//!
//! 1. reads the request head (bounded in size and time);
//! 2. requires `Upgrade: websocket` and `Authorization: Bearer <client token>`,
//!    where the client token's SHA-256 is in `client_tokens_file` (one hex
//!    digest per line). The file is checked on every connection, so a
//!    rotation needs no restart, but re-read and re-parsed only when its
//!    modification time or size changed ([`WatchedFile`]; BQ1-gw measured
//!    the per-connection re-read at 28 µs with 100 clients and 284 µs with
//!    1 000);
//! 3. opens the upstream engine, rewrites the head with the engine's own
//!    token (`upstream_token_file`) and the upstream `Host`, and splices the
//!    rest of the session.
//!
//! So the engine token lives only in the gateway, a client token can be
//! revoked by deleting a line, and every session is logged with its client
//! and outcome (never a token). Nothing else is served: other paths answer
//! 404, a missing or unknown token 401. The gateway does not parse CDP; the
//! network policy is the provider's (D2112) and the browser service's
//! NetworkPolicy's (D2114).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// The only path the module serves.
pub const DEVTOOLS_PATH: &str = "/devtools/browser";

/// The longest request head the module reads.
const MAX_HEAD: usize = 16 * 1024;

/// `browser.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where agents connect.
    pub listen: SocketAddr,
    /// The engine, `host:port` on the private network (`browser:9222`).
    pub upstream: String,
    /// A file holding the engine's `OBSCURA_CDP_TOKEN`.
    pub upstream_token_file: PathBuf,
    /// A file of client token SHA-256 digests, hex, one per line; `#`
    /// comments and blank lines are ignored.
    pub client_tokens_file: PathBuf,
    /// Concurrent sessions.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// How long a client has to send its handshake, in milliseconds.
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    /// How long connecting to the engine may take, in milliseconds.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// The splice's buffer per direction, in bytes. Smaller holds more
    /// sessions per GB; CDP frames larger than this are copied in several
    /// reads.
    #[serde(default = "default_copy_buffer_bytes")]
    pub copy_buffer_bytes: usize,
}

fn default_copy_buffer_bytes() -> usize {
    4 * 1024
}

fn default_max_connections() -> usize {
    64
}
fn default_handshake_timeout_ms() -> u64 {
    10_000
}
fn default_connect_timeout_ms() -> u64 {
    5_000
}

/// Reads and checks a `browser` module configuration file.
pub fn load(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| format!("browser: {e}"))?;
    check(&config)?;
    Ok(config)
}

/// Checks a configuration.
pub fn check(config: &Config) -> Result<(), String> {
    if config.upstream.contains("://") || !config.upstream.contains(':') {
        return Err(format!(
            "browser: upstream {:?} must be host:port, e.g. browser:9222",
            config.upstream
        ));
    }
    if config.max_connections == 0 {
        return Err("browser: max_connections must be at least 1".to_string());
    }
    if config.handshake_timeout_ms == 0 || config.connect_timeout_ms == 0 {
        return Err("browser: timeouts must be more than 0".to_string());
    }
    if !(512..=1 << 20).contains(&config.copy_buffer_bytes) {
        return Err("browser: copy_buffer_bytes must be between 512 and 1048576".to_string());
    }
    Ok(())
}

/// Serves the module until `shutdown`.
pub async fn serve(
    config: Config,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("browser: binding {}: {e}", config.listen))?;
    tracing::info!(listen = %config.listen, upstream = %config.upstream, "browser module listening");
    serve_on(listener, Arc::new(config), shutdown).await;
    Ok(())
}

/// Serves on a bound listener (tests bind port 0).
pub async fn serve_on(
    listener: TcpListener,
    config: Arc<Config>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) {
    let permits = Arc::new(Semaphore::new(config.max_connections));
    let state = Arc::new(State {
        clients: WatchedFile::new(config.client_tokens_file.clone(), digests),
        engine_token: WatchedFile::new(config.upstream_token_file.clone(), |text| {
            text.trim().to_string()
        }),
        config,
    });
    tokio::pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => return,
            accepted = listener.accept() => accepted,
        };
        let (client, peer) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(%error, "browser module accept failed");
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            tracing::warn!(%peer, "browser module at max_connections; refused");
            let mut client = client;
            let _ = client.write_all(&status(503, "Service Unavailable")).await;
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let _permit = permit;
            match session(client, &state).await {
                Ok(()) => tracing::info!(%peer, "browser session ended"),
                Err(refusal) => tracing::info!(%peer, %refusal, "browser session refused"),
            }
        });
    }
}

/// Why a session did not start.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The head was not a WebSocket handshake for the devtools path.
    NotFound,
    /// Not a WebSocket upgrade.
    BadRequest(&'static str),
    /// No token, or one not in the client list.
    Unauthorized,
    /// The engine could not be reached or its token not read.
    Upstream(String),
    /// The client went away or was too slow.
    Client(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotFound => f.write_str("not the devtools path"),
            Refusal::BadRequest(why) => write!(f, "bad request: {why}"),
            Refusal::Unauthorized => f.write_str("no valid client token"),
            Refusal::Upstream(why) => write!(f, "upstream: {why}"),
            Refusal::Client(why) => write!(f, "client: {why}"),
        }
    }
}

/// What every session reads: the configuration and the two watched files.
#[derive(Debug)]
struct State {
    config: Arc<Config>,
    clients: WatchedFile<HashSet<String>>,
    engine_token: WatchedFile<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

/// A file that is checked on every use and re-read only when its
/// modification time or size changed, so a rotation still takes effect on
/// the next connection without a restart.
#[derive(Debug)]
pub struct WatchedFile<T> {
    path: PathBuf,
    parse: fn(&str) -> T,
    cached: std::sync::Mutex<Option<(Stamp, Arc<T>)>>,
}

impl<T> WatchedFile<T> {
    /// Watch `path`, turning its text into a `T` with `parse`.
    pub fn new(path: PathBuf, parse: fn(&str) -> T) -> Self {
        Self {
            path,
            parse,
            cached: std::sync::Mutex::new(None),
        }
    }

    /// The current value: from the cache when the file has not changed.
    pub fn get(&self) -> Result<Arc<T>, String> {
        let meta = std::fs::metadata(&self.path)
            .map_err(|e| format!("reading {}: {e}", self.path.display()))?;
        let stamp = Stamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        };
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((seen, value)) = cached.as_ref()
            && *seen == stamp
            && stamp.modified.is_some()
        {
            return Ok(Arc::clone(value));
        }
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("reading {}: {e}", self.path.display()))?;
        let value = Arc::new((self.parse)(&text));
        *cached = Some((stamp, Arc::clone(&value)));
        Ok(value)
    }
}

async fn session(mut client: TcpStream, state: &State) -> Result<(), Refusal> {
    let config = &*state.config;
    let handshake = Duration::from_millis(config.handshake_timeout_ms);
    let (head, rest) = match tokio::time::timeout(handshake, read_head(&mut client)).await {
        Ok(Ok(read)) => read,
        Ok(Err(why)) => return Err(Refusal::Client(why)),
        Err(_) => return Err(Refusal::Client("handshake timed out".to_string())),
    };
    let request = match parse(&head) {
        Ok(request) => request,
        Err(refusal) => {
            let _ = client.write_all(&refusal_response(&refusal)).await;
            return Err(refusal);
        }
    };
    let allowed = state.clients.get().map_err(Refusal::Upstream)?;
    if !request
        .bearer
        .as_deref()
        .is_some_and(|token| allowed.contains(&digest(token)))
    {
        let _ = client.write_all(&status(401, "Unauthorized")).await;
        return Err(Refusal::Unauthorized);
    }
    let upstream_token = state
        .engine_token
        .get()
        .map_err(|e| Refusal::Upstream(format!("the engine token: {e}")))?;
    let upstream_token = upstream_token.as_str();
    let connect = Duration::from_millis(config.connect_timeout_ms);
    let mut upstream =
        match tokio::time::timeout(connect, TcpStream::connect(&config.upstream)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                let _ = client.write_all(&status(502, "Bad Gateway")).await;
                return Err(Refusal::Upstream(format!("connecting: {e}")));
            }
            Err(_) => {
                let _ = client.write_all(&status(504, "Gateway Timeout")).await;
                return Err(Refusal::Upstream("connect timed out".to_string()));
            }
        };
    let rewritten = rewrite(&request, &config.upstream, upstream_token);
    upstream
        .write_all(rewritten.as_bytes())
        .await
        .map_err(|e| Refusal::Upstream(e.to_string()))?;
    if !rest.is_empty() {
        upstream
            .write_all(&rest)
            .await
            .map_err(|e| Refusal::Upstream(e.to_string()))?;
    }
    let _ = tokio::io::copy_bidirectional_with_sizes(
        &mut client,
        &mut upstream,
        config.copy_buffer_bytes,
        config.copy_buffer_bytes,
    )
    .await;
    Ok(())
}

/// A parsed handshake.
#[derive(Debug, PartialEq, Eq)]
pub struct Handshake {
    /// `GET /devtools/browser HTTP/1.1`.
    pub request_line: String,
    /// Every header but `Host` and `Authorization`, as written.
    pub headers: Vec<(String, String)>,
    /// The bearer token, if one was sent.
    pub bearer: Option<String>,
}

/// Reads up to the end of the head; answers the head and any bytes after it.
async fn read_head(client: &mut TcpStream) -> Result<(String, Vec<u8>), String> {
    let mut buffer = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = find_end(&buffer) {
            let rest = buffer.split_off(end + 4);
            buffer.truncate(end);
            let head = String::from_utf8(buffer).map_err(|_| "the head is not UTF-8")?;
            return Ok((head, rest));
        }
        if buffer.len() > MAX_HEAD {
            return Err(format!("the head is over {MAX_HEAD} bytes"));
        }
        let read = client.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("closed before the head ended".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parses a handshake head (without its final blank line).
pub fn parse(head: &str) -> Result<Handshake, Refusal> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split(' ');
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    if method != Some("GET") || version.is_none_or(|v| !v.starts_with("HTTP/1.")) {
        return Err(Refusal::BadRequest("not an HTTP/1.1 GET"));
    }
    let path = target
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    if path != DEVTOOLS_PATH {
        return Err(Refusal::NotFound);
    }
    let mut headers = Vec::new();
    let mut bearer = None;
    let mut upgrade = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(Refusal::BadRequest("a header line without a colon"));
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("authorization") {
            bearer = value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty());
            continue;
        }
        if name.eq_ignore_ascii_case("host") {
            continue;
        }
        if name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket") {
            upgrade = true;
        }
        headers.push((name.to_string(), value.to_string()));
    }
    if !upgrade {
        return Err(Refusal::BadRequest("not a WebSocket upgrade"));
    }
    Ok(Handshake {
        request_line,
        headers,
        bearer,
    })
}

/// The head the engine receives: the client's, with the engine's token and
/// host.
pub fn rewrite(request: &Handshake, upstream: &str, token: &str) -> String {
    let mut out = format!("{}\r\nHost: {upstream}\r\n", request.request_line);
    for (name, value) in &request.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Authorization: Bearer {token}\r\n\r\n"));
    out
}

/// The digests in a client tokens file.
pub fn digests(text: &str) -> HashSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// A token's SHA-256, hex.
pub fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn status(code: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

fn refusal_response(refusal: &Refusal) -> Vec<u8> {
    match refusal {
        Refusal::NotFound => status(404, "Not Found"),
        Refusal::Unauthorized => status(401, "Unauthorized"),
        _ => status(400, "Bad Request"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "GET /devtools/browser HTTP/1.1\r\nHost: gateway:9223\r\n\
                        Upgrade: websocket\r\nConnection: Upgrade\r\n\
                        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                        Sec-WebSocket-Version: 13\r\nAuthorization: Bearer client-token-1";

    #[test]
    fn a_handshake_is_parsed_and_its_secrets_set_aside() {
        let request = parse(HEAD).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(request.bearer.as_deref(), Some("client-token-1"));
        assert!(
            request
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")
                    && !name.eq_ignore_ascii_case("host"))
        );
    }

    #[test]
    fn the_engine_sees_its_own_token_and_never_the_clients() {
        let request = parse(HEAD).unwrap_or_else(|e| panic!("{e}"));
        let out = rewrite(&request, "browser:9222", "engine-token");
        assert!(out.starts_with("GET /devtools/browser HTTP/1.1\r\nHost: browser:9222\r\n"));
        assert!(out.contains("Authorization: Bearer engine-token\r\n"));
        assert!(out.contains("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
        assert!(!out.contains("client-token-1"));
        assert!(out.ends_with("\r\n\r\n"));
    }

    #[test]
    fn only_the_devtools_websocket_is_served() {
        assert_eq!(
            parse("GET /json/version HTTP/1.1\r\nUpgrade: websocket"),
            Err(Refusal::NotFound)
        );
        assert!(matches!(
            parse("GET /devtools/browser HTTP/1.1\r\nHost: x"),
            Err(Refusal::BadRequest(_))
        ));
        assert!(matches!(
            parse("POST /devtools/browser HTTP/1.1\r\nUpgrade: websocket"),
            Err(Refusal::BadRequest(_))
        ));
    }

    #[test]
    fn a_watched_file_is_reread_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("clients");
        std::fs::write(&path, format!("{}\n", digest("a"))).unwrap_or_else(|e| panic!("{e}"));
        let watched = WatchedFile::new(path.clone(), digests);
        let first = watched.get().unwrap_or_else(|e| panic!("{e}"));
        let again = watched.get().unwrap_or_else(|e| panic!("{e}"));
        assert!(
            Arc::ptr_eq(&first, &again),
            "an unchanged file is not re-read"
        );
        // A rotation: different size, so it is seen on the next call.
        std::fs::write(&path, format!("{}\n{}\n", digest("a"), digest("b")))
            .unwrap_or_else(|e| panic!("{e}"));
        let rotated = watched.get().unwrap_or_else(|e| panic!("{e}"));
        assert!(rotated.contains(&digest("b")));
        // A revocation that keeps the size: the modification time moves.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, format!("{}\n{}\n", digest("c"), digest("b")))
            .unwrap_or_else(|e| panic!("{e}"));
        let revoked = watched.get().unwrap_or_else(|e| panic!("{e}"));
        assert!(!revoked.contains(&digest("a")), "revoked at once");
    }

    #[test]
    fn client_tokens_are_compared_by_digest() {
        let file = format!("# agents\n\n{}\n", digest("client-token-1").to_uppercase());
        let allowed = digests(&file);
        assert!(allowed.contains(&digest("client-token-1")));
        assert!(!allowed.contains(&digest("client-token-2")));
        assert_eq!(allowed.len(), 1);
    }

    #[test]
    fn a_config_names_its_engine_as_host_and_port() {
        let ok = load(
            r#"
listen = "0.0.0.0:9223"
upstream = "browser:9222"
upstream_token_file = "/run/secrets/browser-token"
client_tokens_file = "/etc/loams-gateway/browser-clients"
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ok.max_connections, 64);
        assert!(
            load(
                r#"
listen = "0.0.0.0:9223"
upstream = "ws://browser:9222/devtools/browser"
upstream_token_file = "/a"
client_tokens_file = "/b"
"#
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn a_session_is_spliced_with_the_engines_token_and_an_unknown_client_is_refused() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let engine_token = dir.path().join("engine");
        std::fs::write(&engine_token, "engine-token\n").unwrap_or_else(|e| panic!("{e}"));
        let clients = dir.path().join("clients");
        std::fs::write(&clients, format!("{}\n", digest("client-token-1")))
            .unwrap_or_else(|e| panic!("{e}"));

        // A stand-in engine: records the head it receives and answers 101.
        let engine = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let engine_addr = engine.local_addr().unwrap_or_else(|e| panic!("{e}"));
        let engine_task = tokio::spawn(async move {
            let (mut stream, _) = engine.accept().await.unwrap_or_else(|e| panic!("{e}"));
            let (head, _) = read_head(&mut stream)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\nhello")
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            head
        });

        let config = Arc::new(Config {
            listen: "127.0.0.1:0".parse().unwrap_or_else(|e| panic!("{e}")),
            upstream: engine_addr.to_string(),
            upstream_token_file: engine_token,
            client_tokens_file: clients,
            max_connections: 4,
            handshake_timeout_ms: 2_000,
            connect_timeout_ms: 2_000,
            copy_buffer_bytes: 4096,
        });
        let gateway = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let gateway_addr = gateway.local_addr().unwrap_or_else(|e| panic!("{e}"));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_on(gateway, Arc::clone(&config), async move {
            let _ = stopped.await;
        }));

        // An unknown client: 401, and the engine is never dialled.
        let mut stranger = TcpStream::connect(gateway_addr)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        stranger
            .write_all(HEAD.replace("client-token-1", "nope").as_bytes())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        stranger
            .write_all(b"\r\n\r\n")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut answer = String::new();
        stranger
            .read_to_string(&mut answer)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");

        // The known client: spliced through.
        let mut agent = TcpStream::connect(gateway_addr)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        agent
            .write_all(format!("{HEAD}\r\n\r\n").as_bytes())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut reply = vec![0u8; 64];
        let mut got = Vec::new();
        while !String::from_utf8_lossy(&got).contains("hello") {
            let n = agent
                .read(&mut reply)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(n > 0, "closed early: {}", String::from_utf8_lossy(&got));
            got.extend_from_slice(&reply[..n]);
        }
        assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 101"));
        let seen = engine_task.await.unwrap_or_else(|e| panic!("{e}"));
        assert!(
            seen.contains("Authorization: Bearer engine-token"),
            "{seen}"
        );
        assert!(!seen.contains("client-token-1"), "{seen}");

        let _ = stop.send(());
        let _ = server.await;
    }
}
