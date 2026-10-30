//! The API over the network (DQ1 Task 2): HTTP/3, HTTP/2+TLS, plain
//! loopback, the race and its memory, pinning and auth.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use futures::stream;
use loams_transfer::api::{Client, ClientError};
use loams_transfer::keys::new_id;
use loams_transfer::net::race::udp_remembered_failed;
use loams_transfer::net::{ByteStream, ConnectOpts, Mode, ServerIdentity, Transport, Trust};
use loams_transfer::standalone::{self, Listeners, Serve};
use loams_transfer::{CreateTransfer, RunOutcome, TransferState, TransfersConfig};
use sha2::{Digest, Sha256};

const TOKEN: &str = "test-token-0123456789";
const CHUNK: u64 = 64 << 10;

struct Server {
    l: Listeners,
    pin: String,
}

async fn server(which: Serve) -> Server {
    let cfg = TransfersConfig {
        min_chunk: 4 << 10,
        ..TransfersConfig::default()
    };
    let (api, _) = standalone::mem_api(TOKEN, cfg);
    let (id, _, _) = ServerIdentity::self_signed(&["localhost".into()]).expect("cert");
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    let l = standalone::bind(addr, &id, standalone::app(api), which)
        .await
        .expect("bind");
    Server {
        l,
        pin: id.fingerprint(),
    }
}

