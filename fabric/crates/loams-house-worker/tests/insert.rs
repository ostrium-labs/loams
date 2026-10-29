//! Lake writes (HS1 Task 12, §49 §6.1), over the ClickHouse HTTP interface with
//! sealed worker processes, a `file://` bucket shared by the catalog,
//! `house-cache` and the front, as `loams-fabric house` runs them.
//!
//! Rows are written with `INSERT` and read back with `SELECT` (HS1 Task 11's
//! reads); what reached the bucket and the catalog is checked directly.

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

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
use loams_house_lake::spec::TableMetadata;
use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent, TableRequirement, TableUpdate};
use loams_store::Store;

const ALICE: (&str, &str) = ("alice", "a");
const READER: (&str, &str) = ("reader", "r");

/// A catalog whose next `fail` commits fail as the catalog being unreachable,
/// after (`landed`) or before applying them.
#[derive(Debug)]
struct Flaky {
    inner: Arc<MemCatalog>,
    fail: AtomicU32,
    landed: AtomicBool,
    commits: AtomicU32,
}

#[async_trait]
impl HouseCatalog for Flaky {
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
        self.commits.fetch_add(1, Ordering::SeqCst);
        let failing = self
            .fail
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if !failing {
            return self.inner.commit(table, requirements, updates).await;
        }
        if self.landed.load(Ordering::SeqCst) {
            self.inner.commit(table, requirements, updates).await?;
        }
        Err(CatalogError::Unavailable(
            "injected: the catalog timed out".to_string(),
        ))
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
    flaky: Arc<Flaky>,
    store: Store,
    bucket: std::path::PathBuf,
    pool: WorkerPool,
    proxy: HouseCache,
}

impl Lakehouse {
    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }
}

