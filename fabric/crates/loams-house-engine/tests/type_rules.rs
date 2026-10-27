//! ClickHouse's type rules on the engine (plan HE1 Task 24). The full check
//! is the oracle's `corpus/he1/24`; these tests pin the rules without it.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, BooleanArray, Date32Array, Int8Array, Int64Array, RecordBatch, StringArray,
    UInt8Array, UInt32Array,
};
use datafusion::arrow::datatypes::Schema;
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::Table;
use loams_house_types::arrow::to_arrow;
use loams_house_types::parse;

/// The engine's answer as the oracle writes it: the types line, then rows
/// (JSONCompactEachRowWithNamesAndTypes without the names).
async fn answer(engine: &Engine, sql: &str) -> Result<Vec<String>, i32> {
    let a = engine
        .query(sql, &[], &[], "default")
        .await
        .map_err(|e| e.code)?;
    let cols = loams_house_formats::columns(&a.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(&cols, &a.batches, &Default::default(), &mut out)
        .unwrap();
    Ok(String::from_utf8(out)
        .unwrap()
        .lines()
        .skip(1)
        .map(str::to_string)
        .collect())
}

/// A table `t` with one column per (name, ClickHouse type, values).
fn table(engine: &Engine, columns: Vec<(&str, &str, ArrayRef)>) {
    let mut fields = Vec::new();
    let mut arrays = Vec::new();
    for (name, ty, values) in columns {
        fields.push(to_arrow(name, &parse(ty).unwrap()).unwrap());
        arrays.push(values);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    engine.catalog.insert(
        "default",
        "t",
        Table {
            data: Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
            engine: "MergeTree".into(),
            sorting_key: "id".into(),
        },
    );
}

/// `fixtures/type_rules.tsv`: types × operators on constants, with the
/// oracle's types and values (`scripts/house/gen-type-cases.py --fixture`).
#[tokio::test]
async fn arithmetic_result_types_match_the_oracle() {
    let engine = Engine::new();
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/type_rules.tsv"
    ))
    .unwrap();
    let mut problems = Vec::new();
    let mut checked = 0;
    for line in fixture.lines().filter(|l| !l.starts_with('#')) {
        let mut parts = line.split('\t');
        let sql = parts.next().unwrap();
        let want: Vec<String> = parts.map(str::to_string).collect();
        checked += 1;
        match answer(&engine, sql).await {
            Ok(got) if got == want => {}
            Ok(got) => problems.push(format!("{sql}: {got:?}, the oracle {want:?}")),
            Err(code) => problems.push(format!("{sql}: error {code}, the oracle {want:?}")),
        }
    }
    assert!(checked > 1000, "only {checked} cases");
    assert!(
        problems.is_empty(),
        "{} of {checked}:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[tokio::test]
async fn string_literals_convert_to_the_column_type() {
    let engine = Engine::new();
    table(
        &engine,
        vec![
            ("id", "UInt8", Arc::new(UInt8Array::from(vec![1, 2, 3]))),
            (
                "d",
                "Date",
                Arc::new(Date32Array::from(vec![20_727, 20_728, 1])),
            ),
            (
                "e",
                "Enum8('a' = 1, 'b' = 2)",
                Arc::new(Int8Array::from(vec![1, 2, 1])),
            ),
            (
                "ip",
                "IPv4",
                Arc::new(UInt32Array::from(vec![0x0102_0304, 0x0a00_0001, 0])),
            ),
            ("u8", "UInt8", Arc::new(UInt8Array::from(vec![7, 1, 255]))),
        ],
    );
    let ids = |rows: Vec<String>| rows[1..].join(",");
    for (sql, want) in [
        ("SELECT id FROM t WHERE d = '2026-10-01' ORDER BY id", "[1]"),
        (
            "SELECT id FROM t WHERE d >= '2026-10-01' ORDER BY id",
            "[1],[2]",
        ),
        ("SELECT id FROM t WHERE e = 'b' ORDER BY id", "[2]"),
        ("SELECT id FROM t WHERE e != 'z' ORDER BY id", "[1],[2],[3]"),
        ("SELECT id FROM t WHERE ip = '10.0.0.1' ORDER BY id", "[2]"),
        ("SELECT id FROM t WHERE u8 = '255' ORDER BY id", "[3]"),
        (
            "SELECT id FROM t WHERE u8 < '300' ORDER BY id",
            "[1],[2],[3]",
        ),
    ] {
        assert_eq!(ids(answer(&engine, sql).await.unwrap()), want, "{sql}");
    }
    // Text that is no value of the type is ClickHouse's 53.
    for sql in [
        "SELECT u8 = 'x' FROM t",
        "SELECT u8 = '1.5' FROM t",
        "SELECT d = '2026-10-01 00:00:00' FROM t",
    ] {
        assert_eq!(answer(&engine, sql).await.unwrap_err(), 53, "{sql}");
    }
    // The comparison is a UInt8.
    assert_eq!(
        answer(&engine, "SELECT d = '2026-10-01' FROM t ORDER BY id")
            .await
            .unwrap(),
        vec!["[\"UInt8\"]", "[1]", "[0]", "[0]"]
    );
}

#[tokio::test]
async fn boolean_contexts_take_numbers() {
    let engine = Engine::new();
    table(
        &engine,
        vec![
            ("id", "UInt8", Arc::new(UInt8Array::from(vec![1, 2, 3]))),
            ("u8", "UInt8", Arc::new(UInt8Array::from(vec![0, 7, 1]))),
            ("i64", "Int64", Arc::new(Int64Array::from(vec![-1, 0, 5]))),
            (
                "b",
                "Bool",
                Arc::new(BooleanArray::from(vec![true, false, true])),
            ),
            (
                "s",
                "String",
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
            ),
        ],
    );
    for (sql, want) in [
        (
            "SELECT id FROM t WHERE u8 ORDER BY id",
            vec!["[\"UInt8\"]", "[2]", "[3]"],
        ),
        (
            "SELECT id FROM t WHERE i64 AND u8 ORDER BY id",
            vec!["[\"UInt8\"]", "[3]"],
        ),
        (
            "SELECT id FROM t WHERE NOT i64 ORDER BY id",
            vec!["[\"UInt8\"]", "[2]"],
        ),
        (
            "SELECT u8 AND i64, NOT u8, u8 > 1 FROM t ORDER BY id",
            vec![
                "[\"UInt8\",\"UInt8\",\"UInt8\"]",
                "[0,1,0]",
                "[0,0,1]",
                "[1,0,0]",
            ],
        ),
        (
            "SELECT b AND u8, (u8 > 1) + 1 FROM t ORDER BY id",
            vec![
                "[\"Bool\",\"UInt16\"]",
                "[false,1]",
                "[false,2]",
                "[true,1]",
            ],
        ),
        (
            "SELECT CASE WHEN u8 > 1 THEN u8 ELSE i64 END FROM t ORDER BY id",
            vec!["[\"Int64\"]", "[-1]", "[7]", "[5]"],
        ),
    ] {
        assert_eq!(answer(&engine, sql).await.unwrap(), want, "{sql}");
    }
    assert_eq!(
        answer(&engine, "SELECT id FROM t WHERE s")
            .await
            .unwrap_err(),
        59
    );
    assert_eq!(
        answer(&engine, "SELECT s AND 1 FROM t").await.unwrap_err(),
        43
    );
    assert_eq!(
        answer(&engine, "SELECT CASE WHEN i64 THEN 1 ELSE 0 END FROM t")
            .await
            .unwrap_err(),
        43
    );
}
