//! The catalog and DDL for lake tables (HS1 Task 10, FL2 Task 5 on the lake class),
//! over the ClickHouse HTTP interface with real workers: ClickHouse reads every
//! column list (`DESCRIBE TABLE null(…)` in a worker), and the tables land in a
//! `MemCatalog` whose Iceberg metadata the tests read back.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::http::{Response, request, target};
use loams_house::api::{ApiCatalog, LakeCatalog};
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, LakeTable, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house_lake::spec::{NestedField, PrimitiveType, Schema, Transform, Type};
use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent};
use loams_house_proto::loams::house::v1 as pb;

/// What `house-cache` was asked to grant.
#[derive(Debug, Default)]
struct Vouched(Mutex<Vec<(String, String)>>);

impl PrefixGrants for Vouched {
    fn vouch(&self, namespace: &str, prefix: &str) {
        self.0
            .lock()
            .expect("lock")
            .push((namespace.to_string(), prefix.to_string()));
    }
}

struct Lakehouse {
    handle: HouseHandle,
    lake: Arc<Lake>,
    catalog: Arc<MemCatalog>,
    vouched: Arc<Vouched>,
}

impl Lakehouse {
    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }
}

async fn lakehouse(test: &str, retention: Duration) -> Lakehouse {
    let pool = common::pool(test, common::small(2)).await;
    let catalog = Arc::new(MemCatalog::in_memory());
    let vouched = Arc::new(Vouched::default());
    let lake = Arc::new(
        Lake::new(catalog.clone() as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_retention(retention)
            .with_grants(vouched.clone() as Arc<dyn PrefixGrants>),
    );
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: common::tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool, lake.clone())
        .await
        .expect("serves");
    Lakehouse {
        handle,
        lake,
        catalog,
        vouched,
    }
}

const DAY: Duration = Duration::from_secs(24 * 3600);

