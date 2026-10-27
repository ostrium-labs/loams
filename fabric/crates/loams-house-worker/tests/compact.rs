//! Replacing, Summing and VersionedCollapsing on lake tables, and compaction
//! (HS1 Task 13, §49 §6.2, D1570): `FINAL` merges at read time as ClickHouse's own `FINAL` does,
//! `OPTIMIZE` and the background scheduler rewrite the merged rows, and a
//! compaction that loses to another commit is abandoned without losing it.
//!
//! The reference is ClickHouse itself (`clickhouse local`, from
//! `CLICKHOUSE_REFERENCE` or `PATH`) with the same tables as `MergeTree`
//! family tables; the comparisons that need it are skipped, saying so, where
//! it is not installed.

#![cfg(target_os = "linux")]

mod common;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use common::http::{Response, request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{
    CatalogError, HouseCatalog, Lake, LakeRoot, LakeTable, MemCatalog, PrefixGrants,
};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_lake::spec::{Operation, TableMetadata};
use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent, TableRequirement, TableUpdate};
use loams_store::Store;

const ALICE: (&str, &str) = ("alice", "a");

/// A catalog that, once armed, makes a compaction's `replace` lose its first
/// race: before the `replace` commits, the House takes an `INSERT` (as a
/// concurrent client would), which moves `main`.
#[derive(Debug)]
struct Racing {
    inner: Arc<MemCatalog>,
    armed: AtomicBool,
    house: OnceLock<SocketAddr>,
    replaces: AtomicU32,
}

#[async_trait]
impl HouseCatalog for Racing {
    async fn list_databases(
        &self,
        parent: &NamespaceIdent,
    ) -> Result<Vec<NamespaceIdent>, CatalogError> {
        self.inner.list_databases(parent).await
    }
    async fn create_database(
        &self,
        namespace: &NamespaceIdent,
        properties: HashMap<String, String>,
    ) -> Result<(), CatalogError> {
        self.inner.create_database(namespace, properties).await
    }
    async fn database_properties(
        &self,
        namespace: &NamespaceIdent,
    ) -> Result<HashMap<String, String>, CatalogError> {
        self.inner.database_properties(namespace).await
    }
    async fn drop_database(&self, namespace: &NamespaceIdent) -> Result<(), CatalogError> {
        self.inner.drop_database(namespace).await
    }
    async fn list_tables(
        &self,
        namespace: &NamespaceIdent,
    ) -> Result<Vec<TableIdent>, CatalogError> {
        self.inner.list_tables(namespace).await
    }
    async fn load_table(&self, table: &TableIdent) -> Result<LakeTable, CatalogError> {
        self.inner.load_table(table).await
    }
    async fn create_table(
        &self,
        namespace: &NamespaceIdent,
        creation: TableCreation,
    ) -> Result<LakeTable, CatalogError> {
        self.inner.create_table(namespace, creation).await
    }
    async fn drop_table(&self, table: &TableIdent) -> Result<(), CatalogError> {
        self.inner.drop_table(table).await
    }
    async fn rename_table(&self, from: &TableIdent, to: &TableIdent) -> Result<(), CatalogError> {
        self.inner.rename_table(from, to).await
    }
    async fn commit(
        &self,
        table: &TableIdent,
        requirements: Vec<TableRequirement>,
        updates: Vec<TableUpdate>,
    ) -> Result<LakeTable, CatalogError> {
        let replace = updates.iter().any(|u| {
            matches!(u, TableUpdate::AddSnapshot { snapshot }
                if snapshot.summary().operation == Operation::Replace)
        });
        if replace {
            self.replaces.fetch_add(1, Ordering::SeqCst);
            if self.armed.swap(false, Ordering::SeqCst)
                && let Some(addr) = self.house.get().copied()
            {
                let response = tokio::task::spawn_blocking(move || {
                    request(
                        addr,
                        "POST",
                        &target(&[("user", "alice"), ("password", "a")]),
                        &[],
                        b"INSERT INTO r VALUES (100, 'raced', 1)",
                    )
                })
                .await
                .expect("client");
                assert_eq!(response.status, 200, "{}", response.text());
            }
        }
        self.inner.commit(table, requirements, updates).await
    }
    async fn register_table(
        &self,
        table: &TableIdent,
        metadata_location: &str,
    ) -> Result<LakeTable, CatalogError> {
        self.inner.register_table(table, metadata_location).await
    }
}

