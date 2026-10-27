//! Mutations of lake tables (HE1 Task 9, ruling R9.5): `ALTER TABLE … DELETE`,
//! `UPDATE`, `CLEAR COLUMN`, `DROP PARTITION`, and the lightweight `DELETE
//! FROM` and `UPDATE … SET`, rewritten copy-on-write and committed as one
//! `replace` snapshot. The expected rows were measured on the 26.9.2.8 oracle
//! (`clickhouse-server` with `mutations_sync = 2`).

#![cfg(target_os = "linux")]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::http::{Response, request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_store::Store;

struct Lakehouse {
    handle: HouseHandle,
    _pool: WorkerPool,
}

async fn lakehouse(test: &str) -> Lakehouse {
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
            .with_grants(Arc::new(proxy) as Arc<dyn PrefixGrants>),
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
        _pool: pool,
    }
}

async fn post(addr: SocketAddr, sql: &str) -> Response {
    let target = target(&[("user", "alice"), ("password", "a")]);
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

async fn refused(addr: SocketAddr, sql: &str, code: &str) -> String {
    let response = post(addr, sql).await;
    assert_eq!(
        response.header("X-ClickHouse-Exception-Code"),
        Some(code),
        "{sql}: {}",
        response.text()
    );
    response.text()
}

const ROWS: &str = "SELECT * FROM m1 ORDER BY k";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutations_match_the_oracle() {
    let house = lakehouse("mutations").await;
    let addr = house.handle.local_addr();
    ok(
        addr,
        "CREATE TABLE m1 (k UInt64, d Date, s String, n Nullable(Int32)) ENGINE = MergeTree \
         PARTITION BY toYYYYMM(d) ORDER BY k",
    )
    .await;
    ok(
        addr,
        "INSERT INTO m1 VALUES (1,'2026-09-01','a',1),(2,'2026-09-02','b',NULL),(3,'2026-10-01','c',3)",
    )
    .await;
    ok(
        addr,
        "INSERT INTO m1 VALUES (4,'2026-10-02','d',NULL),(5,'2026-10-03','e',5),(6,'2026-11-01','f',6)",
    )
    .await;

    // A NULL condition keeps the row.
    let response = post(addr, "ALTER TABLE m1 DELETE WHERE n > 4").await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(
        response.header("X-Loams-Snapshot-Id").is_some(),
        "committed"
    );
    assert_eq!(
        ok(addr, ROWS).await,
        "1\t2026-09-01\ta\t1\n2\t2026-09-02\tb\t\\N\n3\t2026-10-01\tc\t3\n4\t2026-10-02\td\t\\N\n"
    );
    ok(
        addr,
        "ALTER TABLE m1 UPDATE s = concat(s, '!'), n = n * 10 WHERE k >= 3",
    )
    .await;
    assert_eq!(
        ok(addr, ROWS).await,
        "1\t2026-09-01\ta\t1\n2\t2026-09-02\tb\t\\N\n3\t2026-10-01\tc!\t30\n4\t2026-10-02\td!\t\\N\n"
    );
    ok(addr, "DELETE FROM m1 WHERE s = 'b'").await;
    ok(addr, "UPDATE m1 SET s = 'z' WHERE k = 1").await;
    assert_eq!(
        ok(addr, ROWS).await,
        "1\t2026-09-01\tz\t1\n3\t2026-10-01\tc!\t30\n4\t2026-10-02\td!\t\\N\n"
    );
    ok(addr, "ALTER TABLE m1 ON CLUSTER c DROP PARTITION 202610").await;
    assert_eq!(ok(addr, ROWS).await, "1\t2026-09-01\tz\t1\n");
    ok(addr, "ALTER TABLE m1 CLEAR COLUMN s IN PARTITION 202609").await;
    assert_eq!(ok(addr, ROWS).await, "1\t2026-09-01\t\t1\n");

    // Nothing touched: nothing committed.
    let response = post(
        addr,
        "ALTER TABLE m1 DELETE WHERE k = 100 SETTINGS mutations_sync = 2",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(response.header("X-Loams-Snapshot-Id").is_none());

    // As ClickHouse refuses.
    let text = refused(addr, "ALTER TABLE m1 UPDATE k = 9 WHERE 1", "420").await;
    assert!(text.contains("Cannot UPDATE key column `k`"), "{text}");
    refused(addr, "ALTER TABLE m1 UPDATE d = today() WHERE 1", "420").await;
    refused(addr, "ALTER TABLE m1 UPDATE nope = 9 WHERE 1", "16").await;
    refused(addr, "ALTER TABLE m1 DROP PART 'all_1_1_0'", "48").await;
    refused(addr, "ALTER TABLE m1 DROP PARTITION ID '202609'", "48").await;
    refused(
        addr,
        "ALTER TABLE m1 DELETE WHERE k = 1, ADD COLUMN x UInt8",
        "48",
    )
    .await;
    refused(addr, "ALTER TABLE nope DELETE WHERE 1", "60").await;
    assert_eq!(ok(addr, ROWS).await, "1\t2026-09-01\t\t1\n");
}

/// A mutation reaches every version of a key, and the rows it rewrites keep
/// their order: the last inserted of equal versions still wins `FINAL`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutations_keep_versions_and_their_order() {
    let house = lakehouse("versions").await;
    let addr = house.handle.local_addr();
    ok(
        addr,
        "CREATE TABLE r1 (k UInt64, v UInt32, s String) ENGINE = ReplacingMergeTree(v) ORDER BY k",
    )
    .await;
    for row in ["(1,1,'x')", "(1,2,'y')", "(2,1,'a')", "(2,1,'b')"] {
        ok(addr, &format!("INSERT INTO r1 VALUES {row}")).await;
    }
    ok(addr, "ALTER TABLE r1 DELETE WHERE s = 'y'").await;
    assert_eq!(
        ok(addr, "SELECT * FROM r1 FINAL ORDER BY k").await,
        "1\t1\tx\n2\t1\tb\n"
    );
    ok(addr, "ALTER TABLE r1 UPDATE s = upper(s) WHERE k = 2").await;
    assert_eq!(
        ok(addr, "SELECT * FROM r1 FINAL ORDER BY k").await,
        "1\t1\tx\n2\t1\tB\n"
    );
}

/// OpenPanel's `profile_event_property_summary_mv` (HE1 R9.5): ClickHouse 26.7
/// and later refuse it unless `allow_dimensions_outside_sorting_key = 1`
/// (`property_value` is outside its sorting key); the House does not enforce
/// that rule and accepts it either way (a deliberate difference). Its rows are
/// a lake table under the view's name, so `ALTER TABLE <view> DELETE`, which
/// OpenPanel sends when a project is deleted, mutates them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn openpanel_summary_view_is_accepted_and_mutated() {
    let house = lakehouse("openpanel-view").await;
    let addr = house.handle.local_addr();
    ok(
        addr,
        "CREATE TABLE events (project_id String, profile_id String, device_id String, \
         name LowCardinality(String), properties Map(String, String), \
         created_at DateTime64(3)) ENGINE = MergeTree ORDER BY (project_id, created_at)",
    )
    .await;
    let view = "CREATE MATERIALIZED VIEW IF NOT EXISTS {name}
ENGINE = AggregatingMergeTree()
PARTITION BY toYYYYMM(event_date)
ORDER BY (project_id, profile_id, name, property_key, event_date){settings}
AS SELECT
          project_id,
          profile_id,
          name,
          property_key,
          property_value,
          toStartOfDay(created_at) AS event_date,
          countState() AS event_count,
          minState(created_at) AS first_event_time,
          maxState(created_at) AS last_event_time
        FROM events
        ARRAY JOIN mapKeys(properties) AS property_key, mapValues(properties) AS property_value
        WHERE profile_id != device_id
          AND property_key != ''
          AND property_value != ''
        GROUP BY project_id, profile_id, name, property_key, property_value, event_date";
    ok(
        addr,
        &view
            .replace("{name}", "profile_event_property_summary_mv")
            .replace("{settings}", ""),
    )
    .await;
    ok(
        addr,
        &view.replace("{name}", "summary_with_setting").replace(
            "{settings}",
            "\nSETTINGS allow_dimensions_outside_sorting_key = 1",
        ),
    )
    .await;
    ok(
        addr,
        "INSERT INTO events VALUES \
         ('p1', 'u1', 'd1', 'screen_view', {'plan':'pro','os':'linux'}, '2026-10-21 10:00:00.000'), \
         ('p1', 'u1', 'd1', 'screen_view', {'plan':'pro'}, '2026-10-21 11:00:00.000'), \
         ('p2', 'u2', 'd2', 'sign_up', {'plan':'free'}, '2026-10-21 12:00:00.000')",
    )
    .await;
    let read = "SELECT project_id, property_key, property_value, countMerge(event_count) \
        FROM profile_event_property_summary_mv GROUP BY ALL ORDER BY ALL";
    assert_eq!(
        ok(addr, read).await,
        "p1\tos\tlinux\t1\np1\tplan\tpro\t2\np2\tplan\tfree\t1\n"
    );
    ok(
        addr,
        "ALTER TABLE profile_event_property_summary_mv DELETE WHERE project_id = 'p1'",
    )
    .await;
    assert_eq!(ok(addr, read).await, "p2\tplan\tfree\t1\n");
}
