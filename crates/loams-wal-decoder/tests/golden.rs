//! PG2 Task 31's golden tests: `loams-wal`'s interpreted stream for a
//! recorded Postgres 17 WAL range is byte-identical to what the fork's own
//! safekeeper sent for it, checked body by body against the digests of the
//! fork's bodies (`tests/fixtures/README.md` says how they were recorded),
//! and the pageserver's decoder reads it to the end.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use loams_safekeeper::propose::{PAGESERVER_PROTOCOL, WalRange, push_committed, read_interpreted};
use loams_safekeeper::send::{
    ClientProtocol, Compression, InterpretedProtocol, Interpreter, ShardSpec, WireFormat,
};
use loams_safekeeper::service::{WalService, WalServiceConfig};
use loams_safekeeper::store::MemWalStore;
use loams_safekeeper::types::{Lsn, TimelineId};
use loams_wal_decoder::{Decoder, NeonInterpreter};
use sha2::{Digest as _, Sha256};
use utils::postgres_client::{InterpretedFormat, PostgresClientProtocol};
use wal_decoder::models::InterpretedWalRecords;
use wal_decoder::wire_format::FromWireFormat;

const PROTOBUF_ZSTD1: InterpretedProtocol = InterpretedProtocol {
    format: WireFormat::Protobuf,
    compression: Some(Compression::Zstd { level: 1 }),
};

fn fixture(name: &str) -> Bytes {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    Bytes::from(std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
}

fn range() -> WalRange {
    let raw = zstd::decode_all(&fixture("wal-17.bin.zst")[..]).unwrap();
    WalRange::parse(Bytes::from(raw)).unwrap()
}

/// One body the fork's sender sent: `streaming_lsn`, `commit_lsn`, length
/// and sha256.
#[derive(Debug, PartialEq, Eq)]
struct BodyDigest {
    streaming: u64,
    commit: u64,
    len: usize,
    sha256: String,
}

impl BodyDigest {
    fn of(body: &[u8]) -> Self {
        Self {
            streaming: u64::from_be_bytes(body[1..9].try_into().unwrap()),
            commit: u64::from_be_bytes(body[9..17].try_into().unwrap()),
            len: body.len(),
            sha256: Sha256::digest(body)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }
    }
}

fn fork_digests(name: &str) -> Vec<BodyDigest> {
    let text = String::from_utf8(fixture(name).to_vec()).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            BodyDigest {
                streaming: u64::from_str_radix(f[0], 16).unwrap(),
                commit: u64::from_str_radix(f[1], 16).unwrap(),
                len: f[2].parse().unwrap(),
                sha256: f[3].to_string(),
            }
        })
        .collect()
}

/// The fixtures: name, shard options as the pageserver sends them, shard.
const CASES: [(&str, &str, ShardSpec); 2] = [
    (
        "fork-sender-17-unsharded.sha256",
        "shard_count=0 shard_number=0 shard_stripe_size=2048",
        ShardSpec {
            number: 0,
            count: 0,
            stripe_size: 2048,
        },
    ),
    (
        "fork-sender-17-shard1of2.sha256",
        "shard_count=2 shard_number=1 shard_stripe_size=2048",
        ShardSpec {
            number: 1,
            count: 2,
            stripe_size: 2048,
        },
    ),
];

/// The decoder alone, fed the WAL in the fork's chunks (each fork batch
/// ends at its `streaming_lsn`), encodes the fork's batches byte for byte.
#[tokio::test]
async fn decoder_matches_fork_batches() {
    let range = range();
    for (name, _, shard) in CASES {
        let want = fork_digests(name);
        assert!(!want.is_empty(), "{name}");
        let mut dec = Decoder::new(range.start.0, range.pg_version, PROTOBUF_ZSTD1, shard).unwrap();
        let mut at = range.start.0;
        for (i, w) in want.iter().enumerate() {
            let to = w.streaming;
            let wal = &range.wal[(at - range.start.0) as usize..(to - range.start.0) as usize];
            let batch = dec.decode(at, wal).await.unwrap().expect("a batch");
            let mut body = vec![b'0'];
            body.extend_from_slice(&to.to_be_bytes());
            body.extend_from_slice(&w.commit.to_be_bytes());
            body.extend_from_slice(&batch);
            assert_eq!(
                &BodyDigest::of(&body),
                w,
                "{name}: batch {i} ending at {to:#x}"
            );
            at = to;
        }
    }
}