async fn sql_with(addr: SocketAddr, sql: &str, params: &[(&str, &str)]) -> Response {
    let mut all = vec![("user", "alice"), ("password", "a")];
    all.extend_from_slice(params);
    let target = target(&all);
    let body = sql.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

async fn sql(addr: SocketAddr, sql: &str) -> Response {
    sql_with(addr, sql, &[]).await
}

async fn ok(addr: SocketAddr, statement: &str) -> String {
    let response = sql(addr, statement).await;
    assert_eq!(response.status, 200, "{statement}: {}", response.text());
    response.text()
}

fn code(response: &Response) -> Option<&str> {
    response.header("X-ClickHouse-Exception-Code")
}

async fn refused(addr: SocketAddr, statement: &str, want: &str) -> String {
    let response = sql(addr, statement).await;
    assert_eq!(
        code(&response),
        Some(want),
        "{statement}: {}",
        response.text()
    );
    response.text()
}

async fn table(house: &Lakehouse, database: &str, name: &str) -> LakeTable {
    house
        .catalog
        .load_table(&TableIdent::new(
            Lake::namespace(2, database),
            name.to_string(),
        ))
        .await
        .unwrap_or_else(|err| panic!("{database}.{name}: {err}"))
}

fn field_type(table: &LakeTable, column: &str) -> (Type, bool) {
    let schema = table.metadata.current_schema();
    let field = schema
        .as_struct()
        .fields()
        .iter()
        .find(|f| f.name == column)
        .unwrap_or_else(|| panic!("{column} is a field"));
    (field.field_type.as_ref().clone(), field.required)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_each_tier1_engine() {
    let house = lakehouse("tier1", DAY).await;
    let addr = house.addr();
    for (name, engine, stored) in [
        ("mt", "MergeTree", "MergeTree"),
        ("r0", "ReplacingMergeTree", "ReplacingMergeTree"),
        ("r1", "ReplacingMergeTree(v)", "ReplacingMergeTree(v)"),
        (
            "r2",
            "ReplacingMergeTree(v, del)",
            "ReplacingMergeTree(v, del)",
        ),
        ("s0", "SummingMergeTree", "SummingMergeTree"),
        ("s1", "SummingMergeTree((n, f))", "SummingMergeTree((n, f))"),
        (
            "rep",
            "ReplicatedReplacingMergeTree('/clickhouse/{shard}/t', '{replica}', v)",
            "ReplacingMergeTree(v)",
        ),
    ] {
        let ddl = format!(
            "CREATE TABLE {name} ON CLUSTER default (k UInt64, v UInt32, del UInt8, n Int64, \
             f Float64, s String) ENGINE = {engine} ORDER BY (k, s)"
        );
        ok(addr, &ddl).await;
        let made = table(&house, "default", name).await;
        let properties = made.metadata.properties();
        assert_eq!(
            properties.get("loams.ch.engine").map(String::as_str),
            Some(stored),
            "{name}"
        );
        assert_eq!(
            properties.get("loams.table_class").map(String::as_str),
            Some("lake")
        );
        assert!(
            made.metadata
                .location()
                .starts_with("s3://local/ns/2/house/default/"),
            "{name}: reads through house-cache as is: {}",
            made.metadata.location()
        );
        assert_eq!(made.metadata.default_sort_order().fields.len(), 2, "{name}");
        assert_eq!(ok(addr, &format!("EXISTS TABLE {name}")).await, "1\n");

        // SHOW CREATE answers the statement as written, and it makes the same
        // table again.
        let shown = ok(addr, &format!("SHOW CREATE TABLE {name} FORMAT TSVRaw")).await;
        assert_eq!(shown.trim_end(), ddl, "{name}");
        let described = ok(addr, &format!("DESCRIBE TABLE {name}")).await;
        ok(addr, &format!("DROP TABLE {name}")).await;
        assert_eq!(ok(addr, &format!("EXISTS {name}")).await, "0\n");
        ok(addr, &shown).await;
        assert_eq!(
            ok(addr, &format!("DESCRIBE {name}")).await,
            described,
            "{name}"
        );
    }
    // A second CREATE: 57, unless IF NOT EXISTS.
    refused(
        addr,
        "CREATE TABLE mt (a UInt8) ENGINE = MergeTree ORDER BY a",
        "57",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE IF NOT EXISTS mt (a UInt8) ENGINE = MergeTree ORDER BY a",
    )
    .await;
    // No ENGINE is MergeTree (ClickHouse's `default_table_engine`).
    ok(addr, "CREATE TABLE plain (a UInt8) ORDER BY a").await;
    assert_eq!(
        table(&house, "default", "plain")
            .await
            .metadata
            .properties()
            .get("loams.ch.engine")
            .map(String::as_str),
        Some("MergeTree")
    );
    // AggregatingMergeTree and Null are lake engines (HE1 R9.14); other engines
    // are not; denied ones stay denied.
    ok(
        addr,
        "CREATE TABLE ag (a UInt8) ENGINE = AggregatingMergeTree ORDER BY a",
    )
    .await;
    ok(addr, "CREATE TABLE nl (a UInt8) ENGINE = Null").await;
    refused(addr, "CREATE TABLE lg (a UInt8) ENGINE = Log", "48").await;
    refused(
        addr,
        "CREATE TABLE u (a UInt8) ENGINE = URL('http://x/')",
        "344",
    )
    .await;
    // The engine's columns must exist, and be of a kind it can use.
    refused(
        addr,
        "CREATE TABLE bad (a UInt8) ENGINE = ReplacingMergeTree(nope) ORDER BY a",
        "47",
    )
    .await;
    refused(
        addr,
        "CREATE TABLE bad (a UInt8, s String) ENGINE = ReplacingMergeTree(s) ORDER BY a",
        "36",
    )
    .await;
    refused(
        addr,
        "CREATE TABLE bad (a UInt8) ENGINE = MergeTree ORDER BY nope",
        "47",
    )
    .await;
    refused(
        addr,
        "CREATE TABLE bad (a UInt8, b UInt8) ENGINE = MergeTree PRIMARY KEY b ORDER BY a",
        "36",
    )
    .await;
    // Out of §49 §6.3's surface.
    // HE1 R9.12: a delete TTL, skip indexes, projections, constraints and SAMPLE
    // BY are accepted (lake_tables_take_langfuse_ddl has the rest); a TTL that
    // moves or rolls up rows is 48.
    ok(
        addr,
        "CREATE TABLE ttl1 (a UInt8, d Date) ENGINE = MergeTree ORDER BY a TTL d + INTERVAL 1 DAY",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE idx1 (a UInt8, INDEX i a TYPE minmax) ENGINE = MergeTree ORDER BY a",
    )
    .await;
    refused(
        addr,
        "CREATE TABLE bad (a UInt8, d Date) ENGINE = MergeTree ORDER BY a TTL d TO DISK 'x'",
        "48",
    )
    .await;
    // This House has no store, so it writes no rows (HS1 Task 12) and makes no
    // table for them.
    refused(addr, "CREATE TABLE bad AS SELECT 1", "48").await;
    assert_eq!(ok(addr, "EXISTS TABLE bad").await, "0\n");
    // SETTINGS: MergeTree's are recorded, unknown ones 115, `realtime` needs FL1.
    ok(
        addr,
        "CREATE TABLE st (a UInt8) ENGINE = MergeTree ORDER BY a SETTINGS index_granularity = 1024",
    )
    .await;
    refused(
        addr,
        "CREATE TABLE bad (a UInt8) ENGINE = MergeTree ORDER BY a SETTINGS nonsense_setting = 1",
        "115",
    )
    .await;
    let realtime = refused(
        addr,
        "CREATE TABLE bad (a UInt8) ENGINE = MergeTree ORDER BY a SETTINGS loams_table_class = 'realtime'",
        "36",
    )
    .await;
    assert!(realtime.contains("FL1"), "{realtime}");
    // Column codecs, which sqlparser cannot read, are ClickHouse's to read.
    ok(
        addr,
        "CREATE TABLE cd (a UInt64 CODEC(ZSTD(3)), b String) ENGINE = MergeTree ORDER BY a",
    )
    .await;
    assert!(ok(addr, "DESCRIBE cd").await.contains("ZSTD(3)"));
    // The deny list reads defaults: no host file through a DEFAULT.
    refused(
        addr,
        "CREATE TABLE bad (a String DEFAULT file('/etc/passwd')) ENGINE = MergeTree ORDER BY a",
        "344",
    )
    .await;
}

/// Every row of the type table, through Iceberg and back to the ClickHouse type
/// `DESCRIBE` shows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn type_mapping_roundtrip() {
    let house = lakehouse("types", DAY).await;
    let addr = house.addr();
    let p = |p: PrimitiveType| Type::Primitive(p);
    let rows: Vec<(&str, &str, Type)> = vec![
        ("i8", "Int8", p(PrimitiveType::Int)),
        ("i16", "Int16", p(PrimitiveType::Int)),
        ("i32", "Int32", p(PrimitiveType::Int)),
        ("i64", "Int64", p(PrimitiveType::Long)),
        ("u8", "UInt8", p(PrimitiveType::Int)),
        ("u16", "UInt16", p(PrimitiveType::Int)),
        ("u32", "UInt32", p(PrimitiveType::Long)),
        (
            "u64",
            "UInt64",
            p(PrimitiveType::Decimal {
                precision: 20,
                scale: 0,
            }),
        ),
        ("f32", "Float32", p(PrimitiveType::Float)),
        ("f64", "Float64", p(PrimitiveType::Double)),
        (
            "dec",
            "Decimal(18, 4)",
            p(PrimitiveType::Decimal {
                precision: 18,
                scale: 4,
            }),
        ),
        ("b", "Bool", p(PrimitiveType::Boolean)),
        ("s", "String", p(PrimitiveType::String)),
        ("fs", "FixedString(16)", p(PrimitiveType::Fixed(16))),
        ("d", "Date", p(PrimitiveType::Date)),
        ("d32", "Date32", p(PrimitiveType::Date)),
        ("dt", "DateTime('UTC')", p(PrimitiveType::Timestamptz)),
        (
            "dt64",
            "DateTime64(3, 'Asia/Kolkata')",
            p(PrimitiveType::Timestamptz),
        ),
        ("uuid", "UUID", p(PrimitiveType::Uuid)),
        ("ip4", "IPv4", p(PrimitiveType::String)),
        ("ip6", "IPv6", p(PrimitiveType::String)),
        ("e8", "Enum8('a' = 1, 'b' = 2)", p(PrimitiveType::String)),
        ("lc", "LowCardinality(String)", p(PrimitiveType::String)),
        ("ns", "Nullable(String)", p(PrimitiveType::String)),
    ];
    let columns: Vec<String> = rows.iter().map(|(n, t, _)| format!("{n} {t}")).collect();
    let more = "arr Array(Nullable(Int64)), m Map(String, UInt8), tp Tuple(x UInt8, y String), ut Tuple(UInt8, String)";
    ok(
        addr,
        &format!(
            "CREATE TABLE every ({}, {more}) ENGINE = MergeTree ORDER BY i64",
            columns.join(", ")
        ),
    )
    .await;
    let made = table(&house, "default", "every").await;
    for (name, _, ty) in &rows {
        let (got, required) = field_type(&made, name);
        assert_eq!(&got, ty, "{name}");
        assert_eq!(required, *name != "ns", "{name}: required unless Nullable");
    }
    assert!(matches!(field_type(&made, "arr").0, Type::List(_)));
    assert!(matches!(field_type(&made, "m").0, Type::Map(_)));
    assert!(matches!(field_type(&made, "tp").0, Type::Struct(_)));
    // DESCRIBE answers ClickHouse's own type for every column.
    let described = ok(addr, "DESCRIBE every FORMAT TSVRaw").await;
    let types: Vec<(String, String)> = described
        .lines()
        .map(|l| {
            let mut cells = l.split('\t');
            (
                cells.next().unwrap_or_default().to_string(),
                cells.next().unwrap_or_default().to_string(),
            )
        })
        .collect();
    for (name, ch, _) in &rows {
        assert!(
            types.iter().any(|(n, t)| n == name && t == ch),
            "{name} {ch}: {described}"
        );
    }
    assert!(
        types
            .iter()
            .any(|(n, t)| n == "ut" && t == "Tuple(UInt8, String)"),
        "{described}"
    );
    // Nanoseconds are a long of nanoseconds in a v2 table until the House
    // writes v3 (D1572; HE1 R9.61); inside a container, v3's type.
    ok(
        addr,
        "CREATE TABLE ns9 (t DateTime64(9, 'UTC')) ENGINE = MergeTree ORDER BY t",
    )
    .await;
    let ns9 = table(&house, "default", "ns9").await;
    assert_eq!(field_type(&ns9, "t").0, p(PrimitiveType::Long));
    assert_eq!(
        ns9.metadata.format_version(),
        loams_house_lake::spec::FormatVersion::V2
    );
    ok(
        addr,
        "CREATE TABLE ns9a (t Array(DateTime64(9, 'UTC'))) ENGINE = MergeTree ORDER BY tuple()",
    )
    .await;
    assert_eq!(
        table(&house, "default", "ns9a")
            .await
            .metadata
            .format_version(),
        loams_house_lake::spec::FormatVersion::V3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partition_by_supported_transforms() {
    let house = lakehouse("partitions", DAY).await;
    let addr = house.addr();
    for (name, by, want) in [
        ("p1", "toYYYYMM(ts)", vec![(Transform::Month, "ts_month")]),
        ("p2", "toYYYYMMDD(ts)", vec![(Transform::Day, "ts_day")]),
        ("p3", "toDate(ts)", vec![(Transform::Day, "ts_day")]),
        (
            "p4",
            "toStartOfHour(ts)",
            vec![(Transform::Hour, "ts_hour")],
        ),
        ("p5", "region", vec![(Transform::Identity, "region")]),
        (
            "p6",
            "(region, toYYYYMM(d))",
            vec![
                (Transform::Identity, "region"),
                (Transform::Month, "d_month"),
            ],
        ),
        ("p7", "tuple()", vec![]),
    ] {
        ok(
            addr,
            &format!(
                "CREATE TABLE {name} (ts DateTime64(3, 'UTC'), d Date, region String, n UInt32) \
                 ENGINE = MergeTree PARTITION BY {by} ORDER BY n"
            ),
        )
        .await;
        let made = table(&house, "default", name).await;
        let spec = made.metadata.default_partition_spec();
        let got: Vec<(Transform, String)> = spec
            .fields()
            .iter()
            .map(|f| (f.transform, f.name.clone()))
            .collect();
        let want: Vec<(Transform, String)> =
            want.into_iter().map(|(t, n)| (t, n.to_string())).collect();
        assert_eq!(got, want, "{name}: PARTITION BY {by}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partition_by_other_is_36() {
    let house = lakehouse("partitions36", DAY).await;
    let addr = house.addr();
    // HE1 R9.12: an element no Iceberg transform matches leaves the files
    // unpartitioned by it; the table keeps the expression as written.
    for (i, by) in [
        "n % 2",
        "toMonday(d)",
        "intDiv(n, 10)",
        "(toYYYYMM(d), n % 4)",
    ]
    .into_iter()
    .enumerate()
    {
        ok(
            addr,
            &format!(
                "CREATE TABLE p{i} (ts DateTime, d Date, n UInt32) ENGINE = MergeTree \
                 PARTITION BY {by} ORDER BY n"
            ),
        )
        .await;
        let t = table(&house, "default", &format!("p{i}")).await;
        let fields = t.metadata.default_partition_spec().fields().len();
        assert_eq!(fields, usize::from(by.starts_with('(')), "{by}");
        let shown = ok(addr, &format!("SHOW CREATE TABLE p{i} FORMAT TSVRaw")).await;
        assert!(shown.contains(&format!("PARTITION BY {by}")), "{shown}");
    }
    // R9.16: a transform of a type Iceberg cannot partition by leaves the files
    // unpartitioned by it too (SigNoz's `toDate(<Int64>)`).
    for (i, by) in ["toStartOfHour(d)", "toYYYYMM(region)", "arr", "toDate(n)"]
        .into_iter()
        .enumerate()
    {
        ok(
            addr,
            &format!(
                "CREATE TABLE w{i} (ts DateTime, d Date, region String, n UInt32, arr Array(UInt8)) \
                 ENGINE = MergeTree PARTITION BY {by} ORDER BY n"
            ),
        )
        .await;
        let t = table(&house, "default", &format!("w{i}")).await;
        assert_eq!(
            t.metadata.default_partition_spec().fields().len(),
            0,
            "{by}"
        );
    }
    refused(
        addr,
        "CREATE TABLE t (n UInt32) ENGINE = MergeTree PARTITION BY nope ORDER BY n",
        "47",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_type_is_48() {
    let house = lakehouse("types48", DAY).await;
    let addr = house.addr();
    for ty in [
        "Int128",
        "UInt256",
        "Decimal(50, 2)",
        "Nested(a UInt8, b String)",
        // `JSON` is stored as its text (HE1 R9.63); inside a container it is not.
        "Array(JSON)",
        "Dynamic",
        "IntervalDay",
    ] {
        refused(
            addr,
            &format!("CREATE TABLE t (k UInt8, c {ty}) ENGINE = MergeTree ORDER BY k"),
            "48",
        )
        .await;
    }
    // A type ClickHouse does not know is ClickHouse's 50.
    refused(
        addr,
        "CREATE TABLE t (k UInt8, c Banana) ENGINE = MergeTree ORDER BY k",
        "50",
    )
    .await;
    assert!(
        house
            .catalog
            .list_tables(&Lake::namespace(2, "default"))
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alter_add_column() {
    let house = lakehouse("alter", DAY).await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (a UInt64, b String) ENGINE = MergeTree ORDER BY a",
    )
    .await;
    let before = table(&house, "default", "t").await;
    ok(
        addr,
        "ALTER TABLE t ADD COLUMN c UInt8 DEFAULT 7 COMMENT 'seven' AFTER a",
    )
    .await;
    ok(
        addr,
        "ALTER TABLE t ADD COLUMN z Nullable(String) FIRST, ADD COLUMN w Array(UInt8)",
    )
    .await;
    ok(addr, "ALTER TABLE t ADD COLUMN IF NOT EXISTS c UInt8").await;
    refused(addr, "ALTER TABLE t ADD COLUMN c UInt8", "15").await;
    refused(addr, "ALTER TABLE t ADD COLUMN x UInt8 AFTER nope", "47").await;
    refused(addr, "ALTER TABLE t FREEZE", "48").await;
    refused(addr, "ALTER TABLE nope ADD COLUMN x UInt8", "60").await;
    let described = ok(addr, "DESCRIBE t FORMAT TSVRaw").await;
    let names: Vec<&str> = described
        .lines()
        .map(|l| l.split('\t').next().unwrap_or_default())
        .collect();
    assert_eq!(names, ["z", "a", "c", "b", "w"], "{described}");
    assert!(
        described.contains("c\tUInt8\tDEFAULT\t7\tseven"),
        "{described}"
    );
    // Iceberg: a new schema whose added fields are optional, ids after the old ones.
    let after = table(&house, "default", "t").await;
    assert_ne!(
        after.metadata.current_schema_id(),
        before.metadata.current_schema_id()
    );
    let (ty, required) = field_type(&after, "c");
    assert_eq!(ty, Type::Primitive(PrimitiveType::Int));
    assert!(!required, "Iceberg v2 adds optional fields");
    assert!(after.metadata.last_column_id() > before.metadata.last_column_id());
    // SHOW CREATE is written out again, and makes the same table.
    let shown = ok(addr, "SHOW CREATE TABLE t FORMAT TSVRaw").await;
    assert!(
        shown.contains("`c` UInt8 DEFAULT 7 COMMENT 'seven'")
            || shown.contains("c UInt8 DEFAULT 7 COMMENT 'seven'"),
        "{shown}"
    );
    ok(addr, "DROP TABLE t").await;
    ok(addr, &shown).await;
    assert_eq!(ok(addr, "DESCRIBE t FORMAT TSVRaw").await, described);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drop_database_cascades() {
    let house = lakehouse("dropdb", DAY).await;
    let addr = house.addr();
    ok(addr, "CREATE DATABASE sales COMMENT 'money'").await;
    refused(addr, "CREATE DATABASE sales", "82").await;
    ok(addr, "CREATE DATABASE IF NOT EXISTS sales").await;
    ok(
        addr,
        "CREATE TABLE sales.a (k UInt8) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    let in_db = sql_with(
        addr,
        "CREATE TABLE b (k UInt8) ENGINE = MergeTree ORDER BY k",
        &[("database", "sales")],
    )
    .await;
    assert_eq!(in_db.status, 200, "{}", in_db.text());
    assert_eq!(ok(addr, "EXISTS TABLE sales.b").await, "1\n");
    assert_eq!(ok(addr, "EXISTS DATABASE sales").await, "1\n");
    assert!(
        ok(addr, "SHOW CREATE DATABASE sales FORMAT TSVRaw")
            .await
            .contains("COMMENT 'money'")
    );
    // USE in a session.
    let used = sql_with(addr, "USE sales", &[("session_id", "s1")]).await;
    assert_eq!(used.status, 200, "{}", used.text());
    let exists = sql_with(addr, "EXISTS TABLE b", &[("session_id", "s1")]).await;
    assert_eq!(exists.text(), "1\n");

    ok(addr, "DROP DATABASE sales").await;
    assert_eq!(ok(addr, "EXISTS DATABASE sales").await, "0\n");
    refused(addr, "USE sales", "81").await;
    let gone = sql_with(addr, "SELECT 1", &[("database", "sales")]).await;
    assert_eq!(code(&gone), Some("81"), "{}", gone.text());
    refused(addr, "DROP DATABASE sales", "81").await;
    ok(addr, "DROP DATABASE IF EXISTS sales").await;
    refused(addr, "DROP DATABASE default", "36").await;
    // Its tables were kept for UNDROP, not forgotten.
    let kept = house
        .catalog
        .list_tables(&Lake::namespace(2, "__loams_dropped"))
        .await
        .expect("kept");
    assert_eq!(kept.len(), 2);
    // A recreated database can take its tables back.
    ok(addr, "CREATE DATABASE sales").await;
    ok(addr, "UNDROP TABLE sales.a").await;
    assert_eq!(ok(addr, "EXISTS TABLE sales.a").await, "1\n");
    // Database engines: Atomic is a House database; others are 344 or 48.
    ok(addr, "CREATE DATABASE atom ENGINE = Atomic").await;
    refused(
        addr,
        "CREATE DATABASE m ENGINE = MySQL('h:3306', 'db', 'u', 'p')",
        "344",
    )
    .await;
    refused(addr, "CREATE DATABASE f ENGINE = Filesystem('/etc')", "344").await;
    refused(addr, "CREATE DATABASE mem ENGINE = Memory", "48").await;
    refused(addr, "CREATE DATABASE system", "36").await;
    let databases = LakeCatalog(house.lake.clone())
        .databases(2)
        .await
        .expect("lists");
    let names: Vec<&str> = databases.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["atom", "default", "sales"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_tables_listed_read_only() {
    let house = lakehouse("external", DAY).await;
    let addr = house.addr();
    // Another writer's table: no `loams.ch.*` properties, its own location.
    house.lake.ensure_root(2).await.expect("root");
    let schema = Schema::builder()
        .with_fields(vec![
            NestedField::required(1, "id", Type::Primitive(PrimitiveType::Long)).into(),
            NestedField::optional(2, "name", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(3, "at", Type::Primitive(PrimitiveType::Timestamptz)).into(),
        ])
        .build()
        .expect("schema");
    house
        .catalog
        .create_table(
            &NamespaceIdent::new("ns_2".to_string()),
            TableCreation::builder()
                .name("events".to_string())
                .location("s3://local/spark/warehouse/events".to_string())
                .schema(schema)
                .build(),
        )
        .await
        .expect("another writer creates");
    // Another writer's table inside another namespace's tree is listed, but its
    // location is never vouched to this namespace (PR #448 review).
    house
        .catalog
        .create_table(
            &NamespaceIdent::new("ns_2".to_string()),
            TableCreation::builder()
                .name("intruder".to_string())
                .location("s3://local/ns/3/house/x".to_string())
                .schema(
                    Schema::builder()
                        .with_fields(vec![
                            NestedField::required(1, "id", Type::Primitive(PrimitiveType::Long))
                                .into(),
                        ])
                        .build()
                        .expect("schema"),
                )
                .build(),
        )
        .await
        .expect("another writer creates");
    ok(
        addr,
        "CREATE TABLE mine (k UInt8) ENGINE = MergeTree ORDER BY k",
    )
    .await;

    let listed = LakeCatalog(house.lake.clone())
        .tables(2, "default")
        .await
        .expect("lists");
    let classes: Vec<(String, pb::TableClass)> = listed
        .iter()
        .map(|t| (t.name.clone(), t.table_class.as_known().unwrap_or_default()))
        .collect();
    assert_eq!(
        classes,
        [
            ("events".to_string(), pb::TableClass::TABLE_CLASS_EXTERNAL),
            ("intruder".to_string(), pb::TableClass::TABLE_CLASS_EXTERNAL),
            ("mine".to_string(), pb::TableClass::TABLE_CLASS_LAKE),
        ]
    );
    assert_eq!(listed[0].engine, "Iceberg");
    // Its location is vouched to house-cache for the namespace's workers.
    let vouched = house.vouched.0.lock().expect("lock").clone();
    assert!(
        vouched.contains(&("2".to_string(), "spark/warehouse/events/".to_string())),
        "{vouched:?}"
    );
    assert!(
        vouched.iter().all(|(_, prefix)| !prefix.starts_with("ns/")),
        "nothing in the namespaces' tree is vouched: {vouched:?}"
    );
    // DESCRIBE answers the types chDB reads it as.
    let described = ok(addr, "DESCRIBE events FORMAT TSVRaw").await;
    assert!(described.contains("id\tInt64"), "{described}");
    assert!(described.contains("name\tNullable(String)"), "{described}");
    assert!(
        described.contains("at\tNullable(DateTime64(6, 'UTC'))"),
        "{described}"
    );
    assert!(
        ok(addr, "SHOW CREATE TABLE events FORMAT TSVRaw")
            .await
            .contains("ENGINE = Iceberg")
    );
    // Read-only: INSERT and DDL are 497.
    refused(addr, "INSERT INTO events VALUES (1, 'a', now())", "497").await;
    refused(addr, "INSERT INTO events FORMAT CSV\n1,a,\n", "497").await;
    refused(addr, "DROP TABLE events", "497").await;
    refused(addr, "ALTER TABLE events ADD COLUMN x UInt8", "497").await;
    refused(addr, "RENAME TABLE events TO e2", "497").await;
    // A House without a store writes no lake table (HS1 Task 12).
    refused(addr, "INSERT INTO mine VALUES (1)", "48").await;
    // Still there, unchanged.
    assert_eq!(ok(addr, "EXISTS events").await, "1\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undrop_within_retention() {
    let house = lakehouse("undrop", Duration::from_secs(3)).await;
    let addr = house.addr();
    ok(
        addr,
        "CREATE TABLE t (a UInt8) ENGINE = MergeTree ORDER BY a",
    )
    .await;
    let made = table(&house, "default", "t").await;
    ok(addr, "DROP TABLE t").await;
    assert_eq!(ok(addr, "EXISTS t").await, "0\n");
    refused(addr, "DROP TABLE t", "60").await;
    ok(addr, "DROP TABLE IF EXISTS t").await;
    ok(addr, "UNDROP TABLE t").await;
    let back = table(&house, "default", "t").await;
    assert_eq!(
        back.metadata.uuid(),
        made.metadata.uuid(),
        "the same table came back"
    );
    assert!(
        !back
            .metadata
            .properties()
            .contains_key("loams.dropped.at_ms")
    );
    // A name taken again blocks UNDROP; the newest drop comes back first.
    ok(addr, "DROP TABLE t").await;
    ok(
        addr,
        "CREATE TABLE t (b String) ENGINE = MergeTree ORDER BY b",
    )
    .await;
    refused(addr, "UNDROP TABLE t", "57").await;
    // RENAME keeps the table and writes SHOW CREATE out under the new name.
    ok(addr, "RENAME TABLE t TO t2").await;
    assert!(
        ok(addr, "SHOW CREATE TABLE t2 FORMAT TSVRaw")
            .await
            .starts_with("CREATE TABLE default.t2")
    );
    ok(addr, "UNDROP TABLE t").await;
    assert_eq!(
        table(&house, "default", "t").await.metadata.uuid(),
        made.metadata.uuid()
    );
    // Past the retention it is gone for UNDROP (HS1 Task 15 purges it).
    ok(addr, "DROP TABLE t").await;
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    let text = refused(addr, "UNDROP TABLE t", "60").await;
    assert!(text.contains("retention"), "{text}");
    refused(addr, "UNDROP TABLE never_was", "60").await;
}

/// The same DDL over Lakekeeper (`LOAMS_HOUSE_LAKEKEEPER_URL`, `_WAREHOUSE`,
/// `_ROOT`; skipped without): nested namespaces, a v3 table, schema evolution,
/// DROP and UNDROP, RENAME.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ddl_over_lakekeeper() {
    use loams_house::catalog::{RestCatalog, RestOptions};
    let Ok(uri) = std::env::var("LOAMS_HOUSE_LAKEKEEPER_URL") else {
        eprintln!("skipped: LOAMS_HOUSE_LAKEKEEPER_URL is not set");
        return;
    };
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is needed"));
    let rest: Arc<dyn HouseCatalog> = Arc::new(
        RestCatalog::connect(RestOptions {
            uri,
            warehouse: var("LOAMS_HOUSE_LAKEKEEPER_WAREHOUSE"),
            token: None,
        })
        .await
        .expect("connects"),
    );
    let root = LakeRoot::from_store_url(&var("LOAMS_HOUSE_LAKEKEEPER_ROOT")).expect("root");
    let pool = common::pool("lakekeeper", common::small(2)).await;
    // A namespace number of its own, so runs do not meet.
    let namespace = u64::from(uuid_ns());
    let lake = Arc::new(Lake::new(rest.clone(), root.clone()).with_retention(DAY));
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", namespace, false)],
        tmp_dir: common::tmp_root("lakekeeper-spool"),
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool, lake).await.expect("serves");
    let addr = handle.local_addr();
    ok(addr, "CREATE DATABASE sales").await;
    ok(addr, "CREATE TABLE sales.t (k UInt64, ts DateTime64(9, 'UTC'), s LowCardinality(String)) ENGINE = ReplacingMergeTree PARTITION BY toYYYYMMDD(ts) ORDER BY k").await;
    let loaded = rest
        .load_table(&TableIdent::new(
            Lake::namespace(namespace, "sales"),
            "t".to_string(),
        ))
        .await
        .expect("in Lakekeeper");
    // A nanosecond time is a long in a v2 table (D1572).
    assert_eq!(
        loaded.metadata.format_version(),
        loams_house_lake::spec::FormatVersion::V2
    );
    assert!(
        loaded
            .metadata
            .location()
            .starts_with(&format!("{root}/ns/{namespace}/house/sales/t-")),
        "{}",
        loaded.metadata.location()
    );
    ok(addr, "ALTER TABLE sales.t ADD COLUMN n Nullable(Int32)").await;
    assert!(
        ok(addr, "DESCRIBE sales.t FORMAT TSVRaw")
            .await
            .contains("n\tNullable(Int32)")
    );
    ok(addr, "RENAME TABLE sales.t TO sales.u").await;
    ok(addr, "DROP TABLE sales.u").await;
    assert_eq!(ok(addr, "EXISTS sales.u").await, "0\n");
    ok(addr, "UNDROP TABLE sales.u").await;
    assert_eq!(ok(addr, "EXISTS sales.u").await, "1\n");
    ok(addr, "DROP DATABASE sales").await;
    assert_eq!(ok(addr, "EXISTS DATABASE sales").await, "0\n");
}

fn uuid_ns() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(7);
    1_000_000 + nanos % 1_000_000_000
}
