//! The ClickHouse native protocol (HE0, rulings R9.40–R9.46) against a House
//! with sealed workers and lake tables, driven by a client built on
//! `loams-house-native`'s own encoders. The bytes checked against the pinned
//! clickhouse-server 26.9.2.8 are the oracle's, captured from clickhouse-go.

#![cfg(target_os = "linux")]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::http::{request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_native::packet::{self, ClientInfo, Query, ServerPacket, Setting};
use loams_house_native::{Block, Column, Framing, Hello, wire};
use loams_store::Store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct House {
    handle: HouseHandle,
    _pool: WorkerPool,
    _proxy: HouseCache,
}

impl House {
    fn native(&self) -> SocketAddr {
        self.handle.native_addr().expect("a native listener")
    }
}

/// A House with lake tables in a `file://` bucket; `alice` (namespace 2) writes.
async fn house(test: &str) -> House {
    let dir = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", dir.display());
    let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
    let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
    let upstream = Upstream::open(&url, StoreOptions::default(), RangeCacheConfig::default())
        .await
        .expect("upstream");
    let proxy = HouseCache::new(Arc::new(OneStore(Arc::new(upstream))));
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test))
        .with_forward_target(Arc::new(proxy.clone()));
    let pool = WorkerPool::start(small(2), Arc::new(launcher))
        .await
        .expect("the pool starts");
    let lake = Arc::new(
        Lake::new(catalog as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store)
            .with_grants(Arc::new(proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        native_listen: Some("127.0.0.1:0".parse().expect("addr")),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        compaction_interval: Duration::ZERO,
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool.clone(), lake)
        .await
        .expect("serves");
    House {
        handle,
        _pool: pool,
        _proxy: proxy,
    }
}

/// A native client at one protocol revision.
struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
    revision: u64,
    /// The running query's blocks are compressed.
    compressed: bool,
}

impl Client {
    /// Connects and says `Hello`; the server's answer is the first packet.
    async fn hello(
        addr: SocketAddr,
        user: &str,
        password: &str,
        revision: u64,
    ) -> (Self, ServerPacket) {
        let stream = TcpStream::connect(addr).await.expect("connects");
        let mut client = Self {
            stream,
            buf: Vec::new(),
            revision,
            compressed: false,
        };
        let mut out = Vec::new();
        packet::encode_client_hello(
            &mut out,
            &Hello {
                client_name: "loams-native-test".into(),
                version_major: 1,
                version_minor: 0,
                revision,
                database: "default".into(),
                user: user.into(),
                password: password.into(),
            },
        );
        client.send(&out).await;
        let answer = client.packet().await;
        if matches!(answer, ServerPacket::Hello { .. }) && revision >= 54458 {
            let mut addendum = Vec::new();
            wire::put_str(&mut addendum, "");
            client.send(&addendum).await;
        }
        (client, answer)
    }

    async fn connect(addr: SocketAddr, revision: u64) -> Self {
        let (client, hello) = Self::hello(addr, "alice", "a", revision).await;
        assert!(matches!(hello, ServerPacket::Hello { .. }), "{hello:?}");
        client
    }

    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("writes");
    }

    async fn packet(&mut self) -> ServerPacket {
        loop {
            match packet::decode_server_packet_with(&self.buf, self.revision, self.compressed) {
                Ok((packet, used)) => {
                    self.buf.drain(..used);
                    return packet;
                }
                Err(loams_house_native::Error::Incomplete) => {}
                Err(err) => panic!("the server's bytes do not decode: {err}"),
            }
            let n =
                tokio::time::timeout(Duration::from_secs(60), self.stream.read_buf(&mut self.buf))
                    .await
                    .expect("the server answers within a minute")
                    .expect("reads");
            assert!(n > 0, "the server closed the connection");
        }
    }

    /// Sends a `Query` (and the empty block that ends its external tables).
    async fn send_query(&mut self, sql: &str, settings: &[(&str, &str)], compression: bool) {
        let mut out = Vec::new();
        packet::encode_query(
            &mut out,
            &Query {
                id: String::new(),
                client_info: ClientInfo::default(),
                settings: settings
                    .iter()
                    .map(|(name, value)| Setting {
                        name: (*name).into(),
                        flags: 0,
                        value: (*value).into(),
                    })
                    .collect(),
                stage: 2,
                compression,
                body: sql.into(),
                parameters: Vec::new(),
            },
            self.revision,
        );
        self.send(&out).await;
    }

    /// The packets up to `EndOfStream` or an `Exception`.
    async fn answer(&mut self) -> Vec<ServerPacket> {
        let mut out = Vec::new();
        loop {
            let packet = self.packet().await;
            let last = matches!(
                packet,
                ServerPacket::EndOfStream | ServerPacket::Exception { .. }
            );
            out.push(packet);
            if last {
                return out;
            }
        }
    }

    async fn query(&mut self, sql: &str) -> Vec<ServerPacket> {
        self.send_query(sql, &[], false).await;
        self.answer().await
    }
}

