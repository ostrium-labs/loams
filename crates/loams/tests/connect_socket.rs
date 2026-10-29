//! The connect socket (`--connect-socket`, CC1 ruling R1.1): the main API on
//! a Unix socket for `loams-fabric connect`. Whatever `LOAMS_AUTH_MODE` says,
//! it takes an API key and nothing else, never reads Tailscale identity
//! headers, and its file is mode 0660. The streams API that connect calls
//! (`ProduceCloudEvents`, `FetchCloudEvents`) works over it.

#![cfg(unix)]

use std::net::SocketAddr;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use loams::auth::tailscale::TailscaleConfig;
use loams::auth::{AuthConfig, AuthMode};
use loams::{Server, ServerConfig};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const LOOPBACK0: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);
const WHO_AM_I: &str = "/loams.instance.v1.InstanceService/WhoAmI";

struct Running {
    server: Server,
    socket: PathBuf,
    _dir: TempDir,
}

impl Running {
    async fn start(auth: AuthConfig) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let socket = dir.path().join("run/connect/engine.sock");
        let mut config = ServerConfig::new(dir.path());
        config.listen = LOOPBACK0;
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.link.batch_interval = Duration::ZERO;
        config.auth = auth;
        config.connect_socket = Some(socket.clone());
        let server = Server::start(config).await.expect("start");
        Self {
            server,
            socket,
            _dir: dir,
        }
    }
}

/// One HTTP/1.1 request over the Unix socket: `(status, auth reason, body)`.
async fn call(
    socket: &Path,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, Value) {
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .expect("connect to the socket");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nhost: loams\r\nconnection: close\r\ncontent-length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, rest) = text.split_once("\r\n\r\n").expect("a response");
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("a status");
    let reason = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("loams-auth-reason")
                .then(|| v.trim().to_owned())
        })
        .unwrap_or_default();
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    });
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_owned()
    };
    (
        status,
        reason,
        serde_json::from_str(&body).unwrap_or(Value::Null),
    )
}

fn dechunk(mut rest: &str) -> String {
    let mut out = String::new();
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&tail[..n]);
        rest = &tail[n + 2..];
    }
    out
}

async fn connect_json(
    socket: &Path,
    rpc: &str,
    key: Option<&str>,
    extra: &[(&str, &str)],
    body: &Value,
) -> (u16, String, Value) {
    let bearer = key.map(|k| format!("Bearer {k}"));
    let mut headers = vec![
        ("content-type", "application/json"),
        ("connect-protocol-version", "1"),
    ];
    if let Some(b) = &bearer {
        headers.push(("authorization", b.as_str()));
    }
    headers.extend_from_slice(extra);
    call(socket, "POST", rpc, &headers, &body.to_string()).await
}

fn keys_file(dir: &TempDir) -> (String, PathBuf) {
    let (key, line) = loams::auth::api_key::new_key("connect");
    let path = dir.path().join("api-keys");
    std::fs::write(&path, format!("{line}\n")).expect("test fixture");
    (key, path)
}

/// Tailscale mode with API keys, as the release stack runs: the socket
/// admits the key and refuses everything else, a forged tailnet login
/// included.
fn tailscale_and_keys(path: &Path) -> AuthConfig {
    AuthConfig {
        mode: AuthMode::On {
            api_keys: true,
            oidc: false,
            tailscale: true,
        },
        api_keys_file: Some(path.to_path_buf()),
        tailscale: Some(TailscaleConfig {
            listen: LOOPBACK0,
            allowed_users: ["me@example.com".to_owned()].into(),
        }),
        ..AuthConfig::off()
    }
}