/// A House whose catalog, `house-cache` and front share one `file://` bucket.
/// `alice` (namespace 2) writes; `reader` (namespace 2) is read-only.
async fn lakehouse_with(test: &str, edit: impl FnOnce(&mut HouseConfig)) -> Lakehouse {
    let dir = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", dir.display());
    let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
    let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
    let flaky = Arc::new(Flaky {
        inner: catalog.clone(),
        fail: AtomicU32::new(0),
        landed: AtomicBool::new(false),
        commits: AtomicU32::new(0),
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
        Lake::new(flaky.clone() as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store.clone())
            .with_grants(Arc::new(proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let mut config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![
            UserMap::dev("alice", "a", 2, false),
            UserMap::dev("reader", "r", 2, true),
        ],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        // Compaction is `compact.rs`'s: it would change these tests' files.
        compaction_interval: Duration::ZERO,
        ..HouseConfig::default()
    };
    edit(&mut config);
    let handle = serve_with_lake(config, pool.clone(), lake)
        .await
        .expect("serves");
    Lakehouse {
        handle,
        catalog,
        flaky,
        store,
        bucket: dir,
        pool,
        proxy,
    }
}

async fn lakehouse(test: &str) -> Lakehouse {
    lakehouse_with(test, |_| {}).await
}

/// `POST /?query=<query>` with `body` (an `INSERT`'s data, or a statement).
async fn post(
    addr: SocketAddr,
    user: (&str, &str),
    query: Option<&str>,
    body: &[u8],
    params: &[(&str, &str)],
) -> Response {
    let mut all = vec![("user", user.0), ("password", user.1)];
    if let Some(query) = query {
        all.push(("query", query));
    }
    all.extend_from_slice(params);
    let target = target(&all);
    let body = body.to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

async fn ok(addr: SocketAddr, sql: &str) -> String {
    let response = post(addr, ALICE, None, sql.as_bytes(), &[]).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
}

async fn bytes_of(addr: SocketAddr, sql: &str) -> Vec<u8> {
    let response = post(addr, ALICE, None, sql.as_bytes(), &[]).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.body
}

/// An `INSERT` with its data in the body; the response.
async fn insert(addr: SocketAddr, query: &str, data: &[u8], params: &[(&str, &str)]) -> Response {
    post(addr, ALICE, Some(query), data, params).await
}

async fn inserted(addr: SocketAddr, query: &str, data: &[u8], params: &[(&str, &str)]) -> Response {
    let response = insert(addr, query, data, params).await;
    assert_eq!(response.status, 200, "{query}: {}", response.text());
    response
}

fn written_rows(response: &Response) -> u64 {
    let summary: serde_json::Value =
        serde_json::from_str(response.header("X-ClickHouse-Summary").expect("summary"))
            .expect("json");
    summary["written_rows"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .expect("written_rows")
}

async fn code(addr: SocketAddr, user: (&str, &str), sql: &str) -> (Option<String>, String) {
    let response = post(addr, user, None, sql.as_bytes(), &[]).await;
    (
        response
            .header("X-ClickHouse-Exception-Code")
            .map(str::to_string),
        response.text(),
    )
}

async fn refused(addr: SocketAddr, sql: &str, want: &str) -> String {
    let (code, text) = code(addr, ALICE, sql).await;
    assert_eq!(code.as_deref(), Some(want), "{sql}: {text}");
    text
}

async fn metadata(house: &Lakehouse, database: &str, name: &str) -> TableMetadata {
    house
        .catalog
        .load_table(&TableIdent::new(
            Lake::namespace(2, database),
            name.to_string(),
        ))
        .await
        .unwrap_or_else(|err| panic!("{database}.{name}: {err}"))
        .metadata
}

fn snapshots(metadata: &TableMetadata) -> usize {
    metadata.snapshots().count()
}

/// Every file under the bucket whose path contains `part`.
fn files_under(house: &Lakehouse, part: &str) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&house.bucket, &mut out);
    out.retain(|p| p.contains(part));
    out.sort();
    out
}

const KV: &str = "CREATE TABLE t (k Int64, s String) ENGINE = MergeTree ORDER BY k";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_values_lands_rows_after_the_commit() {
    let house = lakehouse("values").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let response = inserted(addr, "INSERT INTO t VALUES (2, 'b'), (1, 'a')", b"", &[]).await;
    assert_eq!(written_rows(&response), 2);
    let table = metadata(&house, "default", "t").await;
    let snapshot = table.current_snapshot().expect("a snapshot").snapshot_id();
    assert_eq!(
        response.header("X-Loams-Snapshot-Id"),
        Some(format!("default.t={snapshot}").as_str())
    );
    assert_eq!(
        ok(addr, "SELECT k, s FROM t ORDER BY k").await,
        "1\ta\n2\tb\n"
    );

    // The tuples in the body, as ClickHouse takes them.
    inserted(addr, "INSERT INTO t VALUES", b"(3, 'c')", &[]).await;
    // And in the statement text, in the body.
    let response = post(
        addr,
        ALICE,
        None,
        b"INSERT INTO default.t (s, k) VALUES ('d', 4)",
        &[],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "4\t10\n");

    // The data files are Parquet under the table's own data directory, written
    // by the front.
    let files = files_under(&house, "/data/");
    assert_eq!(files.len(), 3, "{files:?}");
    assert!(
        files
            .iter()
            .all(|f| f.ends_with(".parquet") && f.contains("/ns/2/house/default/t-"))
    );
    assert_eq!(snapshots(&metadata(&house, "default", "t").await), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_every_declared_format_lands_identical_rows() {
    let house = lakehouse("formats").await;
    let addr = house.addr();
    ok(addr, KV).await;
    inserted(
        addr,
        "INSERT INTO t SELECT number, concat('s', toString(number)) FROM numbers(100000)",
        b"",
        &[],
    )
    .await;
    let want = ok(addr, "SELECT k, s FROM t ORDER BY k FORMAT TSV").await;
    assert_eq!(want.lines().count(), 100_000);
    // VALUES, JSONEachRow, CSV, TSV, RowBinary, Native, Parquet, ArrowStream
    // (FL2 Task 6's list): each body is what the House itself answers.
    for format in [
        "Values",
        "JSONEachRow",
        "CSV",
        "TSV",
        "RowBinary",
        "Native",
        "Parquet",
        "ArrowStream",
    ] {
        let name = format!("t_{}", format.to_lowercase());
        ok(addr, &KV.replace("TABLE t ", &format!("TABLE {name} "))).await;
        let body = bytes_of(
            addr,
            &format!("SELECT k, s FROM t ORDER BY k FORMAT {format}"),
        )
        .await;
        let response = inserted(
            addr,
            &format!("INSERT INTO {name} FORMAT {format}"),
            &body,
            &[],
        )
        .await;
        assert_eq!(written_rows(&response), 100_000, "{format}");
        assert_eq!(
            ok(
                addr,
                &format!("SELECT k, s FROM {name} ORDER BY k FORMAT TSV")
            )
            .await,
            want,
            "{format}"
        );
    }
}

/// The Tier 1 types of HS1 Task 10's table, written and read back: what
/// `values()` makes of the same rows is what the lake table answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn type_mapping_roundtrip_through_insert() {
    let house = lakehouse("types").await;
    let addr = house.addr();
    let columns = "i8 Int8, i16 Int16, i32 Int32, i64 Int64, u8 UInt8, u16 UInt16, u32 UInt32, \
                   u64 UInt64, f32 Float32, f64 Float64, d Decimal(18, 4), d38 Decimal(38, 10), \
                   b Bool, s String, fs FixedString(3), dt Date, d32 Date32, ts DateTime, \
                   tsz DateTime('Asia/Kolkata'), t64 DateTime64(3), t64z DateTime64(6, 'UTC'), \
                   u UUID, ip4 IPv4, ip6 IPv6, e Enum8('x' = 1, 'y' = 2), lc LowCardinality(String), \
                   n Nullable(Int32), ns Nullable(String), arr Array(Int32), \
                   an Array(Nullable(String)), m Map(String, Int64), tup Tuple(a Int32, b String), \
                   tu Tuple(Int32, String)";
    let rows = "(-128, -32768, -2147483648, -9223372036854775808, 255, 65535, 4294967295, \
                18446744073709551615, 1.5, -2.25, 12345678901234.5678, \
                1234567890123456789012345678.0123456789, true, 'héllo\\tworld', 'abc', \
                '2149-06-06', '1900-01-01', '2026-10-13 10:11:12', '2026-10-13 10:11:12', \
                '2026-10-13 10:11:12.345', '2026-10-13 10:11:12.123456', \
                '61f0c404-5cb3-11e7-907b-a6006ad3dba0', '10.0.0.1', '2001:db8::1', 'y', 'lc', \
                NULL, NULL, [1, 2, 3], ['a', NULL], {'k1': 1, 'k2': 2}, (7, 'seven'), (8, 'eight')), \
               (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, '', '', '1970-01-01', '1970-01-01', \
                '1970-01-01 00:00:00', '1970-01-01 05:30:00', '1970-01-01 00:00:00.000', \
                '1970-01-01 00:00:00.000000', '00000000-0000-0000-0000-000000000000', '0.0.0.0', '::', \
                'x', '', 42, 'n', [], [], {}, (0, ''), (0, ''))";
    ok(
        addr,
        &format!("CREATE TABLE ty ({columns}) ENGINE = MergeTree ORDER BY i64"),
    )
    .await;
    let response = post(
        addr,
        ALICE,
        None,
        format!("INSERT INTO ty VALUES {rows}").as_bytes(),
        &[],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    // The same rows, parsed by `format()` in the worker, never stored.
    let parsed = format!(
        "format(Values, '{}', $${rows}$$)",
        columns.replace('\'', "\\'")
    );
    let want = ok(
        addr,
        &format!("SELECT * FROM {parsed} ORDER BY i64 FORMAT TSV"),
    )
    .await;
    assert_eq!(want.lines().count(), 2, "{want}");
    assert_eq!(
        ok(addr, "SELECT * FROM ty ORDER BY i64 FORMAT TSV").await,
        want
    );
    let names: Vec<String> =
        "i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 d d38 b s fs dt d32 ts tsz t64 \
                               t64z u ip4 ip6 e lc n ns arr an m tup tu"
            .split_whitespace()
            .map(|n| format!("toTypeName({n})"))
            .collect();
    assert_eq!(names.len(), 33);
    let types = |from: &str| format!("SELECT {} FROM {from} LIMIT 1 FORMAT TSV", names.join(", "));
    assert_eq!(
        ok(addr, &types("ty")).await,
        ok(addr, &types(&parsed)).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn defaults_and_column_lists_apply_as_in_clickhouse() {
    let house = lakehouse("defaults").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE d (k Int64, s String DEFAULT concat('s', toString(k)), \
         m UInt32 MATERIALIZED k * 2, n Nullable(Int32)) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    inserted(addr, "INSERT INTO d (k) VALUES", b"(1)", &[]).await;
    inserted(
        addr,
        "INSERT INTO d FORMAT JSONEachRow",
        b"{\"k\": 2}\n{\"k\": 3, \"s\": \"x\", \"n\": 5}\n",
        &[],
    )
    .await;
    inserted(addr, "INSERT INTO d (n, k) FORMAT CSV", b"7,4\n", &[]).await;
    assert_eq!(
        ok(addr, "SELECT k, s, m, n FROM d ORDER BY k").await,
        "1\ts1\t2\t\\N\n2\ts2\t4\t\\N\n3\tx\t6\t5\n4\ts4\t8\t7\n"
    );
    // What ClickHouse refuses, chDB refuses here: a MATERIALIZED column named,
    // a column the table does not have, a value that does not parse.
    let (code, text) = code(addr, ALICE, "INSERT INTO d (k, m) VALUES (9, 9)").await;
    assert!(code.is_some(), "{text}");
    refused(addr, "INSERT INTO d (k, nope) VALUES (9, 9)", "16").await;
    let response = insert(addr, "INSERT INTO d (k) FORMAT CSV", b"notanumber\n", &[]).await;
    assert_ne!(response.status, 200, "{}", response.text());
    // Nothing of those landed.
    assert_eq!(ok(addr, "SELECT count() FROM d").await, "4\n");
}

/// HE1 R14.10: an equality on a `UUID` column finds its rows (Laminar filters
/// every read by `project_id`): the House writes no Iceberg bounds for `uuid`
/// columns, which ClickHouse 26.9's Iceberg reader misreads and prunes on.
/// Every answer is the 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uuid_equalities_find_their_rows() {
    let house = lakehouse("uuid-filters").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE u (project_id UUID, trace_id Nullable(UUID), n Int64) \
         ENGINE = MergeTree ORDER BY (project_id, n)",
    )
    .await;
    ok(
        addr,
        "INSERT INTO u VALUES ('00000000-0000-0000-0000-000000000001', \
         '00000000-0000-0000-0000-00000000000a', 1), ('00000000-0000-0000-0000-000000000001', \
         NULL, 2), ('ffffffff-0000-0000-0000-000000000002', '00000000-0000-0000-0000-00000000000b', 3)",
    )
    .await;
    for (sql, want) in [
        (
            "SELECT count() FROM u WHERE project_id = toUUID('00000000-0000-0000-0000-000000000001')",
            "2\n",
        ),
        (
            "SELECT n FROM u WHERE project_id = 'ffffffff-0000-0000-0000-000000000002'",
            "3\n",
        ),
        (
            "SELECT n FROM u WHERE trace_id = '00000000-0000-0000-0000-00000000000a'",
            "1\n",
        ),
        (
            "SELECT count() FROM u WHERE project_id IN ('00000000-0000-0000-0000-000000000001', \
             'ffffffff-0000-0000-0000-000000000002')",
            "3\n",
        ),
        (
            "SELECT count() FROM u WHERE project_id > '00000000-0000-0000-0000-000000000001'",
            "1\n",
        ),
    ] {
        assert_eq!(ok(addr, &format!("{sql} FORMAT TSV")).await, want, "{sql}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_select_reads_lake_tables_and_table_functions() {
    let house = lakehouse("select").await;
    let addr = house.addr();
    ok(addr, KV).await;
    ok(addr, &KV.replace("TABLE t ", "TABLE t2 ")).await;
    inserted(
        addr,
        "INSERT INTO t SELECT number, toString(number) FROM numbers(10)",
        b"",
        &[],
    )
    .await;
    // A lake table read through its pinned view, into another.
    let response = post(
        addr,
        ALICE,
        None,
        b"INSERT INTO t2 (s, k) WITH 5 AS f SELECT s, k * f FROM t WHERE k < 4",
        &[],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(written_rows(&response), 4);
    assert_eq!(
        ok(addr, "SELECT k, s FROM t2 ORDER BY k").await,
        "0\t0\n5\t1\n10\t2\n15\t3\n"
    );
    // Host functions answer the House's values, as in any query.
    ok(
        addr,
        "CREATE TABLE h (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    inserted(addr, "INSERT INTO h SELECT 1, hostName()", b"", &[]).await;
    assert_eq!(ok(addr, "SELECT s FROM h").await, "loams-house\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_paths_keep_the_deny_list_and_the_grants() {
    let house = lakehouse("deny").await;
    let addr = house.addr();
    ok(addr, KV).await;
    // A source in the query of an INSERT … SELECT, anywhere in it.
    for sql in [
        "INSERT INTO t SELECT 1, line FROM file('/etc/passwd', 'LineAsString', 'line String')",
        "INSERT INTO t SELECT 1, * FROM url('http://127.0.0.1:1/x', 'LineAsString', 'l String')",
        "INSERT INTO t SELECT 1, toString(x) FROM (SELECT * FROM s3('http://127.0.0.1:1/b/k', 'CSV', 'x UInt8'))",
        "INSERT INTO t WITH (SELECT count() FROM remote('127.0.0.1', system.one)) AS c SELECT c, ''",
        "INSERT INTO t SELECT 1, toString(x) FROM executable('cat', 'TSV', 'x UInt8')",
    ] {
        refused(addr, sql, "344").await;
    }
    // A denied setting in the statement's own SETTINGS, or the request's.
    refused(
        addr,
        "INSERT INTO t SETTINGS format_schema = 'x' VALUES (1, 'a')",
        "164",
    )
    .await;
    let response = insert(
        addr,
        "INSERT INTO t FORMAT CSV",
        b"1,a\n",
        &[("user_files_path", "/")],
    )
    .await;
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("164"));
    // A read-only user, and GET, write nothing.
    let (code_ro, text) = code(addr, READER, "INSERT INTO t VALUES (1, 'a')").await;
    assert_eq!(code_ro.as_deref(), Some("164"), "{text}");
    let get = tokio::task::spawn_blocking(move || {
        request(
            addr,
            "GET",
            &target(&[
                ("user", "alice"),
                ("password", "a"),
                ("query", "INSERT INTO t VALUES (1, 'a')"),
            ]),
            &[],
            b"",
        )
    })
    .await
    .expect("client");
    assert_eq!(get.header("X-ClickHouse-Exception-Code"), Some("164"));
    // An unknown format is refused before any worker parses it.
    let response = insert(addr, "INSERT INTO t FORMAT `CSV; DROP`", b"1,a\n", &[]).await;
    assert_ne!(response.status, 200, "{}", response.text());
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "0\n");
    assert!(files_under(&house, "/data/").is_empty());
}

/// Catalog text in a table's properties — a `DEFAULT` expression, the sorting
/// key — reaches the worker only printed again from its tree (R11.11), on the
/// write path too: an injection is `497` and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_expressions_are_reprinted_on_insert() {
    let house = lakehouse("reprint").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE e (k Int64, s String DEFAULT 'x') ENGINE = MergeTree ORDER BY k",
    )
    .await;
    let ident = TableIdent::new(Lake::namespace(2, "default"), "e".to_string());
    let table = house.catalog.load_table(&ident).await.expect("table");
    let types = table.metadata.properties()["loams.ch.types"].clone();
    for (property, from, to) in [
        ("loams.ch.types", "'x'", "1); DROP TABLE e; --"),
        (
            "loams.ch.types",
            "'x'",
            "(SELECT * FROM file('/etc/passwd'))",
        ),
        ("loams.ch.order_by", "k", "k) SETTINGS max_threads = 1 --"),
        (
            "loams.ch.order_by",
            "k",
            "url('http://127.0.0.1:1/', 'CSV', 'x UInt8')",
        ),
    ] {
        let table = house.catalog.load_table(&ident).await.expect("table");
        let current = table.metadata.properties()[property].clone();
        let poisoned = if property == "loams.ch.types" {
            types.replace(
                &serde_json::to_string(from).expect("json")[1..from.len() + 1],
                &serde_json::to_string(to).expect("json")
                    [1..serde_json::to_string(to).expect("json").len() - 1],
            )
        } else {
            to.to_string()
        };
        assert_ne!(current, poisoned, "{to}");
        house
            .catalog
            .commit(
                &ident,
                Vec::new(),
                vec![TableUpdate::SetProperties {
                    updates: HashMap::from([(property.to_string(), poisoned)]),
                }],
            )
            .await
            .expect("poisoned");
        let (code, text) = code(addr, ALICE, "INSERT INTO e (k) VALUES (1)").await;
        assert_eq!(code.as_deref(), Some("497"), "{to}: {text}");
        house
            .catalog
            .commit(
                &ident,
                Vec::new(),
                vec![TableUpdate::SetProperties {
                    updates: HashMap::from([(property.to_string(), current)]),
                }],
            )
            .await
            .expect("restored");
    }
    inserted(addr, "INSERT INTO e (k) VALUES", b"(1)", &[]).await;
    assert_eq!(ok(addr, "SELECT k, s FROM e").await, "1\tx\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_into_temporary_tables_and_functions_stays_chdbs() {
    let house = lakehouse("temporary").await;
    let addr = house.addr();
    let session = [("session_id", "tmp")];
    for sql in [
        "CREATE TEMPORARY TABLE tmp (k Int64) ENGINE = Memory",
        "INSERT INTO tmp VALUES (1), (2)",
    ] {
        let response = post(addr, ALICE, None, sql.as_bytes(), &session).await;
        assert_eq!(response.status, 200, "{sql}: {}", response.text());
    }
    let response = post(addr, ALICE, None, b"SELECT sum(k) FROM tmp", &session).await;
    assert_eq!(response.text(), "3\n");
    // A table that is not the catalog's is chDB's to answer.
    refused(addr, "INSERT INTO nope VALUES (1)", "60").await;
    assert!(files_under(&house, "/data/").is_empty());
}

fn snapshot_of(response: &Response) -> Option<String> {
    response.header("X-Loams-Snapshot-Id").map(str::to_string)
}

/// Rows appear only with their commit: a commit that fails answers an error,
/// and nothing of the insert is visible (its files are orphans for HS1 Task 15).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ack_only_after_commit() {
    let house = lakehouse("ack").await;
    let addr = house.addr();
    ok(addr, KV).await;
    house.flaky.fail.store(u32::MAX, Ordering::SeqCst);
    let response = insert(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &[]).await;
    assert_eq!(
        response.header("X-ClickHouse-Exception-Code"),
        Some("210"),
        "{}",
        response.text()
    );
    assert!(snapshot_of(&response).is_none());
    house.flaky.fail.store(0, Ordering::SeqCst);
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "0\n");
    assert!(
        metadata(&house, "default", "t")
            .await
            .current_snapshot()
            .is_none()
    );
    assert_eq!(files_under(&house, "/data/").len(), 1, "the orphan");

    // A commit the catalog applied but did not confirm is found by its id on the
    // retry, and lands once.
    house.flaky.landed.store(true, Ordering::SeqCst);
    house.flaky.fail.store(1, Ordering::SeqCst);
    let response = inserted(addr, "INSERT INTO t FORMAT CSV", b"2,b\n", &[]).await;
    assert_eq!(written_rows(&response), 1);
    assert_eq!(ok(addr, "SELECT k, s FROM t").await, "2\tb\n");
    assert_eq!(snapshots(&metadata(&house, "default", "t").await), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_with_same_token_writes_once() {
    let house = lakehouse("token").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let token = [("insert_deduplication_token", "batch-1")];
    let first = inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n2,b\n", &token).await;
    assert_eq!(written_rows(&first), 2);
    // The retry, even with other data (the token is the key), writes nothing and
    // names the snapshot that holds the first.
    let retry = inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n2,b\n3,c\n", &token).await;
    assert_eq!(written_rows(&retry), 0);
    assert_eq!(snapshot_of(&retry), snapshot_of(&first));
    // Concurrent retries of one token: one lands.
    let token2 = [("insert_deduplication_token", "batch-2")];
    let tries: Vec<_> = (0..6)
        .map(|_| {
            tokio::spawn(async move {
                insert(addr, "INSERT INTO t FORMAT CSV", b"9,z\n", &token2).await
            })
        })
        .collect();
    let mut written = 0;
    for task in tries {
        let response = task.await.expect("task");
        assert_eq!(response.status, 200, "{}", response.text());
        written += written_rows(&response);
    }
    assert_eq!(written, 1);
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "3\t12\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn block_hash_dedup_window() {
    let house = lakehouse("window").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let window = [("loams_dedup_window", "2")];
    let rows = |response: Response| written_rows(&response);
    assert_eq!(
        rows(inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &window).await),
        1
    );
    // The same block again: held.
    assert_eq!(
        rows(inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &window).await),
        0
    );
    // The same rows in another format or column list are another block.
    assert_eq!(
        rows(inserted(addr, "INSERT INTO t FORMAT TSV", b"1\ta\n", &window).await),
        1
    );
    assert_eq!(
        rows(inserted(addr, "INSERT INTO t FORMAT CSV", b"2,b\n", &window).await),
        1
    );
    // Two blocks later, the first has left the window.
    assert_eq!(
        rows(inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &window).await),
        1
    );
    // An INSERT … SELECT's block is its rows.
    let select = "INSERT INTO t SELECT number + 10, 'q' FROM numbers(3)";
    assert_eq!(rows(inserted(addr, select, b"", &[]).await), 3);
    assert_eq!(rows(inserted(addr, select, b"", &[]).await), 0);
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "7\n");
}

/// `TRUNCATE` then the same load writes the rows again (the local e2e seed's
/// reload, 2026-10-18, which left `nyc.yellow_trips` empty): the block's key
/// from before the `TRUNCATE` is not held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_insert_after_truncate_is_written() {
    let house = lakehouse("retruncate").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let load = |addr| async move {
        written_rows(&inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n2,b\n", &[]).await)
    };
    assert_eq!(load(addr).await, 2);
    // Without a TRUNCATE between them, the same block is a duplicate.
    assert_eq!(load(addr).await, 0);
    ok(addr, "TRUNCATE TABLE t").await;
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "0\n");
    assert_eq!(load(addr).await, 2);
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "2\t3\n");
    assert_eq!(load(addr).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_deduplicate_0_writes_twice() {
    let house = lakehouse("nodedup").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let off = [("insert_deduplicate", "0")];
    for _ in 0..2 {
        let response = inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &off).await;
        assert_eq!(written_rows(&response), 1);
    }
    // The statement's own SETTINGS clause too.
    let response = inserted(
        addr,
        "INSERT INTO t SETTINGS insert_deduplicate = 0 FORMAT CSV",
        b"1,a\n",
        &[],
    )
    .await;
    assert_eq!(written_rows(&response), 1);
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "3\n");
}

/// 16 writers × 50 inserts through two fronts on one catalog (so their
/// commits conflict and rebase): every row lands exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_commits_all_land() {
    let house = lakehouse("concurrent").await;
    let addr = house.addr();
    ok(addr, KV).await;
    // A second front: its own lake and commit queues, the same catalog and bucket.
    let lake = Arc::new(
        Lake::new(
            house.catalog.clone() as Arc<dyn HouseCatalog>,
            LakeRoot::local(),
        )
        .with_store(house.store.clone())
        .with_grants(Arc::new(house.proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: tmp_root("concurrent-spool-2"),
        ..HouseConfig::default()
    };
    let second = serve_with_lake(config, house.pool.clone(), lake)
        .await
        .expect("serves");
    let fronts = [addr, second.local_addr()];
    let started = Instant::now();
    let writers: Vec<_> = (0..16u64)
        .map(|w| {
            let front = fronts[(w % 2) as usize];
            tokio::spawn(async move {
                for i in 0..50u64 {
                    let k = w * 1000 + i;
                    let body = format!("{k},w{w}\n");
                    let response = insert(
                        front,
                        "INSERT INTO t FORMAT CSV",
                        body.as_bytes(),
                        &[("loams_lake_commit_interval_ms", "0")],
                    )
                    .await;
                    assert_eq!(response.status, 200, "{}", response.text());
                }
            })
        })
        .collect();
    for writer in writers {
        writer.await.expect("writer");
    }
    eprintln!("800 inserts in {:?}", started.elapsed());
    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        "800\t800\n"
    );
    assert!(house.flaky.commits.load(Ordering::SeqCst) >= 1);
}

/// Inserts arriving within the interval share one commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_commit_coalesces() {
    let house = lakehouse("group").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let interval = [("loams_lake_commit_interval_ms", "2000")];
    inserted(addr, "INSERT INTO t FORMAT CSV", b"0,first\n", &interval).await;
    let started = Instant::now();
    let burst: Vec<_> = (1..=8u64)
        .map(|k| {
            tokio::spawn(async move {
                let body = format!("{k},b\n");
                insert(addr, "INSERT INTO t FORMAT CSV", body.as_bytes(), &interval).await
            })
        })
        .collect();
    let mut ids = std::collections::HashSet::new();
    for task in burst {
        let response = task.await.expect("task");
        assert_eq!(response.status, 200, "{}", response.text());
        ids.insert(snapshot_of(&response));
    }
    let table = metadata(&house, "default", "t").await;
    assert!(snapshots(&table) <= 3, "{} snapshots", snapshots(&table));
    assert!(ids.len() <= 2, "{ids:?}");
    // They waited for the interval after the first commit.
    assert!(
        started.elapsed() >= Duration::from_millis(1000),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "9\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_for_async_insert_0_acks_early() {
    let house = lakehouse("async").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let interval = ("loams_lake_commit_interval_ms", "4000");
    inserted(addr, "INSERT INTO t FORMAT CSV", b"1,a\n", &[interval]).await;
    let started = Instant::now();
    let response = inserted(
        addr,
        "INSERT INTO t FORMAT CSV",
        b"2,b\n",
        &[
            interval,
            ("async_insert", "1"),
            ("wait_for_async_insert", "0"),
        ],
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_millis(3000),
        "{:?}",
        started.elapsed()
    );
    assert!(
        snapshot_of(&response).is_none(),
        "answered before its commit"
    );
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "1\n");
    assert!(
        common::eventually(Duration::from_secs(15), || {
            std::thread::spawn(move || {
                let r = request(
                    addr,
                    "POST",
                    &target(&[("user", "alice"), ("password", "a")]),
                    &[],
                    b"SELECT count() FROM t",
                );
                r.text() == "2\n"
            })
            .join()
            .unwrap_or(false)
        })
        .await,
        "the queued rows were committed"
    );
}