/// The data blocks of an answer.
fn blocks(answer: &[ServerPacket]) -> Vec<&Block> {
    answer
        .iter()
        .filter_map(|p| match p {
            ServerPacket::Data { block, .. } => Some(block),
            _ => None,
        })
        .collect()
}

fn exception(answer: &[ServerPacket]) -> Option<(i32, &str)> {
    match answer.last() {
        Some(ServerPacket::Exception { code, message }) => Some((*code, message.as_str())),
        _ => None,
    }
}

fn encoded(block: &Block, revision: u64) -> Vec<u8> {
    let mut out = Vec::new();
    block.write(&mut out, Framing::protocol(revision));
    out
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_session_runs_on_the_shared_path() {
    let house = house("native-session").await;
    let addr = house.native();

    // A wrong password is ClickHouse's 516 at once.
    let (_, refused) = Client::hello(addr, "alice", "wrong", 54460).await;
    assert!(
        matches!(refused, ServerPacket::Exception { code: 516, .. }),
        "{refused:?}"
    );

    let mut client = Client::connect(addr, 54460).await;
    client.send(&[4]).await;
    assert_eq!(client.packet().await, ServerPacket::Pong);

    // SELECT 1: the oracle's header and row, byte for byte, then its trailer.
    let answer = client.query("SELECT 1").await;
    let data = blocks(&answer);
    assert_eq!(
        encoded(data[0], 54460),
        hex("010002ffffffff00010001310555496e743800")
    );
    assert_eq!(
        encoded(data[1], 54460),
        hex("010002ffffffff00010101310555496e74380001")
    );
    assert!(data[2].is_empty(), "the end-of-data block");
    assert!(
        matches!(answer[2], ServerPacket::ProfileInfo(_)),
        "{answer:?}"
    );
    assert_eq!(answer.last(), Some(&ServerPacket::EndOfStream));

    // DDL, then an INSERT whose data the client sends as a block.
    let answer = client
        .query("CREATE TABLE t (id UInt64, s String, at DateTime('UTC')) ENGINE = MergeTree ORDER BY id")
        .await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    client
        .send_query("INSERT INTO t (id, s) FORMAT Native", &[], false)
        .await;
    // The table's columns first (R9.68), as clickhouse-server 26.9 sends them.
    assert_eq!(
        client.packet().await,
        ServerPacket::TableColumns(
            "columns format version: 1\n3 columns:\n`id` UInt64\n`s` String\n`at` DateTime('UTC')\n"
                .into()
        )
    );
    let ServerPacket::Data { block: header, .. } = client.packet().await else {
        panic!("the INSERT's header");
    };
    assert_eq!(
        header
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str()))
            .collect::<Vec<_>>(),
        [("id", "UInt64"), ("s", "String")]
    );
    let rows = Block {
        rows: 2,
        columns: vec![
            Column {
                name: "id".into(),
                type_name: "UInt64".into(),
                data: [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat(),
            },
            Column {
                name: "s".into(),
                type_name: "String".into(),
                data: vec![1, b'a', 2, b'b', b'c'],
            },
        ],
        ..Block::default()
    };
    let mut out = Vec::new();
    packet::encode_client_data(&mut out, &rows, 54460);
    packet::encode_client_data(&mut out, &Block::empty(), 54460);
    client.send(&out).await;
    let answer = client.answer().await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    assert!(
        answer
            .iter()
            .any(|p| matches!(p, ServerPacket::Progress(p) if p.written_rows == 2)),
        "{answer:?}"
    );

    // The rows come back as they went in.
    let answer = client.query("SELECT id, s FROM t ORDER BY id").await;
    let data = blocks(&answer);
    assert_eq!(data[1].columns, rows.columns, "{answer:?}");

    // No rows still has its columns (R9.43).
    let answer = client.query("SELECT id, s FROM t WHERE id > 100").await;
    let data = blocks(&answer);
    assert_eq!(data[0].rows, 0);
    assert_eq!(
        data[0]
            .columns
            .iter()
            .map(|c| c.type_name.as_str())
            .collect::<Vec<_>>(),
        ["UInt64", "String"]
    );

    // Errors are the statement's, and the connection serves on.
    let answer = client.query("SELECT * FROM no_such_table").await;
    assert_eq!(exception(&answer).map(|e| e.0), Some(60), "{answer:?}");
    client.send(&[4]).await;
    assert_eq!(client.packet().await, ServerPacket::Pong);

    // `SET` holds for the connection; `USE` of an unknown database is 81.
    let answer = client.query("SET max_threads = 1").await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    let answer = client.query("SELECT getSetting('max_threads')").await;
    assert!(exception(&answer).is_none(), "{answer:?}");
    let answer = client.query("USE no_such_database").await;
    assert_eq!(exception(&answer).map(|e| e.0), Some(81), "{answer:?}");

    // A setting is refused exactly as over HTTP: one allowlist.
    let http = house.handle.local_addr();
    for (name, value) in [("allow_ddl", "1"), ("no_such_setting_at_all", "1")] {
        let over_http = request(
            http,
            "GET",
            &target(&[("query", "SELECT 1"), (name, value)]),
            &[("X-ClickHouse-User", "alice"), ("X-ClickHouse-Key", "a")],
            b"",
        );
        let want: Option<i32> = over_http
            .header("X-ClickHouse-Exception-Code")
            .and_then(|c| c.parse().ok());
        client.send_query("SELECT 1", &[(name, value)], false).await;
        let answer = client.answer().await;
        assert_eq!(exception(&answer).map(|e| e.0), want, "{name}: {answer:?}");
    }

    // Compression (R9.48): a query's blocks both ways in LZ4 frames.
    client.compressed = true;
    client
        .send_query("INSERT INTO t (id, s) FORMAT Native", &[], true)
        .await;
    let ServerPacket::TableColumns(_) = client.packet().await else {
        panic!("the compressed INSERT's table columns");
    };
    let ServerPacket::Data { .. } = client.packet().await else {
        panic!("the compressed INSERT's header");
    };
    let mut more = rows.clone();
    more.columns[0].data = [3u64.to_le_bytes(), 4u64.to_le_bytes()].concat();
    let mut out = Vec::new();
    packet::encode_client_data_with(&mut out, &more, 54460, true);
    packet::encode_client_data_with(&mut out, &Block::empty(), 54460, true);
    client.send(&out).await;
    let answer = client.answer().await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    client
        .send_query("SELECT id, s FROM t WHERE id > 2 ORDER BY id", &[], true)
        .await;
    let answer = client.answer().await;
    client.compressed = false;
    assert_eq!(blocks(&answer)[1].columns, more.columns, "{answer:?}");
    let answer = client.query("SELECT count() FROM t").await;
    assert_eq!(blocks(&answer)[1].columns[0].data, 4u64.to_le_bytes());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn golang_migrate_at_revision_54213() {
    let house = house("native-v1").await;
    let addr = house.native();
    // clickhouse-go v1, golang-migrate's driver (R9.41).
    let mut client = Client::connect(addr, 54213).await;
    // Its first look for the migrations table finds nothing, with a header.
    let answer = client
        .query("SHOW TABLES FROM \"default\" LIKE 'schema_migrations'")
        .await;
    let data = blocks(&answer);
    assert_eq!(data[0].rows, 0);
    assert_eq!(data[0].columns[0].name, "name");
    assert_eq!(
        encoded(data[0], 54213),
        hex("010002ffffffff000100046e616d6506537472696e67")
    );
    let answer = client
        .query("CREATE TABLE schema_migrations (version Int64, dirty UInt8, sequence UInt64) Engine=MergeTree ORDER BY sequence")
        .await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    // `INSERT … VALUES ` with the row as a block, as v1 sends it.
    client
        .send_query(
            "INSERT INTO schema_migrations (version, dirty, sequence) VALUES ",
            &[],
            false,
        )
        .await;
    let ServerPacket::Data { block: header, .. } = client.packet().await else {
        panic!("the INSERT's header");
    };
    assert_eq!(header.columns.len(), 3);
    let row = Block {
        rows: 1,
        columns: vec![
            Column {
                name: "version".into(),
                type_name: "Int64".into(),
                data: 1i64.to_le_bytes().to_vec(),
            },
            Column {
                name: "dirty".into(),
                type_name: "UInt8".into(),
                data: vec![0],
            },
            Column {
                name: "sequence".into(),
                type_name: "UInt64".into(),
                data: 7u64.to_le_bytes().to_vec(),
            },
        ],
        ..Block::default()
    };
    let mut out = Vec::new();
    packet::encode_client_data(&mut out, &row, 54213);
    packet::encode_client_data(&mut out, &Block::empty(), 54213);
    client.send(&out).await;
    let answer = client.answer().await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    let answer = client
        .query("SELECT version, dirty FROM `schema_migrations` ORDER BY sequence DESC LIMIT 1")
        .await;
    let data = blocks(&answer);
    assert_eq!(data[1].rows, 1, "{answer:?}");
    assert_eq!(data[1].columns[0].data, 1i64.to_le_bytes());
}

/// HE1 R9.69: a `Cancel` while the statement still runs, before its first
/// block, stops it and answers `EndOfStream` with no `Data`; the connection
/// serves on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_before_the_first_result() {
    let house = house("native-cancel-early").await;
    let mut client = Client::connect(house.native(), 54460).await;
    let started = std::time::Instant::now();
    client
        .send_query(
            "SELECT number FROM numbers(3) WHERE sleepEachRow(1) = 0",
            &[],
            false,
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut cancel = Vec::new();
    wire::put_varuint(&mut cancel, packet::client::CANCEL);
    client.send(&cancel).await;
    let answer = client.answer().await;
    assert_eq!(
        answer.last(),
        Some(&ServerPacket::EndOfStream),
        "{answer:?}"
    );
    assert!(blocks(&answer).is_empty(), "{answer:?}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "cancelled after {:?}",
        started.elapsed()
    );
    let answer = client.query("SELECT 1").await;
    assert_eq!(blocks(&answer)[1].rows, 1, "{answer:?}");
}

/// HE1 R9.66: a top-level `DateTime` column keeps its time zone in a result's
/// blocks, as clickhouse-server names it to a client at 54337 or later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn datetime_columns_keep_their_time_zone() {
    let house = house("native-zones").await;
    let mut client = Client::connect(house.native(), 54460).await;
    let answer = client
        .query("SELECT toDateTime(0, 'Asia/Kolkata') AS d, toDateTime(0) AS u, [toDateTime(0, 'Asia/Kolkata')] AS a")
        .await;
    let data = blocks(&answer);
    for block in [data[0], data[1]] {
        assert_eq!(
            block
                .columns
                .iter()
                .map(|c| c.type_name.as_str())
                .collect::<Vec<_>>(),
            [
                "DateTime('Asia/Kolkata')",
                "DateTime",
                "Array(DateTime('Asia/Kolkata'))"
            ],
            "{answer:?}"
        );
    }
    assert_eq!(data[1].columns[0].data, 0u32.to_le_bytes());
    // With the client's own output format, as clickhouse-client sends it.
    let answer = client
        .query("SELECT toDateTime(0, 'Asia/Kolkata') AS d FORMAT TSVWithNamesAndTypes")
        .await;
    assert_eq!(
        blocks(&answer)[0].columns[0].type_name,
        "DateTime('Asia/Kolkata')",
        "{answer:?}"
    );
}
