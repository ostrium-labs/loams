//! Lake reads and `system` tables (HS1 Task 11, §49 §7), over the ClickHouse
//! HTTP interface with sealed worker processes that read a `file://` bucket
//! through `house-cache`, as `loams-fabric house` runs them.
//!
//! Tables are made with DDL (Task 10); their rows are Parquet files written here
//! with Iceberg field ids and committed as `append` snapshots with
//! `loams_house_lake::append` (Task 12 writes them from `INSERT`).

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arrow::array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int32Array,
    Int64Array, StringArray, TimestampMicrosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use bytes::Bytes;
use common::http::{Response, request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, LakeTable, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_lake::append::{NewDataFile, fast_append};
use loams_house_lake::spec::{NestedField, PrimitiveType, Schema as IcebergSchema, Struct, Type};
use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent};
use loams_store::Store;
use parquet::arrow::ArrowWriter;

/// A value of a test row.
#[derive(Debug, Clone)]
enum V {
    I(i64),
    F(f64),
    S(&'static str),
    O(String),
    Null,
}

struct Lakehouse {
    handle: HouseHandle,
    lake: Arc<Lake>,
    catalog: Arc<MemCatalog>,
    store: Store,
    proxy: HouseCache,
    pool: WorkerPool,
}

impl Lakehouse {
    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }
}

/// A House whose catalog, `house-cache` and front share one `file://` bucket.
/// Users: `alice` in namespace 2, `bob` in namespace 3.
async fn lakehouse(test: &str) -> Lakehouse {
    let dir = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", dir.display());
    let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
    let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
    let upstream = Upstream::open(&url, StoreOptions::default(), RangeCacheConfig::default())
        .await
        .expect("upstream");
    let proxy = HouseCache::new(Arc::new(OneStore(Arc::new(upstream))));
    proxy.record_requests(true);
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test))
        .with_forward_target(Arc::new(proxy.clone()));
    let pool = WorkerPool::start(small(3), Arc::new(launcher))
        .await
        .expect("the pool starts");
    let lake = Arc::new(
        Lake::new(catalog.clone() as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store.clone())
            .with_grants(Arc::new(proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![
            UserMap::dev("alice", "a", 2, false),
            UserMap::dev("bob", "b", 3, false),
        ],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool.clone(), lake.clone())
        .await
        .expect("serves");
    Lakehouse {
        handle,
        lake,
        catalog,
        store,
        proxy,
        pool,
    }
}

async fn as_user(
    addr: SocketAddr,
    user: (&str, &str),
    sql: &str,
    params: &[(&str, &str)],
) -> Response {
    let mut all = vec![("user", user.0), ("password", user.1)];
    all.extend_from_slice(params);
    let target = target(&all);
    let body = sql.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

const ALICE: (&str, &str) = ("alice", "a");
const BOB: (&str, &str) = ("bob", "b");

async fn ok_as(addr: SocketAddr, user: (&str, &str), sql: &str, params: &[(&str, &str)]) -> String {
    let response = as_user(addr, user, sql, params).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
}

async fn ok(addr: SocketAddr, sql: &str) -> String {
    ok_as(addr, ALICE, sql, &[]).await
}

async fn code_as(
    addr: SocketAddr,
    user: (&str, &str),
    sql: &str,
    params: &[(&str, &str)],
) -> (Option<String>, String) {
    let response = as_user(addr, user, sql, params).await;
    (
        response
            .header("X-ClickHouse-Exception-Code")
            .map(str::to_string),
        response.text(),
    )
}

async fn refused(addr: SocketAddr, sql: &str, params: &[(&str, &str)], want: &str) -> String {
    let (code, text) = code_as(addr, ALICE, sql, params).await;
    assert_eq!(code.as_deref(), Some(want), "{sql}: {text}");
    text
}

fn namespace_of(user: (&str, &str)) -> u64 {
    if user.0 == "bob" { 3 } else { 2 }
}

async fn load(house: &Lakehouse, namespace: u64, database: &str, name: &str) -> LakeTable {
    house
        .catalog
        .load_table(&TableIdent::new(
            Lake::namespace(namespace, database),
            name.to_string(),
        ))
        .await
        .unwrap_or_else(|err| panic!("{database}.{name}: {err}"))
}

/// The Arrow array of one Iceberg primitive column.
fn array(ty: &PrimitiveType, values: &[&V]) -> (DataType, ArrayRef) {
    let int = |v: &V| match v {
        V::I(i) => Some(*i),
        V::Null => None,
        other => panic!("{other:?} is not an integer"),
    };
    let text = |v: &V| match v {
        V::S(s) => Some(s.to_string()),
        V::O(s) => Some(s.clone()),
        V::Null => None,
        other => panic!("{other:?} is not a string"),
    };
    let float = |v: &V| match v {
        V::F(f) => Some(*f),
        V::I(i) => Some(*i as f64),
        V::Null => None,
        other => panic!("{other:?} is not a number"),
    };
    match ty {
        PrimitiveType::Int => (
            DataType::Int32,
            Arc::new(Int32Array::from_iter(
                values.iter().map(|v| int(v).map(|i| i as i32)),
            )),
        ),
        PrimitiveType::Long => (
            DataType::Int64,
            Arc::new(Int64Array::from_iter(values.iter().map(|v| int(v)))),
        ),
        PrimitiveType::Boolean => (
            DataType::Boolean,
            Arc::new(BooleanArray::from_iter(
                values.iter().map(|v| int(v).map(|i| i != 0)),
            )),
        ),
        PrimitiveType::Float => (
            DataType::Float32,
            Arc::new(Float32Array::from_iter(
                values.iter().map(|v| float(v).map(|f| f as f32)),
            )),
        ),
        PrimitiveType::Double => (
            DataType::Float64,
            Arc::new(Float64Array::from_iter(values.iter().map(|v| float(v)))),
        ),
        PrimitiveType::String => (
            DataType::Utf8,
            Arc::new(StringArray::from_iter(values.iter().map(|v| text(v)))),
        ),
        PrimitiveType::Date => (
            DataType::Date32,
            Arc::new(Date32Array::from_iter(
                values.iter().map(|v| int(v).map(|i| i as i32)),
            )),
        ),
        PrimitiveType::Timestamptz => (
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            Arc::new(
                TimestampMicrosecondArray::from_iter(values.iter().map(|v| int(v)))
                    .with_timezone("UTC"),
            ),
        ),
        PrimitiveType::Decimal { precision, scale } => {
            let p = *precision as u8;
            let s = *scale as i8;
            (
                DataType::Decimal128(p, s),
                Arc::new(
                    Decimal128Array::from_iter(values.iter().map(|v| int(v).map(i128::from)))
                        .with_precision_and_scale(p, s)
                        .expect("decimal"),
                ),
            )
        }
        other => panic!("no test array for {other:?}"),
    }
}

static FILES: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis() as i64
}

/// Appends `rows` (in the table's column order) as one Parquet file and one
/// snapshot; answers the snapshot id.
async fn append(
    house: &Lakehouse,
    namespace: u64,
    database: &str,
    name: &str,
    rows: &[Vec<V>],
) -> i64 {
    let table = load(house, namespace, database, name).await;
    let metadata = &table.metadata;
    let schema = metadata.current_schema();
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (i, field) in schema.as_struct().fields().iter().enumerate() {
        let Type::Primitive(ty) = field.field_type.as_ref() else {
            panic!("{}: only primitive columns in these tests", field.name);
        };
        let values: Vec<&V> = rows.iter().map(|row| &row[i]).collect();
        let (data_type, column) = array(ty, &values);
        fields.push(
            Field::new(&field.name, data_type, !field.required).with_metadata(HashMap::from([(
                "PARQUET:field_id".to_string(),
                field.id.to_string(),
            )])),
        );
        columns.push(column);
    }
    let arrow_schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(arrow_schema.clone(), columns).expect("batch");
    let mut bytes = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut bytes, arrow_schema, None).expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    let location = format!(
        "{}/data/{}-{}.parquet",
        metadata.location(),
        std::process::id(),
        FILES.fetch_add(1, Ordering::SeqCst)
    );
    let root = LakeRoot::local();
    let key = |location: &str| root.relative(location).expect("in the bucket").to_string();
    let size = bytes.len() as u64;
    house
        .store
        .put(&key(&location), Bytes::from(bytes))
        .await
        .expect("put");
    let parent = match metadata.current_snapshot() {
        Some(snapshot) => Some(
            house
                .store
                .get(&key(snapshot.manifest_list()))
                .await
                .expect("list")
                .0,
        ),
        None => None,
    };
    let append = fast_append(
        metadata,
        parent.as_deref(),
        vec![NewDataFile {
            location,
            record_count: rows.len() as u64,
            size_bytes: size,
            partition: Struct::empty(),
        }],
        now_ms(),
    )
    .await
    .expect("append");
    for (location, bytes) in append.files {
        house.store.put(&key(&location), bytes).await.expect("put");
    }
    house
        .catalog
        .commit(&table.ident, append.requirements, append.updates)
        .await
        .expect("commit");
    append.snapshot_id
}

