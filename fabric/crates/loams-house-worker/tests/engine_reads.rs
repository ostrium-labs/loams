//! `chdb_written_tables_read_identically` (plan HE1 Task 26, R26.9): tables
//! the chDB House writes (DDL, `INSERT`, `ALTER … ADD COLUMN`, an `ALIAS`, a
//! delete `TTL`) answer the same through the Rust engine's lake reader
//! (`loams-house-iceberg`) as through chDB's read views (`views.rs`).
//!
//! It needs libchdb, so it lives here, in the House job, rather than with the
//! engine's crates; the engine crates are dev-dependencies of this package
//! only, so `engine_crates_stay_apart` still holds for every binary.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::http::{Response, request, target};
use common::{small, tmp_root};
use datafusion::object_store::local::LocalFileSystem;
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_engine::Engine;
use loams_house_iceberg::TablePin;
use loams_house_lake::TableIdent;
use loams_store::Store;

const ALICE: (&str, &str) = ("alice", "a");

struct Lakehouse {
    handle: HouseHandle,
    catalog: Arc<MemCatalog>,
    bucket: PathBuf,
    _pool: WorkerPool,
    _proxy: HouseCache,
}

/// A House whose catalog, `house-cache` and front share one `file://` bucket;
/// `alice` writes in namespace 2. Compaction is off, so the files are the
/// inserts' own.
async fn lakehouse(test: &str) -> Lakehouse {
    let bucket = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", bucket.display());
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
        Lake::new(catalog.clone() as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store)
            .with_grants(Arc::new(proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        compaction_interval: Duration::ZERO,
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool.clone(), lake)
        .await
        .expect("serves");
    Lakehouse {
        handle,
        catalog,
        bucket,
        _pool: pool,
        _proxy: proxy,
    }
}

async fn post(addr: SocketAddr, sql: &str) -> Response {
    let target = target(&[("user", ALICE.0), ("password", ALICE.1)]);
    let body = sql.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

async fn ok(addr: SocketAddr, sql: &str) -> String {
    let response = post(addr, sql).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
}

/// The store key of a House location (`s3://local/ns/2/…`): the path under
/// the bucket.
fn key(location: &str) -> &str {
    location
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map_or(location, |(_, key)| key)
}

/// The engine, with `default.<table>` read from the House's lake through the
/// table's current pin.
async fn engine_over(house: &Lakehouse, tables: &[&str]) -> Engine {
    let engine = Engine::new();
    let store = Arc::new(LocalFileSystem::new_with_prefix(&house.bucket).unwrap());
    for table in tables {
        let loaded = house
            .catalog
            .load_table(&TableIdent::new(
                Lake::namespace(2, "default"),
                (*table).to_string(),
            ))
            .await
            .unwrap_or_else(|e| panic!("{table}: {e}"));
        let bytes = std::fs::read(house.bucket.join(key(&loaded.metadata_location))).unwrap();
        let pin = TablePin::from_metadata_json("default", table, &loaded.metadata_location, &bytes)
            .unwrap();
        assert!(!pin.external, "{table} carries the House's properties");
        let provider = loams_house_iceberg::provider(&pin, store.clone(), engine.registry())
            .await
            .unwrap_or_else(|e| panic!("{table}: {e}"));
        engine.catalog.insert_lake("default", table, provider);
    }
    engine
}

/// The engine's answer as `JSONCompactEachRowWithNamesAndTypes`.
async fn engine_answer(engine: &Engine, sql: &str) -> String {
    let answer = engine
        .query(sql, &[], &[], "default")
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    let columns = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(
        &columns,
        &answer.batches,
        &Default::default(),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

/// Line by line, so a failure names the first row that differs.
fn same(sql: &str, chdb: &str, engine: &str) {
    let chdb: Vec<&str> = chdb.lines().collect();
    let engine: Vec<&str> = engine.lines().collect();
    for (n, (c, e)) in chdb.iter().zip(&engine).enumerate() {
        assert_eq!(e, c, "{sql}: line {n} differs (engine, then chDB)");
    }
    assert_eq!(engine.len(), chdb.len(), "{sql}: line counts");
}

#[tokio::test(flavor = "multi_thread")]
async fn chdb_written_tables_read_identically() {
    let house = lakehouse("engine-reads").await;
    let addr = house.handle.local_addr();
    ok(
        addr,
        "CREATE TABLE t (
            id UInt32, u64 UInt64, i8 Int8, f64 Float64, dec Decimal(18, 4),
            s String, fs FixedString(3), d Date, dt DateTime('UTC'),
            dt64 DateTime64(3, 'UTC'), dt64n DateTime64(9, 'UTC'), uuid UUID,
            ip4 IPv4, ip6 IPv6, e Enum8('a' = 1, 'b' = 2), ns Nullable(String),
            lc LowCardinality(String), arr Array(String), m Map(String, UInt64),
            tup Tuple(a UInt8, b String), plus UInt64 ALIAS id + 1
        ) ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "INSERT INTO t (id, u64, i8, f64, dec, s, fs, d, dt, dt64, dt64n, uuid, ip4, ip6, e, ns, lc, arr, m, tup) VALUES
            (1, 18446744073709551615, -128, 1e21, 1.5, 'a\\tb', 'ab', '2026-10-23', '2026-10-23 01:02:03',
             '2026-10-23 01:02:03.5', '2026-10-23 01:02:03.123456789', '61f0c404-5cb3-11e7-907b-a6006ad3dba0',
             '1.2.3.4', '2001:db8::1', 'a', NULL, 'x', ['a', 'b'], {'k': 1}, (1, 'x')),
            (2, 9223372036854775808, 127, -0.0, -0.0001, 'it''s', 'xyz', '1970-01-01', '1970-01-01 00:00:00',
             '1969-12-31 23:59:59.5', '2262-04-11 23:47:16', '00000000-0000-0000-0000-000000000000',
             '0.0.0.0', '::ffff:1.2.3.4', 'b', '', '', [], {}, (0, ''))",
    )
    .await;
    // A column added later reads its DEFAULT for the rows written before it.
    ok(addr, "ALTER TABLE t ADD COLUMN n UInt32 DEFAULT id * 10").await;
    ok(
        addr,
        "INSERT INTO t (id, u64, i8, f64, dec, s, fs, d, dt, dt64, dt64n, uuid, ip4, ip6, e, ns, lc, arr, m, tup, n) VALUES
            (3, 7, 7, 7.25, 7, 'seven', 'sev', '2000-02-29', '2000-02-29 12:00:00',
             '2000-02-29 12:00:00.007', '2000-02-29 12:00:00.000000007', 'ffffffff-ffff-ffff-ffff-ffffffffffff',
             '255.255.255.255', '::', 'a', 'n', 'lc', ['c'], {'a': 1, 'b': 2}, (7, 's'), 7)",
    )
    .await;
    // A delete TTL: the expired rows are hidden by both readers.
    ok(
        addr,
        "CREATE TABLE ttl (id UInt32, ts DateTime('UTC')) ENGINE = MergeTree ORDER BY id TTL ts",
    )
    .await;
    ok(
        addr,
        "INSERT INTO ttl VALUES (1, '2001-09-09 01:46:40'), (2, '2096-10-02 07:06:40'), \
         (3, '2020-01-01 00:00:00'), (4, '2100-01-01 00:00:00')",
    )
    .await;

    let engine = engine_over(&house, &["t", "ttl"]).await;
    let columns = "id, u64, i8, f64, dec, s, fs, d, dt, dt64, dt64n, uuid, ip4, ip6, e, ns, lc, \
                   arr, m, tup, plus, n";
    for sql in [
        format!("SELECT {columns} FROM t ORDER BY id"),
        "SELECT id, n, plus FROM t WHERE u64 > 9223372036854775807 ORDER BY id".to_string(),
        "SELECT id FROM t WHERE e = 'a' AND ip4 != toIPv4('0.0.0.0') ORDER BY id".to_string(),
        "SELECT id, ts FROM ttl ORDER BY id".to_string(),
    ] {
        let chdb = ok(
            addr,
            &format!("{sql} FORMAT JSONCompactEachRowWithNamesAndTypes"),
        )
        .await;
        let ours = engine_answer(&engine, &sql).await;
        same(&sql, &chdb, &ours);
    }
}
