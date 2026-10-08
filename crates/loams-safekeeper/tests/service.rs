//! The WAL service over TCP, driven by a hand-rolled proposer and reader
//! speaking walproposer's bytes.
#![cfg(feature = "server")]
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use loams_safekeeper::pgwire::client;
use loams_safekeeper::proto::{
    AcceptorMessage, AppendRequest, AppendRequestHeader, ProposerElected, ProposerGreeting,
    ProposerMessage, VoteRequest,
};
use loams_safekeeper::service::{WalService, WalServiceConfig};
use loams_safekeeper::store::MemWalStore;
use loams_safekeeper::store::WalStore;
use loams_safekeeper::types::{Configuration, Id, Lsn, TermHistory, TermLsn};
use tokio::net::TcpStream;

const TENANT: &str = "cf0480929707ee75372337efaa5ecf96";
const TIMELINE: &str = "112ded66422aa5e953e5440fa5427ac4";

async fn start() -> (SocketAddr, SocketAddr, Arc<WalService<MemWalStore>>) {
    start_cfg(None).await
}

async fn start_cfg(
    auth_token: Option<String>,
) -> (SocketAddr, SocketAddr, Arc<WalService<MemWalStore>>) {
    start_svc(WalServiceConfig {
        auth_token,
        ..Default::default()
    })
    .await
}

async fn start_svc(
    cfg: WalServiceConfig,
) -> (SocketAddr, SocketAddr, Arc<WalService<MemWalStore>>) {
    let svc = WalService::new(
        Arc::new(MemWalStore::new()),
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            ..cfg
        },
    );
    let pg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pg_addr = pg.local_addr().unwrap();
    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_addr = web.local_addr().unwrap();
    let app = loams_safekeeper::http::router(svc.clone());
    tokio::spawn(async move { axum::serve(web, app).await });
    tokio::spawn(svc.clone().serve(pg, std::future::pending()));
    (pg_addr, web_addr, svc)
}

async fn connect(addr: SocketAddr) -> TcpStream {
    connect_with(addr, "").await
}

/// Connect with more startup options after the timeline's.
async fn connect_with(addr: SocketAddr, extra: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let options = format!("-c timeline_id={TIMELINE} tenant_id={TENANT} {extra}");
    client::startup(
        &mut s,
        &[
            ("user", "cloud_admin"),
            ("dbname", "replication"),
            ("options", &options),
        ],
    )
    .await
    .unwrap();
    s
}

async fn send(s: &mut TcpStream, m: ProposerMessage) {
    let mut buf = BytesMut::new();
    m.serialize(&mut buf);
    client::send_copy_data(s, &buf).await.unwrap();
}

async fn recv(s: &mut TcpStream) -> AcceptorMessage {
    AcceptorMessage::parse(client::recv_copy_data(s).await.unwrap().unwrap()).unwrap()
}

fn append(term: u64, begin: u64, data: &[u8], commit: u64) -> ProposerMessage {
    ProposerMessage::Append(AppendRequest {
        h: AppendRequestHeader {
            generation: 0,
            term,
            begin_lsn: Lsn(begin),
            end_lsn: Lsn(begin + data.len() as u64),
            commit_lsn: Lsn(commit),
            truncate_lsn: Lsn(0),
        },
        wal: Bytes::copy_from_slice(data),
    })
}

const START: u64 = 0x0149_6F10;

