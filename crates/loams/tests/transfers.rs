//! Transfers and runs on the engine (design §85, DQ1 Task 5): the routes are
//! mounted behind the engine's authentication, HTTP/3 is served on UDP at
//! the TLS main port, and HTTP/2 over TLS is the same router on TCP.
#![cfg(feature = "transfer")]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use loams::auth::AuthConfig;
use loams::tls::TlsConfig;
use loams::{Server, ServerConfig};
use loams_transfer::api::{Client, ClientError};
use loams_transfer::keys::new_id;
use loams_transfer::net::{ConnectOpts, Mode, Transport, Trust};
use loams_transfer::{CreateTransfer, TransferState};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const LOOPBACK0: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);
const CHUNK: u64 = 8 << 20;

fn config(dir: &TempDir) -> (ServerConfig, String) {
    let mut config = ServerConfig::new(dir.path());
    config.listen = LOOPBACK0;
    config.log.flush_interval = Duration::from_millis(20);
    config.worker_poll_interval = Duration::from_millis(50);
    config.live = None;
    let (key, line) = loams::auth::api_key::new_key("transfers");
    let keys = dir.path().join("api-keys");
    std::fs::write(&keys, format!("{line}\n")).expect("the key file");
    config.auth = AuthConfig::api_keys(&keys);
    (config, key)
}

/// A self-signed certificate: its path, its key's path, and its SHA-256.
fn self_signed(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let made =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("a certificate");
    let cert = dir.join("main.crt");
    let key = dir.join("main.key");
    std::fs::write(&cert, made.cert.pem()).expect("write");
    std::fs::write(&key, made.signing_key.serialize_pem()).expect("write");
    let sha = to_hex(&Sha256::digest(made.cert.der()));
    (cert, key, sha)
}

/// Creates namespace `name` through the engine's REST API (a loopback test
/// server with a self-signed certificate).
async fn create_namespace(base: &str, key: &str, name: &str) {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("client");
    let r = client
        .post(format!("{base}/v1/namespaces"))
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(format!("{{\"name\":\"{name}\"}}"))
        .send()
        .await
        .expect("create namespace");
    assert!(r.status().is_success(), "{}", r.status());
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha(b: &[u8]) -> String {
    to_hex(&Sha256::digest(b))
}

async fn round_trip(c: &Client) {
    let tid = new_id();
    let data: Vec<u8> = (0..(CHUNK as usize * 2 + 1234))
        .map(|n| (n % 241) as u8)
        .collect();
    c.create(
        &tid,
        &CreateTransfer {
            name: "blob.bin".into(),
            size: data.len() as u64,
            chunk_size: CHUNK,
            client_id: "install-test".into(),
        },
    )
    .await
    .expect("create");
    let g = c.lease(&tid, "install-test").await.expect("lease");
    for (i, part) in data.chunks(CHUNK as usize).enumerate() {
        let body = futures::stream::iter(
            part.chunks(64 << 10)
                .map(|p| Ok(Bytes::copy_from_slice(p)))
                .collect::<Vec<_>>(),
        )
        .boxed();
        c.put_chunk(&tid, i as u32, &sha(part), g.epoch, body)
            .await
            .expect("put");
    }
    let v = c
        .complete(&tid, g.epoch, &sha(&data))
        .await
        .expect("complete");
    assert_eq!(v.state, TransferState::Complete);
    let mut back = Vec::new();
    for i in 0..3 {
        let (_, mut body) = c.get_chunk(&tid, i).await.expect("get");
        while let Some(b) = body.next().await {
            back.extend_from_slice(&b.expect("bytes"));
        }
    }
    assert_eq!(back, data);
}

/// With TLS files, the engine serves the transfer API over HTTP/3 on UDP at
/// the main port and over HTTP/2 on TCP, behind its API keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfers_mounted_and_authorized_h3_on_main_port_with_tls() {
    let dir = TempDir::new().expect("a temp dir");
    let (cert, key_file, pin) = self_signed(dir.path());
    let (mut config, key) = config(&dir);
    config.tls = TlsConfig::files(&cert, &key_file);
    let server = Server::start(config).await.expect("starts");
    let port = server.local_addr().port();
    let base = format!("https://127.0.0.1:{port}");
    create_namespace(&base, &key, "acme").await;

    for (mode, want) in [(Mode::H3, Transport::H3), (Mode::H2, Transport::H2)] {
        let mut o = ConnectOpts::new(base.clone(), Trust::Pin(pin.clone()));
        o.mode = mode;
        let c = Client::new(o, "acme", &key).expect("client");
        assert_eq!(c.transport().await.expect("connects"), want);
        round_trip(&c).await;
    }

    // No key: the engine's layer refuses before the routes.
    let mut o = ConnectOpts::new(base.clone(), Trust::Pin(pin.clone()));
    o.mode = Mode::H3;
    let anon = Client::new(o, "acme", "not-a-key").expect("client");
    match anon.list().await {
        Err(ClientError::Api { status, .. }) => assert_eq!(status.as_u16(), 401),
        other => panic!("expected 401, got {other:?}"),
    }
    // An unknown namespace is not found; a runner may claim with an API key.
    let mut o = ConnectOpts::new(base.clone(), Trust::Pin(pin.clone()));
    o.mode = Mode::H3;
    let c = Client::new(o.clone(), "nope", &key).expect("client");
    assert_eq!(
        c.list()
            .await
            .err()
            .and_then(|e| e.code().map(str::to_string))
            .as_deref(),
        Some("not_found")
    );
    let c = Client::new(o, "acme", &key).expect("client");
    let rid = new_id();
    c.create_run(&rid, "ci", BTreeMap::new())
        .await
        .expect("run");
    let claimed = c.claim("runner-1", &["ci".into()]).await.expect("claim");
    assert_eq!(claimed.map(|r| r.rid), Some(rid));
    server.shutdown().await.expect("stops");
}

/// Without TLS there is no QUIC listener; a loopback client reaches the
/// routes over plain HTTP/2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_h3_without_tls() {
    let dir = TempDir::new().expect("a temp dir");
    let (config, key) = config(&dir);
    let server = Server::start(config).await.expect("starts");
    let port = server.local_addr().port();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/v1/namespaces"))
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body("{\"name\":\"acme\"}")
        .send()
        .await
        .expect("create namespace");
    assert!(r.status().is_success());
    let c =
        Client::new(ConnectOpts::new(base.clone(), Trust::System), "acme", &key).expect("client");
    assert_eq!(c.transport().await.expect("connects"), Transport::H2c);
    assert!(c.list().await.expect("list").is_empty());
    let mut o = ConnectOpts::new(
        format!("https://127.0.0.1:{port}"),
        Trust::Pin("0".repeat(64)),
    );
    o.mode = Mode::H3;
    o.h3_timeout = Duration::from_millis(500);
    let h3 = Client::new(o, "acme", &key).expect("client");
    assert!(h3.transport().await.is_err(), "no QUIC without TLS files");
    server.shutdown().await.expect("stops");
}
