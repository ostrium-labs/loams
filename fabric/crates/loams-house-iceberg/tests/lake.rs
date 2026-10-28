//! Lake-table reads (plan HE1 Task 26) over tables written with the House's
//! own writer (`loams-house-lake`), as the chDB House writes them.

#![allow(clippy::unwrap_used)]

mod common;

use std::net::Ipv6Addr;
use std::sync::Arc;

use common::{Lake, column, context, dir, local};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Date32Array, Decimal128Array, FixedSizeBinaryArray,
    Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, ListArray, MapArray,
    RecordBatch, StringArray, StructArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use datafusion::arrow::compute::concat_batches;
use datafusion::arrow::datatypes::{DataType, Field, Int64Type, UInt32Type};
use loams_house_iceberg::{LakeTable, ReadMode, TablePin, provider};
use loams_house_sql::Registry;
use loams_house_types::arrow::{CH_TYPE, data_type};

async fn rows(pin: &TablePin, sql: &str) -> RecordBatch {
    let ctx = context();
    let p = provider(pin, local(), &Registry::default()).await.unwrap();
    ctx.register_table("t", p).unwrap();
    let df = ctx.sql(sql).await.unwrap();
    let schema = Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await.unwrap();
    concat_batches(&schema, &batches).unwrap()
}

fn engine_array(ty: &str, array: ArrayRef) -> ArrayRef {
    let t = loams_house_types::parse(ty).unwrap();
    let (dt, _) = data_type(&t).unwrap();
    assert_eq!(array.data_type(), &dt, "{ty}");
    array
}