/// Greeting, vote, election and a stream of appends, as walproposer does.
async fn propose(pg: SocketAddr, payload: &[u8]) -> TcpStream {
    let mut p = connect(pg).await;
    client::query(
        &mut p,
        "START_WAL_PUSH (proto_version '3', allow_timeline_creation 'true')",
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut p).await.unwrap();
    send(
        &mut p,
        ProposerMessage::Greeting(ProposerGreeting {
            tenant_id: TENANT.parse::<Id>().unwrap(),
            timeline_id: TIMELINE.parse::<Id>().unwrap(),
            mconf: Configuration::default(),
            pg_version: 160_009,
            system_id: 99,
            wal_seg_size: 16 << 20,
        }),
    )
    .await;
    assert!(matches!(recv(&mut p).await, AcceptorMessage::Greeting(g) if g.term == 0));
    send(
        &mut p,
        ProposerMessage::VoteRequest(VoteRequest {
            generation: 0,
            term: 1,
        }),
    )
    .await;
    match recv(&mut p).await {
        AcceptorMessage::VoteResponse(v) => assert!(v.vote_given),
        other => panic!("{other:?}"),
    }
    send(
        &mut p,
        ProposerMessage::Elected(ProposerElected {
            generation: 0,
            term: 1,
            start_streaming_at: Lsn(START),
            term_history: TermHistory(vec![TermLsn {
                term: 1,
                lsn: Lsn(START),
            }]),
        }),
    )
    .await;
    // Stream without waiting, as walproposer does; acks may cover several.
    let mut at = START;
    for chunk in payload.chunks(7) {
        send(&mut p, append(1, at, chunk, 0)).await;
        at += chunk.len() as u64;
    }
    let end = START + payload.len() as u64;
    loop {
        match recv(&mut p).await {
            AcceptorMessage::AppendResponse(r) if r.flush_lsn == Lsn(end) => break,
            AcceptorMessage::AppendResponse(r) => assert!(r.flush_lsn < Lsn(end)),
            other => panic!("{other:?}"),
        }
    }
    // The quorum commit arrives in a heartbeat. (A pipelined store may
    // still owe responses for writes the one above already covered.)
    send(&mut p, append(1, end, b"", end)).await;
    loop {
        match recv(&mut p).await {
            AcceptorMessage::AppendResponse(r) if r.commit_lsn == Lsn(end) => break,
            AcceptorMessage::AppendResponse(r) => assert_eq!(r.flush_lsn, Lsn(end)),
            other => panic!("{other:?}"),
        }
    }
    p
}

#[tokio::test]
async fn walproposer_flow_then_replication_and_status() {
    let (pg, web, _svc) = start().await;
    let payload: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
    let _p = propose(pg, &payload).await;
    let end = START + payload.len() as u64;

    let mut q = connect(pg).await;
    let rows = client::query_rows(&mut q, "TIMELINE_STATUS").await.unwrap();
    assert_eq!(
        rows,
        vec![vec![Some(Lsn(end).to_string()), Some(Lsn(end).to_string())]]
    );
    let rows = client::query_rows(&mut q, "IDENTIFY_SYSTEM").await.unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("99"));
    assert_eq!(rows[0][2], Some(Lsn(end).to_string()));
    assert!(client::query_rows(&mut q, "SELECT 1").await.is_err());

    // A vanilla reader gets the committed WAL, from an offset.
    let mut r = connect(pg).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START + 10)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let mut got = Vec::new();
    while got.len() < payload.len() - 10 {
        let mut m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
        match m.get_u8() {
            b'w' => {
                let start = m.get_u64();
                let _end = m.get_u64();
                let _ts = m.get_i64();
                assert_eq!(start, START + 10 + got.len() as u64);
                got.extend_from_slice(&m);
            }
            b'k' => {}
            t => panic!("unexpected {}", t as char),
        }
    }
    assert_eq!(got, &payload[10..]);

    // The HTTP API shows the same head.
    let body = http_get(web, &format!("/v1/tenant/{TENANT}/timeline/{TIMELINE}")).await;
    assert!(
        body.contains(&format!("\"flush_lsn\":\"{}\"", Lsn(end))),
        "{body}"
    );
    assert!(body.contains("\"term\":1"), "{body}");
}