fn kv(range: std::ops::Range<i64>) -> Vec<Vec<V>> {
    range
        .map(|k| vec![V::I(k), V::O(format!("s{k}"))])
        .collect()
}

/// No worker asked `house-cache` for a listing: pinned reads never list (R1.2).
fn assert_no_listing(house: &Lakehouse) {
    let listed: Vec<String> = house
        .proxy
        .requests_seen()
        .into_iter()
        .filter(|r| r.target.contains("list-type") || r.method != "GET" && r.method != "HEAD")
        .map(|r| format!("{} {}", r.method, r.target))
        .collect();
    assert!(listed.is_empty(), "{listed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pin_is_stable_while_commits_land() {
    let house = lakehouse("pin-stable").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    // A table with no snapshot reads no rows.
    assert_eq!(ok(addr, "SELECT count() FROM t").await, "0\n");
    append(&house, 2, "default", "t", &kv(0..3)).await;
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "3\t3\n");

    // A slow read pinned before a commit lands answers the pinned snapshot.
    let slow = tokio::spawn(as_user(
        addr,
        ALICE,
        "SELECT count(), sum(k) FROM t WHERE sleepEachRow(0.5) = 0",
        &[],
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    append(&house, 2, "default", "t", &kv(3..5)).await;
    let answered = slow.await.expect("slow");
    assert_eq!(answered.status, 200, "{}", answered.text());
    assert_eq!(answered.text(), "3\t3\n", "the pin held");
    // The next statement takes the new snapshot.
    assert_eq!(ok(addr, "SELECT count(), sum(k) FROM t").await, "5\t10\n");
    assert_eq!(
        ok(addr, "SELECT s FROM t WHERE k = 4").await,
        "s4\n",
        "rows read back as written"
    );
    assert_no_listing(&house);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn time_travel_by_snapshot_and_timestamp() {
    let house = lakehouse("time-travel").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    let mut snapshots = Vec::new();
    for round in 0..3 {
        let id = append(&house, 2, "default", "t", &kv(round * 10..round * 10 + 10)).await;
        let table = load(&house, 2, "default", "t").await;
        let at = table
            .metadata
            .snapshot_by_id(id)
            .expect("snapshot")
            .timestamp_ms();
        snapshots.push((id, at));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let count = |params: Vec<(&'static str, String)>| async move {
        let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        ok_as(addr, ALICE, "SELECT count() FROM t", &params).await
    };
    assert_eq!(count(vec![]).await, "30\n");
    let (first, first_at) = snapshots[0];
    let (second, second_at) = snapshots[1];
    assert_eq!(
        count(vec![("loams_snapshot_id", first.to_string())]).await,
        "10\n"
    );
    assert_eq!(
        count(vec![("loams_snapshot_id", format!("default.t={second}"))]).await,
        "20\n"
    );
    assert_eq!(
        count(vec![("loams_as_of", second_at.to_string())]).await,
        "20\n"
    );
    assert_eq!(
        count(vec![("loams_as_of", first_at.to_string())]).await,
        "10\n"
    );
    assert_eq!(
        count(vec![("loams_as_of", "2000-01-01 00:00:00".to_string())]).await,
        "0\n",
        "before the first snapshot there is nothing"
    );
    // Two tables at their own snapshots in one statement.
    ok(
        addr,
        "CREATE TABLE u (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    let u1 = append(&house, 2, "default", "u", &kv(0..1)).await;
    append(&house, 2, "default", "u", &kv(1..2)).await;
    assert_eq!(
        ok_as(
            addr,
            ALICE,
            "SELECT (SELECT count() FROM t), (SELECT count() FROM u)",
            &[("loams_snapshot_id", &format!("t={first},u={u1}"))]
        )
        .await,
        "10\t1\n"
    );
    // A session keeps the setting.
    let session = [("session_id", "tt")];
    ok_as(
        addr,
        ALICE,
        &format!("SET loams_snapshot_id = {first}"),
        &session,
    )
    .await;
    assert_eq!(
        ok_as(addr, ALICE, "SELECT count() FROM t", &session).await,
        "10\n"
    );
    // An older snapshot reads the schema of its time.
    ok(addr, "ALTER TABLE t ADD COLUMN n UInt32 DEFAULT 7").await;
    assert_eq!(
        ok(addr, "SELECT count(), sum(n) FROM t").await,
        "30\t210\n",
        "rows written before the column read its default"
    );
    assert_eq!(
        ok_as(
            addr,
            ALICE,
            "SELECT * FROM t ORDER BY k LIMIT 1",
            &[("loams_snapshot_id", &first.to_string())]
        )
        .await,
        "0\ts0\n"
    );
    // Refusals.
    refused(
        addr,
        "SELECT count() FROM t",
        &[("loams_snapshot_id", "12345")],
        "36",
    )
    .await;
    refused(
        addr,
        "SELECT count() FROM t",
        &[("loams_snapshot_id", "1"), ("loams_as_of", "1")],
        "36",
    )
    .await;
    refused(
        addr,
        "SELECT count() FROM t",
        &[("loams_as_of", "yesterday")],
        "36",
    )
    .await;
    // A statement's own SETTINGS clause is chDB's, which knows no Loams setting.
    refused(
        addr,
        &format!("SELECT count() FROM t SETTINGS loams_snapshot_id = {first}"),
        &[],
        "115",
    )
    .await;
    assert_no_listing(&house);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn qualified_name_outside_namespace_is_60() {
    let house = lakehouse("other-namespace").await;
    let addr = house.addr();
    ok_as(
        addr,
        BOB,
        "CREATE TABLE secret (k Int64, s String) ENGINE = MergeTree ORDER BY k",
        &[],
    )
    .await;
    ok_as(addr, BOB, "CREATE DATABASE private", &[]).await;
    ok_as(
        addr,
        BOB,
        "CREATE TABLE private.t (k Int64, s String) ENGINE = MergeTree ORDER BY k",
        &[],
    )
    .await;
    append(&house, namespace_of(BOB), "default", "secret", &kv(0..5)).await;
    append(&house, namespace_of(BOB), "private", "t", &kv(0..5)).await;
    assert_eq!(
        ok_as(addr, BOB, "SELECT count() FROM secret", &[]).await,
        "5\n"
    );
    assert_eq!(
        ok_as(addr, BOB, "SELECT count() FROM private.t", &[]).await,
        "5\n"
    );

    // Alice's namespace has a `default` too, but not Bob's table in it.
    for sql in [
        "SELECT * FROM secret",
        "SELECT * FROM default.secret",
        "SELECT * FROM `default`.`secret`",
        "SELECT 1 FROM numbers(1) WHERE 1 IN (SELECT k FROM default.secret)",
    ] {
        refused(addr, sql, &[], "60").await;
    }
    // A database only Bob has is unknown to Alice: ClickHouse's 81.
    refused(addr, "SELECT * FROM private.t", &[], "81").await;
    refused(addr, "SELECT * FROM ns_3.secret", &[], "81").await;
    // The catalog's own Iceberg names are not ClickHouse names.
    refused(addr, "SELECT * FROM `ns_3`.`secret`", &[], "81").await;
    // Even a worker that read Bob's table before is not Alice's: workers are
    // bound to one namespace, and Alice's never made a view of it.
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM system.tables WHERE name IN ('secret', 't')"
        )
        .await,
        "0\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_table_reads() {
    let house = lakehouse("external").await;
    let addr = house.addr();
    // Another writer's table in the namespace's catalog, outside the House's
    // `ns/` tree: no `loams.ch.*` properties.
    house.lake.ensure_root(2).await.expect("root");
    let schema = IcebergSchema::builder()
        .with_fields(vec![
            NestedField::required(1, "k", Type::Primitive(PrimitiveType::Long)).into(),
            NestedField::optional(2, "label", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(3, "amount", Type::Primitive(PrimitiveType::Double)).into(),
        ])
        .build()
        .expect("schema");
    let creation = TableCreation::builder()
        .name("events".to_string())
        .location("s3://local/external/events".to_string())
        .schema(schema)
        .properties(HashMap::new())
        .build();
    house
        .catalog
        .create_table(&NamespaceIdent::new("ns_2".to_string()), creation)
        .await
        .expect("external table");
    append(
        &house,
        2,
        "default",
        "events",
        &[
            vec![V::I(1), V::S("a"), V::F(1.5)],
            vec![V::I(2), V::Null, V::F(2.5)],
            vec![V::I(3), V::S("c"), V::Null],
        ],
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT k, label, amount FROM events ORDER BY k").await,
        "1\ta\t1.5\n2\t\\N\t2.5\n3\tc\t\\N\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT toTypeName(k), toTypeName(label) FROM events LIMIT 1"
        )
        .await,
        "Int64\tNullable(String)\n"
    );
    assert_eq!(
        ok(addr, "SELECT engine, total_rows FROM system.tables WHERE database = 'default' AND name = 'events'").await,
        "Iceberg\t3\n"
    );
    refused(addr, "INSERT INTO events VALUES (4, 'd', 1)", &[], "497").await;
    assert_no_listing(&house);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_tables_list_namespace_tables_only() {
    let house = lakehouse("system-tables").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t1 (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok(addr, "CREATE DATABASE d COMMENT 'mine'").await;
    ok(
        addr,
        "CREATE TABLE d.t2 (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok_as(
        addr,
        BOB,
        "CREATE TABLE theirs (k Int64, s String) ENGINE = MergeTree ORDER BY k",
        &[],
    )
    .await;
    append(&house, 2, "default", "t1", &kv(0..4)).await;
    append(&house, 2, "default", "t1", &kv(4..6)).await;
    // A statement that reads a lake table first leaves views behind in its
    // worker; the next statements must not see them.
    assert_eq!(ok(addr, "SELECT count() FROM t1").await, "6\n");

    let own = "database NOT IN ('system', 'information_schema', 'INFORMATION_SCHEMA')";
    assert_eq!(
        ok(
            addr,
            &format!("SELECT database, name FROM system.tables WHERE {own} ORDER BY 1, 2")
        )
        .await,
        "d\tt2\ndefault\tt1\n"
    );
    assert_eq!(ok(addr, "SHOW TABLES").await, "t1\n");
    assert_eq!(ok(addr, "SHOW TABLES FROM d").await, "t2\n");
    assert_eq!(
        ok_as(addr, ALICE, "SHOW TABLES", &[("database", "d")]).await,
        "t2\n"
    );
    refused(addr, "SHOW TABLES FROM nope", &[], "81").await;
    assert_eq!(
        ok(addr, "SHOW DATABASES").await,
        "INFORMATION_SCHEMA\nd\ndefault\ninformation_schema\nsystem\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT name, engine, comment FROM system.databases WHERE name = 'd'"
        )
        .await,
        "d\tAtomic\tmine\n"
    );
    // chDB's own tables are still listed (those the deny list allows).
    assert_eq!(
        ok(
            addr,
            "SELECT count() > 10 FROM system.tables WHERE database = 'system'"
        )
        .await,
        "1\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM system.tables WHERE database = 'system' AND name = 'disks'"
        )
        .await,
        "0\n"
    );
    // Nothing of the worker: no view databases, no paths.
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM system.tables WHERE startsWith(database, '__loams') OR \
             notEmpty(data_paths) OR metadata_path != '' OR (engine = 'View' AND database \
             NOT IN ('information_schema', 'INFORMATION_SCHEMA'))"
        )
        .await,
        "0\n"
    );
    assert_eq!(
        ok(addr, "SELECT count() FROM system.databases WHERE data_path != '' OR metadata_path != '' OR startsWith(name, '__loams')").await,
        "0\n"
    );
    // system.parts: one row per data file.
    assert_eq!(
        ok(addr, "SELECT database, table, partition, sum(rows), count() FROM system.parts GROUP BY 1, 2, 3").await,
        "default\tt1\ttuple()\t6\t2\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT count() FROM system.parts WHERE path != '' OR disk_name != 'default'"
        )
        .await,
        "0\n"
    );
    // system.processes shows the House user and no host details.
    assert_eq!(
        ok(
            addr,
            "SELECT user, os_user, client_hostname, toString(address) FROM system.processes"
        )
        .await,
        "alice\t\t\t::\n"
    );
    let shown = ok(addr, "SHOW PROCESSLIST").await;
    assert!(shown.contains("alice"), "{shown}");
    // system.errors keeps the counts and drops the engine's words.
    let (code, _) = code_as(addr, ALICE, "SELECT * FROM t_missing", &[]).await;
    assert_eq!(code.as_deref(), Some("60"));
    assert_eq!(
        ok(addr, "SELECT count() FROM system.errors WHERE last_error_message != '' OR notEmpty(last_error_trace)").await,
        "0\n"
    );
    // A session's temporary table is listed as ClickHouse lists it.
    let session = [("session_id", "sys")];
    ok_as(
        addr,
        ALICE,
        "CREATE TEMPORARY TABLE tmp (x UInt8)",
        &session,
    )
    .await;
    assert_eq!(
        ok_as(
            addr,
            ALICE,
            "SELECT name FROM system.tables WHERE is_temporary",
            &session
        )
        .await,
        "tmp\n"
    );
    // Bob sees only his own.
    assert_eq!(
        ok_as(
            addr,
            BOB,
            &format!("SELECT database, name FROM system.tables WHERE {own}"),
            &[]
        )
        .await,
        "default\ttheirs\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_tables_engine_is_clickhouse_engine() {
    let house = lakehouse("system-engines").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE r (k Int64, v Int64, s String) ENGINE = ReplacingMergeTree(v) \
         ORDER BY (k, s) COMMENT 'versions'",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE m (k Int64, s String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE sm (k Int64, n Int64, s String) ENGINE = SummingMergeTree ORDER BY k",
    )
    .await;
    append(
        &house,
        2,
        "default",
        "r",
        &[
            vec![V::I(1), V::I(1), V::S("a")],
            vec![V::I(1), V::I(2), V::S("a")],
        ],
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT name, engine, sorting_key, primary_key, total_rows, comment FROM \
             system.tables WHERE database = 'default' ORDER BY name"
        )
        .await,
        "m\tMergeTree\tk\tk\t0\t\nr\tReplacingMergeTree\tk, s\tk, s\t2\tversions\nsm\tSummingMergeTree\tk\tk\t0\t\n"
    );
    let full = ok(
        addr,
        "SELECT engine_full FROM system.tables WHERE name = 'r'",
    )
    .await;
    assert!(
        full.starts_with("ReplacingMergeTree(v) ORDER BY (k, s)"),
        "{full}"
    );
    let ddl = ok(
        addr,
        "SELECT create_table_query FROM system.tables WHERE name = 'r' FORMAT TSVRaw",
    )
    .await;
    assert!(ddl.starts_with("CREATE TABLE r"), "{ddl}");
    assert_eq!(
        ok(addr, "SHOW FULL TABLES").await,
        "m\tMergeTree\nr\tReplacingMergeTree\nsm\tSummingMergeTree\n"
    );

    // FINAL reads the merge view; without it the unmerged rows (§49 §6.2).
    assert_eq!(ok(addr, "SELECT count() FROM r").await, "2\n");
    assert_eq!(ok(addr, "SELECT k, v, s FROM r FINAL").await, "1\t2\ta\n");
    assert_eq!(
        ok(addr, "SELECT k, v FROM r SETTINGS final = 1").await,
        "1\t2\n"
    );
    append(
        &house,
        2,
        "default",
        "r",
        &[vec![V::I(1), V::I(0), V::S("a")]],
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT v FROM r FINAL").await,
        "2\n",
        "the greatest version wins"
    );
    append(
        &house,
        2,
        "default",
        "sm",
        &[
            vec![V::I(1), V::I(2), V::S("x")],
            vec![V::I(1), V::I(3), V::S("x")],
            vec![V::I(2), V::I(0), V::S("y")],
        ],
    )
    .await;
    assert_eq!(
        ok(addr, "SELECT k, n FROM sm FINAL ORDER BY k").await,
        "1\t5\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_columns_types_are_clickhouse_types() {
    let house = lakehouse("system-columns").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE typed (id UInt64, small UInt8, label LowCardinality(String), \
         kind Enum8('a' = 1, 'b' = 2), at DateTime, maybe Nullable(UInt32) COMMENT 'opt', \
         day Date) ENGINE = MergeTree PRIMARY KEY id ORDER BY (id, at)",
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT name, type, position, is_in_partition_key, is_in_sorting_key, \
             is_in_primary_key, comment FROM system.columns WHERE table = 'typed' ORDER BY \
             position"
        )
        .await,
        "id\tUInt64\t1\t0\t1\t1\t\n\
         small\tUInt8\t2\t0\t0\t0\t\n\
         label\tLowCardinality(String)\t3\t0\t0\t0\t\n\
         kind\tEnum8(\\'a\\' = 1, \\'b\\' = 2)\t4\t0\t0\t0\t\n\
         at\tDateTime\t5\t0\t1\t0\t\n\
         maybe\tNullable(UInt32)\t6\t0\t0\t0\topt\n\
         day\tDate\t7\t0\t0\t0\t\n"
    );
    // The rows read back in those types, through Iceberg's.
    append(
        &house,
        2,
        "default",
        "typed",
        &[vec![
            V::I(18_000_000_000),
            V::I(200),
            V::S("x"),
            V::S("b"),
            V::I(1_700_000_000_000_000),
            V::Null,
            V::I(19_000),
        ]],
    )
    .await;
    assert_eq!(
        ok(
            addr,
            "SELECT toTypeName(id), toTypeName(small), toTypeName(label), toTypeName(kind), \
             toTypeName(at), toTypeName(maybe), toTypeName(day) FROM typed"
        )
        .await,
        "UInt64\tUInt8\tLowCardinality(String)\tEnum8(\\'a\\' = 1, \\'b\\' = 2)\tDateTime\tNullable(UInt32)\tDate\n"
    );
    assert_eq!(
        ok(
            addr,
            "SELECT id, small, label, kind, at, maybe, day FROM typed"
        )
        .await,
        "18000000000\t200\tx\tb\t2023-11-14 22:13:20\t\\N\t2022-01-08\n"
    );
    assert_eq!(
        ok(addr, "DESCRIBE TABLE (SELECT * FROM typed) FORMAT TSV")
            .await
            .lines()
            .map(|l| l.split('\t').take(2).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>(),
        vec![
            "id UInt64",
            "small UInt8",
            "label LowCardinality(String)",
            "kind Enum8(\\'a\\' = 1, \\'b\\' = 2)",
            "at DateTime",
            "maybe Nullable(UInt32)",
            "day Date"
        ]
    );
    assert_eq!(
        ok(
            addr,
            "SELECT partition, rows FROM system.parts WHERE table = 'typed'"
        )
        .await,
        "tuple()\t1\n"
    );
}

/// The `select/` corpus's table: 1 000 rows of every column kind it reads.
const CORPUS_TABLE: &str = "(k Int64, g LowCardinality(String), v Float64, d Date, flag UInt8)";

fn corpus_rows() -> Vec<(i64, &'static str, f64, i64, i64)> {
    const GROUPS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];
    (0..1000i64)
        .map(|k| {
            let v = ((k * 37) % 101) as f64 + k as f64 / 8.0;
            // 2024-01-01 is day 19 723.
            (
                k,
                GROUPS[(k % 4) as usize],
                v,
                19_723 + k % 60,
                i64::from(k % 3 == 0),
            )
        })
        .collect()
}