/// One statement writes at most `lake_insert_max_bytes` (R12.2): past it the
/// INSERT is refused and nothing lands.
/// HE1 R14.14: ClickHouse's own compressed framing over HTTP, as the
/// `clickhouse` Rust crate sends it by default (Laminar's app-server): an
/// `INSERT … FORMAT RowBinary` whose body is LZ4 frames (`decompress=1`), and
/// a read whose answer comes back in frames (`compress=1`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clickhouse_compressed_framing_both_ways() {
    let house = lakehouse("framing").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE fr (id UInt64, name String) ENGINE = MergeTree ORDER BY id",
    )
    .await;
    // RowBinary: UInt64 little-endian, String as a varint length and bytes.
    let mut rows = Vec::new();
    for (id, name) in [(1u64, "one"), (2, "two"), (3, "three")] {
        rows.extend_from_slice(&id.to_le_bytes());
        rows.push(name.len() as u8);
        rows.extend_from_slice(name.as_bytes());
    }
    let mut body = Vec::new();
    loams_house_native::compress::write_frames(&mut body, &rows);
    inserted(
        addr,
        "INSERT INTO fr (id, name) FORMAT RowBinary",
        &body,
        &[("decompress", "1")],
    )
    .await;
    let response = post(
        addr,
        ALICE,
        Some("SELECT id, name FROM fr ORDER BY id FORMAT TSV"),
        b"",
        &[("compress", "1")],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(response.header("Content-Encoding").is_none());
    let mut reader = loams_house_native::wire::Reader::new(&response.body);
    let mut plain = Vec::new();
    while reader.position() < response.body.len() {
        loams_house_native::compress::read_frame(&mut reader, &mut plain).expect("a frame");
    }
    assert_eq!(
        String::from_utf8(plain).expect("utf8"),
        "1\tone\n2\ttwo\n3\tthree\n"
    );
    // A body that is not in frames is refused, and nothing is written.
    let refused = insert(
        addr,
        "INSERT INTO fr (id, name) FORMAT RowBinary",
        &rows,
        &[("decompress", "1")],
    )
    .await;
    assert_ne!(refused.status, 200, "{}", refused.text());
    assert_eq!(ok(addr, "SELECT count() FROM fr FORMAT TSV").await, "3\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_over_the_cap_is_refused() {
    let house = lakehouse_with("cap", |config| config.lake_insert_max_bytes = 64 * 1024).await;
    let addr = house.addr();
    ok(addr, KV).await;
    refused(
        addr,
        "INSERT INTO t SELECT number, toString(number) FROM numbers(100000)",
        "36",
    )
    .await;
    inserted(
        addr,
        "INSERT INTO t SELECT number, toString(number) FROM numbers(100)",
        b"",
        &[],
    )
    .await;
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "100\n");
}

/// HE1 R14.13: `CoalescingMergeTree` (Laminar's `traces_static`): per
/// sorting key, each column's last value that is not `NULL`, merged within an
/// insert and read so with `FINAL`. Every answer is the 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coalescing_merge_tree_keeps_the_last_non_null() {
    let house = lakehouse("coalescing").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE co (k UInt64, a Nullable(String), b Nullable(UInt8), c String) \
         ENGINE = CoalescingMergeTree() ORDER BY k",
    )
    .await;
    ok(
        addr,
        "INSERT INTO co VALUES (1, 'x', NULL, 'c1'), (1, NULL, 5, 'c2'), (2, 'y', NULL, 'd')",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT * FROM co ORDER BY k FORMAT TSV").await,
        "1\tx\t5\tc2\n2\ty\t\\N\td\n"
    );
    ok(
        addr,
        "INSERT INTO co VALUES (1, 'z', NULL, 'c3'), (2, NULL, 7, 'd2')",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT * FROM co FINAL ORDER BY k FORMAT TSV").await,
        "1\tz\t5\tc3\n2\ty\t7\td2\n"
    );
    assert_eq!(ok(addr, "SELECT count() FROM co FORMAT TSV").await, "4\n");
    refused(
        addr,
        "CREATE TABLE co2 (k UInt64, a Nullable(String)) ENGINE = CoalescingMergeTree(a) ORDER BY k",
        "48",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partition_split_files() {
    let house = lakehouse("partitions").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE p (k Int64, s String, d Date) ENGINE = MergeTree \
         PARTITION BY d ORDER BY k",
    )
    .await;
    inserted(
        addr,
        "INSERT INTO p SELECT number, toString(number), toDate('2026-10-01') + number % 3 FROM numbers(30)",
        b"",
        &[],
    )
    .await;
    let files = files_under(&house, "/data/");
    assert_eq!(files.len(), 3, "one file per partition value: {files:?}");
    let table = metadata(&house, "default", "p").await;
    let summary = &table
        .current_snapshot()
        .expect("snapshot")
        .summary()
        .additional_properties;
    assert_eq!(
        summary.get("added-data-files").map(String::as_str),
        Some("3")
    );
    assert_eq!(
        ok(addr, "SELECT d, count() FROM p GROUP BY d ORDER BY d").await,
        "2026-10-01\t10\n2026-10-02\t10\n2026-10-03\t10\n"
    );
    // And by size: a 1 MiB target rolls files.
    ok(addr, KV).await;
    inserted(
        addr,
        "INSERT INTO t SELECT number, randomPrintableASCII(64) FROM numbers(100000)",
        b"",
        &[("loams_lake_target_file_bytes", "1048576")],
    )
    .await;
    let rolled = files_under(&house, "/t-")
        .into_iter()
        .filter(|f| f.contains("/data/"))
        .count();
    assert!(rolled > 1, "{rolled} files");
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "100000\n");
}

/// HE1 R14.8: `BFloat16` columns (Laminar's `Array(BFloat16)` cluster
/// centroids, with their `vector_similarity` index) are stored as Iceberg
/// `float` and read back as `BFloat16`, exactly. Every answer is the 26.9.2.8
/// oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bfloat16_columns_round_trip() {
    let house = lakehouse("bfloat16").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE clusters (id UInt64, centroid Array(BFloat16), one BFloat16, \
         maybe Nullable(BFloat16), INDEX centroid_idx centroid TYPE \
         vector_similarity('hnsw', 'cosineDistance', 3)) ENGINE = ReplacingMergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "INSERT INTO clusters VALUES (1, [0.1, 0.2, 0.3], 1.5, NULL), \
         (2, [1, 0, 3.14159], -2.25, 0.333)",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT id, centroid, one, maybe, toTypeName(centroid), toTypeName(maybe) \
             FROM clusters ORDER BY id FORMAT TSV"
        )
        .await,
        "1\t[0.099609375,0.19921875,0.29882812]\t1.5\t\\N\tArray(BFloat16)\tNullable(BFloat16)\n\
         2\t[1,0,3.140625]\t-2.25\t0.33203125\tArray(BFloat16)\tNullable(BFloat16)\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT id, round(cosineDistance(centroid, [0.1, 0.2, 0.3]::Array(BFloat16)), 4) AS d \
             FROM clusters ORDER BY d LIMIT 1 FORMAT TSV"
        )
        .await,
        "1\t0\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT id, centroid FROM clusters FINAL WHERE has(centroid, toBFloat16(0.1)) FORMAT TSV"
        )
        .await,
        "1\t[0.099609375,0.19921875,0.29882812]\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_table_as_select_and_truncate() {
    let house = lakehouse("ctas").await;
    let addr = house.addr();
    ok(addr, KV).await;
    inserted(
        addr,
        "INSERT INTO t SELECT number, toString(number) FROM numbers(5)",
        b"",
        &[],
    )
    .await;
    // Columns from the query (read through the pinned view), engine and keys
    // as written.
    let response = post(
        addr,
        ALICE,
        None,
        b"CREATE TABLE c ENGINE = MergeTree ORDER BY k AS SELECT k, s, k * 2 AS twice FROM t WHERE k > 1",
        &[],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(written_rows(&response), 3);
    assert_eq!(
        ok(
            addr,
            "SELECT name, type FROM system.columns WHERE table = 'c' ORDER BY position"
        )
        .await,
        "k\tInt64\ns\tString\ntwice\tInt64\n"
    );
    assert_eq!(ok(addr, "SELECT sum(twice) FROM c").await, "18\n");
    // Named columns, and EMPTY AS.
    ok(
        addr,
        "CREATE TABLE c2 (k Int64, s String) ENGINE = MergeTree ORDER BY k AS SELECT 7, 'x'",
    )
    .await;
    assert_eq!(ok(addr, "SELECT k, s FROM c2").await, "7\tx\n");
    ok(
        addr,
        "CREATE TABLE c3 ENGINE = MergeTree ORDER BY k EMPTY AS SELECT k FROM t",
    )
    .await;
    assert_eq!(ok(addr, "SELECT count() FROM c3").await, "0\n");
    // An existing name: 57, unless IF NOT EXISTS (which writes nothing).
    refused(
        addr,
        "CREATE TABLE c ENGINE = MergeTree ORDER BY k AS SELECT 1 AS k",
        "57",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE IF NOT EXISTS c ENGINE = MergeTree ORDER BY k AS SELECT 1 AS k",
    )
    .await;
    assert_eq!(ok(addr, "SELECT count() FROM c").await, "3\n");
    // The query is held to the deny list, and its sources to the grants.
    refused(
        addr,
        "CREATE TABLE bad ENGINE = MergeTree ORDER BY l AS SELECT * FROM file('/etc/passwd', 'LineAsString', 'l String')",
        "344",
    )
    .await;
    assert_eq!(ok(addr, "EXISTS TABLE bad").await, "0\n");

    // TRUNCATE: a snapshot with no files; history stays for time travel.
    let before = metadata(&house, "default", "t")
        .await
        .current_snapshot()
        .expect("snapshot")
        .snapshot_id();
    ok(addr, "TRUNCATE TABLE t").await;
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "0\n");
    let table = metadata(&house, "default", "t").await;
    assert_eq!(
        table
            .current_snapshot()
            .expect("snapshot")
            .summary()
            .operation
            .as_str(),
        "delete"
    );
    let pinned = post(
        addr,
        ALICE,
        None,
        b"SELECT count() FROM t",
        &[("loams_snapshot_id", &before.to_string())],
    )
    .await;
    assert_eq!(pinned.text(), "5\n");
    ok(addr, "TRUNCATE TABLE IF EXISTS nope").await;
    inserted(addr, "INSERT INTO t VALUES (9, 'z')", b"", &[]).await;
    assert_eq!(ok(addr, "SELECT k FROM t").await, "9\n");
    let (code_ro, text) = code(addr, READER, "TRUNCATE TABLE t").await;
    assert_eq!(code_ro.as_deref(), Some("164"), "{text}");
}

/// HE1 R14.7: columns named like a column list's keywords (`index`,
/// `projection`; Laminar's `evaluation_datapoints.index`) are backquoted in
/// every column list the House writes, so `INSERT … SELECT` stages them
/// (Laminar's migrations 19 and 32). Every answer is the 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyword_named_columns_insert_select() {
    let house = lakehouse("keyword-columns").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE ed (id UInt64, `index` UInt64, `projection` String, \
         data String CODEC(ZSTD(3)), INDEX idx (id, `index`) TYPE bloom_filter) \
         ENGINE = ReplacingMergeTree ORDER BY (id, `index`)",
    )
    .await;
    ok(
        addr,
        "INSERT INTO ed VALUES (1, 0, 'p', 'a'), (1, 1, 'q', 'b')",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE ed2 (id UInt64, `index` UInt64, `projection` String, data String) \
         ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(addr, "INSERT INTO ed2 SELECT * FROM ed").await;
    ok(
        addr,
        "INSERT INTO ed2 (id, index, projection, data) WITH x AS (SELECT id + 10 AS id, index, \
         projection, data FROM ed) SELECT id, index, projection, data FROM x",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT id, index, projection, data FROM ed2 ORDER BY id, index FORMAT TSV"
        )
        .await,
        "1\t0\tp\ta\n1\t1\tq\tb\n11\t0\tp\ta\n11\t1\tq\tb\n"
    );
    let shown = ok(addr, "SHOW CREATE TABLE ed2 FORMAT TSVRaw").await;
    assert!(
        shown.contains("`index` UInt64") && shown.contains("`projection` String"),
        "{shown}"
    );
}