fn opts(s: &Server, mode: Mode) -> ConnectOpts {
    let mut o = ConnectOpts::new(
        format!("https://localhost:{}", s.l.addr.port()),
        Trust::Pin(s.pin.clone()),
    );
    o.mode = mode;
    o.h3_timeout = Duration::from_millis(700);
    o
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn frames(b: &[u8], frame: usize) -> ByteStream {
    let parts: Vec<std::io::Result<Bytes>> = b
        .chunks(frame)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    stream::iter(parts).boxed()
}

fn content(size: u64) -> Vec<u8> {
    (0..size)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect()
}

async fn collect(mut s: ByteStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(b) = s.next().await {
        out.extend_from_slice(&b.expect("bytes"));
    }
    out
}

/// A whole upload and download through `c`.
async fn round_trip(c: &Client, chunk: u64, n: u64) {
    let tid = new_id();
    let data = content(chunk * n - 17);
    c.create(
        &tid,
        &CreateTransfer {
            name: "x.bin".into(),
            size: data.len() as u64,
            chunk_size: chunk,
            client_id: "install-1".into(),
        },
    )
    .await
    .expect("create");
    let g = c.lease(&tid, "install-1").await.expect("lease");
    for (i, part) in data.chunks(chunk as usize).enumerate() {
        c.put_chunk(&tid, i as u32, &sha(part), g.epoch, frames(part, 16 << 10))
            .await
            .expect("put");
    }
    let v = c
        .complete(&tid, g.epoch, &sha(&data))
        .await
        .expect("complete");
    assert_eq!(v.state, TransferState::Complete);
    let mut back = Vec::new();
    for i in 0..n as u32 {
        let (h, body) = c.get_chunk(&tid, i).await.expect("get");
        let bytes = collect(body).await;
        assert_eq!(sha(&bytes), h);
        back.extend(bytes);
    }
    assert_eq!(back, data);
}

#[tokio::test]
async fn h3_and_h2_serve_the_same_router() {
    let s = server(Serve { h3: true, h2: true }).await;
    for (mode, want) in [(Mode::H3, Transport::H3), (Mode::H2, Transport::H2)] {
        let c = Client::new(opts(&s, mode), "acme", TOKEN).expect("client");
        assert_eq!(c.transport().await.expect("transport"), want);
        round_trip(&c, CHUNK, 4).await;
    }
    // Auto prefers QUIC when it works.
    let c = Client::new(opts(&s, Mode::Auto), "acme", TOKEN).expect("client");
    assert_eq!(c.transport().await.expect("transport"), Transport::H3);
}

#[tokio::test]
async fn large_chunk_streams_over_h3() {
    // One 8 MiB chunk arrives as 64 KiB frames and streams into the bucket.
    let s = server(Serve { h3: true, h2: true }).await;
    let c = Client::new(opts(&s, Mode::H3), "acme", TOKEN).expect("client");
    round_trip(&c, 8 << 20, 2).await;
}

#[tokio::test]
async fn udp_blocked_falls_back_to_h2_and_is_remembered() {
    let s = server(Serve {
        h3: false,
        h2: true,
    })
    .await;
    let dir = tempfile::tempdir().expect("dir");
    let mut o = opts(&s, Mode::Auto);
    o.memory = Some(dir.path().join("transport.json"));
    let c = Client::new(o.clone(), "acme", TOKEN).expect("client");
    assert_eq!(c.transport().await.expect("transport"), Transport::H2);
    round_trip(&c, CHUNK, 2).await;
    // QUIC's attempt finishes in the background and is remembered.
    let mut remembered = false;
    for _ in 0..40 {
        if udp_remembered_failed(&o) {
            remembered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(remembered, "the UDP failure must be remembered");
    // The next connection goes straight to HTTP/2.
    let start = std::time::Instant::now();
    let c2 = Client::new(o, "acme", TOKEN).expect("client");
    assert_eq!(c2.transport().await.expect("transport"), Transport::H2);
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "no race the second time"
    );
}

#[tokio::test]
async fn plain_loopback_speaks_h2c() {
    let cfg = TransfersConfig {
        min_chunk: 4 << 10,
        ..TransfersConfig::default()
    };
    let (api, _) = standalone::mem_api(TOKEN, cfg);
    let l = standalone::bind_plain("127.0.0.1:0".parse().expect("addr"), standalone::app(api))
        .await
        .expect("bind");
    let o = ConnectOpts::new(format!("http://127.0.0.1:{}", l.addr.port()), Trust::System);
    let c = Client::new(o, "acme", TOKEN).expect("client");
    assert_eq!(c.transport().await.expect("transport"), Transport::H2c);
    round_trip(&c, CHUNK, 2).await;
}

#[tokio::test]
async fn pinned_cert_accepted_other_refused() {
    let s = server(Serve { h3: true, h2: true }).await;
    let other = "0".repeat(64);
    for mode in [Mode::H3, Mode::H2] {
        let mut o = opts(&s, mode);
        o.trust = Trust::Pin(other.clone());
        let c = Client::new(o, "acme", TOKEN).expect("client");
        assert!(
            c.transport().await.is_err(),
            "{mode:?} must refuse a wrong pin"
        );
    }
}

#[tokio::test]
async fn unauthorized_refused() {
    let s = server(Serve { h3: true, h2: true }).await;
    let c = Client::new(opts(&s, Mode::H3), "acme", "wrong-token-xxxxxxxx").expect("client");
    match c.list().await {
        Err(ClientError::Api { status, body }) => {
            assert_eq!(status.as_u16(), 401);
            assert_eq!(body.code, "unauthenticated");
        }
        other => panic!("expected 401, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_hash_refused_422() {
    let s = server(Serve { h3: true, h2: true }).await;
    let c = Client::new(opts(&s, Mode::H3), "acme", TOKEN).expect("client");
    let tid = new_id();
    let data = content(CHUNK);
    c.create(
        &tid,
        &CreateTransfer {
            name: "x".into(),
            size: CHUNK,
            chunk_size: CHUNK,
            client_id: "install-1".into(),
        },
    )
    .await
    .expect("create");
    let g = c.lease(&tid, "install-1").await.expect("lease");
    let r = c
        .put_chunk(&tid, 0, &sha(b"nope"), g.epoch, frames(&data, 8192))
        .await;
    match r {
        Err(ClientError::Api { status, body }) => {
            assert_eq!(status.as_u16(), 422);
            assert_eq!(body.code, "integrity");
        }
        other => panic!("expected 422, got {other:?}"),
    }
    // The connection is still usable, and the lease conflict is typed.
    let r = c.lease(&tid, "install-2").await;
    assert_eq!(
        r.err()
            .and_then(|e| e.code().map(str::to_string))
            .as_deref(),
        Some("lease_held")
    );
}

#[tokio::test]
async fn followed_log_streams_until_the_run_ends() {
    let s = server(Serve { h3: true, h2: true }).await;
    let c = Client::new(opts(&s, Mode::H3), "acme", TOKEN).expect("client");
    let rid = new_id();
    c.create_run(&rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let claimed = c
        .claim("runner-a", &["ci".to_string()])
        .await
        .expect("claim")
        .expect("run");
    c.append_log(&rid, claimed.epoch, 0, Bytes::from_static(b"one\n"))
        .await
        .expect("append");
    let tail = c.read_log(&rid, 0, true).await.expect("tail");
    assert_eq!(tail.committed, 4);
    let reader = tokio::spawn(collect(tail.bytes));
    tokio::time::sleep(Duration::from_millis(400)).await;
    c.append_log(&rid, claimed.epoch, 4, Bytes::from_static(b"two\n"))
        .await
        .expect("append");
    c.finish_run(&rid, claimed.epoch, RunOutcome::Succeeded, Some(0), None)
        .await
        .expect("finish");
    let got = tokio::time::timeout(Duration::from_secs(10), reader)
        .await
        .expect("the tail ends with the run")
        .expect("join");
    assert_eq!(got, b"one\ntwo\n");
    // A reader that disconnected resumes at its offset.
    let rest = c.read_log(&rid, 4, false).await.expect("resume");
    assert_eq!(collect(rest.bytes).await, b"two\n");
}
