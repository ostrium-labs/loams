//! Path 1 of a major upgrade (design §62 §9.3, §10; NF2 Task 13): the new
//! root timeline that `ImportPgdata` builds starts at the import's end LSN,
//! and `loams-wal` must take a timeline whose first LSN is that one, then a
//! walproposer of the target major that streams from it. The end LSN is the
//! imported checkpoint's, aligned (`importbucket_client.rs`, `base_lsn`):
//! not a segment or page boundary.
#![cfg(feature = "server")]
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use loams_safekeeper::pgwire::client;
use loams_safekeeper::proto::{
    AcceptorMessage, AppendRequest, AppendRequestHeader, ProposerElected, ProposerGreeting,
    ProposerMessage, VoteRequest,
};
use loams_safekeeper::service::{WalService, WalServiceConfig};
use loams_safekeeper::store::{MemWalStore, WalStore};
use loams_safekeeper::types::{Configuration, Id, Lsn, TermHistory, TermLsn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TENANT: &str = "5d0c3a2b1e9f48d7a6c5b4e3f2a1d0c9";
const TIMELINE: &str = "0e1f2a3b4c5d6e7f8091a2b3c4d5e6f7";

/// An import's end LSN: a checkpoint's, 8-byte aligned, inside a page.
const IMPORT_END: u64 = 0x0000_0002_3A5F_1C28;

async fn serve<S: WalStore + 'static>(store: Arc<S>) -> (SocketAddr, SocketAddr) {
    let svc = WalService::new(
        store,
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            ..Default::default()
        },
    );
    let pg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pg_addr = pg.local_addr().unwrap();
    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_addr = web.local_addr().unwrap();
    let app = loams_safekeeper::http::router(svc.clone());
    tokio::spawn(async move { axum::serve(web, app).await });
    tokio::spawn(svc.serve(pg, std::future::pending()));
    (pg_addr, web_addr)
}

async fn http(addr: SocketAddr, req: String) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