/// `loams.house.v1.ExecuteQuery` writes lake tables as HTTP does (§49 §18.2):
/// the summary carries `written_rows` and the snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_insert_answers_written_rows_and_snapshot() {
    use base64::Engine as _;
    use connectrpc::client::{ClientConfig, HttpClient};
    use loams_house_proto::loams::house::v1 as pb;
    use loams_house_proto::loams::house::v1::__buffa::oneof::execute_query_response;
    use loams_house_proto::loams::house::v1::HouseServiceClient;

    let house = lakehouse("api").await;
    let addr = house.addr();
    ok(addr, KV).await;
    let client = |user: &str, password: &str| {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        let uri: http::Uri = format!("http://{addr}").parse().expect("uri");
        HouseServiceClient::new(
            HttpClient::plaintext(),
            ClientConfig::new(uri).with_default_header("authorization", format!("Basic {basic}")),
        )
    };
    let alice = client("alice", "a");
    for sql in [
        "INSERT INTO t VALUES (1, 'a'), (2, 'b')",
        "INSERT INTO t FORMAT CSV\n3,c\n",
        "INSERT INTO t SELECT number + 10, 'n' FROM numbers(2)",
    ] {
        let mut stream = alice
            .execute_query(pb::ExecuteQueryRequest {
                sql: sql.to_string(),
                format: pb::ResultFormat::RESULT_FORMAT_JSON_ROWS.into(),
                read_only: Some(false),
                ..Default::default()
            })
            .await
            .expect("starts");
        let mut summary = None;
        while let Some(message) = stream.message().await.expect("frames") {
            if let Some(execute_query_response::Frame::Summary(s)) =
                message.to_owned_message().frame
            {
                summary = Some(*s);
            }
        }
        let summary = summary.expect("a summary");
        assert!(summary.written_rows >= 1, "{sql}");
        let table = metadata(&house, "default", "t").await;
        assert_eq!(
            summary.snapshot_ids.get("default.t").copied(),
            table.current_snapshot().map(|s| s.snapshot_id()),
            "{sql}"
        );
    }
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "5\t27\n");
    // Without `read_only = false` the API writes nothing.
    let refused = alice
        .execute_query(pb::ExecuteQueryRequest {
            sql: "INSERT INTO t VALUES (4, 'd')".to_string(),
            ..Default::default()
        })
        .await;
    let failed = match refused {
        Err(_) => true,
        Ok(mut stream) => stream.message().await.is_err(),
    };
    assert!(failed, "read_only refuses the write");
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "5\n");
}

/// HE1 R14.9: `EXCHANGE TABLES a AND b` (Laminar's migrations 35 and 48)
/// swaps what the two names hold, `SHOW CREATE` included. Every answer is the
/// 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exchange_tables_swaps_the_names() {
    let house = lakehouse("exchange").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE m1 (id UInt64, body String) ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE m2 (id UInt64, body String, ttl_at DateTime DEFAULT now()) \
         ENGINE = MergeTree ORDER BY id TTL ttl_at + INTERVAL 1 DAY",
    )
    .await;
    ok(addr, "INSERT INTO m1 VALUES (1, 'old')").await;
    ok(
        addr,
        "INSERT INTO m2 (id, body) VALUES (2, 'new'), (3, 'newer')",
    )
    .await;
    ok(addr, "EXCHANGE TABLES m1 AND m2").await;
    assert_eq!(
        ok(
            addr,
            "SELECT 'm1', count(), groupArray(body) FROM m1 FORMAT TSV"
        )
        .await,
        "m1\t2\t['new','newer']\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT 'm2', count(), groupArray(body) FROM m2 FORMAT TSV"
        )
        .await,
        "m2\t1\t['old']\n"
    );
    let shown = ok(addr, "SHOW CREATE TABLE m1 FORMAT TSVRaw").await;
    assert!(
        shown.starts_with("CREATE TABLE default.m1") && shown.contains("ttl_at"),
        "{shown}"
    );
    // Writes land in the table the name now holds.
    ok(addr, "INSERT INTO m2 VALUES (4, 'later')").await;
    assert_eq!(
        ok(
            addr,
            "SELECT groupArray(body) FROM (SELECT body FROM m2 ORDER BY id) FORMAT TSV"
        )
        .await,
        "['old','later']\n"
    );
    ok(addr, "EXCHANGE TABLES m1 AND m1").await;
    refused(addr, "EXCHANGE TABLES m1 AND nope", "60").await;
    refused(addr, "EXCHANGE DICTIONARIES m1 AND m2", "48").await;
    assert_eq!(
        ok(
            addr,
            "SELECT name FROM system.tables WHERE database = 'default' ORDER BY name FORMAT TSV"
        )
        .await,
        "m1\nm2\n"
    );
}

/// HE1 R9.12: the DDL Langfuse v4's migrations send, on lake tables: skip
/// indexes, projections and constraints in the column list, `SAMPLE BY`, a
/// delete `TTL`, `ALIAS` and `MATERIALIZED` columns with lambdas, the query
/// settings ClickHouse takes in `CREATE`'s SETTINGS, and the `ALTER`s that
/// follow, each served as ClickHouse answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lake_tables_take_langfuse_ddl() {
    let house = lakehouse("langfuse-ddl").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE ev (\
            project_id String, trace_id String, start_time DateTime64(6), \
            cost_details Map(LowCardinality(String), Decimal(18, 12)), \
            calculated_input_cost Decimal(18, 12) MATERIALIZED arraySum(mapValues(mapFilter(x -> positionCaseInsensitive(x.1, 'input') > 0, cost_details))), \
            total_cost Decimal(18, 12) ALIAS cost_details['total'], \
            input String CODEC(ZSTD(3)), \
            input_length UInt64 MATERIALIZED lengthUTF8(input), \
            metadata_names Array(String), metadata_values Array(String), \
            event_ts DateTime64(6), is_deleted UInt8, \
            INDEX idx_trace_id trace_id TYPE bloom_filter(0.01) GRANULARITY 1, \
            INDEX idx_fts lower(input) TYPE text(tokenizer = splitByNonAlpha), \
            PROJECTION by_trace (SELECT trace_id, start_time ORDER BY trace_id), \
            CONSTRAINT has_project CHECK notEmpty(project_id)\
         ) ENGINE = ReplacingMergeTree(event_ts, is_deleted) \
         PARTITION BY toYYYYMM(start_time) \
         PRIMARY KEY (project_id, toStartOfMinute(start_time), xxHash32(trace_id)) \
         ORDER BY (project_id, toStartOfMinute(start_time), xxHash32(trace_id), start_time) \
         SAMPLE BY xxHash32(trace_id) \
         TTL toDateTime(start_time) + INTERVAL 30 DAY \
         SETTINGS index_granularity_bytes = '64Mi', enable_full_text_index = 1",
    )
    .await;
    let shown = ok(addr, "SHOW CREATE TABLE ev FORMAT TSVRaw").await;
    for part in [
        "INDEX idx_trace_id",
        "PROJECTION by_trace",
        "CONSTRAINT has_project",
        "SAMPLE BY",
        "TTL",
    ] {
        assert!(shown.contains(part), "{part}: {shown}");
    }
    // A query setting is the statement's, not the table's.
    let created = metadata(&house, "default", "ev").await;
    let settings = created
        .properties()
        .get("loams.ch.settings")
        .cloned()
        .unwrap_or_default();
    assert!(
        settings.contains("index_granularity_bytes")
            && !settings.contains("enable_full_text_index"),
        "{settings}"
    );

    // Rows: one fresh, one past the TTL.
    ok(
        addr,
        "INSERT INTO ev (project_id, trace_id, start_time, cost_details, input, metadata_names, metadata_values, event_ts, is_deleted) \
         SELECT 'p', 't1', now64(6), map('input', 1.5, 'output', 2, 'total', 3.5), 'hello', ['evaluator_id'], ['e1'], now64(6), 0 \
         UNION ALL SELECT 'p', 't2', now64(6) - INTERVAL 40 DAY, map('total', 1), 'old', [], [], now64(6), 0",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT trace_id, total_cost, calculated_input_cost, input_length FROM ev ORDER BY trace_id FORMAT TSV").await,
        "t1\t3.5\t1.5\t5\n",
        "the expired row is not read"
    );
    assert_eq!(ok(addr, "SELECT count() FROM ev FINAL").await, "1\n");
    refused(
        addr,
        "INSERT INTO ev (project_id, trace_id, start_time, event_ts, is_deleted) SELECT '', 't3', now64(6), now64(6), 0",
        "469",
    )
    .await;

    // The ALTERs Langfuse sends, with its trailing settings.
    ok(addr, "ALTER TABLE ev ADD INDEX IF NOT EXISTS idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1 SETTINGS mutations_sync = 2").await;
    ok(
        addr,
        "ALTER TABLE ev MATERIALIZE INDEX IF EXISTS idx_trace_id SETTINGS mutations_sync = 2",
    )
    .await;
    ok(addr, "ALTER TABLE ev DROP INDEX IF EXISTS idx_fts").await;
    ok(addr, "ALTER TABLE ev DROP INDEX IF EXISTS idx_fts").await;
    refused(addr, "ALTER TABLE ev MATERIALIZE INDEX idx_fts", "36").await;
    ok(addr, "ALTER TABLE ev MODIFY SETTING enable_block_number_column = 1, enable_block_offset_column = 1").await;
    ok(
        addr,
        "ALTER TABLE ev ADD COLUMN IF NOT EXISTS evaluator_id String DEFAULT arrayElement(metadata_values, indexOf(metadata_names, 'evaluator_id')) AFTER metadata_values SETTINGS enable_full_text_index = 1",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT evaluator_id FROM ev FORMAT TSV").await,
        "e1\n",
        "older rows read the default"
    );
    ok(
        addr,
        "ALTER TABLE ev ADD COLUMN session_id String DEFAULT '' SETTINGS alter_sync = 2",
    )
    .await;
    ok(
        addr,
        "ALTER TABLE ev MODIFY COLUMN session_id Nullable(String) SETTINGS mutations_sync = 2",
    )
    .await;
    let after = metadata(&house, "default", "ev").await;
    let field = after
        .current_schema()
        .as_struct()
        .fields()
        .iter()
        .find(|f| f.name == "session_id")
        .cloned()
        .expect("a field");
    assert!(!field.required, "Nullable makes the field optional");
    refused(addr, "ALTER TABLE ev MODIFY COLUMN input UInt64", "48").await;
    refused(addr, "ALTER TABLE ev MODIFY COLUMN session_id String", "48").await;
    refused(addr, "ALTER TABLE ev DROP COLUMN project_id", "524").await;
    refused(addr, "ALTER TABLE ev RENAME COLUMN start_time TO st", "524").await;
    ok(addr, "ALTER TABLE ev RENAME COLUMN session_id TO sid").await;
    ok(addr, "ALTER TABLE ev DROP COLUMN sid").await;
    refused(addr, "ALTER TABLE ev DROP COLUMN sid", "10").await;
    ok(
        addr,
        "ALTER TABLE ev MODIFY TTL toDateTime(start_time) + INTERVAL 90 DAY",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT count() FROM ev").await,
        "2\n",
        "a longer TTL reads the row again"
    );
    ok(addr, "ALTER TABLE ev REMOVE TTL").await;
    refused(addr, "ALTER TABLE ev REMOVE TTL", "36").await;
    // Mutations are the mutation path's (HE1 R9.5, tests/mutate.rs).
    ok(addr, "ALTER TABLE ev DELETE WHERE trace_id = 't1'").await;
    refused(
        addr,
        "ALTER TABLE ev DELETE WHERE 1, ADD INDEX i trace_id TYPE minmax",
        "48",
    )
    .await;

    // A materialized view's query.
    ok(addr, "CREATE TABLE core (project_id String, trace_id String, input String) ENGINE = MergeTree ORDER BY project_id").await;
    ok(
        addr,
        "CREATE MATERIALIZED VIEW core_mv TO core AS SELECT project_id, trace_id, input FROM ev",
    )
    .await;
    ok(
        addr,
        "ALTER TABLE core_mv MODIFY QUERY SELECT project_id, trace_id, leftUTF8(input, 2) AS input FROM ev SETTINGS enable_full_text_index = 1",
    )
    .await;
    ok(addr, "INSERT INTO ev (project_id, trace_id, start_time, input, event_ts, is_deleted) SELECT 'p', 't4', now64(6), 'abcdef', now64(6), 0").await;
    assert_eq!(
        ok(addr, "SELECT trace_id, input FROM core FORMAT TSV").await,
        "t4\tab\n"
    );
    refused(
        addr,
        "ALTER TABLE core_mv MODIFY QUERY SELECT project_id, trace_id, input FROM core",
        // R9.16: a new source is taken, but not the view's own target.
        "36",
    )
    .await;
}