/// `INSERT … VALUES` of the corpus rows (dates as text: VALUES is literal-only
/// on the House, R5.6).
fn corpus_insert(table: &str) -> String {
    let rows: Vec<String> = corpus_rows()
        .into_iter()
        .map(|(k, g, v, d, flag)| {
            let offset = d - 19_723;
            let date = if offset < 31 {
                format!("2024-01-{:02}", offset + 1)
            } else {
                format!("2024-02-{:02}", offset - 30)
            };
            format!("({k}, '{g}', {v}, '{date}', {flag})")
        })
        .collect();
    format!("INSERT INTO {table} VALUES {}", rows.join(", "))
}

/// FL2's `select/` corpus on a lake table answers what ClickHouse answers over
/// the same rows in a native table: the engine's own `Memory` table in a
/// session (which shadows the lake table's name there), and, when
/// `LOAMS_HOUSE_REFERENCE_URL` names a reference server (`http://host:port`), a
/// `MergeTree` table on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn differential_select_corpus_on_lake() {
    let house = lakehouse("select-corpus").await;
    let addr = house.addr();
    ok(
        addr,
        &format!("CREATE TABLE t {CORPUS_TABLE} ENGINE = MergeTree ORDER BY k"),
    )
    .await;
    let rows: Vec<Vec<V>> = corpus_rows()
        .into_iter()
        .map(|(k, g, v, d, flag)| vec![V::I(k), V::S(g), V::F(v), V::I(d), V::I(flag)])
        .collect();
    // Two files, two snapshots.
    append(&house, 2, "default", "t", &rows[..400]).await;
    append(&house, 2, "default", "t", &rows[400..]).await;

    let session = [("session_id", "reference")];
    // (`INSERT INTO t` would name the lake table: the front does not know a
    // session's temporary tables, so the rows come with the CREATE.)
    let values = corpus_insert("t");
    let tuples = values.trim_start_matches("INSERT INTO t VALUES ");
    let structure = CORPUS_TABLE.trim_start_matches('(').trim_end_matches(')');
    ok_as(
        addr,
        ALICE,
        &format!(
            "CREATE TEMPORARY TABLE t ENGINE = Memory AS SELECT * FROM values('{structure}', {tuples})"
        ),
        &session,
    )
    .await;

    let reference = std::env::var("LOAMS_HOUSE_REFERENCE_URL").ok().map(|url| {
        let authority = url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        std::net::ToSocketAddrs::to_socket_addrs(&authority)
            .expect("the reference's address")
            .next()
            .expect("an address")
    });
    if let Some(server) = reference {
        let run = |sql: String| async move {
            let target = target(&[("database", "loams_house_corpus")]);
            let body = sql.into_bytes();
            let response =
                tokio::task::spawn_blocking(move || request(server, "POST", &target, &[], &body))
                    .await
                    .expect("client");
            assert_eq!(response.status, 200, "reference: {}", response.text());
        };
        let plain = |sql: &'static str| async move {
            let target = target(&[]);
            let body = sql.as_bytes().to_vec();
            tokio::task::spawn_blocking(move || request(server, "POST", &target, &[], &body))
                .await
                .expect("client")
        };
        plain("CREATE DATABASE IF NOT EXISTS loams_house_corpus").await;
        run("DROP TABLE IF EXISTS t".to_string()).await;
        run(format!(
            "CREATE TABLE t {CORPUS_TABLE} ENGINE = MergeTree ORDER BY k"
        ))
        .await;
        run(corpus_insert("t")).await;
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../conformance/clickhouse/corpus/select");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the select corpus")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    files.sort();
    assert!(files.len() >= 20, "{}", files.len());
    for file in files {
        let sql = std::fs::read_to_string(&file).expect("read");
        let name = file
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let lake = ok(addr, &sql).await;
        let native = ok_as(addr, ALICE, &sql, &session).await;
        assert!(!lake.is_empty(), "{name}: no rows");
        assert_eq!(
            lake, native,
            "{name}: the lake table differs from a native one"
        );
        if let Some(server) = reference {
            let target = target(&[("database", "loams_house_corpus")]);
            let body = sql.clone().into_bytes();
            let answer =
                tokio::task::spawn_blocking(move || request(server, "POST", &target, &[], &body))
                    .await
                    .expect("client");
            assert_eq!(answer.status, 200, "{name}: reference: {}", answer.text());
            assert_eq!(
                lake,
                answer.text(),
                "{name}: the lake table differs from the reference"
            );
        }
    }
    assert_no_listing(&house);
    no_mergetree_in_chdb(&house).await;
}