#[tokio::test]
async fn connect_socket_requires_an_api_key() {
    let keys = TempDir::new().unwrap();
    let (key, path) = keys_file(&keys);
    let running = Running::start(tailscale_and_keys(&path)).await;
    let socket = &running.socket;

    let (status, reason, _) = connect_json(socket, WHO_AM_I, None, &[], &json!({})).await;
    assert_eq!((status, reason.as_str()), (401, "token_missing"));

    let (status, _, _) = connect_json(
        socket,
        WHO_AM_I,
        Some("lk_not_a_real_key_000000000000000000"),
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(status, 401);

    let (status, _, body) = connect_json(socket, WHO_AM_I, Some(&key), &[], &json!({})).await;
    assert_eq!(status, 200, "{body}");

    // A JWT-shaped bearer is not an API key: refused without a lookup.
    let (status, reason, _) = connect_json(
        socket,
        WHO_AM_I,
        Some("eyJhbGciOi.e30.sig"),
        &[],
        &json!({}),
    )
    .await;
    assert_eq!((status, reason.as_str()), (401, "credential_not_accepted"));

    // The probe is public.
    let (status, _, _) = call(socket, "GET", "/health", &[], "").await;
    assert_eq!(status, 200);
    running.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_socket_ignores_tailscale_user_login() {
    let keys = TempDir::new().unwrap();
    let (_key, path) = keys_file(&keys);
    let running = Running::start(tailscale_and_keys(&path)).await;
    // The same login is admitted on the serve target...
    let target = running.server.tailscale_addr().expect("tailscale target");
    let on_target = reqwest::Client::new()
        .post(format!("http://{target}{WHO_AM_I}"))
        .header("content-type", "application/json")
        .header("connect-protocol-version", "1")
        .header("tailscale-user-login", "me@example.com")
        .body("{}")
        .send()
        .await
        .expect("call");
    assert_eq!(on_target.status().as_u16(), 200);
    // ...and is no credential at all on the socket.
    let (status, reason, _) = connect_json(
        &running.socket,
        WHO_AM_I,
        None,
        &[
            ("tailscale-user-login", "me@example.com"),
            ("tailscale-user-name", "Me"),
        ],
        &json!({}),
    )
    .await;
    assert_eq!((status, reason.as_str()), (401, "token_missing"));
    running.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_socket_needs_a_key_also_with_auth_off() {
    let running = Running::start(AuthConfig::off()).await;
    // The main port lets the developer on loopback in...
    let main = running.server.local_addr();
    let on_main = reqwest::Client::new()
        .post(format!("http://{main}{WHO_AM_I}"))
        .header("content-type", "application/json")
        .header("connect-protocol-version", "1")
        .body("{}")
        .send()
        .await
        .expect("call");
    assert_eq!(on_main.status().as_u16(), 200);
    // ...the socket does not.
    let (status, _, _) = connect_json(&running.socket, WHO_AM_I, None, &[], &json!({})).await;
    assert_eq!(status, 401);
    running.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_socket_file_is_0660_and_removed_on_shutdown() {
    let keys = TempDir::new().unwrap();
    let (_key, path) = keys_file(&keys);
    let running = Running::start(AuthConfig::api_keys(&path)).await;
    let socket = running.socket.clone();
    assert_eq!(running.server.connect_socket_path(), Some(socket.as_path()));
    let meta = std::fs::symlink_metadata(&socket).expect("the socket exists");
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o660);
    running.server.shutdown().await.unwrap();
    assert!(!socket.exists(), "the socket is removed on shutdown");
}

#[tokio::test]
async fn connect_socket_refuses_to_replace_a_regular_file() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("engine.sock");
    std::fs::write(&socket, "not a socket").unwrap();
    let mut config = ServerConfig::new(dir.path());
    config.listen = LOOPBACK0;
    config.connect_socket = Some(socket.clone());
    let Err(err) = Server::start(config).await else {
        panic!("a regular file at the socket's path was replaced");
    };
    assert!(err.to_string().contains("other than a socket"), "{err}");
    assert_eq!(std::fs::read_to_string(&socket).unwrap(), "not a socket");
}

/// What `loams-fabric connect` does over the socket: produce CloudEvents
/// (deduplicated by source + id) and fetch them back.
#[tokio::test]
async fn streams_work_over_the_connect_socket() {
    let keys = TempDir::new().unwrap();
    let (key, path) = keys_file(&keys);
    let running = Running::start(AuthConfig::api_keys(&path)).await;
    let socket = &running.socket;
    let k = Some(key.as_str());
    let (status, _, body) = connect_json(
        socket,
        "/loams.collection.v1.NamespaceService/CreateNamespace",
        k,
        &[],
        &json!({"namespace": "w"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = connect_json(
        socket,
        "/loams.stream.v1.StreamService/CreateStream",
        k,
        &[],
        &json!({"namespace": "w", "name": "orders", "partitions": 1}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let batch = json!([
        {"specversion": "1.0", "id": "o/0/0", "source": "/kafka/orders", "type": "t", "data": {"n": 1}},
        {"specversion": "1.0", "id": "o/0/1", "source": "/kafka/orders", "type": "t", "data": {"n": 2}},
    ]);
    let produce = json!({"namespace": "w", "stream": "orders", "jsonBatch": STANDARD.encode(batch.to_string())});
    let produce_rpc = "/loams.stream.v1.StreamService/ProduceCloudEvents";
    let (status, _, first) = connect_json(socket, produce_rpc, k, &[], &produce).await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["results"][0]["status"], "EVENT_STATUS_APPENDED");
    let (_, _, again) = connect_json(socket, produce_rpc, k, &[], &produce).await;
    assert_eq!(again["results"][1]["status"], "EVENT_STATUS_DUPLICATE");
    let (status, _, fetched) = connect_json(
        socket,
        "/loams.stream.v1.StreamService/FetchCloudEvents",
        k,
        &[],
        &json!({"namespace": "w", "stream": "orders", "offset": 0, "maxWaitMs": 2000}),
    )
    .await;
    assert_eq!(status, 200, "{fetched}");
    assert_eq!(fetched["count"], json!(2));
    let events: Value = serde_json::from_slice(
        &STANDARD
            .decode(fetched["jsonBatch"].as_str().expect("bytes"))
            .expect("base64"),
    )
    .expect("a JSON batch");
    assert_eq!(events[1]["id"], "o/0/1");
    running.server.shutdown().await.unwrap();
}