/// HE1 R14.6: a column list whose types run over several lines (Laminar's
/// `traces_agg` and `spans_v2`: `SimpleAggregateFunction(f,\n Array(…))`,
/// `Array(Tuple(\n …))`, with a `PROJECTION`) and a default holding a
/// backslash reach ClickHouse's reading of the list as written. Every answer is
/// the 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn column_lists_over_several_lines() {
    let house = lakehouse("multiline-columns").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE agg (id UInt64, project_id UInt64, \
         start_time SimpleAggregateFunction(min, DateTime64(9, 'UTC')), \
         tokens SimpleAggregateFunction(sum, Int64), \
         tags SimpleAggregateFunction(groupUniqArrayArray,\n    Array(String)), \
         statuses SimpleAggregateFunction(groupUniqArrayArray, Array(Enum8('success' = 1,\n    'error' = 2))), \
         metadata SimpleAggregateFunction(maxMap, Map(String, String)), \
         PROJECTION p_start (SELECT * ORDER BY project_id, start_time, id)) \
         ENGINE = AggregatingMergeTree ORDER BY (project_id, id) \
         SETTINGS index_granularity = 8192, deduplicate_merge_projection_mode = 'rebuild'",
    )
    .await;
    ok(
        addr,
        "INSERT INTO agg VALUES (1, 1, '2026-01-01 00:00:02', 3, ['x'], ['success'], {'k':'a'}), \
         (1, 1, '2026-01-01 00:00:01', 4, ['y','x'], ['error'], {'k':'b'}), \
         (2, 1, '2026-01-03 00:00:00', 5, [], [], {})",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT id, min(start_time), sum(tokens), arraySort(groupUniqArrayArray(tags)), \
             arraySort(groupUniqArrayArray(statuses)) FROM agg GROUP BY id ORDER BY id FORMAT TSV"
        )
        .await,
        "1\t2026-01-01 00:00:01.000000000\t7\t['x','y']\t['success','error']\n\
         2\t2026-01-03 00:00:00.000000000\t5\t[]\t[]\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT id, start_time, tokens, arraySort(tags), arraySort(statuses), metadata \
             FROM agg FINAL ORDER BY id FORMAT TSV"
        )
        .await,
        "1\t2026-01-01 00:00:01.000000000\t7\t['x','y']\t['success','error']\t{'k':'b'}\n\
         2\t2026-01-03 00:00:00.000000000\t5\t[]\t[]\t{}\n"
    );
    assert_eq!(
        ok(addr, "DESCRIBE agg FORMAT TSV")
            .await
            .lines()
            .map(|l| l.split('\t').take(2).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>(),
        [
            "id\tUInt64",
            "project_id\tUInt64",
            "start_time\tSimpleAggregateFunction(min, DateTime64(9, \\'UTC\\'))",
            "tokens\tSimpleAggregateFunction(sum, Int64)",
            "tags\tSimpleAggregateFunction(groupUniqArrayArray, Array(String))",
            "statuses\tSimpleAggregateFunction(groupUniqArrayArray, Array(Enum8(\\'success\\' = 1, \\'error\\' = 2)))",
            "metadata\tSimpleAggregateFunction(maxMap, Map(String, String))",
        ]
    );
    ok(
        addr,
        "CREATE TABLE plain (id UInt64, note String DEFAULT 'a\\\\b', events Array(Tuple(\n        \
         timestamp Int64,\n        name String)), PROJECTION p (SELECT id, events ORDER BY id)) \
         ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "INSERT INTO plain (id, events) VALUES (1, [(5, 'e')])",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT id, note, events FROM plain FORMAT TSV").await,
        "1\ta\\\\b\t[(5,'e')]\n"
    );
}

/// HE1 R9.13: plain views (Langfuse's `analytics_*`) and `Distributed` tables.
/// A view is a catalog entry the read path expands over the read views of what
/// it names, nesting included; a `Distributed` table reads and writes its local
/// table; a view takes no rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_views_and_distributed_tables() {
    let house = lakehouse("catalog-views").await;
    let addr = house.addr();
    ok(addr, "CREATE TABLE s (p String, id String, v Float64, ver UInt64, is_deleted UInt8) ENGINE = ReplacingMergeTree(ver, is_deleted) ORDER BY (p, id)").await;
    ok(
        addr,
        "INSERT INTO s VALUES ('a', '1', 1, 1, 0), ('a', '1', 5, 2, 0), ('b', '2', 2, 1, 0)",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW agg AS SELECT p, uniq(id) AS n, max(v) AS top FROM s GROUP BY p",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT * FROM agg ORDER BY p FORMAT TSV").await,
        "a\t1\t5\nb\t1\t2\n"
    );
    // A view over a view, and FINAL inside a view.
    ok(
        addr,
        "CREATE VIEW latest AS SELECT p, sum(v) AS total FROM s FINAL GROUP BY p",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW nested AS SELECT p, top FROM agg WHERE p = 'a'",
    )
    .await;
    assert_eq!(ok(addr, "SELECT * FROM nested FORMAT TSV").await, "a\t5\n");
    assert_eq!(
        ok(addr, "SELECT total FROM latest WHERE p = 'a' FORMAT TSV").await,
        "5\n"
    );
    assert_eq!(
        ok(addr, "DESCRIBE agg FORMAT TSVRaw").await.lines().count(),
        3
    );
    assert!(
        ok(addr, "SHOW CREATE TABLE agg FORMAT TSVRaw")
            .await
            .starts_with("CREATE VIEW agg AS SELECT")
    );
    assert_eq!(
        ok(
            addr,
            "SELECT engine FROM system.tables WHERE name = 'agg' FORMAT TSV"
        )
        .await,
        "View\n"
    );
    refused(addr, "CREATE VIEW agg AS SELECT 1", "57").await;
    ok(addr, "CREATE VIEW IF NOT EXISTS agg AS SELECT 1").await;
    ok(
        addr,
        "CREATE OR REPLACE VIEW agg AS SELECT p, count() AS n FROM s GROUP BY p",
    )
    .await;
    // One INSERT's two versions of ('a', '1') are merged as ClickHouse merges an
    // inserted block (`optimize_on_insert`, R9.4): the 26.9.2.8 oracle answers 1.
    assert_eq!(
        ok(addr, "SELECT n FROM agg WHERE p = 'a' FORMAT TSV").await,
        "1\n"
    );
    refused(addr, "INSERT INTO agg VALUES ('x', 1)", "48").await;
    refused(addr, "ALTER TABLE agg ADD COLUMN z UInt8", "48").await;
    // A view's text is held to the deny list when it is read.
    refused(
        addr,
        "CREATE VIEW bad AS SELECT * FROM file('/etc/passwd')",
        "344",
    )
    .await;

    // Distributed: topology syntax accepted, the local table read and written.
    ok(addr, "CREATE TABLE s_dist ON CLUSTER default AS s ENGINE = Distributed(default, currentDatabase(), s, rand())").await;
    ok(addr, "INSERT INTO s_dist VALUES ('c', '3', 7, 1, 0)").await;
    // Three stored rows: the first INSERT's two versions merged (R9.4), and one.
    assert_eq!(ok(addr, "SELECT count() FROM s FORMAT TSV").await, "3\n");
    assert_eq!(
        ok(addr, "SELECT count() FROM s_dist FINAL FORMAT TSV").await,
        "3\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT engine FROM system.tables WHERE name = 's_dist' FORMAT TSV"
        )
        .await,
        "Distributed\n"
    );
    refused(
        addr,
        "CREATE TABLE d2 AS s ENGINE = Distributed(default, default, nope)",
        "60",
    )
    .await;

    ok(addr, "DROP VIEW nested").await;
    ok(addr, "DROP TABLE latest").await;
    refused(addr, "DROP VIEW nested", "60").await;
    ok(addr, "DROP VIEW IF EXISTS nested").await;
    ok(addr, "DROP TABLE s_dist").await;
}

/// HE1 R14.5: a parameterised view is called as a table function (Laminar's
/// `traces_v0(project_id = …, …)` over `raw_traces_v0(…)`), with literal
/// arguments and with the request's query parameters, through a view of a
/// view; views two others read are made first. Every answer is the 26.9.2.8
/// oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parameterised_views_are_called_as_table_functions() {
    let house = lakehouse("param-views").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE spans (project_id UInt64, trace_id UUID, start_time DateTime64(9, 'UTC'), \
         tokens Int64) ENGINE = MergeTree ORDER BY (project_id, trace_id)",
    )
    .await;
    ok(
        addr,
        "INSERT INTO spans VALUES \
         (1, '00000000-0000-0000-0000-00000000000a', '2026-01-01 00:00:01', 3), \
         (1, '00000000-0000-0000-0000-00000000000a', '2026-01-01 00:00:02', 4), \
         (1, '00000000-0000-0000-0000-00000000000b', '2026-01-02 00:00:00', 5), \
         (2, '00000000-0000-0000-0000-00000000000c', '2026-01-01 00:00:00', 7)",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW raw_v SQL SECURITY INVOKER AS SELECT trace_id, sum(tokens) AS tokens, \
         min(start_time) AS first_start FROM spans WHERE project_id = {project_id:UInt64} \
         AND start_time >= {start:DateTime64(9)} GROUP BY trace_id",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW traces_v SQL SECURITY INVOKER AS SELECT t.trace_id AS id, t.tokens AS tokens \
         FROM raw_v(project_id = {project_id:UInt64}, start = {start:DateTime64(9)}) AS t",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT id, tokens FROM traces_v(project_id = 1, \
             start = '2026-01-01 00:00:02') ORDER BY id FORMAT TSV"
        )
        .await,
        "00000000-0000-0000-0000-00000000000a\t4\n00000000-0000-0000-0000-00000000000b\t5\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT count(), sum(tokens) FROM default.raw_v(project_id = \
             2, start = '2025-01-01 00:00:00') FORMAT TSV"
        )
        .await,
        "1\t7\n"
    );
    // The application's form: the arguments are the request's parameters.
    let response = post(
        addr,
        ALICE,
        None,
        b"SELECT id, tokens FROM traces_v(project_id = {p:UInt64}, start = {s:DateTime64(9)}) \
          ORDER BY id FORMAT TSV",
        &[("param_p", "1"), ("param_s", "2026-01-01 00:00:00")],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(
        response.text(),
        "00000000-0000-0000-0000-00000000000a\t7\n00000000-0000-0000-0000-00000000000b\t5\n"
    );
    // A view two others read (directly and through a view) is made first.
    ok(
        addr,
        "CREATE VIEW a_v AS SELECT trace_id, tokens FROM spans",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW b_v AS SELECT trace_id, sum(tokens) AS total FROM a_v GROUP BY trace_id",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT b.trace_id, b.total, count() FROM b_v AS b INNER JOIN a_v AS a \
             USING (trace_id) GROUP BY b.trace_id, b.total ORDER BY b.trace_id FORMAT TSV"
        )
        .await,
        "00000000-0000-0000-0000-00000000000a\t7\t2\n\
         00000000-0000-0000-0000-00000000000b\t5\t1\n\
         00000000-0000-0000-0000-00000000000c\t7\t1\n"
    );
    // A call of a name that is no view of the namespace is still refused,
    // and so is a view whose body reads outside it.
    refused(addr, "SELECT * FROM nope(x = 1)", "344").await;
    refused(addr, "SELECT * FROM spans(project_id = 1)", "344").await;
    refused(
        addr,
        "CREATE VIEW bad_v AS SELECT * FROM traces_v(project_id = {project_id:UInt64}, \
         start = {start:DateTime64(9)}) WHERE id IN (SELECT toUUID(c1) FROM url('http://127.0.0.1:1/x', CSV))",
        "344",
    )
    .await;
}

/// HE1 R14.11: SQL functions (Laminar's `apply_pii_masks`, `trace_visible`
/// and the two it calls, as migration 66 writes them) are the namespace's:
/// called in queries and in views, replaced, dropped, and held to the deny
/// list. Every answer is the 26.9.2.8 oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sql_functions_are_the_namespaces() {
    let house = lakehouse("sql-functions").await;
    let addr = house.addr();
    for function in [
        r#"CREATE FUNCTION IF NOT EXISTS apply_pii_masks AS (text, masks) -> if(
    empty(masks),
    text,
    arrayStringConcat(
        arrayMap(
            (pii_m, pii_prev_end) -> substring(text, pii_prev_end + 1, tupleElement(pii_m, 1) - pii_prev_end)
                || '[REDACTED_' || upper(tupleElement(pii_m, 3)) || ']',
            masks,
            arrayPopBack(arrayPushFront(arrayMap(pii_x -> tupleElement(pii_x, 2), masks), toUInt32(0)))
        ),
        ''
    ) || substring(text, tupleElement(masks[-1], 2) + 1)
)"#,
        r#"CREATE FUNCTION IF NOT EXISTS pii_json_scalar AS (doc, key) -> if(
    JSONType(doc, key) = 'String',
    JSONExtractString(doc, key),
    JSONExtractRaw(doc, key)
)"#,
        r#"CREATE FUNCTION IF NOT EXISTS pii_str_op AS (actual, op, expected) -> multiIf(
    op = 'eq', actual = expected,
    op = 'ne', actual != expected,
    false
)"#,
        r#"CREATE FUNCTION IF NOT EXISTS trace_visible AS (user_id, metadata, policy) -> if(
    NOT JSONHas(policy, 'traceFilters'),
    isValidJSON(policy),
    if(
        JSONType(policy, 'traceFilters') = 'Array',
        arrayAll(
            pii_f -> multiIf(
                tupleElement(pii_f, 'column') = 'user_id',
                    pii_str_op(user_id, tupleElement(pii_f, 'operator'), tupleElement(pii_f, 'value')),
                tupleElement(pii_f, 'column') = 'metadata',
                    pii_str_op(pii_json_scalar(metadata, tupleElement(pii_f, 'key')), tupleElement(pii_f, 'operator'), tupleElement(pii_f, 'value')),
                false
            ),
            JSONExtract(policy, 'traceFilters', 'Array(Tuple(column String, key String, operator String, value String))')
        ),
        false
    )
)"#,
    ] {
        ok(addr, function).await;
    }
    assert_eq!(
        ok(
            addr,
            "SELECT apply_pii_masks('my name is bob and mail b@x.io', \
             [(11, 14, 'name'), (24, 30, 'email')]) FORMAT TSV"
        )
        .await,
        "my name is [REDACTED_NAME] and mail [REDACTED_EMAIL]\n"
    );
    assert_eq!(
        ok(
            addr,
            r#"SELECT trace_visible('u1', '{"team":"a"}', '{"traceFilters":[{"column":"metadata","key":"team","operator":"eq","value":"a"}]}'), trace_visible('u2', '{}', '{"traceFilters":[{"column":"user_id","key":"","operator":"eq","value":"u1"}]}'), trace_visible('u', '', '{}') FORMAT TSV"#
        )
        .await,
        "1\t0\t1\n"
    );
    // In a view over a lake table, and over another function.
    ok(
        addr,
        "CREATE TABLE notes (id UInt64, body String) ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "INSERT INTO notes VALUES (1, 'call bob'), (2, 'plain')",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW masked AS SELECT id, apply_pii_masks(body, \
         if(id = 1, [(toUInt32(5), toUInt32(8), 'name')], [])) AS body FROM notes",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT id, body FROM masked ORDER BY id FORMAT TSV").await,
        "1\tcall [REDACTED_NAME]\n2\tplain\n"
    );
    ok(addr, "CREATE FUNCTION twice AS (x) -> x * 2").await;
    ok(addr, "CREATE FUNCTION plus_twice AS (x, y) -> twice(x) + y").await;
    assert_eq!(
        ok(
            addr,
            "SELECT plus_twice(id, 1) FROM notes ORDER BY id FORMAT TSV"
        )
        .await,
        "3\n5\n"
    );
    refused(addr, "CREATE FUNCTION twice AS (x) -> x", "609").await;
    ok(addr, "CREATE FUNCTION IF NOT EXISTS twice AS (x) -> x").await;
    ok(addr, "CREATE OR REPLACE FUNCTION twice AS (x) -> x * 3").await;
    assert_eq!(ok(addr, "SELECT plus_twice(2, 1) FORMAT TSV").await, "7\n");
    refused(addr, "CREATE FUNCTION plus AS (a) -> a", "609").await;
    refused(addr, "DROP FUNCTION plus", "610").await;
    refused(addr, "DROP FUNCTION nope", "46").await;
    ok(addr, "DROP FUNCTION IF EXISTS nope").await;
    ok(addr, "DROP FUNCTION twice").await;
    refused(addr, "SELECT plus_twice(2, 1)", "46").await;
    // A lambda reading outside the namespace is refused when it is made.
    refused(
        addr,
        "CREATE FUNCTION leak AS (x) -> (SELECT count() FROM url('http://127.0.0.1:1/x', CSV))",
        "344",
    )
    .await;
    // The read-only user may not make one.
    let (code, text) = code(addr, READER, "CREATE FUNCTION ro AS (x) -> x").await;
    assert!(matches!(code.as_deref(), Some("164" | "497")), "{text}");
}