fn tier0() -> Vec<(&'static str, &'static str, ArrayRef)> {
    let list_field = Arc::new(
        Field::new("item", DataType::Utf8, false)
            .with_metadata([(CH_TYPE.to_string(), "String".to_string())].into()),
    );
    let arr = ListArray::try_new(
        list_field,
        datafusion::arrow::buffer::OffsetBuffer::from_lengths([2, 0]),
        Arc::new(StringArray::from(vec!["a", "b"])),
        None,
    )
    .unwrap();
    let t = loams_house_types::parse("Map(String, UInt64)").unwrap();
    let DataType::Map(entries, _) = data_type(&t).unwrap().0 else {
        unreachable!()
    };
    let DataType::Struct(kv) = entries.data_type() else {
        unreachable!()
    };
    let map = MapArray::try_new(
        entries.clone(),
        datafusion::arrow::buffer::OffsetBuffer::from_lengths([1, 0]),
        StructArray::try_new(
            kv.clone(),
            vec![
                Arc::new(StringArray::from(vec!["k"])),
                Arc::new(UInt64Array::from(vec![1])),
            ],
            None,
        )
        .unwrap(),
        None,
        false,
    )
    .unwrap();
    let tt = loams_house_types::parse("Tuple(a UInt8, b String)").unwrap();
    let DataType::Struct(tf) = data_type(&tt).unwrap().0 else {
        unreachable!()
    };
    let tup = StructArray::try_new(
        tf,
        vec![
            Arc::new(UInt8Array::from(vec![1, 0])),
            Arc::new(StringArray::from(vec!["x", ""])),
        ],
        None,
    )
    .unwrap();
    let lc = datafusion::arrow::compute::cast(
        &(Arc::new(StringArray::from(vec!["x", ""])) as ArrayRef),
        &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
    )
    .unwrap();
    let ip6 = FixedSizeBinaryArray::try_from_iter(
        vec![
            "2001:db8::1".parse::<Ipv6Addr>().unwrap().octets().to_vec(),
            "::ffff:1.2.3.4"
                .parse::<Ipv6Addr>()
                .unwrap()
                .octets()
                .to_vec(),
        ]
        .into_iter(),
    )
    .unwrap();
    let uuid = FixedSizeBinaryArray::try_from_iter(
        vec![[0x61u8; 16].to_vec(), [0u8; 16].to_vec()].into_iter(),
    )
    .unwrap();
    vec![
        ("id", "UInt32", Arc::new(UInt32Array::from(vec![1, 2]))),
        ("u8", "UInt8", Arc::new(UInt8Array::from(vec![0, 255]))),
        ("u16", "UInt16", Arc::new(UInt16Array::from(vec![0, 65535]))),
        (
            "u64",
            "UInt64",
            Arc::new(UInt64Array::from(vec![0, u64::MAX])),
        ),
        ("i8", "Int8", Arc::new(Int8Array::from(vec![-128, 127]))),
        (
            "i16",
            "Int16",
            Arc::new(Int16Array::from(vec![-32768, 32767])),
        ),
        (
            "i32",
            "Int32",
            Arc::new(Int32Array::from(vec![i32::MIN, i32::MAX])),
        ),
        (
            "i64",
            "Int64",
            Arc::new(Int64Array::from(vec![i64::MIN, i64::MAX])),
        ),
        (
            "f32",
            "Float32",
            Arc::new(Float32Array::from(vec![0.1, -0.0])),
        ),
        (
            "f64",
            "Float64",
            Arc::new(Float64Array::from(vec![1e21, -1e-7])),
        ),
        ("b", "Bool", Arc::new(BooleanArray::from(vec![true, false]))),
        (
            "dec",
            "Decimal(18, 4)",
            Arc::new(
                Decimal128Array::from(vec![15000, -1])
                    .with_precision_and_scale(18, 4)
                    .unwrap(),
            ),
        ),
        (
            "s",
            "String",
            Arc::new(StringArray::from(vec!["a\tb", "it's"])),
        ),
        (
            "fs",
            "FixedString(3)",
            Arc::new(
                FixedSizeBinaryArray::try_from_iter(
                    vec![b"ab\0".to_vec(), b"xyz".to_vec()].into_iter(),
                )
                .unwrap(),
            ),
        ),
        ("d", "Date", Arc::new(Date32Array::from(vec![20384, 0]))),
        (
            "d32",
            "Date32",
            Arc::new(Date32Array::from(vec![-25567, 65535])),
        ),
        (
            "dt",
            "DateTime('UTC')",
            Arc::new(TimestampSecondArray::from(vec![1_792_717_323, 0]).with_timezone("UTC")),
        ),
        (
            "dtn",
            "DateTime",
            Arc::new(TimestampSecondArray::from(vec![1_792_717_323, 0])),
        ),
        (
            "dt64",
            "DateTime64(3, 'UTC')",
            Arc::new(
                TimestampMillisecondArray::from(vec![1_792_717_323_500, -500]).with_timezone("UTC"),
            ),
        ),
        (
            "dt64n",
            "DateTime64(9, 'UTC')",
            Arc::new(
                TimestampNanosecondArray::from(vec![1_792_717_323_123_456_789, 0])
                    .with_timezone("UTC"),
            ),
        ),
        ("uuid", "UUID", Arc::new(uuid)),
        (
            "ip4",
            "IPv4",
            Arc::new(UInt32Array::from(vec![0x0102_0304, 0])),
        ),
        ("ip6", "IPv6", Arc::new(ip6)),
        (
            "e",
            "Enum8('a' = 1, 'b' = 2)",
            Arc::new(Int8Array::from(vec![1, 2])),
        ),
        (
            "ns",
            "Nullable(String)",
            Arc::new(StringArray::from(vec![None, Some("")])),
        ),
        (
            "ni",
            "Nullable(Int32)",
            Arc::new(Int32Array::from(vec![None, Some(0)])),
        ),
        ("lc", "LowCardinality(String)", lc),
        ("arr", "Array(String)", Arc::new(arr)),
        ("m", "Map(String, UInt64)", Arc::new(map)),
        ("ntup", "Tuple(a UInt8, b String)", Arc::new(tup)),
        (
            "j",
            "JSON",
            Arc::new(StringArray::from(vec![r#"{"a":1}"#, "{}"])),
        ),
        (
            "st",
            "AggregateFunction(uniq, UInt64)",
            Arc::new(datafusion::arrow::array::BinaryArray::from(vec![
                b"\x01\x02".as_ref(),
                b"".as_ref(),
            ])),
        ),
    ]
}

/// Every Tier 0 type and every encoding of the House reads back as written,
/// in the engine's layout with its ClickHouse type.
#[tokio::test]
async fn every_encoding_reads_back_as_written() {
    let root = dir("every_encoding");
    let cases = tier0();
    let columns = cases.iter().map(|(n, t, _)| column(n, t)).collect();
    let mut lake = Lake::create(&root, "default", "t", columns, "MergeTree", "id", None, "");
    lake.insert(
        cases
            .iter()
            .map(|(n, t, a)| (*n, engine_array(t, a.clone())))
            .collect(),
    )
    .await;
    let batch = rows(&lake.pin(), "SELECT * FROM t ORDER BY id").await;
    assert_eq!(batch.num_rows(), 2);
    for (i, (name, ty, array)) in cases.iter().enumerate() {
        let field = batch.schema().field(i).clone();
        assert_eq!(field.name(), name);
        let t = loams_house_types::parse(ty).unwrap();
        assert_eq!(
            field.metadata().get(CH_TYPE).unwrap(),
            &t.to_compact_string(),
            "{name}"
        );
        let got = batch.column(i);
        let want = datafusion::arrow::compute::cast(array, got.data_type()).unwrap();
        assert_eq!(format!("{got:?}"), format!("{want:?}"), "{name} {ty}");
    }
}

/// A column added by `ALTER` reads its `DEFAULT` for rows written before it
/// (R10.6); `ALIAS` columns are computed; a column renamed reads its older
/// files by field id.
#[tokio::test]
async fn added_alias_and_renamed_columns() {
    let root = dir("added_alias");
    let mut a = column("plus", "UInt64");
    a.default_kind = "ALIAS".into();
    a.default_expression = "id + 1".into();
    let mut lake = Lake::create(
        &root,
        "default",
        "t",
        vec![column("id", "UInt32"), column("s", "String"), a],
        "MergeTree",
        "id",
        None,
        "",
    );
    lake.insert(vec![
        ("id", Arc::new(UInt32Array::from(vec![1, 2])) as ArrayRef),
        ("s", Arc::new(StringArray::from(vec!["a", "b"]))),
    ])
    .await;
    let mut n = column("n", "UInt32");
    n.default_kind = "DEFAULT".into();
    n.default_expression = "id * 10".into();
    lake.add_column(n);
    lake.add_column(column("z", "String"));
    lake.rename_column("s", "label");
    lake.insert(vec![
        ("id", Arc::new(UInt32Array::from(vec![3])) as ArrayRef),
        ("label", Arc::new(StringArray::from(vec!["c"]))),
        ("n", Arc::new(UInt32Array::from(vec![7]))),
        ("z", Arc::new(StringArray::from(vec!["zz"]))),
    ])
    .await;
    let batch = rows(
        &lake.pin(),
        "SELECT id, label, plus, n, z FROM t ORDER BY id",
    )
    .await;
    let label = batch.column(1).as_string::<i32>();
    assert_eq!(
        (0..3)
            .map(|i| label.value(i).to_string())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"],
        "renamed"
    );
    let plus = datafusion::arrow::compute::cast(batch.column(2), &DataType::UInt64).unwrap();
    assert_eq!(
        plus.as_primitive::<datafusion::arrow::datatypes::UInt64Type>()
            .values()
            .to_vec(),
        vec![2, 3, 4]
    );
    assert_eq!(
        batch
            .column(3)
            .as_primitive::<UInt32Type>()
            .values()
            .to_vec(),
        vec![10, 20, 7],
        "the DEFAULT for older rows"
    );
    assert_eq!(batch.column(3).null_count(), 0);
    let z = batch.column(4).as_string::<i32>();
    assert_eq!((z.value(0), z.value(2)), ("", "zz"), "the type's default");
    assert_eq!(
        batch
            .schema()
            .field(2)
            .metadata()
            .get(CH_TYPE)
            .map(String::as_str),
        Some("UInt64")
    );
}

/// A delete `TTL` hides expired rows (R9.12).
#[tokio::test]
async fn ttl_hides_expired_rows() {
    let root = dir("ttl");
    let mut lake = Lake::create(
        &root,
        "default",
        "t",
        vec![column("id", "UInt32"), column("ts", "DateTime")],
        "MergeTree",
        "id",
        None,
        "ts",
    );
    lake.insert(vec![
        ("id", Arc::new(UInt32Array::from(vec![1, 2])) as ArrayRef),
        (
            "ts",
            Arc::new(TimestampSecondArray::from(vec![
                1_000_000_000,
                4_000_000_000,
            ])),
        ),
    ])
    .await;
    let batch = rows(&lake.pin(), "SELECT id FROM t").await;
    assert_eq!(
        batch
            .column(0)
            .as_primitive::<UInt32Type>()
            .values()
            .to_vec(),
        vec![2]
    );
}

/// Pruning reads fewer files on partition and key filters.
#[tokio::test]
async fn pruning_skips_files() {
    let root = dir("pruning");
    let mut lake = Lake::create(
        &root,
        "default",
        "t",
        vec![
            column("p", "UInt8"),
            column("id", "UInt64"),
            column("e", "Enum8('a' = 1, 'b' = 2)"),
        ],
        "MergeTree",
        "id",
        Some("p"),
        "",
    );
    for p in 0..4u8 {
        let ids: Vec<u64> = (0..100).map(|i| u64::from(p) * 1000 + i).collect();
        lake.insert(vec![
            ("p", Arc::new(UInt8Array::from(vec![p; 100])) as ArrayRef),
            ("id", Arc::new(UInt64Array::from(ids))),
            ("e", Arc::new(Int8Array::from(vec![1; 100]))),
        ])
        .await;
    }
    let pin = lake.pin();
    let count = |sql: &'static str| {
        let pin = pin.clone();
        async move {
            let table = Arc::new(LakeTable::open(&pin, local()).await.unwrap());
            let ctx = context();
            ctx.register_table("t", table.clone()).unwrap();
            let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
            let n = batches[0].column(0).as_primitive::<Int64Type>().value(0);
            (n, table.report().files)
        }
    };
    assert_eq!(count("SELECT count(*) FROM t").await, (400, 4));
    assert_eq!(
        count("SELECT count(*) FROM t WHERE p = 2").await,
        (100, 1),
        "partition"
    );
    assert_eq!(
        count("SELECT count(*) FROM t WHERE id >= 3050").await,
        (50, 1),
        "file statistics"
    );
    assert_eq!(
        count("SELECT count(*) FROM t WHERE id BETWEEN 1000 AND 2005").await,
        (106, 2)
    );
    // An Enum is stored by name: not pruned, still filtered.
    assert_eq!(count("SELECT count(*) FROM t WHERE e = 2").await, (0, 4));
}

/// `Sequenced` reads carry each row's file sequence number and position, as
/// `views.rs` gives chDB; no snapshot and a `Null` table read no rows.
#[tokio::test]
async fn sequenced_reads_and_empty_tables() {
    let root = dir("sequenced");
    let mut lake = Lake::create(
        &root,
        "default",
        "t",
        vec![column("id", "UInt32")],
        "ReplacingMergeTree",
        "id",
        None,
        "",
    );
    let mut empty = lake.pin();
    assert_eq!(empty.snapshot_id, None);
    empty.mode = ReadMode::Sequenced;
    assert_eq!(rows(&empty, "SELECT * FROM t").await.num_rows(), 0);
    for ids in [vec![5u32, 6], vec![5]] {
        lake.insert(vec![("id", Arc::new(UInt32Array::from(ids)) as ArrayRef)])
            .await;
    }
    let mut pin = lake.pin();
    pin.mode = ReadMode::Sequenced;
    let batch = rows(
        &pin,
        "SELECT id, __loams_seq, __loams_row FROM t ORDER BY __loams_seq, __loams_row",
    )
    .await;
    let seq = batch
        .column(1)
        .as_primitive::<Int64Type>()
        .values()
        .to_vec();
    let row = batch
        .column(2)
        .as_primitive::<Int64Type>()
        .values()
        .to_vec();
    assert_eq!(seq, vec![1, 1, 2]);
    assert_eq!(row, vec![0, 1, 0]);
    // Time travel to the first snapshot sees its rows.
    let first = lake
        .metadata
        .snapshots()
        .min_by_key(|s| s.sequence_number())
        .unwrap()
        .snapshot_id();
    let mut old = lake.pin();
    old.snapshot_id = Some(first);
    assert_eq!(rows(&old, "SELECT * FROM t").await.num_rows(), 2);
    let mut null = lake.pin();
    null.ch.engine = "Null".into();
    assert_eq!(rows(&null, "SELECT * FROM t").await.num_rows(), 0);
}