/// No `MergeTree`-family table exists inside chDB (D762): the lake tables are
/// views over Iceberg, read with the deny list off on the namespace's worker.
async fn no_mergetree_in_chdb(house: &Lakehouse) {
    let mut lease = house.pool.acquire("2").await.expect("worker");
    let found = lease
        .run(common::statement(
            "SELECT count() FROM system.tables WHERE engine LIKE '%MergeTree' AND NOT startsWith(engine, 'System')",
            "TSV",
        ))
        .await
        .expect("runs");
    assert_eq!(String::from_utf8_lossy(&found.bytes), "0\n");
    house.pool.release(lease, loams_house::Outcome::Completed);
}

/// Expression text a view takes from the catalog's properties (an added
/// column's `DEFAULT`, the sorting key of a `FINAL` read) passes the deny list
/// before a worker's control connection runs it: another writer of a shared
/// catalog could have set them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_expressions_are_deny_checked() {
    let house = lakehouse("catalog-expressions").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (k Int64, s String) ENGINE = ReplacingMergeTree ORDER BY k",
    )
    .await;
    append(&house, 2, "default", "t", &kv(0..3)).await;
    ok(addr, "ALTER TABLE t ADD COLUMN n UInt32 DEFAULT plus(1, 1)").await;
    assert_eq!(
        ok(addr, "SELECT sum(n) FROM t").await,
        "6\n",
        "a safe default runs"
    );
    assert_eq!(ok(addr, "SELECT count() FROM t FINAL").await, "3\n");

    let tamper = |key: &'static str, from: &'static str, to: &'static str| {
        let house = &house;
        async move {
            let table = load(house, 2, "default", "t").await;
            let value = table.metadata.properties()[key].replace(from, to);
            house
                .catalog
                .commit(
                    &table.ident,
                    Vec::new(),
                    vec![loams_house_lake::TableUpdate::SetProperties {
                        updates: HashMap::from([(key.to_string(), value)]),
                    }],
                )
                .await
                .expect("tamper");
        }
    };
    tamper(
        "loams.ch.types",
        "plus(1, 1)",
        "length(file('/etc/hostname'))",
    )
    .await;
    refused(addr, "SELECT sum(n) FROM t", &[], "497").await;
    tamper(
        "loams.ch.types",
        "length(file('/etc/hostname'))",
        "plus(1, 1)",
    )
    .await;
    assert_eq!(ok(addr, "SELECT sum(n) FROM t").await, "6\n");
    tamper(
        "loams.ch.order_by",
        "k",
        "k, url('http://127.0.0.1:1/x', 'CSV', 'a String')",
    )
    .await;
    refused(addr, "SELECT count() FROM t FINAL", &[], "497").await;
}