/// HE1 R14.12: dictionaries over the namespace's own tables (Laminar's
/// `deduped_content_dict`, a `COMPLEX_KEY_CACHE` with `USER`/`PASSWORD`/`DB`
/// and a `QUERY … FINAL`, read by `dictGetOrNull` in a view; a `HASHED`
/// dictionary over a `TABLE` with `WHERE`). Every answer is the 26.9.2.8
/// oracle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictionaries_read_the_namespaces_tables() {
    let house = lakehouse("dictionaries").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE dc (project_id UInt64, content_hash FixedString(32), content String, \
         last_seen_at DateTime64(9, 'UTC') DEFAULT now()) \
         ENGINE = ReplacingMergeTree(last_seen_at) ORDER BY (project_id, content_hash)",
    )
    .await;
    ok(
        addr,
        "INSERT INTO dc (project_id, content_hash, content, last_seen_at) VALUES \
         (1, toFixedString('aa', 32), 'old', '2026-01-01 00:00:00'), \
         (1, toFixedString('aa', 32), 'new', '2026-01-02 00:00:00'), \
         (1, toFixedString('bb', 32), 'bee', '2026-01-01 00:00:00')",
    )
    .await;
    ok(
        addr,
        "CREATE OR REPLACE DICTIONARY dc_dict (project_id UInt64, content_hash String, \
         content String) PRIMARY KEY project_id, content_hash SOURCE(CLICKHOUSE(USER 'alice' \
         PASSWORD 'a' DB 'default' QUERY 'SELECT project_id, toString(content_hash) AS \
         content_hash, content FROM dc FINAL')) LAYOUT(COMPLEX_KEY_CACHE(SIZE_IN_CELLS 1024)) \
         LIFETIME(MIN 1800 MAX 3600)",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE sp (project_id UInt64, id UInt64, hashes Array(FixedString(32))) \
         ENGINE = MergeTree ORDER BY id",
    )
    .await;
    ok(
        addr,
        "INSERT INTO sp VALUES (1, 1, [toFixedString('aa', 32), toFixedString('bb', 32), \
         toFixedString('cc', 32)])",
    )
    .await;
    ok(
        addr,
        "CREATE VIEW sp_v AS SELECT id, arrayMap(h -> coalesce(dictGetOrNull('dc_dict', \
         'content', tuple(project_id, toString(h))), 'null'), hashes) AS contents FROM sp",
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT * FROM sp_v FORMAT TSV").await,
        "1\t['new','bee','null']\n"
    );
    ok(
        addr,
        "CREATE TABLE kv (k UInt64, v String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok(addr, "INSERT INTO kv VALUES (1, 'one'), (2, 'two')").await;
    ok(
        addr,
        "CREATE DICTIONARY kv_dict (k UInt64, v String) PRIMARY KEY k SOURCE(CLICKHOUSE(\
         TABLE 'kv' WHERE 'k > 1' USER 'alice' PASSWORD 'a')) LAYOUT(HASHED()) LIFETIME(0)",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT dictGet('kv_dict', 'v', toUInt64(2)), dictHas('kv_dict', toUInt64(1)), \
             dictGetOrDefault('default.kv_dict', 'v', toUInt64(3), 'none') FORMAT TSV"
        )
        .await,
        "two\t0\tnone\n"
    );
    // A dictionary reads the statement's snapshot: a row written later is
    // found by the next statement.
    ok(addr, "INSERT INTO kv VALUES (3, 'three')").await;
    assert_eq!(
        ok(
            addr,
            "SELECT dictGet('kv_dict', 'v', toUInt64(3)) FORMAT TSV"
        )
        .await,
        "three\n"
    );
    // Credentials are not kept.
    let shown = ok(addr, "SHOW CREATE TABLE dc_dict FORMAT TSVRaw").await;
    assert!(
        shown.starts_with("CREATE DICTIONARY default.dc_dict") && !shown.contains("PASSWORD"),
        "{shown}"
    );
    refused(
        addr,
        "CREATE DICTIONARY kv_dict (k UInt64, v String) PRIMARY KEY k \
         SOURCE(CLICKHOUSE(TABLE 'kv')) LAYOUT(HASHED())",
        "57",
    )
    .await;
    refused(
        addr,
        "CREATE DICTIONARY far (k UInt64, v String) PRIMARY KEY k \
         SOURCE(CLICKHOUSE(HOST 'example.com' TABLE 'kv')) LAYOUT(HASHED())",
        "344",
    )
    .await;
    refused(
        addr,
        "CREATE DICTIONARY far (k UInt64, v String) PRIMARY KEY k \
         SOURCE(MYSQL(host 'example.com' table 'kv')) LAYOUT(HASHED())",
        "344",
    )
    .await;
    refused(
        addr,
        "CREATE DICTIONARY far (k UInt64, v String) PRIMARY KEY k \
         SOURCE(CLICKHOUSE(QUERY 'SELECT * FROM url(''http://127.0.0.1:1/x'', CSV)')) \
         LAYOUT(HASHED())",
        "344",
    )
    .await;
    refused(
        addr,
        "CREATE DICTIONARY far (k UInt64, v String) PRIMARY KEY k \
         SOURCE(CLICKHOUSE(TABLE 'kv')) LAYOUT(RANGE_HASHED())",
        "48",
    )
    .await;
    ok(addr, "DROP DICTIONARY kv_dict").await;
    refused(addr, "DROP DICTIONARY kv_dict", "60").await;
    ok(addr, "DROP DICTIONARY IF EXISTS kv_dict").await;
    refused(addr, "SELECT dictGet('kv_dict', 'v', toUInt64(2))", "36").await;
}

/// HE1 R9.14: readable `AggregatingMergeTree` and `Null`, against the oracle.
/// OpenPanel 2.3.0's inner-table materialized views (agent O's pack: states,
/// `count()` dimensions, `ARRAY JOIN`, and the binding by identity that a
/// `RENAME TABLE` of the source shows) and Langfuse's `Null` → `AggregatingMergeTree`
/// views with `SimpleAggregateFunction` columns. Every answer below is what
/// `clickhouse-server` 26.9.2.8 gave for the same statements (rows joined by `|`),
/// read with `-Merge`, plain aggregates and `FINAL`, before and after `OPTIMIZE`.
/// The oracle needs `allow_dimensions_outside_sorting_key = 1` for the property
/// views (26.7+); the House does not check that rule. `event_property_values_mv`
/// reads its source inside a FROM subquery, which still makes `events` its trigger;
/// `direct_amt` checks that one INSERT's equal keys are merged as ClickHouse's
/// `optimize_on_insert` merges them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aggregating_merge_tree_matches_the_oracle() {
    let house = lakehouse("amt-oracle").await;
    let addr = house.addr();
    for statement in [
        "CREATE TABLE events (project_id String, profile_id String, device_id String, name String, created_at DateTime, duration UInt64, properties Map(String, String)) ENGINE = MergeTree ORDER BY (project_id, created_at)",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS dau_mv ENGINE = AggregatingMergeTree() PARTITION BY toYYYYMMDD(date) ORDER BY (project_id, date) AS SELECT toDate(created_at) as date, uniqState(profile_id) as profile_id, project_id FROM events GROUP BY date, project_id",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS cohort_events_mv ENGINE = AggregatingMergeTree() ORDER BY (project_id, name, created_at, profile_id) AS SELECT project_id, name, toDate(created_at) AS created_at, profile_id, COUNT() AS event_count FROM events WHERE profile_id != device_id GROUP BY project_id, name, created_at, profile_id",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS distinct_event_names_mv ENGINE = AggregatingMergeTree() ORDER BY (project_id, name, created_at) AS SELECT project_id, name, max(created_at) AS created_at, count() AS event_count FROM events GROUP BY project_id, name",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS event_property_values_mv ENGINE = AggregatingMergeTree() ORDER BY (project_id, name, property_key, property_value) SETTINGS allow_dimensions_outside_sorting_key = 1 AS SELECT project_id, name, key_value.keys AS property_key, key_value.values AS property_value, created_at FROM (SELECT project_id, name, untuple(arrayJoin(properties)) AS key_value, max(created_at) AS created_at FROM events GROUP BY project_id, name, key_value) WHERE property_value != '' AND property_key != '' AND property_key NOT IN ('__duration_from', '__properties_from') GROUP BY project_id, name, property_key, property_value, created_at",
        "INSERT INTO events VALUES ('p1', 'u1', 'd1', 'sign_up', '2026-10-15 10:00:00', 5, {'a':'1','b':'2'}), ('p1', 'u2', 'd2', 'screen_view', '2026-10-15 11:00:00', 7, {'a':'3'}), ('p1', 'u1', 'd1', 'screen_view', '2026-10-16 09:00:00', 1, {}), ('p1', 'd3', 'd3', 'screen_view', '2026-10-16 09:30:00', 2, {'c':''})",
        "INSERT INTO events VALUES ('p1', 'u1', 'd1', 'screen_view', '2026-10-16 12:00:00', 4, {'a':'1'}), ('p2', 'u3', 'd4', 'sign_up', '2026-10-17 08:00:00', 9, {'z':'9'}), ('p1', 'u2', 'd2', 'screen_view', '2026-10-15 23:00:00', 3, {'a':'4'})",
        "RENAME TABLE events TO events_20251123",
        "CREATE TABLE events (project_id String, profile_id String, device_id String, name String, created_at DateTime, duration UInt64, properties Map(String, String)) ENGINE = MergeTree ORDER BY (project_id, created_at)",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS profile_event_summary_mv ENGINE = AggregatingMergeTree() PARTITION BY toYYYYMM(event_date) ORDER BY (project_id, profile_id, name, event_date) AS SELECT project_id, profile_id, name, toStartOfDay(created_at) AS event_date, countState() AS event_count, minState(created_at) AS first_event_time, maxState(created_at) AS last_event_time, sumState(duration) AS total_duration FROM events WHERE profile_id != device_id GROUP BY project_id, profile_id, name, event_date",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS profile_event_property_summary_mv ENGINE = AggregatingMergeTree() PARTITION BY toYYYYMM(event_date) ORDER BY (project_id, profile_id, name, property_key, event_date) SETTINGS allow_dimensions_outside_sorting_key = 1 AS SELECT project_id, profile_id, name, property_key, property_value, toStartOfDay(created_at) AS event_date, countState() AS event_count, minState(created_at) AS first_event_time, maxState(created_at) AS last_event_time FROM events ARRAY JOIN mapKeys(properties) AS property_key, mapValues(properties) AS property_value WHERE profile_id != device_id AND property_key != '' AND property_value != '' GROUP BY project_id, profile_id, name, property_key, property_value, event_date",
        "INSERT INTO events VALUES ('p1', 'u1', 'd1', 'screen_view', '2026-10-18 10:00:00', 5, {'a':'1','b':'2'}), ('p1', 'u1', 'd1', 'screen_view', '2026-10-18 11:00:00', 6, {'a':'1'}), ('p1', 'd3', 'd3', 'screen_view', '2026-10-18 12:00:00', 2, {'a':'x'})",
        "INSERT INTO events VALUES ('p1', 'u1', 'd1', 'screen_view', '2026-10-18 15:00:00', 8, {'b':'2'}), ('p1', 'u2', 'd2', 'sign_up', '2026-10-19 10:00:00', 1, {'a':'5','c':''})",
        "CREATE TABLE traces_null (project_id String, id String, start_time DateTime64(3), name Nullable(String), tags Array(String), input String, event_ts DateTime64(3)) ENGINE = Null()",
        "CREATE TABLE traces_all_amt (project_id String, id String, start_time SimpleAggregateFunction(min, DateTime64(3)), name SimpleAggregateFunction(anyLast, Nullable(String)), tags SimpleAggregateFunction(groupUniqArrayArray, Array(String)), input AggregateFunction(argMax, String, DateTime64(3))) ENGINE = AggregatingMergeTree() ORDER BY (project_id, id)",
        "CREATE MATERIALIZED VIEW traces_all_amt_mv TO traces_all_amt AS SELECT tn.project_id AS project_id, tn.id AS id, min(tn.start_time) AS start_time, anyLast(tn.name) AS name, groupUniqArrayArray(tn.tags) AS tags, argMaxState(tn.input, if(tn.input <> '', tn.event_ts, toDateTime64(0, 3))) AS input FROM traces_null tn GROUP BY project_id, id",
        "INSERT INTO traces_null VALUES ('p', 't1', '2026-10-18 10:00:00.000', 'first', ['a'], 'in-1', '2026-10-18 10:00:00.000'), ('p', 't2', '2026-10-18 10:01:00.000', NULL, ['b'], '', '2026-10-18 10:01:00.000')",
        "INSERT INTO traces_null VALUES ('p', 't1', '2026-10-18 09:00:00.000', 'first', ['c', 'a'], 'in-2', '2026-10-18 10:05:00.000'), ('p', 't2', '2026-10-18 10:02:00.000', 'second', [], 'late', '2026-10-18 10:03:00.000')",
        "CREATE TABLE direct_amt (k String, c AggregateFunction(count, UInt64), m SimpleAggregateFunction(max, UInt64)) ENGINE = AggregatingMergeTree ORDER BY k",
        "INSERT INTO direct_amt SELECT if(number % 2 = 0, 'a', 'b'), initializeAggregation('countState', number), number FROM numbers(6)",
    ] {
        ok(addr, statement).await;
    }
    let expected: &[(&str, &str)] = &[
        (
            "SELECT project_id, profile_id, name, event_date, countMerge(event_count), minMerge(first_event_time), maxMerge(last_event_time), sumMerge(total_duration) FROM profile_event_summary_mv GROUP BY project_id, profile_id, name, event_date ORDER BY ALL",
            "p1\tu1\tscreen_view\t2026-10-18 00:00:00\t3\t2026-10-18 10:00:00\t2026-10-18 15:00:00\t19|p1\tu2\tsign_up\t2026-10-19 00:00:00\t1\t2026-10-19 10:00:00\t2026-10-19 10:00:00\t1|",
        ),
        (
            "SELECT project_id, profile_id, name, property_key, property_value, event_date, countMerge(event_count), maxMerge(last_event_time) FROM profile_event_property_summary_mv GROUP BY ALL ORDER BY ALL",
            "p1\tu1\tscreen_view\ta\t1\t2026-10-18 00:00:00\t2\t2026-10-18 11:00:00|p1\tu1\tscreen_view\tb\t2\t2026-10-18 00:00:00\t2\t2026-10-18 15:00:00|p1\tu2\tsign_up\ta\t5\t2026-10-19 00:00:00\t1\t2026-10-19 10:00:00|",
        ),
        (
            "SELECT project_id, profile_id, name, event_date, finalizeAggregation(event_count), finalizeAggregation(total_duration) FROM profile_event_summary_mv FINAL ORDER BY ALL",
            "p1\tu1\tscreen_view\t2026-10-18 00:00:00\t3\t19|p1\tu2\tsign_up\t2026-10-19 00:00:00\t1\t1|",
        ),
        (
            "SELECT project_id, date, uniqMerge(profile_id) FROM dau_mv GROUP BY project_id, date ORDER BY ALL",
            "p1\t2026-10-15\t2|p1\t2026-10-16\t2|p2\t2026-10-17\t1|",
        ),
        (
            "SELECT name, sum(event_count) FROM cohort_events_mv WHERE project_id = 'p1' GROUP BY name ORDER BY ALL",
            "screen_view\t4|sign_up\t1|",
        ),
        (
            "SELECT name, max(created_at), sum(event_count) FROM distinct_event_names_mv WHERE project_id = 'p1' GROUP BY name ORDER BY name",
            "screen_view\t2026-10-16 12:00:00\t5|sign_up\t2026-10-15 10:00:00\t1|",
        ),
        ("SELECT count() FROM events_20251123", "7|"),
        (
            "SELECT id, min(start_time), anyLast(name), arraySort(groupUniqArrayArray(tags)), argMaxMerge(input) FROM traces_all_amt GROUP BY project_id, id ORDER BY id",
            "t1\t2026-10-18 09:00:00.000\tfirst\t['a','c']\tin-2|t2\t2026-10-18 10:01:00.000\tsecond\t['b']\tlate|",
        ),
        (
            "SELECT id, start_time, name, arraySort(tags), finalizeAggregation(input) FROM traces_all_amt FINAL ORDER BY id",
            "t1\t2026-10-18 09:00:00.000\tfirst\t['a','c']\tin-2|t2\t2026-10-18 10:01:00.000\tsecond\t['b']\tlate|",
        ),
        ("SELECT count() FROM traces_null", "0|"),
        (
            "SELECT distinct property_key, max(created_at) as created_at FROM event_property_values_mv WHERE project_id = 'p1' GROUP BY property_key ORDER BY created_at DESC, property_key ASC LIMIT 50000",
            "a\t2026-10-16 12:00:00|b\t2026-10-15 10:00:00|",
        ),
        (
            "SELECT k, finalizeAggregation(c), m FROM direct_amt ORDER BY k",
            "a\t3\t4|b\t3\t5|",
        ),
        ("SELECT count() FROM direct_amt", "2|"),
    ];
    let check = |when: &'static str| async move {
        for (read, want) in expected {
            let got = ok(addr, &format!("{read} FORMAT TSV"))
                .await
                .replace('\n', "|");
            assert_eq!(&got, want, "{when}: {read}");
        }
    };
    check("before OPTIMIZE").await;
    ok(addr, "OPTIMIZE TABLE profile_event_summary_mv FINAL").await;
    ok(addr, "OPTIMIZE TABLE traces_all_amt FINAL").await;
    check("after OPTIMIZE").await;
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM profile_event_summary_mv FORMAT TSV"
        )
        .await,
        "2\n"
    );
    // Dropping an inner-table view drops its table.
    ok(addr, "DROP VIEW dau_mv").await;
    assert_eq!(ok(addr, "EXISTS TABLE dau_mv FORMAT TSV").await, "0\n");
    ok(addr, "DROP TABLE profile_event_summary_mv").await;
    assert_eq!(
        ok(addr, "EXISTS TABLE profile_event_summary_mv FORMAT TSV").await,
        "0\n"
    );
    refused(addr, "CREATE MATERIALIZED VIEW p ENGINE = AggregatingMergeTree ORDER BY a POPULATE AS SELECT 1 AS a", "48").await;
}

