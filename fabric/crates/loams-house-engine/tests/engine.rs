//! The engine and its seeding (plan HE1 Task 23). The full check is the
//! oracle job (`house-engine-oracle`), which seeds every Tier 0 type from the
//! pinned clickhouse-server; these tests pin the conversions without it.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Int64Array, ListArray, RecordBatch, StringArray, UInt8Array,
    UInt32Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Int8Type, Schema, TimeUnit};
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::Table;
use loams_house_engine::seed::to_engine;
use loams_house_types::arrow::to_arrow;
use loams_house_types::parse;

#[test]
fn enum_names_become_values() {
    let names: ArrayRef = Arc::new(StringArray::from(vec![Some("b"), None, Some("a")]));
    let t = parse("Nullable(Enum8('a' = 1, 'b' = 2))").unwrap();
    let out = to_engine(&t, &names).unwrap();
    let v = out.as_primitive::<Int8Type>();
    assert_eq!((v.value(0), v.is_null(1), v.value(2)), (2, true, 1));
    assert!(
        to_engine(
            &parse("Enum8('a' = 1)").unwrap(),
            &(Arc::new(StringArray::from(vec!["z"])) as ArrayRef)
        )
        .is_err()
    );
}

#[test]
fn integer_times_become_timestamps() {
    let secs: ArrayRef = Arc::new(UInt32Array::from(vec![1_792_717_323]));
    let out = to_engine(&parse("DateTime('UTC')").unwrap(), &secs).unwrap();
    assert_eq!(
        out.data_type(),
        &DataType::Timestamp(TimeUnit::Second, Some("UTC".into()))
    );
    // DateTime64(2) ticks are hundredths: milliseconds times ten.
    let ticks: ArrayRef = Arc::new(Int64Array::from(vec![179_271_732_325]));
    let out = to_engine(&parse("DateTime64(2, 'UTC')").unwrap(), &ticks).unwrap();
    let ms = out.as_primitive::<datafusion::arrow::datatypes::TimestampMillisecondType>();
    assert_eq!(ms.value(0), 1_792_717_323_250);
}

#[test]
fn plain_strings_become_low_cardinality_and_nested_values_convert() {
    let plain: ArrayRef = Arc::new(StringArray::from(vec!["x", "y", "x"]));
    let out = to_engine(&parse("LowCardinality(String)").unwrap(), &plain).unwrap();
    assert!(matches!(out.data_type(), DataType::Dictionary(..)));
    // Array(Bool) from ClickHouse's UInt8 values.
    let values: ArrayRef = Arc::new(UInt8Array::from(vec![1, 0]));
    let field = Arc::new(Field::new("item", DataType::UInt8, false));
    let list: ArrayRef = Arc::new(ListArray::new(
        field,
        datafusion::arrow::buffer::OffsetBuffer::new(vec![0, 2].into()),
        values,
        None,
    ));
    let t = parse("Array(Bool)").unwrap();
    let out = to_engine(&t, &list).unwrap();
    assert_eq!(out.data_type(), to_arrow("v", &t).unwrap().data_type());
}

#[tokio::test]
async fn queries_run_over_seeded_tables() {
    let engine = Engine::new();
    let t = parse("LowCardinality(String)").unwrap();
    let schema = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Int64, false),
        to_arrow("g", &t).unwrap(),
    ]));
    let g = to_engine(
        &t,
        &(Arc::new(StringArray::from(vec!["x", "y", "x"])) as ArrayRef),
    )
    .unwrap();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(vec![1, 2, 3])), g],
    )
    .unwrap();
    let data = Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap());
    engine.catalog.insert(
        "default",
        "t",
        Table {
            data,
            engine: "MergeTree".into(),
            sorting_key: "a".into(),
        },
    );
    let answer = engine.query("SELECT g, a + 1 FROM t WHERE a > 1 ORDER BY a FORMAT JSONCompactEachRowWithNamesAndTypes", &[], &[], "default").await.unwrap();
    let cols = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(&cols, &answer.batches, &Default::default(), &mut out)
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    // `g` keeps its ClickHouse type through the plan; `plus(a, 1)` is
    // ClickHouse's name (its type, Int64 here, is Task 24's).
    assert!(
        text.starts_with("[\"g\",\"plus(a, 1)\"]\n[\"LowCardinality(String)\","),
        "{text}"
    );
    assert!(text.ends_with("[\"y\",3]\n[\"x\",4]\n"), "{text}");
    assert_eq!(
        answer.format.as_deref(),
        Some("JSONCompactEachRowWithNamesAndTypes")
    );
}

#[tokio::test]
async fn settings_are_classed_never_ignored() {
    let engine = Engine::new();
    // No outer join: join_use_nulls asks for nothing here (Task 37).
    engine
        .query("SELECT 1 SETTINGS join_use_nulls = 1", &[], &[], "default")
        .await
        .unwrap();
    engine
        .query(
            "SELECT 1",
            &[],
            &[("max_threads".into(), "2".into())],
            "default",
        )
        .await
        .unwrap();
    let e = engine
        .query(
            "SELECT 1 SETTINGS prefer_column_name_to_alias = 1",
            &[],
            &[],
            "default",
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, 48);
    let e = engine
        .query("SELECT 1 SETTINGS no_such_setting = 1", &[], &[], "default")
        .await
        .unwrap_err();
    assert_eq!(e.code, 115);
    let e = engine
        .query("SELECT a FROM nope", &[], &[], "default")
        .await
        .unwrap_err();
    assert_eq!(e.code, 60);
}