struct Lakehouse {
    handle: HouseHandle,
    catalog: Arc<MemCatalog>,
    racing: Arc<Racing>,
    _pool: WorkerPool,
}

async fn lakehouse_with(test: &str, compaction: Duration) -> Lakehouse {
    let dir = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", dir.display());
    let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
    let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
    let racing = Arc::new(Racing {
        inner: catalog.clone(),
        armed: AtomicBool::new(false),
        house: OnceLock::new(),
        replaces: AtomicU32::new(0),
    });
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
        Lake::new(racing.clone() as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store)
            .with_grants(Arc::new(proxy) as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        compaction_interval: compaction,
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool.clone(), lake)
        .await
        .expect("serves");
    let _ = racing.house.set(handle.local_addr());
    Lakehouse {
        handle,
        catalog,
        racing,
        _pool: pool,
    }
}

async fn lakehouse(test: &str) -> Lakehouse {
    lakehouse_with(test, Duration::ZERO).await
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

async fn metadata(house: &Lakehouse, name: &str) -> TableMetadata {
    house
        .catalog
        .load_table(&TableIdent::new(
            Lake::namespace(2, "default"),
            name.to_string(),
        ))
        .await
        .expect("table")
        .metadata
}

/// `clickhouse local` with `statements`, separated by `;`, when installed.
fn reference(statements: &str) -> Option<String> {
    let binary = std::env::var("CLICKHOUSE_REFERENCE").unwrap_or_else(|_| "clickhouse".to_string());
    let output = std::process::Command::new(&binary)
        .args(["local", "--multiquery", "--query", statements])
        .output()
        .ok()?;
    if !output.status.success() {
        panic!(
            "the reference failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn skip(test: &str) {
    eprintln!(
        "{test}: no `clickhouse` reference (set CLICKHOUSE_REFERENCE); compared with the House alone"
    );
}

/// Three batches of a Replacing table with a version: keys overlap across
/// batches, some versions tie.
const REPLACING: &str =
    "CREATE TABLE r (k Int64, s String, ver UInt32) ENGINE = ReplacingMergeTree(ver) ORDER BY k";
const BATCHES: [&str; 3] = [
    "INSERT INTO r VALUES (1, 'a1', 1), (2, 'b1', 1), (3, 'c1', 5)",
    "INSERT INTO r VALUES (1, 'a2', 2), (2, 'b2', 0), (4, 'd1', 1)",
    "INSERT INTO r VALUES (1, 'a3', 2), (3, 'c2', 5), (5, 'e1', 1)",
];

fn data_files(metadata: &TableMetadata) -> u64 {
    metadata
        .current_snapshot()
        .and_then(|s| {
            s.summary()
                .additional_properties
                .get("total-data-files")
                .cloned()
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_final_equals_reference_final() {
    let house = lakehouse("replacing").await;
    let addr = house.handle.local_addr();
    ok(addr, REPLACING).await;
    for batch in BATCHES {
        ok(addr, batch).await;
    }
    let final_rows = ok(addr, "SELECT k, s, ver FROM r FINAL ORDER BY k").await;
    // The newest version wins; a tie goes to the later insert.
    assert_eq!(
        final_rows,
        "1\ta3\t2\n2\tb1\t1\n3\tc2\t5\n4\td1\t1\n5\te1\t1\n"
    );
    match reference(&format!(
        "{REPLACING}; {}; SELECT k, s, ver FROM r FINAL ORDER BY k",
        BATCHES.join("; ")
    )) {
        Some(want) => assert_eq!(final_rows, want),
        None => skip("replacing_final_equals_reference_final"),
    }
    // An INSERT … SELECT … FINAL reads merged rows too.
    ok(addr, &REPLACING.replace("TABLE r ", "TABLE r2 ")).await;
    ok(addr, "INSERT INTO r2 SELECT k, s, ver FROM r FINAL").await;
    assert_eq!(
        ok(addr, "SELECT k, s, ver FROM r2 ORDER BY k").await,
        final_rows
    );
}

/// Without `FINAL`, reads see every inserted row until a merge, as ClickHouse
/// does before its parts merge (compared as multisets, "unmerged" mode).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_without_final_may_show_duplicates_like_reference() {
    let house = lakehouse("unmerged").await;
    let addr = house.handle.local_addr();
    ok(addr, REPLACING).await;
    for batch in BATCHES {
        ok(addr, batch).await;
    }
    assert_eq!(ok(addr, "SELECT count() FROM r").await, "9\n");
    let rows = ok(addr, "SELECT k, s, ver FROM r ORDER BY k, s").await;
    match reference(&format!(
        "{REPLACING} SETTINGS min_bytes_for_wide_part = 0; SYSTEM STOP MERGES r; {}; \
         SELECT k, s, ver FROM r ORDER BY k, s",
        BATCHES.join("; ")
    )) {
        Some(want) => assert_eq!(rows, want),
        None => skip("replacing_without_final_may_show_duplicates_like_reference"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn summing_final_equals_reference() {
    let house = lakehouse("summing").await;
    let addr = house.handle.local_addr();
    let create = "CREATE TABLE s (k String, n Int64, x Float64, tag String) ENGINE = SummingMergeTree ORDER BY k";
    let batches = [
        "INSERT INTO s VALUES ('a', 1, 0.5, 't1'), ('b', 2, 1.5, 't1')",
        "INSERT INTO s VALUES ('a', 3, 0.25, 't2'), ('c', 0, 0, 't3')",
        "INSERT INTO s VALUES ('b', -2, -1.5, 't4')",
    ];
    ok(addr, create).await;
    for batch in batches {
        ok(addr, batch).await;
    }
    // `a` sums; `b` and `c` sum to zero and are dropped, as ClickHouse drops
    // all-zero rows.
    let rows = ok(addr, "SELECT k, n, x FROM s FINAL ORDER BY k").await;
    assert_eq!(rows, "a\t4\t0.75\n");
    match reference(&format!(
        "{create}; {}; SELECT k, n, x FROM s FINAL ORDER BY k",
        batches.join("; ")
    )) {
        Some(want) => assert_eq!(rows, want),
        None => skip("summing_final_equals_reference"),
    }
    // OPTIMIZE writes the sums: the next read without FINAL sees them.
    ok(addr, "OPTIMIZE TABLE s FINAL").await;
    assert_eq!(
        ok(addr, "SELECT k, n, x FROM s ORDER BY k").await,
        "a\t4\t0.75\n"
    );
}

/// VersionedCollapsing (D1570): within one sorting key and version, a row of
/// the other sign cancels the most recent row left (a stack, not a queue).
/// One row per `INSERT`, so each is its own file (and, in ClickHouse, its own
/// part). The expected rows were measured on the 26.9.2.8 oracle (plan HE1
/// R9.2) and are also compared with a local reference when there is one.
const COLLAPSING: &str = "CREATE TABLE vc (k UInt8, v UInt8, x String, sign Int8) \
    ENGINE = VersionedCollapsingMergeTree(sign, v) ORDER BY k";
const COLLAPSING_ROWS: [&str; 17] = [
    "(1,1,'a',1)",
    "(1,1,'b',1)",
    "(1,1,'c',-1)",
    "(2,1,'a',1)",
    "(2,1,'b',-1)",
    "(2,1,'c',1)",
    "(2,1,'d',1)",
    "(2,1,'e',-1)",
    "(3,1,'a',-1)",
    "(3,1,'b',1)",
    "(3,1,'c',-1)",
    "(4,1,'a',1)",
    "(4,2,'b',1)",
    "(4,1,'c',-1)",
    "(5,1,'a',-1)",
    "(5,1,'b',-1)",
    "(5,1,'c',1)",
];
const COLLAPSING_FINAL: &str = "1\t1\ta\t1\n2\t1\tc\t1\n3\t1\tc\t-1\n4\t2\tb\t1\n5\t1\ta\t-1\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collapsing_final_matches_the_oracle() {
    let house = lakehouse("collapsing").await;
    let addr = house.handle.local_addr();
    ok(addr, COLLAPSING).await;
    let inserts: Vec<String> = COLLAPSING_ROWS
        .iter()
        .map(|r| format!("INSERT INTO vc VALUES {r}"))
        .collect();
    for insert in &inserts {
        ok(addr, insert).await;
    }
    assert_eq!(
        ok(
            addr,
            "SELECT engine_full FROM system.tables WHERE name = 'vc'"
        )
        .await,
        "VersionedCollapsingMergeTree(sign, v) ORDER BY k\n"
    );
    // Without FINAL, every row as written.
    assert_eq!(ok(addr, "SELECT count() FROM vc").await, "17\n");
    let query = "SELECT k, v, x, sign FROM vc FINAL ORDER BY k, v, x";
    let rows = ok(addr, query).await;
    assert_eq!(rows, COLLAPSING_FINAL);
    assert_eq!(
        ok(
            addr,
            "SELECT k, v, x, sign FROM vc ORDER BY k, v, x SETTINGS final = 1"
        )
        .await,
        COLLAPSING_FINAL
    );
    match reference(&format!("{COLLAPSING}; {}; {query}", inserts.join("; "))) {
        Some(want) => assert_eq!(rows, want),
        None => skip("collapsing_final_matches_the_oracle"),
    }
    // OPTIMIZE writes the collapsed rows; FINAL answers as before.
    ok(addr, "OPTIMIZE TABLE vc FINAL").await;
    assert_eq!(
        ok(addr, "SELECT k, v, x, sign FROM vc ORDER BY k, v, x").await,
        COLLAPSING_FINAL
    );
    assert_eq!(ok(addr, query).await, COLLAPSING_FINAL);
}

/// OpenPanel's `sessions` (plan HE1 R4.3): each update writes the old state
/// with sign -1 and the new with sign 1 and the next version; `FINAL` keeps the
/// newest state, and `sum(sign)` counts live sessions with or without it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collapsing_sessions_keep_their_latest_state() {
    let house = lakehouse("sessions").await;
    let addr = house.handle.local_addr();
    let create = "CREATE TABLE sessions (id String, project_id String, \
        created_at DateTime64(3), screen_view_count UInt32, sign Int8, version UInt64) \
        ENGINE = VersionedCollapsingMergeTree(sign, version) \
        PARTITION BY toYYYYMM(created_at) ORDER BY (project_id, toDate(created_at), created_at)";
    let batches = [
        "INSERT INTO sessions VALUES ('s1', 'p', '2026-10-21 10:00:00.000', 1, 1, 1), \
         ('s2', 'p', '2026-10-21 11:00:00.000', 1, 1, 1)",
        "INSERT INTO sessions VALUES ('s1', 'p', '2026-10-21 10:00:00.000', 1, -1, 1), \
         ('s1', 'p', '2026-10-21 10:00:00.000', 2, 1, 2)",
        "INSERT INTO sessions VALUES ('s1', 'p', '2026-10-21 10:00:00.000', 2, -1, 2), \
         ('s1', 'p', '2026-10-21 10:00:00.000', 5, 1, 3), \
         ('s2', 'p', '2026-10-21 11:00:00.000', 1, -1, 1), \
         ('s2', 'p', '2026-10-21 11:00:00.000', 3, 1, 2)",
    ];
    ok(addr, create).await;
    for batch in batches {
        ok(addr, batch).await;
    }
    let latest = "SELECT id, screen_view_count, version FROM sessions FINAL ORDER BY id";
    assert_eq!(ok(addr, latest).await, "s1\t5\t3\ns2\t3\t2\n");
    let live = "SELECT sum(sign) AS sessions, sum(sign * screen_view_count) AS views \
        FROM sessions WHERE project_id = 'p' HAVING sum(sign) > 0";
    assert_eq!(ok(addr, live).await, "2\t8\n");
    assert_eq!(
        ok(addr, &live.replace("FROM sessions", "FROM sessions FINAL")).await,
        "2\t8\n"
    );
    if let Some(want) = reference(&format!("{create}; {}; {latest}", batches.join("; "))) {
        assert_eq!(ok(addr, latest).await, want);
    } else {
        skip("collapsing_sessions_keep_their_latest_state");
    }
}

/// One `INSERT` holding several rows of one key (R9.4): ClickHouse sorts an
/// inserted block stably and merges it (`optimize_on_insert`), so the House
/// writes rows of one key in insert order and merges them the same way. The
/// expected rows were measured on the 26.9.2.8 oracle; reads with and without
/// `FINAL` agree with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_insert_merges_as_clickhouse_does() {
    let house = lakehouse("one-insert").await;
    let addr = house.handle.local_addr();
    let cases = [
        (
            "CREATE TABLE rs (k UInt8, v UInt8, x String) ENGINE = ReplacingMergeTree(v) ORDER BY k",
            "INSERT INTO rs VALUES (1,1,'a'),(2,1,'a'),(1,1,'b'),(1,1,'c'),(2,0,'b'),(3,5,'z'),\
             (2,1,'c'),(1,0,'d')",
            "SELECT * FROM rs ORDER BY k",
            "1\t1\tc\n2\t1\tc\n3\t5\tz\n",
        ),
        (
            "CREATE TABLE rd (k UInt8, v UInt8, d UInt8) ENGINE = ReplacingMergeTree(v, d) ORDER BY k",
            "INSERT INTO rd VALUES (1,1,0),(1,2,1),(2,1,0)",
            "SELECT * FROM rd ORDER BY k",
            "1\t2\t1\n2\t1\t0\n",
        ),
        (
            "CREATE TABLE ss (k UInt8, n Int64, f Float64, t String) ENGINE = SummingMergeTree ORDER BY k",
            "INSERT INTO ss VALUES (1,1,0.5,'a'),(2,2,1,'b'),(1,3,0.25,'c'),(2,-2,-1,'d'),(3,0,0,'e')",
            "SELECT * FROM ss ORDER BY k",
            "1\t4\t0.75\ta\n",
        ),
        (
            "CREATE TABLE vs (k UInt8, v UInt8, x String, sign Int8) \
             ENGINE = VersionedCollapsingMergeTree(sign, v) ORDER BY k",
            "INSERT INTO vs VALUES (1,1,'a',1),(2,1,'a',1),(1,1,'b',1),(2,1,'b',-1),(1,1,'c',-1),\
             (2,1,'c',1),(2,1,'d',1),(2,1,'e',-1),(3,1,'a',-1),(3,1,'b',-1),(3,1,'c',1),(4,2,'x',1)",
            "SELECT * FROM vs ORDER BY k, v, x",
            "1\t1\ta\t1\n2\t1\tc\t1\n3\t1\ta\t-1\n4\t2\tx\t1\n",
        ),
        (
            "CREATE TABLE m (k UInt8, x String) ENGINE = MergeTree ORDER BY k",
            "INSERT INTO m VALUES (2,'a'),(1,'b'),(2,'c'),(1,'d')",
            "SELECT * FROM m",
            "1\tb\n1\td\n2\ta\n2\tc\n",
        ),
    ];
    for (create, insert, read, want) in cases {
        ok(addr, create).await;
        ok(addr, insert).await;
        assert_eq!(ok(addr, read).await, want, "{insert}");
        if read.contains("FROM m") {
            continue;
        }
        let fin = read.replace(" ORDER BY", " FINAL ORDER BY");
        assert_eq!(
            ok(addr, &fin).await,
            if read.contains("FROM rd") {
                "2\t1\t0\n"
            } else {
                want
            }
        );
        if let Some(reference) = reference(&format!("{create}; {insert}; {read}")) {
            assert_eq!(ok(addr, read).await, reference, "{insert}");
        } else {
            skip("one_insert_merges_as_clickhouse_does");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn is_deleted_dropped_by_final_and_compaction() {
    let house = lakehouse("deleted").await;
    let addr = house.handle.local_addr();
    let create = "CREATE TABLE d (k Int64, v String, ver UInt32, del UInt8) ENGINE = ReplacingMergeTree(ver, del) ORDER BY k";
    let batches = [
        "INSERT INTO d VALUES (1, 'x', 1, 0), (2, 'y', 1, 0)",
        "INSERT INTO d VALUES (1, 'x', 2, 1), (3, 'z', 1, 0)",
    ];
    ok(addr, create).await;
    for batch in batches {
        ok(addr, batch).await;
    }
    let final_rows = ok(addr, "SELECT k, v FROM d FINAL ORDER BY k").await;
    assert_eq!(final_rows, "2\ty\n3\tz\n");
    if let Some(want) = reference(&format!(
        "{create}; {}; SELECT k, v FROM d FINAL ORDER BY k",
        batches.join("; ")
    )) {
        assert_eq!(final_rows, want);
    } else {
        skip("is_deleted_dropped_by_final_and_compaction");
    }
    ok(addr, "OPTIMIZE TABLE d FINAL").await;
    assert_eq!(
        ok(addr, "SELECT k, v FROM d ORDER BY k").await,
        "2\ty\n3\tz\n"
    );
    assert_eq!(ok(addr, "SELECT count() FROM d WHERE del = 1").await, "0\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn optimize_final_compacts_and_answers_after_commit() {
    let house = lakehouse("optimize").await;
    let addr = house.handle.local_addr();
    ok(addr, REPLACING).await;
    for batch in BATCHES {
        ok(addr, batch).await;
    }
    let before = ok(addr, "SELECT k, s, ver FROM r FINAL ORDER BY k").await;
    assert_eq!(data_files(&metadata(&house, "r").await), 3);

    let response = post(addr, "OPTIMIZE TABLE r FINAL").await;
    assert_eq!(response.status, 200, "{}", response.text());
    let table = metadata(&house, "r").await;
    let snapshot = table.current_snapshot().expect("snapshot");
    // Answered after the commit: the replace is already main.
    assert_eq!(
        response.header("X-Loams-Snapshot-Id"),
        Some(format!("default.r={}", snapshot.snapshot_id()).as_str())
    );
    assert_eq!(snapshot.summary().operation, Operation::Replace);
    assert_eq!(data_files(&table), 1);
    // The merged rows, now without FINAL too; FINAL answers as before.
    assert_eq!(ok(addr, "SELECT k, s, ver FROM r ORDER BY k").await, before);
    assert_eq!(
        ok(addr, "SELECT k, s, ver FROM r FINAL ORDER BY k").await,
        before
    );
    // Time travel still reads the snapshot before it.
    let parent = snapshot.parent_snapshot_id().expect("parent");
    let response = tokio::task::spawn_blocking(move || {
        request(
            addr,
            "POST",
            &target(&[
                ("user", "alice"),
                ("password", "a"),
                ("loams_snapshot_id", &parent.to_string()),
            ]),
            &[],
            b"SELECT count() FROM r",
        )
    })
    .await
    .expect("client");
    assert_eq!(response.text(), "9\n");

    // DEDUPLICATE on a MergeTree drops whole-row duplicates; an OPTIMIZE with
    // nothing to merge answers with no snapshot.
    ok(
        addr,
        "CREATE TABLE m (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok(addr, "INSERT INTO m VALUES (1, 'a'), (1, 'a'), (2, 'b')").await;
    let response = post(addr, "OPTIMIZE TABLE m").await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(
        response.header("X-Loams-Snapshot-Id").is_none(),
        "one file, nothing to merge"
    );
    ok(addr, "OPTIMIZE TABLE m PARTITION tuple() FINAL DEDUPLICATE").await;
    assert_eq!(
        ok(addr, "SELECT k, s FROM m ORDER BY k").await,
        "1\ta\n2\tb\n"
    );
    // An external table, a missing one, DEDUPLICATE BY.
    assert_eq!(
        post(addr, "OPTIMIZE TABLE nope FINAL")
            .await
            .header("X-ClickHouse-Exception-Code"),
        Some("60")
    );
    assert_eq!(
        post(addr, "OPTIMIZE TABLE m FINAL DEDUPLICATE BY k")
            .await
            .header("X-ClickHouse-Exception-Code"),
        Some("48")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_conflict_is_abandoned_not_lost() {
    let house = lakehouse("conflict").await;
    let addr = house.handle.local_addr();
    ok(addr, REPLACING).await;
    for batch in BATCHES {
        ok(addr, batch).await;
    }
    // The first replace loses to an INSERT that lands while it commits.
    house.racing.armed.store(true, Ordering::SeqCst);
    let response = post(addr, "OPTIMIZE TABLE r FINAL").await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(
        house.racing.replaces.load(Ordering::SeqCst) >= 2,
        "the first replace was abandoned and the next one landed"
    );
    // The racing row is there; nothing of the batches was lost.
    assert_eq!(
        ok(addr, "SELECT k, s, ver FROM r FINAL ORDER BY k").await,
        "1\ta3\t2\n2\tb1\t1\n3\tc2\t5\n4\td1\t1\n5\te1\t1\n100\traced\t1\n"
    );
    assert_eq!(ok(addr, "SELECT count() FROM r").await, "6\n");
}

/// Any insert sequence, any compaction schedule: `FINAL` answers what the
/// inserts mean (the newest version per key; a tie goes to the later insert).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_preserves_rows_property() {
    let house = lakehouse("property").await;
    let addr = house.handle.local_addr();
    let mut seed: u64 = 0x5eed_1234_abcd;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    for case in 0..3 {
        let table = format!("p{case}");
        ok(
            addr,
            &format!("CREATE TABLE {table} (k Int64, s String, ver UInt32) ENGINE = ReplacingMergeTree(ver) ORDER BY k"),
        )
        .await;
        let mut want: BTreeMap<i64, (u32, String)> = BTreeMap::new();
        for step in 0..6 {
            let rows: Vec<(i64, String, u32)> = (0..1 + next(5))
                .map(|i| {
                    (
                        next(8) as i64,
                        format!("c{case}s{step}r{i}"),
                        next(4) as u32,
                    )
                })
                .collect();
            let values: Vec<String> = rows
                .iter()
                .map(|(k, s, v)| format!("({k}, '{s}', {v})"))
                .collect();
            ok(
                addr,
                &format!("INSERT INTO {table} VALUES {}", values.join(", ")),
            )
            .await;
            for (k, s, v) in rows {
                // Within one insert a later row is later; across inserts too.
                match want.get(&k) {
                    Some((held, _)) if *held > v => {}
                    _ => {
                        want.insert(k, (v, s));
                    }
                }
            }
            if next(3) == 0 {
                ok(addr, &format!("OPTIMIZE TABLE {table} FINAL")).await;
            }
        }
        let expected: String = want
            .iter()
            .map(|(k, (v, s))| format!("{k}\t{s}\t{v}\n"))
            .collect();
        assert_eq!(
            ok(
                addr,
                &format!("SELECT k, s, ver FROM {table} FINAL ORDER BY k")
            )
            .await,
            expected,
            "case {case}"
        );
        ok(addr, &format!("OPTIMIZE TABLE {table} FINAL")).await;
        assert_eq!(
            ok(addr, &format!("SELECT k, s, ver FROM {table} ORDER BY k")).await,
            expected,
            "case {case}, compacted"
        );
    }
}

/// The scheduler compacts a table that has gathered small files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_files_are_compacted_in_the_background() {
    let house = lakehouse_with("background", Duration::from_millis(300)).await;
    let addr = house.handle.local_addr();
    ok(
        addr,
        "CREATE TABLE b (k Int64) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    for k in 0..16 {
        ok(addr, &format!("INSERT INTO b VALUES ({k})")).await;
    }
    let mut compacted = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if data_files(&metadata(&house, "b").await) < 16 {
            compacted = true;
            break;
        }
    }
    assert!(compacted, "the scheduler compacted the small files");
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM b").await, "16\t120\n");
}