/// HE1 R9.15: `SAMPLE` on a lake table reads every row and `_sample_factor` is
/// 1, so Langfuse's sampled filter options answer exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sample_reads_every_row_of_a_lake_table() {
    let house = lakehouse("sample").await;
    let addr = house.addr();
    ok(addr, "CREATE TABLE ev (p String, t String, u String) ENGINE = MergeTree ORDER BY (p, xxHash32(t)) SAMPLE BY xxHash32(t)").await;
    ok(
        addr,
        "INSERT INTO ev VALUES ('p', 'a', 'u1'), ('p', 'b', 'u1'), ('p', 'c', 'u2')",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT e.u AS value, toUInt64(round(count() * any(e._sample_factor))) AS count \
             FROM ev e SAMPLE 1000000 WHERE e.p = 'p' GROUP BY value ORDER BY value FORMAT TSV",
        )
        .await,
        "u1\t2\nu2\t1\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM ev SAMPLE 1/10 OFFSET 1/2 FORMAT TSV"
        )
        .await,
        "3\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT _sample_factor FROM ev SAMPLE 0.5 LIMIT 1 FORMAT TSV"
        )
        .await,
        "1\n"
    );
}

/// D1572, HE1 R9.61: `DateTime64` with more than 6 digits is a
/// `long` of nanoseconds in a format v2 table, and reads, filters, orders,
/// converts and merges as ClickHouse does. The answers were measured on the
/// 26.9.2.8 oracle (2026-10-22), before and after `OPTIMIZE … FINAL`; they
/// include a time before the epoch, ClickHouse's `toDate` of it (1970-01-01),
/// zones on the column, a column added by `ALTER` with a nanosecond default,
/// `ReplacingMergeTree` versions one nanosecond apart, and
/// `SimpleAggregateFunction(min|max, DateTime64(9))`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nanosecond_times_match_the_oracle() {
    let house = lakehouse("nanos-oracle").await;
    let addr = house.addr();
    for statement in [
        "CREATE TABLE ns (k UInt32, ts DateTime64(9), utc DateTime64(9, 'UTC'), kol Nullable(DateTime64(9, 'Asia/Kolkata')), ver DateTime64(9)) ENGINE = ReplacingMergeTree(ver) PARTITION BY toDate(ts) ORDER BY (k, ts)",
        "INSERT INTO ns VALUES (1, '2026-10-22 03:05:45.123456789', '2026-10-22 03:05:45.123456789', '2026-10-22 08:35:45.000000001', '2026-10-22 00:00:00.000000001'), (2, '1969-12-31 23:59:59.999999999', '1969-12-31 23:59:59.999999999', NULL, '2026-10-22 00:00:00.000000001'), (3, '2026-10-21 23:59:59.999999999', '2026-10-22 00:00:00.000000000', '2026-10-22 05:30:00.000000000', '2026-10-22 00:00:00.000000001')",
        "INSERT INTO ns VALUES (1, '2026-10-22 03:05:45.123456789', '2026-10-22 03:05:45.123456790', '2026-10-22 08:35:45.000000002', '2026-10-22 00:00:00.000000002')",
        "ALTER TABLE ns ADD COLUMN later DateTime64(9, 'UTC') DEFAULT toDateTime64('2026-01-01 00:00:00.000000005', 9, 'UTC')",
        "INSERT INTO ns (k, ts, utc, kol, ver, later) VALUES (4, '2026-10-22 03:05:45.123456788', '2026-10-22 03:05:45.123456788', NULL, '2026-10-22 00:00:00.000000001', '2026-10-23 01:02:03.000000009')",
        "CREATE TABLE sa (id String, start SimpleAggregateFunction(min, DateTime64(9)), end SimpleAggregateFunction(max, DateTime64(9))) ENGINE = AggregatingMergeTree ORDER BY id",
        "INSERT INTO sa VALUES ('t', '2026-10-22 03:05:45.000000003', '2026-10-22 03:05:45.000000007')",
        "INSERT INTO sa VALUES ('t', '2026-10-22 03:05:45.000000001', '2026-10-22 03:05:45.000000009')",
    ] {
        ok(addr, statement).await;
    }
    for name in ["ns", "sa"] {
        let table = metadata(&house, "default", name).await;
        assert_eq!(
            table.format_version(),
            loams_house_lake::spec::FormatVersion::V2,
            "{name}"
        );
        for field in table.current_schema().as_struct().fields() {
            if field.name != "k" && field.name != "id" {
                assert_eq!(
                    *field.field_type,
                    loams_house_lake::spec::Type::Primitive(
                        loams_house_lake::spec::PrimitiveType::Long
                    ),
                    "{name}.{}",
                    field.name
                );
            }
        }
    }
    let check = |when: &'static str, expected: &'static [(&'static str, &'static str)]| async move {
        for (read, want) in expected {
            let got = ok(addr, &format!("{read} FORMAT TSV"))
                .await
                .replace('\n', "|");
            assert_eq!(&got, want, "{when}: {read}");
        }
    };
    check(
        "as written",
        &[
            ("SELECT k, ts, utc, kol, ver, later FROM ns FINAL ORDER BY k, ts", "1\t2026-10-22 03:05:45.123456789\t2026-10-22 03:05:45.123456790\t2026-10-22 08:35:45.000000002\t2026-10-22 00:00:00.000000002\t2026-01-01 00:00:00.000000005|2\t1969-12-31 23:59:59.999999999\t1969-12-31 23:59:59.999999999\t\\N\t2026-10-22 00:00:00.000000001\t2026-01-01 00:00:00.000000005|3\t2026-10-21 23:59:59.999999999\t2026-10-22 00:00:00.000000000\t2026-10-22 05:30:00.000000000\t2026-10-22 00:00:00.000000001\t2026-01-01 00:00:00.000000005|4\t2026-10-22 03:05:45.123456788\t2026-10-22 03:05:45.123456788\t\\N\t2026-10-22 00:00:00.000000001\t2026-10-23 01:02:03.000000009|"),
            ("SELECT k, ts, ver FROM ns ORDER BY k, ver", "1\t2026-10-22 03:05:45.123456789\t2026-10-22 00:00:00.000000001|1\t2026-10-22 03:05:45.123456789\t2026-10-22 00:00:00.000000002|2\t1969-12-31 23:59:59.999999999\t2026-10-22 00:00:00.000000001|3\t2026-10-21 23:59:59.999999999\t2026-10-22 00:00:00.000000001|4\t2026-10-22 03:05:45.123456788\t2026-10-22 00:00:00.000000001|"),
            ("SELECT count() FROM ns WHERE ts > '2026-10-22 03:05:45.123456788'", "2|"),
            ("SELECT k FROM ns FINAL WHERE utc = toDateTime64('2026-10-22 03:05:45.123456790', 9, 'UTC') ORDER BY k", "1|"),
            ("SELECT k, toUnixTimestamp64Nano(ts), toDate(ts), toStartOfHour(utc), toString(kol), toDate(kol) FROM ns FINAL ORDER BY k", "1\t1792638345123456789\t2026-10-22\t2026-10-22 03:00:00\t2026-10-22 08:35:45.000000002\t2026-10-22|2\t-1\t1970-01-01\t1970-01-01 00:00:00\t\\N\t\\N|3\t1792627199999999999\t2026-10-21\t2026-10-22 00:00:00\t2026-10-22 05:30:00.000000000\t2026-10-22|4\t1792638345123456788\t2026-10-22\t2026-10-22 03:00:00\t\\N\t\\N|"),
            ("SELECT k FROM ns FINAL ORDER BY ts DESC, k", "1|4|3|2|"),
            ("SELECT toTypeName(ts), toTypeName(utc), toTypeName(kol), toTypeName(later) FROM ns LIMIT 1", "DateTime64(9)\tDateTime64(9, \\'UTC\\')\tNullable(DateTime64(9, \\'Asia/Kolkata\\'))\tDateTime64(9, \\'UTC\\')|"),
            ("SELECT k, ts + INTERVAL 1 NANOSECOND, dateDiff('nanosecond', ts, utc) FROM ns FINAL ORDER BY k", "1\t2026-10-22 03:05:45.123456790\t1|2\t1970-01-01 00:00:00.000000000\t0|3\t2026-10-22 00:00:00.000000000\t1|4\t2026-10-22 03:05:45.123456789\t0|"),
            ("SELECT k FROM ns FINAL WHERE kol BETWEEN '2026-10-22 05:30:00' AND '2026-10-22 08:35:45.000000002' ORDER BY k", "1|3|"),
            ("SELECT toDate(ts) AS d, count() FROM ns FINAL GROUP BY d ORDER BY d", "1970-01-01\t1|2026-10-21\t1|2026-10-22\t2|"),
            ("SELECT id, start, end FROM sa FINAL", "t\t2026-10-22 03:05:45.000000001\t2026-10-22 03:05:45.000000009|"),
            ("SELECT min(start), max(end) FROM sa", "2026-10-22 03:05:45.000000001\t2026-10-22 03:05:45.000000009|"),
        ],
    )
    .await;
    ok(addr, "OPTIMIZE TABLE ns FINAL").await;
    ok(addr, "OPTIMIZE TABLE sa FINAL").await;
    check(
        "after OPTIMIZE",
        &[
            ("SELECT k, ts, ver FROM ns ORDER BY k, ver", "1\t2026-10-22 03:05:45.123456789\t2026-10-22 00:00:00.000000002|2\t1969-12-31 23:59:59.999999999\t2026-10-22 00:00:00.000000001|3\t2026-10-21 23:59:59.999999999\t2026-10-22 00:00:00.000000001|4\t2026-10-22 03:05:45.123456788\t2026-10-22 00:00:00.000000001|"),
            ("SELECT count() FROM ns WHERE ts > '2026-10-22 03:05:45.123456788'", "1|"),
        ],
    )
    .await;
}