async fn serve() -> (String, Arc<WalService<MemWalStore>>) {
    let svc = WalService::new(
        Arc::new(MemWalStore::new()),
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            interpreter: Some(Interpreter(Arc::new(NeonInterpreter))),
            ..Default::default()
        },
    );
    let pg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = pg.local_addr().unwrap().to_string();
    tokio::spawn(svc.clone().serve(pg, std::future::pending()));
    (addr, svc)
}

fn timeline(n: u8) -> TimelineId {
    let id = format!("{n:02x}").repeat(16);
    TimelineId::new(id.parse().unwrap(), id.parse().unwrap())
}

/// `loams-wal` with the decoder streams exactly what the fork's safekeeper
/// streamed for the same range and request: framing, LSNs and batches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interpreted_stream_matches_fork_sender() {
    let range = range();
    let (addr, _svc) = serve().await;
    for (i, (name, opts, _)) in CASES.into_iter().enumerate() {
        let tl = timeline(i as u8 + 1);
        let _proposer = push_committed(&addr, tl, &range).await.unwrap();
        let got = read_interpreted(&addr, tl, opts, range.start, range.end())
            .await
            .unwrap();
        let want = fork_digests(name);
        assert_eq!(got.len(), want.len(), "{name}: batch count");
        for (n, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(&BodyDigest::of(g), w, "{name}: batch {n}");
        }
    }
}

/// The pageserver's own decoder (`FromWireFormat`) reads every batch, and
/// its `next_record_lsn` (the pageserver's `last_record_lsn`) reaches the
/// commit. Shard 1 of 2 gets a strict subset of the unsharded records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pageserver_decodes_the_stream_to_the_commit() {
    let range = range();
    let (addr, _svc) = serve().await;
    let mut counts = Vec::new();
    for (i, (name, opts, _)) in CASES.into_iter().enumerate() {
        let tl = timeline(i as u8 + 10);
        let _proposer = push_committed(&addr, tl, &range).await.unwrap();
        let bodies = read_interpreted(&addr, tl, opts, range.start, range.end())
            .await
            .unwrap();
        let mut last = Lsn(0);
        let mut records = 0;
        for body in &bodies {
            let batch = InterpretedWalRecords::from_wire(
                &body.slice(17..),
                InterpretedFormat::Protobuf,
                Some(utils::postgres_client::Compression::Zstd { level: 1 }),
            )
            .await
            .unwrap();
            assert!(batch.next_record_lsn.0 >= last.0, "{name}");
            last = Lsn(batch.next_record_lsn.0);
            records += batch.records.len();
        }
        assert_eq!(
            last,
            range.end(),
            "{name}: last_record_lsn reaches the commit"
        );
        counts.push(records);
    }
    assert!(counts[1] > 0 && counts[1] < counts[0], "{counts:?}");
}

/// `loams-wal` parses the `protocol` option exactly as Neon serializes it.
#[test]
fn protocol_json_matches_the_fork() {
    let fork = serde_json::to_string(&PostgresClientProtocol::Interpreted {
        format: InterpretedFormat::Protobuf,
        compression: Some(utils::postgres_client::Compression::Zstd { level: 1 }),
    })
    .unwrap();
    assert_eq!(fork, PAGESERVER_PROTOCOL);
    assert_eq!(
        ClientProtocol::from_option(Some(&fork)).unwrap(),
        ClientProtocol::Interpreted(PROTOBUF_ZSTD1)
    );
    let vanilla = serde_json::to_string(&PostgresClientProtocol::Vanilla).unwrap();
    assert_eq!(
        ClientProtocol::from_option(Some(&vanilla)).unwrap(),
        ClientProtocol::Vanilla
    );
}