/// HS1 R11.11: catalog expression text is parsed as one expression and printed
/// again from its tree, never spliced: a `DEFAULT` or a sorting key that holds a
/// statement, a subquery, a table function or a second expression is `497`, and
/// a pure expression of the allowed functions reads as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_expressions_are_reprinted_from_the_ast() {
    let house = lakehouse("catalog-reprint").await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (k Int64, s String) ENGINE = ReplacingMergeTree ORDER BY k",
    )
    .await;
    append(&house, 2, "default", "t", &kv(0..3)).await;
    // ClickHouse checks an ALTER's default alone (no other column), so the
    // stored text is set to expressions over the table's columns below.
    ok(addr, "ALTER TABLE t ADD COLUMN n UInt32 DEFAULT plus(1, 1)").await;
    assert_eq!(
        ok(addr, "SELECT sum(n) FROM t").await,
        "6\n",
        "plus(1, 1) runs"
    );

    let set = |key: &'static str, value: String| {
        let house = &house;
        async move {
            let table = load(house, 2, "default", "t").await;
            house
                .catalog
                .commit(
                    &table.ident,
                    Vec::new(),
                    vec![loams_house_lake::TableUpdate::SetProperties {
                        updates: HashMap::from([(key.to_string(), value)]),
                    }],
                )
                .await
                .expect("tamper");
        }
    };
    let types =
        load(&house, 2, "default", "t").await.metadata.properties()["loams.ch.types"].clone();
    assert!(types.contains("plus(1, 1)"), "{types}");

    // Legitimate expressions still read.
    for (default, sum) in [
        ("plus(k, 2)", "9"),
        ("CAST(k * 2 AS UInt32)", "6"),
        ("if(k > 0, 1, 0)", "2"),
        ("toUInt32(length(concat(s, 'x')))", "9"),
    ] {
        set("loams.ch.types", types.replace("plus(1, 1)", default)).await;
        assert_eq!(
            ok(addr, "SELECT sum(n) FROM t").await,
            format!("{sum}\n"),
            "{default}"
        );
    }
    for default in [
        "1); DROP TABLE t; --",
        "1, 2",
        "(SELECT count() FROM system.one)",
        "(SELECT max(k) FROM numbers(10))",
        "k IN (SELECT 1)",
        "numbers(1)",
        "url('http://127.0.0.1:1/x', 'CSV', 'a String')",
        "1 SETTINGS max_threads = 1",
        "x -> 1",
    ] {
        set("loams.ch.types", types.replace("plus(1, 1)", default)).await;
        let text = refused(addr, "SELECT sum(n) FROM t", &[], "497").await;
        assert!(text.contains("catalog properties"), "{default}: {text}");
    }
    set("loams.ch.types", types.clone()).await;
    assert_eq!(ok(addr, "SELECT sum(n) FROM t").await, "6\n");

    // The sorting key of a FINAL merge view.
    for key in ["(k, toString(k))", "tuple(k)", "cityHash64(k)"] {
        set("loams.ch.order_by", key.to_string()).await;
        assert_eq!(
            ok(addr, "SELECT count() FROM t FINAL").await,
            "3\n",
            "{key}"
        );
    }
    for key in [
        "k); DROP TABLE t; --",
        "k, (SELECT 1)",
        "(k, (SELECT max(number) FROM numbers(1)))",
        "`k\\` FROM numbers(1) --`(k)",
        "k, url('http://127.0.0.1:1/x', 'CSV', 'a String')",
        "k; DROP DATABASE default",
    ] {
        set("loams.ch.order_by", key.to_string()).await;
        let text = refused(addr, "SELECT count() FROM t FINAL", &[], "497").await;
        assert!(text.contains("catalog properties"), "{key}: {text}");
    }
    set("loams.ch.order_by", "k".to_string()).await;
    assert_eq!(ok(addr, "SELECT count() FROM t FINAL").await, "3\n");
    assert_eq!(ok(addr, "SELECT sum(n) FROM t").await, "6\n", "nothing ran");
    no_mergetree_in_chdb(&house).await;
}