/// A RawKv whose writes complete out of order: each put sleeps a little,
/// longer for some than for later ones.
#[derive(Debug, Default)]
struct Shuffled {
    inner: loams_safekeeper::tikv_raw::MemRawKv,
    n: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl loams_safekeeper::tikv_raw::RawKv for Shuffled {
    async fn get(&self, key: Vec<u8>) -> Result<Option<Vec<u8>>, loams_safekeeper::Error> {
        self.inner.get(key).await
    }
    async fn batch_put(
        &self,
        pairs: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), loams_safekeeper::Error> {
        let i = self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis((i * 7) % 5)).await;
        self.inner.batch_put(pairs).await
    }
    async fn compare_and_swap(
        &self,
        key: Vec<u8>,
        expected: Option<Vec<u8>>,
        new: Vec<u8>,
    ) -> Result<(Option<Vec<u8>>, bool), loams_safekeeper::Error> {
        self.inner.compare_and_swap(key, expected, new).await
    }
    async fn scan(
        &self,
        from: Vec<u8>,
        to: Vec<u8>,
        limit: u32,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, loams_safekeeper::Error> {
        self.inner.scan(from, to, limit).await
    }
    async fn delete_range(
        &self,
        from: Vec<u8>,
        to: Vec<u8>,
    ) -> Result<(), loams_safekeeper::Error> {
        self.inner.delete_range(from, to).await
    }
}

/// The walproposer flow over the raw store with 8 appends in flight whose
/// writes land out of order: acks stay monotonic and cover only contiguous
/// WAL, and readers get exactly the stream.
#[tokio::test]
async fn pipelined_appends_over_the_raw_store() {
    let store = Arc::new(loams_safekeeper::tikv_raw::RawWalStore::new(
        Arc::new(Shuffled::default()),
        b"t/".to_vec(),
        8,
    ));
    assert_eq!(store.max_in_flight(), 8);
    let svc = WalService::new(
        store,
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            ..Default::default()
        },
    );
    let pg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pg_addr = pg.local_addr().unwrap();
    tokio::spawn(svc.clone().serve(pg, std::future::pending()));

    let payload: Vec<u8> = (0..3000u32).map(|i| (i % 253) as u8).collect();
    let _p = propose(pg_addr, &payload).await;
    let end = START + payload.len() as u64;
    let mut q = connect(pg_addr).await;
    let rows = client::query_rows(&mut q, "TIMELINE_STATUS").await.unwrap();
    assert_eq!(rows[0][0], Some(Lsn(end).to_string()));

    let mut r = connect(pg_addr).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let mut got = Vec::new();
    while got.len() < payload.len() {
        let mut m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
        if m.get_u8() == b'w' {
            let start = m.get_u64();
            let _end = m.get_u64();
            let _ts = m.get_i64();
            assert_eq!(start, START + got.len() as u64);
            got.extend_from_slice(&m);
        }
    }
    assert_eq!(got, payload);
}

#[tokio::test]
async fn proto_v2_and_missing_timeline_are_refused() {
    let (pg, _web, _svc) = start().await;
    let mut p = connect(pg).await;
    client::query(&mut p, "START_WAL_PUSH (proto_version '2')")
        .await
        .unwrap();
    assert!(client::expect_copy_both(&mut p).await.is_err());

    let mut r = connect(pg).await;
    client::query(&mut r, "START_REPLICATION PHYSICAL 0/0")
        .await
        .unwrap();
    let err = client::expect_copy_both(&mut r).await;
    assert!(err.is_err() || client::recv_copy_data(&mut r).await.is_err());
}

#[tokio::test]
async fn http_create_then_walproposer_without_creation() {
    let (pg, web, _svc) = start().await;
    let body = http_post(
        web,
        "/v1/tenant/timeline",
        &format!(
            r#"{{"tenant_id":"{TENANT}","timeline_id":"{TIMELINE}","pg_version":16,"start_lsn":"0/1496F10","mconf":{{"generation":1}}}}"#
        ),
    )
    .await;
    assert!(
        body.contains("\"timeline_start_lsn\":\"0/1496F10\""),
        "{body}"
    );
    assert!(body.contains("\"pg_version\":160000"), "{body}");

    let mut p = connect(pg).await;
    client::query(
        &mut p,
        "START_WAL_PUSH (proto_version '3', allow_timeline_creation 'false')",
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut p).await.unwrap();
    send(
        &mut p,
        ProposerMessage::Greeting(ProposerGreeting {
            tenant_id: TENANT.parse::<Id>().unwrap(),
            timeline_id: TIMELINE.parse::<Id>().unwrap(),
            mconf: Configuration::default(),
            pg_version: 160_009,
            system_id: 0,
            wal_seg_size: 16 << 20,
        }),
    )
    .await;
    assert!(matches!(recv(&mut p).await, AcceptorMessage::Greeting(_)));
}