/// HE1 R9.16: the DDL SigNoz's migrations send beside Langfuse's: `ON CLUSTER`
/// everywhere, Distributed tables altered with their local ones, views `TO` a
/// table with its column list, `MODIFY QUERY` onto another source, a key
/// `Enum8` extended, `MATERIALIZE COLUMN`, partitions Iceberg cannot take, and
/// `final` as a request setting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signoz_ddl_on_lake_tables() {
    let house = lakehouse("signoz-ddl").await;
    let addr = house.addr();
    ok(addr, "CREATE TABLE keys ON CLUSTER cluster (name String, tagType Enum8('tag' = 1, 'resource' = 2), seen Int64, ver UInt64) ENGINE = ReplacingMergeTree(ver) PARTITION BY toDate(seen) ORDER BY (name, tagType)").await;
    ok(addr, "CREATE TABLE dist_keys ON CLUSTER cluster AS keys ENGINE = Distributed(cluster, default, keys, rand())").await;
    ok(addr, "ALTER TABLE keys ON CLUSTER cluster MODIFY COLUMN IF EXISTS tagType Enum8('tag' = 1, 'resource' = 2, 'scope' = 3)").await;
    ok(addr, "ALTER TABLE dist_keys ON CLUSTER cluster MODIFY COLUMN IF EXISTS tagType Enum8('tag' = 1, 'resource' = 2, 'scope' = 3)").await;
    refused(
        addr,
        "ALTER TABLE keys MODIFY COLUMN tagType Enum8('tag' = 1)",
        "524",
    )
    .await;
    ok(addr, "ALTER TABLE keys ON CLUSTER cluster ADD COLUMN IF NOT EXISTS flags UInt32 DEFAULT 0 CODEC(ZSTD(1))").await;
    ok(addr, "ALTER TABLE dist_keys ON CLUSTER cluster ADD COLUMN IF NOT EXISTS flags UInt32 DEFAULT 0 CODEC(ZSTD(1))").await;
    // HE1 R9.71: the Distributed table's columns are its local table's as
    // altered, so a client that inserts through them (SigNoz's collector)
    // finds `flags` and the extended Enum.
    assert_eq!(
        ok(addr, "SELECT name, type FROM system.columns WHERE table = 'dist_keys' AND name IN ('tagType', 'flags') ORDER BY name FORMAT TSV").await,
        "flags\tUInt32\ntagType\tEnum8(\\'tag\\' = 1, \\'resource\\' = 2, \\'scope\\' = 3)\n"
    );
    ok(
        addr,
        "ALTER TABLE keys ON CLUSTER cluster MATERIALIZE COLUMN flags",
    )
    .await;
    ok(addr, "ALTER TABLE keys ON CLUSTER cluster MODIFY TTL toDateTime(seen) + INTERVAL 15 DAY SETTINGS materialize_ttl_after_modify = 0").await;
    ok(addr, "INSERT INTO keys (name, tagType, seen, ver) SELECT 'a', 'scope', toUnixTimestamp(now()), 1").await;
    ok(addr, "INSERT INTO keys (name, tagType, seen, ver) SELECT 'a', 'scope', toUnixTimestamp(now()), 2").await;
    // `final` on the request reads the merge view.
    let response = insert(
        addr,
        "SELECT count(), max(ver) FROM dist_keys FORMAT TSV",
        b"",
        &[("final", "1")],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.text(), "1\t2\n");
    assert_eq!(ok(addr, "SELECT count() FROM keys FORMAT TSV").await, "2\n");
    // A view TO a table, with its column list; MODIFY QUERY onto another source.
    ok(
        addr,
        "CREATE TABLE names (name String) ENGINE = MergeTree ORDER BY name",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE keys_v2 (name String, tagType String) ENGINE = MergeTree ORDER BY name",
    )
    .await;
    ok(addr, "CREATE MATERIALIZED VIEW names_mv ON CLUSTER cluster TO names (name String CODEC(ZSTD(1))) AS SELECT name FROM keys").await;
    ok(addr, "ALTER TABLE names_mv ON CLUSTER cluster MODIFY QUERY SELECT upper(name) AS name FROM keys_v2").await;
    ok(
        addr,
        "INSERT INTO keys (name, tagType, seen, ver) SELECT 'old', 'tag', toUnixTimestamp(now()), 1",
    )
    .await;
    ok(addr, "INSERT INTO keys_v2 VALUES ('new', 'tag')").await;
    assert_eq!(
        ok(addr, "SELECT name FROM names ORDER BY name FORMAT TSV").await,
        "NEW\n"
    );
    ok(
        addr,
        "DROP TABLE IF EXISTS names_mv ON CLUSTER cluster SYNC",
    )
    .await;
    ok(addr, "DROP TABLE IF EXISTS dist_keys ON CLUSTER cluster").await;
    assert_eq!(ok(addr, "EXISTS TABLE dist_keys FORMAT TSV").await, "0\n");
}

/// HE1 R9.63 (interim; Variant in HE4): a `JSON` column is an Iceberg `string`
/// of the value's JSON text, cast back to its type on read, so subcolumns
/// (`resource.\`service.name\``), typed paths (`message String`), dynamic
/// paths' types, `JSONAllPaths`, a column added by `ALTER` and `FINAL` read
/// as ClickHouse reads them. The answers were measured on the 26.9.2.8 oracle
/// (2026-10-22) for SigNoz's column types, before and after `OPTIMIZE … FINAL`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn json_columns_match_the_oracle() {
    let house = lakehouse("json-oracle").await;
    let addr = house.addr();
    ok(addr, "CREATE TABLE logs (ts UInt64, id String, body String, resource JSON(max_dynamic_paths=100)) ENGINE = ReplacingMergeTree ORDER BY (id, ts)").await;
    ok(addr, "INSERT INTO logs VALUES (1, 'a', 'one', '{\"service.name\":\"checkout\",\"host\":{\"name\":\"h1\"},\"n\":3}'), (2, 'b', 'two', '{\"service.name\":\"cart\",\"flag\":true}')").await;
    ok(
        addr,
        "ALTER TABLE logs ADD COLUMN body_v2 JSON(max_dynamic_paths=0, message String) AFTER body",
    )
    .await;
    inserted(addr, "INSERT INTO logs FORMAT JSONEachRow", "{\"ts\":3,\"id\":\"c\",\"body\":\"three\",\"body_v2\":{\"message\":\"hello\",\"level\":\"warn\"},\"resource\":{\"service\":{\"name\":\"checkout\"},\"tags\":[\"x\",\"y\"]}}".as_bytes(), &[]).await;
    ok(addr, "INSERT INTO logs VALUES (1, 'a', 'one again', '{\"message\":\"replaced\"}', '{\"service.name\":\"checkout\",\"n\":4}')").await;
    let logs = metadata(&house, "default", "logs").await;
    for field in logs.current_schema().as_struct().fields() {
        if field.name == "resource" || field.name == "body_v2" {
            assert_eq!(
                *field.field_type,
                loams_house_lake::spec::Type::Primitive(
                    loams_house_lake::spec::PrimitiveType::String
                ),
                "{}",
                field.name
            );
        }
    }
    let check = |when: &'static str, expected: &'static [(&'static str, &'static str)]| async move {
        for (read, want) in expected {
            let got = ok(addr, &format!("{read} FORMAT TSV"))
                .await
                .replace('\n', "|");
            assert_eq!(&got, want, "{when}: {read}");
        }
    };
    check(
        "as written",
        &[
            ("SELECT id, resource.`service.name`, resource.`service.name`::String, resource.host.name, resource.n, body_v2.message, toTypeName(body_v2.message) FROM logs FINAL ORDER BY id", "a\tcheckout\tcheckout\t\\N\t4\treplaced\tString|b\tcart\tcart\t\\N\t\\N\t\tString|c\tcheckout\tcheckout\t\\N\t\\N\thello\tString|"),
            ("SELECT id FROM logs FINAL WHERE resource.`service.name` IS NOT NULL AND resource.`service.name`::String = 'checkout' ORDER BY id", "a|c|"),
            ("SELECT resource.`service.name`::String AS s, count() FROM logs GROUP BY s ORDER BY s", "cart\t1|checkout\t3|"),
            ("SELECT id, arraySort(JSONAllPaths(resource)), toString(resource), toString(body_v2) FROM logs FINAL ORDER BY id", "a\t['n','service.name']\t{\"n\":4,\"service\":{\"name\":\"checkout\"}}\t{\"message\":\"replaced\"}|b\t['flag','service.name']\t{\"flag\":true,\"service\":{\"name\":\"cart\"}}\t{\"message\":\"\"}|c\t['service.name','tags']\t{\"service\":{\"name\":\"checkout\"},\"tags\":[\"x\",\"y\"]}\t{\"level\":\"warn\",\"message\":\"hello\"}|"),
            ("SELECT id, dynamicType(resource.n), dynamicType(resource.flag), dynamicType(resource.tags), toTypeName(resource) FROM logs FINAL ORDER BY id", "a\tInt64\tNone\tNone\tJSON(max_dynamic_paths=100)|b\tNone\tBool\tNone\tJSON(max_dynamic_paths=100)|c\tNone\tNone\tArray(Nullable(String))\tJSON(max_dynamic_paths=100)|"),
            ("SELECT toString(multiIf(resource.`service.name` IS NOT NULL, resource.`service.name`::String, NULL)) AS svc, count() FROM logs FINAL GROUP BY svc ORDER BY svc", "cart\t1|checkout\t2|"),
        ],
    )
    .await;
    ok(addr, "OPTIMIZE TABLE logs FINAL").await;
    check(
        "after OPTIMIZE",
        &[
            ("SELECT id, resource.`service.name`, resource.`service.name`::String, resource.host.name, resource.n, body_v2.message, toTypeName(body_v2.message) FROM logs FINAL ORDER BY id", "a\tcheckout\tcheckout\t\\N\t4\treplaced\tString|b\tcart\tcart\t\\N\t\\N\t\tString|c\tcheckout\tcheckout\t\\N\t\\N\thello\tString|"),
            ("SELECT id FROM logs FINAL WHERE resource.`service.name` IS NOT NULL AND resource.`service.name`::String = 'checkout' ORDER BY id", "a|c|"),
            ("SELECT resource.`service.name`::String AS s, count() FROM logs GROUP BY s ORDER BY s", "cart\t1|checkout\t2|"),
            ("SELECT id, arraySort(JSONAllPaths(resource)), toString(resource), toString(body_v2) FROM logs FINAL ORDER BY id", "a\t['n','service.name']\t{\"n\":4,\"service\":{\"name\":\"checkout\"}}\t{\"message\":\"replaced\"}|b\t['flag','service.name']\t{\"flag\":true,\"service\":{\"name\":\"cart\"}}\t{\"message\":\"\"}|c\t['service.name','tags']\t{\"service\":{\"name\":\"checkout\"},\"tags\":[\"x\",\"y\"]}\t{\"level\":\"warn\",\"message\":\"hello\"}|"),
            ("SELECT id, dynamicType(resource.n), dynamicType(resource.flag), dynamicType(resource.tags), toTypeName(resource) FROM logs FINAL ORDER BY id", "a\tInt64\tNone\tNone\tJSON(max_dynamic_paths=100)|b\tNone\tBool\tNone\tJSON(max_dynamic_paths=100)|c\tNone\tNone\tArray(Nullable(String))\tJSON(max_dynamic_paths=100)|"),
            ("SELECT toString(multiIf(resource.`service.name` IS NOT NULL, resource.`service.name`::String, NULL)) AS svc, count() FROM logs FINAL GROUP BY svc ORDER BY svc", "cart\t1|checkout\t2|"),
        ],
    )
    .await;
}

/// HE1 R9.18: a column list with `--` and `/* */` comments between and after its
/// entries and a comma after its last `INDEX`, the shape of Langfuse v4's
/// migration 0022 (written here, not copied: no application source is
/// vendored). The House makes the table ClickHouse 26.9.2.8 makes: `DESCRIBE`'s
/// names, types and default kinds as the oracle answered them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn comments_and_trailing_commas_in_create_table() {
    let house = lakehouse("ddl-comments").await;
    let addr = house.addr();
    ok(addr, "CREATE TABLE runs (\n    -- identifiers\n    `id` String,\n    `run_id` String,\n\n    -- an error\n    `error` Nullable(String),\n     /* times */\n    `created_at` DateTime64(3) DEFAULT now(),\n    `input` Nullable(String) CODEC(ZSTD(3)), -- json\n    `meta` Map(LowCardinality(String), String),\n    `event_ts` DateTime64(3),\n    `is_deleted` UInt8,\n\n    -- lookups\n    INDEX idx_run run_id TYPE bloom_filter(0.001) GRANULARITY 1,\n) ENGINE = ReplacingMergeTree(event_ts, is_deleted)\nORDER BY (run_id, id)").await;
    let described = ok(addr, "DESCRIBE runs FORMAT TSV").await;
    let got: Vec<String> = described
        .lines()
        .map(|l| l.split('\t').take(3).collect::<Vec<_>>().join("\t"))
        .collect();
    assert_eq!(
        got.join("|"),
        "id\tString\t|run_id\tString\t|error\tNullable(String)\t|created_at\tDateTime64(3)\tDEFAULT|input\tNullable(String)\t|meta\tMap(LowCardinality(String), String)\t|event_ts\tDateTime64(3)\t|is_deleted\tUInt8\t"
    );
    assert!(
        ok(addr, "SHOW CREATE TABLE runs FORMAT TSVRaw")
            .await
            .contains("INDEX idx_run")
    );
}