/// `loams.house.v1.ExecuteQuery` reads lake tables as HTTP does: the Arrow
/// header described through the views, the request's `database`, and time
/// travel from its `settings`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_reads_lake_tables() {
    use arrow::array::AsArray;
    use base64::Engine as _;
    use connectrpc::client::{ClientConfig, HttpClient};
    use loams_house_proto::loams::house::v1 as pb;
    use loams_house_proto::loams::house::v1::__buffa::oneof::{
        execute_query_response, result_batch,
    };

    let house = lakehouse("api-lake").await;
    let addr = house.addr();
    ok(addr, "CREATE DATABASE d").await;
    ok(
        addr,
        "CREATE TABLE d.t (k Int64, s LowCardinality(String)) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    let first = append(&house, 2, "d", "t", &kv(0..2)).await;
    append(&house, 2, "d", "t", &kv(2..5)).await;

    let basic = base64::engine::general_purpose::STANDARD.encode("alice:a");
    let uri: http::Uri = format!("http://{addr}").parse().expect("uri");
    let client = pb::HouseServiceClient::new(
        HttpClient::plaintext(),
        ClientConfig::new(uri).with_default_header("authorization", format!("Basic {basic}")),
    );
    let run = |settings: HashMap<String, String>| {
        let client = &client;
        async move {
            let request = pb::ExecuteQueryRequest {
                sql: "SELECT k, s FROM t ORDER BY k".to_string(),
                database: "d".to_string(),
                format: pb::ResultFormat::RESULT_FORMAT_ARROW_IPC.into(),
                settings: settings.into_iter().collect(),
                ..Default::default()
            };
            let mut stream = client.execute_query(request).await.expect("runs");
            let mut header = Vec::new();
            let mut arrow = Vec::new();
            while let Some(message) = stream.message().await.expect("a frame") {
                match message.to_owned_message().frame {
                    Some(execute_query_response::Frame::Header(h)) => {
                        header = h
                            .columns
                            .iter()
                            .map(|c| format!("{} {}", c.name, c.clickhouse_type))
                            .collect();
                    }
                    Some(execute_query_response::Frame::Batch(batch)) => {
                        if let Some(result_batch::Data::ArrowIpc(bytes)) = batch.data {
                            arrow.extend_from_slice(&bytes);
                        }
                    }
                    _ => {}
                }
            }
            let keys: Vec<i64> =
                arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(arrow), None)
                    .expect("an Arrow stream")
                    .flat_map(|batch| {
                        let batch = batch.expect("a batch");
                        batch
                            .column(0)
                            .as_primitive::<arrow::datatypes::Int64Type>()
                            .values()
                            .to_vec()
                    })
                    .collect();
            (header, keys)
        }
    };
    let (header, keys) = run(HashMap::new()).await;
    assert_eq!(header, vec!["k Int64", "s LowCardinality(String)"]);
    assert_eq!(keys, vec![0, 1, 2, 3, 4]);
    let (_, keys) = run(HashMap::from([(
        "loams_snapshot_id".to_string(),
        first.to_string(),
    )]))
    .await;
    assert_eq!(keys, vec![0, 1]);
}