async fn create(web: SocketAddr, pg_version: u32, start: Lsn) -> String {
    let body = format!(
        r#"{{"tenant_id":"{TENANT}","timeline_id":"{TIMELINE}","pg_version":{pg_version},"start_lsn":"{start}","mconf":{{"generation":0,"members":[],"new_members":null}}}}"#
    );
    http(
        web,
        format!(
            "POST /v1/tenant/timeline HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

async fn status(web: SocketAddr) -> String {
    http(
        web,
        format!(
            "GET /v1/tenant/{TENANT}/timeline/{TIMELINE} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        ),
    )
    .await
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

/// A walproposer of `pg_version` (full) that greets a timeline it may not
/// create, wins term 1 and is elected at `start`.
async fn elected(pg: SocketAddr, pg_version: u32, start: u64) -> Result<TcpStream, String> {
    let mut p = TcpStream::connect(pg).await.unwrap();
    let options = format!("-c timeline_id={TIMELINE} tenant_id={TENANT}");
    client::startup(
        &mut p,
        &[
            ("user", "cloud_admin"),
            ("dbname", "replication"),
            ("options", &options),
        ],
    )
    .await
    .unwrap();
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
            pg_version,
            system_id: 7_301_234_567_890,
            wal_seg_size: 16 << 20,
        }),
    )
    .await;
    match recv(&mut p).await {
        AcceptorMessage::Greeting(g) => assert_eq!(g.term, 0),
        other => return Err(format!("{other:?}")),
    }
    send(
        &mut p,
        ProposerMessage::VoteRequest(VoteRequest {
            generation: 0,
            term: 1,
        }),
    )
    .await;
    match recv(&mut p).await {
        AcceptorMessage::VoteResponse(v) => {
            assert!(v.vote_given);
            // The acceptor's WAL ends where the import ends.
            assert_eq!(v.flush_lsn, Lsn(start.max(IMPORT_END)));
        }
        other => return Err(format!("{other:?}")),
    }
    send(
        &mut p,
        ProposerMessage::Elected(ProposerElected {
            generation: 0,
            term: 1,
            start_streaming_at: Lsn(start),
            term_history: TermHistory(vec![TermLsn {
                term: 1,
                lsn: Lsn(start),
            }]),
        }),
    )
    .await;
    Ok(p)
}

/// The case, on one store: create at the import's end, stream from it.
async fn timeline_starts_at_import_lsn<S: WalStore + 'static>(store: Arc<S>, major: u32) {
    let (pg, web) = serve(store).await;
    let at = Lsn(IMPORT_END);

    let body = create(web, major, at).await;
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(
        body.contains(&format!("\"timeline_start_lsn\":\"{at}\"")),
        "{body}"
    );
    assert!(body.contains(&format!("\"commit_lsn\":\"{at}\"")), "{body}");
    assert!(
        body.contains(&format!("\"pg_version\":{}", major * 10_000)),
        "{body}"
    );
    // A retried create (the saga resumed after a crash) is the same timeline.
    let again = create(web, major, at).await;
    assert!(
        again.contains(&format!("\"timeline_start_lsn\":\"{at}\"")),
        "{again}"
    );

    // The target major's compute streams from the import's end.
    let mut p = elected(pg, major * 10_000 + 1, IMPORT_END).await.unwrap();
    let payload: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
    let mut lsn = IMPORT_END;
    for chunk in payload.chunks(64) {
        send(&mut p, append(1, lsn, chunk, 0)).await;
        lsn += chunk.len() as u64;
    }
    let end = lsn;
    loop {
        match recv(&mut p).await {
            AcceptorMessage::AppendResponse(r) if r.flush_lsn == Lsn(end) => break,
            AcceptorMessage::AppendResponse(r) => assert!(r.flush_lsn < Lsn(end)),
            other => panic!("{other:?}"),
        }
    }
    send(&mut p, append(1, end, b"", end)).await;
    loop {
        match recv(&mut p).await {
            AcceptorMessage::AppendResponse(r) if r.commit_lsn == Lsn(end) => break,
            AcceptorMessage::AppendResponse(r) => assert_eq!(r.flush_lsn, Lsn(end)),
            other => panic!("{other:?}"),
        }
    }
    drop(p);

    let head = status(web).await;
    assert!(
        head.contains(&format!("\"timeline_start_lsn\":\"{at}\"")),
        "{head}"
    );
    assert!(
        head.contains(&format!("\"flush_lsn\":\"{}\"", Lsn(end))),
        "{head}"
    );
}

#[tokio::test]
async fn loams_wal_timeline_starts_at_import_lsn() {
    // 17 is the source major of today's upgrades, 18 the target (§62 §10);
    // `loams-wal` keeps the full version and nothing else per major.
    for major in [17, 18] {
        timeline_starts_at_import_lsn(Arc::new(MemWalStore::new()), major).await;
    }
}

#[cfg(feature = "nvme")]
#[tokio::test]
async fn loams_wal_timeline_starts_at_import_lsn_nvme() {
    use loams_safekeeper::journal::{JournalConfig, Tier};
    use loams_safekeeper::meta::LocalMeta;
    use loams_safekeeper::nvme::NvmeWalStore;

    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let cfg = JournalConfig {
        segment_size: 1 << 20,
        unit_capacity: 256 << 10,
        ..JournalConfig::new(dir.path().join("journal"), Tier::Buffered)
    };
    let meta = Arc::new(LocalMeta::open(&dir.path().join("meta")).unwrap());
    let store = Arc::new(NvmeWalStore::open(cfg, meta).await.unwrap());
    timeline_starts_at_import_lsn(store, 18).await;
}

/// A walproposer elected below the import's end would rewrite history the
/// pageserver already holds as the imported image: refused.
#[tokio::test]
async fn elected_below_import_lsn_is_refused() {
    let (pg, web) = serve(Arc::new(MemWalStore::new())).await;
    let body = create(web, 18, Lsn(IMPORT_END)).await;
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    let below = IMPORT_END - 0x1000;
    let refused = match elected(pg, 180_001, below).await {
        Err(_) => true,
        Ok(mut p) => {
            // The refusal arrives as the connection's end, not an answer.
            send(&mut p, append(1, below, b"x", 0)).await;
            client::recv_copy_data(&mut p).await.is_err()
                || matches!(client::recv_copy_data(&mut p).await, Ok(None))
        }
    };
    assert!(
        refused,
        "an election below the import's end must be refused"
    );
    let head = status(web).await;
    assert!(
        head.contains(&format!("\"commit_lsn\":\"{}\"", Lsn(IMPORT_END))),
        "{head}"
    );
}