async fn http(addr: SocketAddr, req: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

async fn http_get(addr: SocketAddr, path: &str) -> String {
    http(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn http_post(addr: SocketAddr, path: &str, body: &str) -> String {
    http(
        addr,
        format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

#[tokio::test]
async fn auth_token_is_required_on_both_listeners() {
    let (pg, web, _svc) = start_cfg(Some("s3cret".into())).await;
    let options = format!("-c timeline_id={TIMELINE} tenant_id={TENANT}");
    let mut s = TcpStream::connect(pg).await.unwrap();
    let bad = client::startup(
        &mut s,
        &[("user", "u"), ("options", &options), ("password", "nope")],
    )
    .await;
    assert!(bad.is_err());
    let mut s = TcpStream::connect(pg).await.unwrap();
    client::startup(
        &mut s,
        &[("user", "u"), ("options", &options), ("password", "s3cret")],
    )
    .await
    .unwrap();
    assert!(
        http_get(web, "/v1/status")
            .await
            .starts_with("HTTP/1.1 401")
    );
    let ok = http(
        web,
        "GET /v1/status HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer s3cret\r\nConnection: close\r\n\r\n".into(),
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
}

// ---- The interpreted sender (Task 31), with a stand-in for Neon's decoder.

use loams_safekeeper::send::{
    Compression, InterpretedProtocol, Interpreter, ShardDecoder, ShardSpec, WalInterpreter,
    WireFormat,
};

/// What the pageserver puts in its startup options (`wal_receiver_protocol`
/// at its default: protobuf, zstd level 1).
const INTERPRETED: &str = r#"protocol={"type":"interpreted","args":{"format":"protobuf","compression":{"zstd":{"level":1}}}}"#;

/// A decoder stand-in: every WAL byte is a "record" whose shard is
/// `byte % shard_count`; a shard's batch is its records, in order.
#[derive(Debug, Default)]
struct ByteShards {
    opened: std::sync::Mutex<Vec<(Lsn, u32, InterpretedProtocol, ShardSpec)>>,
}

struct ByteDecoder {
    shard: ShardSpec,
    at: Lsn,
}

impl WalInterpreter for ByteShards {
    fn decoder(
        &self,
        start: Lsn,
        pg_version: u32,
        protocol: InterpretedProtocol,
        shard: ShardSpec,
    ) -> Result<Box<dyn ShardDecoder>, loams_safekeeper::Error> {
        self.opened
            .lock()
            .unwrap()
            .push((start, pg_version, protocol, shard));
        Ok(Box::new(ByteDecoder { shard, at: start }))
    }
}

#[async_trait::async_trait]
impl ShardDecoder for ByteDecoder {
    async fn decode(
        &mut self,
        start: Lsn,
        wal: Bytes,
    ) -> Result<Option<Bytes>, loams_safekeeper::Error> {
        assert_eq!(start, self.at, "WAL fed out of order");
        self.at = Lsn(start.0 + wal.len() as u64);
        let count = u32::from(self.shard.count.max(1));
        let mine = u32::from(self.shard.number);
        Ok(Some(
            wal.iter()
                .copied()
                .filter(|b| u32::from(*b) % count == mine)
                .collect::<Vec<u8>>()
                .into(),
        ))
    }
}

/// One interpreted message: `'0'`, `streaming_lsn`, `commit_lsn`, the batch.
fn interpreted_message(mut m: Bytes) -> Option<(Lsn, Lsn, Bytes)> {
    match m.get_u8() {
        b'0' => {
            let streaming = Lsn(m.get_u64());
            let commit = Lsn(m.get_u64());
            Some((streaming, commit, m))
        }
        b'k' => None,
        t => panic!("unexpected {}", t as char),
    }
}

/// Read interpreted batches until `streaming_lsn` reaches `end`.
async fn read_batches(r: &mut TcpStream, end: Lsn) -> (Vec<u8>, Vec<(Lsn, Lsn)>) {
    let mut data = Vec::new();
    let mut lsns = Vec::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), client::recv_copy_data(r))
            .await
            .expect("a batch within 5 s")
            .unwrap()
            .unwrap();
        if let Some((streaming, commit, batch)) = interpreted_message(m) {
            data.extend_from_slice(&batch);
            lsns.push((streaming, commit));
            if streaming >= end {
                return (data, lsns);
            }
        }
    }
}

async fn start_interpreted(interp: Arc<ByteShards>) -> (SocketAddr, Arc<WalService<MemWalStore>>) {
    let (pg, _, svc) = start_svc(WalServiceConfig {
        interpreter: Some(Interpreter(interp)),
        ..Default::default()
    })
    .await;
    (pg, svc)
}

/// The pageserver's request is served in process: committed WAL from its
/// start LSN, decoded, framed as the fork's sender frames it.
#[tokio::test]
async fn interpreted_replication_streams_decoded_committed_wal() {
    let interp = Arc::new(ByteShards::default());
    let (pg, _svc) = start_interpreted(interp.clone()).await;
    // More than one MAX_SEND_SIZE chunk.
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let _p = propose(pg, &payload).await;
    let end = Lsn(START + payload.len() as u64);

    let mut r = connect_with(pg, &format!("{INTERPRETED} {UNSHARDED}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START + 10)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let (data, lsns) = read_batches(&mut r, end).await;
    assert_eq!(data, &payload[10..]);
    // Each batch ends a read of at most MAX_SEND_SIZE; commit_lsn is the
    // readable end.
    let mut prev = Lsn(START + 10);
    for (streaming, commit) in &lsns {
        assert!(*streaming > prev);
        assert!(streaming.0 - prev.0 <= loams_safekeeper::proto::MAX_SEND_SIZE as u64);
        assert_eq!(*commit, end);
        prev = *streaming;
    }
    assert!(lsns.len() >= 3, "{lsns:?}");
    let opened = interp.opened.lock().unwrap().clone();
    assert_eq!(
        opened,
        vec![(
            Lsn(START + 10),
            160_009,
            InterpretedProtocol {
                format: WireFormat::Protobuf,
                compression: Some(Compression::Zstd { level: 1 }),
            },
            ShardSpec {
                number: 0,
                count: 0,
                stripe_size: 2048
            },
        )]
    );
}

/// Each shard's connection gets a decoder for its own shard, and only that
/// shard's records.
#[tokio::test]
async fn shard_filter_routes_records() {
    let interp = Arc::new(ByteShards::default());
    let (pg, _svc) = start_interpreted(interp.clone()).await;
    let payload: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let _p = propose(pg, &payload).await;
    let end = Lsn(START + payload.len() as u64);

    let mut readers = Vec::new();
    for n in 0..2u8 {
        let opts = format!("{INTERPRETED} shard_count=2 shard_number={n} shard_stripe_size=2048");
        let mut r = connect_with(pg, &opts).await;
        client::query(
            &mut r,
            &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
        )
        .await
        .unwrap();
        client::expect_copy_both(&mut r).await.unwrap();
        readers.push(r);
    }
    for (n, r) in readers.iter_mut().enumerate() {
        let (data, lsns) = read_batches(r, end).await;
        let want: Vec<u8> = payload
            .iter()
            .copied()
            .filter(|b| usize::from(*b) % 2 == n)
            .collect();
        assert_eq!(data, want, "shard {n}");
        // Every shard advances to the end, records or not.
        assert_eq!(lsns.last().map(|l| l.0), Some(end));
    }
    let mut shards: Vec<ShardSpec> = interp.opened.lock().unwrap().iter().map(|o| o.3).collect();
    shards.sort_by_key(|s| s.number);
    assert_eq!(
        shards,
        vec![
            ShardSpec {
                number: 0,
                count: 2,
                stripe_size: 2048
            },
            ShardSpec {
                number: 1,
                count: 2,
                stripe_size: 2048
            },
        ]
    );
}

/// The pageserver's `'z'` feedback on an interpreted stream reaches the head
/// (coalesced to one write a second, while the stream stays open).
#[tokio::test]
async fn interpreted_stream_records_pageserver_feedback() {
    let (pg, svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let payload = vec![7u8; 1000];
    let _p = propose(pg, &payload).await;
    let mut r = connect_with(pg, &format!("{INTERPRETED} {UNSHARDED}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let mut body = BytesMut::new();
    body.put_u8(b'z');
    body.put_u64(0);
    loams_safekeeper::types::PageserverFeedback {
        remote_consistent_lsn: Lsn(START + 500),
        ..Default::default()
    }
    .serialize(&mut body);
    client::send_copy_data(&mut r, &body).await.unwrap();
    let tl = loams_safekeeper::TimelineId::new(TENANT.parse().unwrap(), TIMELINE.parse().unwrap());
    for _ in 0..300 {
        if svc
            .store()
            .load(&tl)
            .await
            .unwrap()
            .unwrap()
            .remote_consistent_lsn
            == Lsn(START + 500)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("remote_consistent_lsn not recorded");
}

/// Without an interpreter (a build without the decoder) the interpreted
/// protocol is refused with a reason; vanilla replication still works.
#[tokio::test]
async fn interpreted_refused_without_an_interpreter() {
    let (pg, _, _svc) = start().await;
    let _p = propose(pg, b"0123456789").await;
    let mut r = connect_with(pg, INTERPRETED).await;
    let rows = client::query_rows(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await;
    let err = rows.unwrap_err().to_string();
    assert!(err.contains("interpreted"), "{err}");
}

/// A malformed `protocol` option is an error, not a vanilla stream.
#[tokio::test]
async fn malformed_protocol_option_is_refused() {
    let (pg, _svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let _p = propose(pg, b"0123456789").await;
    let mut r = connect_with(pg, r#"protocol={"type":"telepathy"}"#).await;
    let rows = client::query_rows(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await;
    assert!(rows.is_err());
}

/// Shard options for an unsharded tenant, as the pageserver sends them.
const UNSHARDED: &str = "shard_count=0 shard_number=0 shard_stripe_size=2048";

/// A `'z'` CopyData body with `remote_consistent_lsn` = `lsn`.
fn ps_feedback(lsn: Lsn) -> BytesMut {
    let mut body = BytesMut::new();
    body.put_u8(b'z');
    body.put_u64(0);
    loams_safekeeper::types::PageserverFeedback {
        last_received_lsn: lsn,
        disk_consistent_lsn: lsn,
        remote_consistent_lsn: lsn,
        ..Default::default()
    }
    .serialize(&mut body);
    body
}

/// A reader far behind, which writes feedback in the same loop it reads in
/// (the pageserver's walreceiver), floods feedback during the catch-up: the
/// stream must not deadlock, and the feedback must reach the head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catch_up_with_a_feedback_flood_completes() {
    let (pg, svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let tl = loams_safekeeper::TimelineId::new(TENANT.parse().unwrap(), TIMELINE.parse().unwrap());
    // 32 MiB of committed WAL: far more than the socket buffers hold.
    let range = loams_safekeeper::propose::WalRange {
        start: Lsn(START),
        pg_version: 160_009,
        system_id: 99,
        wal: (0..32u32 << 20)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>()
            .into(),
    };
    let end = range.end();
    let _p = loams_safekeeper::propose::push_committed(&pg.to_string(), tl, &range)
        .await
        .unwrap();
    let mut r = connect_with(pg, &format!("{INTERPRETED} {UNSHARDED}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let last_fb = Lsn(START + 4096);
    let flood = async {
        // 1024 feedback messages (about 130 KiB) per batch read, in one
        // task: a reader that stops reading the socket while its writes are
        // blocked, with more feedback than the socket buffers hold.
        loop {
            for _ in 0..1024 {
                client::send_copy_data(&mut r, &ps_feedback(last_fb))
                    .await
                    .unwrap();
            }
            let m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
            if let Some((streaming, _, _)) = interpreted_message(m)
                && streaming >= end
            {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), flood)
        .await
        .expect("the catch-up stream completes under a feedback flood");
    for _ in 0..500 {
        if svc
            .store()
            .load(&tl)
            .await
            .unwrap()
            .unwrap()
            .remote_consistent_lsn
            == last_fb
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("remote_consistent_lsn not recorded");
}

/// The pageserver's feedback reaches walproposer: an `AppendResponse` that
/// carries all three LSNs (write, flush, apply) for its backpressure.
#[tokio::test]
async fn pageserver_feedback_reaches_the_append_response() {
    let (pg, _svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let payload = vec![3u8; 1000];
    let mut p = propose(pg, &payload).await;
    let mut r = connect_with(pg, &format!("{INTERPRETED} {UNSHARDED}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let mut body = BytesMut::new();
    body.put_u8(b'z');
    body.put_u64(0);
    let fb = loams_safekeeper::types::PageserverFeedback {
        current_timeline_size: 1 << 20,
        last_received_lsn: Lsn(START + 900),
        disk_consistent_lsn: Lsn(START + 800),
        remote_consistent_lsn: Lsn(START + 700),
        replytime_us: 42,
        ..Default::default()
    };
    fb.serialize(&mut body);
    client::send_copy_data(&mut r, &body).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AcceptorMessage::AppendResponse(a) = recv(&mut p).await
                && let Some(f) = a.pageserver_feedback
            {
                return f;
            }
        }
    })
    .await
    .expect("an AppendResponse with the pageserver's feedback");
    assert_eq!(got.last_received_lsn, fb.last_received_lsn);
    assert_eq!(got.disk_consistent_lsn, fb.disk_consistent_lsn);
    assert_eq!(got.remote_consistent_lsn, fb.remote_consistent_lsn);
}

/// Interpreted keepalives ask for a reply, as the fork's do.
#[tokio::test]
async fn interpreted_keepalives_request_a_reply() {
    let (pg, _svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let _p = propose(pg, b"0123456789").await;
    let mut r = connect_with(pg, &format!("{INTERPRETED} {UNSHARDED}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let k = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
            if m[0] == b'k' {
                return m;
            }
        }
    })
    .await
    .expect("a keepalive");
    assert_eq!(k.len(), 18);
    assert_eq!(k[17], 1, "request_reply");
}

/// Shard options follow the fork's handler: all three for the interpreted
/// protocol, none for vanilla.
#[tokio::test]
async fn shard_options_are_checked_per_protocol() {
    let (pg, _svc) = start_interpreted(Arc::new(ByteShards::default())).await;
    let _p = propose(pg, b"0123456789").await;
    for opts in [
        INTERPRETED.to_string(),
        format!("{INTERPRETED} shard_count=2 shard_number=1"),
        UNSHARDED.to_string(),
    ] {
        let mut r = connect_with(pg, &opts).await;
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            client::query_rows(
                &mut r,
                &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("{opts}: refused, not streamed"));
        assert!(res.is_err(), "{opts}");
    }
}

/// The vanilla stream has the same shape: a catch-up under a feedback flood
/// completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vanilla_catch_up_with_a_feedback_flood_completes() {
    let (pg, _, svc) = start().await;
    let tl = loams_safekeeper::TimelineId::new(TENANT.parse().unwrap(), TIMELINE.parse().unwrap());
    let range = loams_safekeeper::propose::WalRange {
        start: Lsn(START),
        pg_version: 160_009,
        system_id: 99,
        wal: (0..32u32 << 20)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>()
            .into(),
    };
    let end = range.end();
    let _p = loams_safekeeper::propose::push_committed(&pg.to_string(), tl, &range)
        .await
        .unwrap();
    let mut r = connect(pg).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let last_fb = Lsn(START + 4096);
    let flood = async {
        loop {
            for _ in 0..1024 {
                client::send_copy_data(&mut r, &ps_feedback(last_fb))
                    .await
                    .unwrap();
            }
            let mut m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
            if m.get_u8() == b'w' {
                let start = m.get_u64();
                let _ = m.get_u64();
                let _ = m.get_i64();
                if start + m.len() as u64 >= end.0 {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), flood)
        .await
        .expect("the vanilla catch-up completes under a feedback flood");
    for _ in 0..500 {
        if svc
            .store()
            .load(&tl)
            .await
            .unwrap()
            .unwrap()
            .remote_consistent_lsn
            == last_fb
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("remote_consistent_lsn not recorded");
}

/// A WAL service with the stand-in interpreter over `store` (instances
/// sharing a store are a pool).
async fn serve_on(store: Arc<MemWalStore>) -> SocketAddr {
    let svc = WalService::new(
        store,
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            interpreter: Some(Interpreter(Arc::new(ByteShards::default()))),
            ..Default::default()
        },
    );
    let pg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = pg.local_addr().unwrap();
    tokio::spawn(svc.serve(pg, std::future::pending()));
    addr
}

/// Start an interpreted reader on `addr` with `shard` options.
async fn pageserver(addr: SocketAddr, shard: &str) -> TcpStream {
    let mut r = connect_with(addr, &format!("{INTERPRETED} {shard}")).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    r
}

/// A `'z'` body with the given shard number and LSNs.
fn shard_feedback(shard: u32, lsn: Lsn) -> BytesMut {
    let mut body = BytesMut::new();
    body.put_u8(b'z');
    body.put_u64(0);
    loams_safekeeper::types::PageserverFeedback {
        last_received_lsn: lsn,
        disk_consistent_lsn: lsn,
        remote_consistent_lsn: lsn,
        shard_number: shard,
        ..Default::default()
    }
    .serialize(&mut body);
    body
}

/// Send a heartbeat at `end` and collect the proposer's AppendResponses for
/// `wait`.
async fn responses(
    p: &mut TcpStream,
    end: u64,
    wait: Duration,
) -> Vec<loams_safekeeper::proto::AppendResponse> {
    send(p, append(1, end, b"", end)).await;
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(m) = tokio::time::timeout_at(deadline, recv(p)).await {
        if let AcceptorMessage::AppendResponse(r) = m {
            out.push(r);
        }
    }
    out
}

/// Feedback is per instance and per connection: a pageserver on another
/// instance of the pool sends this walproposer nothing, one on its own
/// instance does, and once that pageserver's connection closes the
/// walproposer gets no more feedback (nothing stale is replayed into later
/// responses, as the fork's safekeeper does).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feedback_is_per_instance_and_stops_with_its_connection() {
    let store = Arc::new(MemWalStore::new());
    let (pg1, pg2) = (serve_on(store.clone()).await, serve_on(store).await);
    let payload = vec![5u8; 1000];
    let mut p = propose(pg1, &payload).await;
    let end = START + payload.len() as u64;

    // A pageserver on the other instance.
    let mut r2 = pageserver(pg2, UNSHARDED).await;
    client::send_copy_data(&mut r2, &shard_feedback(0, Lsn(START + 100)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rs = responses(&mut p, end, Duration::from_millis(500)).await;
    assert!(!rs.is_empty());
    assert!(rs.iter().all(|r| r.pageserver_feedback.is_none()), "{rs:?}");

    // One on this instance.
    let mut r1 = pageserver(pg1, UNSHARDED).await;
    client::send_copy_data(&mut r1, &shard_feedback(0, Lsn(START + 200)))
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AcceptorMessage::AppendResponse(a) = recv(&mut p).await
                && let Some(f) = a.pageserver_feedback
            {
                return f;
            }
        }
    })
    .await
    .expect("feedback from this instance's pageserver");
    assert_eq!(got.remote_consistent_lsn, Lsn(START + 200));

    // It goes away: no feedback after that, in any response.
    drop(r1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rs = responses(&mut p, end, Duration::from_millis(500)).await;
    assert!(!rs.is_empty());
    assert!(rs.iter().all(|r| r.pageserver_feedback.is_none()), "{rs:?}");
    drop(r2);
}

/// Feedback from several shards that arrives together is not collapsed:
/// walproposer gets every shard's. Both arrive in one TCP segment, so the
/// server reads them back to back before the push loop can run (a "latest
/// value" relay keeps only the second).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feedback_from_every_shard_reaches_walproposer() {
    use tokio::io::AsyncWriteExt;
    let pg = serve_on(Arc::new(MemWalStore::new())).await;
    let payload = vec![6u8; 1000];
    let mut p = propose(pg, &payload).await;
    let mut r = pageserver(pg, "shard_count=2 shard_number=0 shard_stripe_size=2048").await;
    let mut both = BytesMut::new();
    loams_safekeeper::pgwire::put_copy_data(&mut both, &shard_feedback(0, Lsn(START + 10)));
    loams_safekeeper::pgwire::put_copy_data(&mut both, &shard_feedback(1, Lsn(START + 20)));
    r.write_all(&both).await.unwrap();
    let mut shards = std::collections::BTreeSet::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while shards.len() < 2 {
            if let AcceptorMessage::AppendResponse(a) = recv(&mut p).await
                && let Some(f) = a.pageserver_feedback
            {
                shards.insert(f.shard_number);
            }
        }
    })
    .await;
    assert_eq!(shards.into_iter().collect::<Vec<_>>(), vec![0, 1]);
}

/// Vanilla keepalives ask for a reply too (the fork's `send_wal.rs`).
#[tokio::test]
async fn vanilla_keepalives_request_a_reply() {
    let (pg, _, _svc) = start().await;
    let _p = propose(pg, b"0123456789").await;
    let mut r = connect(pg).await;
    client::query(
        &mut r,
        &format!("START_REPLICATION PHYSICAL {}", Lsn(START)),
    )
    .await
    .unwrap();
    client::expect_copy_both(&mut r).await.unwrap();
    let k = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = client::recv_copy_data(&mut r).await.unwrap().unwrap();
            if m[0] == b'k' {
                return m;
            }
        }
    })
    .await
    .expect("a keepalive");
    assert_eq!(k.len(), 18);
    assert_eq!(k[17], 1, "request_reply");
}