/// `merge()` over the current database or `default` reads the namespace's
/// lake tables: the statement gets a view of every table of that database.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_reads_the_namespaces_lake_tables() {
    let house = lakehouse("merge").await;
    let addr = house.addr();
    for name in ["own_1", "own_2", "other"] {
        ok(
            addr,
            &format!("CREATE TABLE {name} (k Int64, s String) ENGINE = MergeTree ORDER BY k"),
        )
        .await;
    }
    append(&house, 2, "default", "own_1", &kv(1..2)).await;
    append(&house, 2, "default", "own_2", &kv(2..3)).await;
    append(&house, 2, "default", "other", &kv(100..101)).await;
    for sql in [
        "SELECT sum(k) FROM merge('default', '^own_')",
        "SELECT sum(k) FROM merge(currentDatabase(), '^own_')",
        "SELECT sum(k) FROM merge('^own_')",
    ] {
        assert_eq!(ok(addr, sql).await, "3\n", "{sql}");
    }
}

/// HE1 R9.62: the `system` tables a clustered application's migrations read
/// (SigNoz's, D1571), answered for a one-node House: the clusters its
/// `ON CLUSTER` clauses name, each this node; no macros; no mutation or
/// distributed DDL left to wait for; and `clusterAllReplicas` of a `system`
/// table read locally, while any other source of it stays refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_system_tables_answer_for_one_node() {
    let house = lakehouse("cluster-system").await;
    let addr = house.addr();
    ok(addr, "CREATE DATABASE IF NOT EXISTS sig ON CLUSTER cluster").await;
    ok(
        addr,
        "CREATE TABLE IF NOT EXISTS sig.t ON CLUSTER cluster (a UInt8) ENGINE = ReplicatedMergeTree('/clickhouse/tables/{uuid}/{shard}', '{replica}') ORDER BY a",
    )
    .await;
    ok_as(
        addr,
        BOB,
        "CREATE TABLE theirs ON CLUSTER 'bobs' (a UInt8) ENGINE = MergeTree ORDER BY a",
        &[],
    )
    .await;
    // SigNoz's own reads (its schema migrator's), as it sends them.
    assert_eq!(
        ok(
            addr,
            "SELECT DISTINCT host_address, port FROM system.clusters WHERE host_address NOT IN ['localhost', '127.0.0.1', '::1'] AND cluster = 'cluster'"
        )
        .await,
        ""
    );
    assert_eq!(
        ok(
            addr,
            "SELECT database, table, command, mutation_id, latest_fail_reason FROM system.mutations WHERE is_done = 0"
        )
        .await,
        ""
    );
    assert_eq!(
        ok(
            addr,
            "SELECT entry, cluster, query, host, port, status, exception_code FROM system.distributed_ddl_queue WHERE status != 'Finished'"
        )
        .await,
        ""
    );
    assert_eq!(
        ok(
            addr,
            "SELECT count(*) FROM clusterAllReplicas('cluster', system.tables) WHERE database = 'sig' AND name = 't'"
        )
        .await,
        "1\n"
    );
    // The namespace's clusters, this node in each; another namespace's are
    // not shown.
    assert_eq!(
        ok(
            addr,
            "SELECT cluster, shard_num, replica_num, host_name, host_address, is_local, name FROM system.clusters"
        )
        .await,
        "cluster\t1\t1\tlocalhost\t127.0.0.1\t1\tcluster\n"
    );
    assert_eq!(
        ok_as(addr, BOB, "SELECT cluster FROM system.clusters", &[]).await,
        "bobs\n"
    );
    assert_eq!(ok(addr, "SELECT count() FROM system.macros").await, "0\n");
    assert_eq!(
        ok(
            addr,
            "SELECT toTypeName(status) FROM system.distributed_ddl_queue"
        )
        .await,
        ""
    );
    // Any other use of the function stays refused.
    refused(
        addr,
        "SELECT count() FROM clusterAllReplicas('cluster', sig.t)",
        &[],
        "344",
    )
    .await;
    refused(
        addr,
        "SELECT count() FROM clusterAllReplicas('cluster', system.disks)",
        &[],
        "344",
    )
    .await;
    refused(
        addr,
        "SELECT count() FROM cluster('cluster', system.tables)",
        &[],
        "344",
    )
    .await;
}
