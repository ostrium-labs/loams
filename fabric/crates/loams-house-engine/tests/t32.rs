//! Arrays, maps, tuples and lambdas on the Rust engine (plan HE1 Task 32).
//!
//! The authority is the oracle job over `corpus/he1/32`; these tests pin the
//! engine's answers for the same forms over a small in-memory table, with
//! values as ClickHouse documents them (`docs/en/sql-reference/functions/
//! array-functions.md`, `tuple-map-functions.md`).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, RecordBatch, StringArray, StructArray, UInt8Array, UInt32Array, UInt64Array,
};
use datafusion::arrow::datatypes::{DataType, Schema};
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::Table;
use loams_house_exec::func::maps::build_map;
use loams_house_exec::func::nested::values::build_list;
use loams_house_types::arrow::{data_type, to_arrow};
use loams_house_types::{ChType, parse};

fn t(text: &str) -> ChType {
    parse(text).unwrap()
}

/// `t32(id UInt32, arr Array(String), nums Array(UInt8), m Map(String,
/// UInt64), tup Tuple(UInt8, String))` with two rows: `(1, ['a', 'b'],
/// [10, 20, 30], {'k': 1}, (7, 'x'))` and `(2, [], [], {}, (0, ''))`.
fn engine() -> Engine {
    let engine = Engine::new();
    let types = [
        ("id", t("UInt32")),
        ("arr", t("Array(String)")),
        ("nums", t("Array(UInt8)")),
        ("m", t("Map(String, UInt64)")),
        ("tup", t("Tuple(UInt8, String)")),
    ];
    let schema = Arc::new(Schema::new(
        types
            .iter()
            .map(|(n, ty)| to_arrow(n, ty).unwrap())
            .collect::<Vec<_>>(),
    ));
    let dt = |i: usize| data_type(&types[i].1).unwrap().0;
    let arr = build_list(&dt(1), [2, 0], Arc::new(StringArray::from(vec!["a", "b"]))).unwrap();
    let nums = build_list(&dt(2), [3, 0], Arc::new(UInt8Array::from(vec![10, 20, 30]))).unwrap();
    let m = build_map(
        &dt(3),
        &[1, 0],
        Arc::new(StringArray::from(vec!["k"])),
        Arc::new(UInt64Array::from(vec![1])),
    )
    .unwrap();
    let DataType::Struct(fields) = dt(4) else {
        panic!("a tuple is a struct")
    };
    let tup: ArrayRef = Arc::new(
        StructArray::try_new(
            fields,
            vec![
                Arc::new(UInt8Array::from(vec![7, 0])),
                Arc::new(StringArray::from(vec!["x", ""])),
            ],
            None,
        )
        .unwrap(),
    );
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(UInt32Array::from(vec![1, 2])), arr, nums, m, tup],
    )
    .unwrap();
    engine.catalog.insert(
        "default",
        "t32",
        Table {
            data: Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
            engine: "MergeTree".into(),
            sorting_key: "id".into(),
        },
    );
    engine
}

/// The answer as `JSONCompactEachRowWithNamesAndTypes` lines: names, types,
/// then one line per row.
async fn answer(engine: &Engine, sql: &str) -> Vec<String> {
    let answer = engine.query(sql, &[], &[], "default").await.unwrap();
    let cols = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(&cols, &answer.batches, &Default::default(), &mut out)
        .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

async fn rows(engine: &Engine, sql: &str) -> Vec<String> {
    answer(engine, sql).await.into_iter().skip(2).collect()
}

#[tokio::test]
async fn array_functions_match_the_oracle() {
    let e = engine();
    assert_eq!(
        rows(
            &e,
            "SELECT nums[1], nums[-1], nums[10], arrayElement(nums, 0) FROM t32 ORDER BY id"
        )
        .await,
        vec!["[10,30,0,0]", "[0,0,0,0]"]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT has(arr, 'b'), indexOf(arr, 'b'), length(arr), empty(arr), notEmpty(arr) FROM t32 ORDER BY id"
        )
        .await,
        vec!["[1,2,2,0,1]", "[0,0,0,1,0]"]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT arraySlice(nums, 2), arraySlice(nums, -2, 1), arrayReverse(nums), arrayEnumerate(arr) FROM t32 WHERE id = 1"
        )
        .await,
        vec!["[[20,30],[20],[30,20,10],[1,2]]"]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT arrayConcat(arr, ['c']), arrayStringConcat(arr, '-'), arrayDistinct(arrayConcat(arr, arr)), arrayEnumerateUniq(arrayConcat(arr, arr)) FROM t32 WHERE id = 1"
        )
        .await,
        vec![r#"[["a","b","c"],"a-b",["a","b"],[1,1,2,2]]"#]
    );
    assert_eq!(
        rows(&e, "SELECT arrayZip(arr, ['x', 'y']), hasAll(arr, ['a']), hasAny(arr, ['z']) FROM t32 WHERE id = 1").await,
        vec![r#"[[["a","x"],["b","y"]],1,0]"#]
    );
    assert_eq!(
        rows(&e, "SELECT m['k'], m['zz'], mapKeys(m), mapValues(m), mapContains(m, 'k') FROM t32 ORDER BY id").await,
        vec![r#"[1,0,["k"],[1],1]"#, "[0,0,[],[],0]"]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT tupleElement(tup, 2), tup.1, tuple(id, 'z') FROM t32 WHERE id = 1"
        )
        .await,
        vec![r#"["x",7,[1,"z"]]"#]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT mapFromArrays(arr, arr)['b'], {'a': 1}['a'] FROM t32 WHERE id = 1"
        )
        .await,
        vec![r#"["b",1]"#]
    );
    // Arrays of different lengths are 190, as ClickHouse.
    let err = e
        .query(
            "SELECT arrayZip(arr, nums) FROM t32 WHERE id = 1",
            &[],
            &[],
            "default",
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, 190, "{err}");
    // arrayJoin is typed but runs only once Task 35 hoists it.
    let err = e
        .query("SELECT arrayJoin(arr) FROM t32", &[], &[], "default")
        .await
        .unwrap_err();
    assert_eq!(err.code, 48, "{err}");
}

#[tokio::test]
async fn lambdas_over_several_arrays() {
    let e = engine();
    assert_eq!(
        rows(
            &e,
            "SELECT arrayMap((x, i) -> i, arr, arrayEnumerate(arr)), arrayFilter((x, n) -> n > 15, nums, nums), arrayExists(x -> x = 'b', arr), arrayFirstIndex(x -> x = 'b', arr), arrayCount(x -> x > 10, nums) FROM t32 WHERE id = 1"
        )
        .await,
        vec!["[[1,2],[20,30],1,2,2]"]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT arraySort((x, k) -> k, arr, [2, 1]), arrayFirst(x -> x > 10, nums), arrayAll(x -> x > 5, nums), arraySum(nums) FROM t32 WHERE id = 1"
        )
        .await,
        vec![r#"[["b","a"],20,1,60]"#]
    );
    assert_eq!(
        rows(
            &e,
            "SELECT mapFilter((k, v) -> v > 0, m), mapFilter(kv -> kv.1 = 'zz', m) FROM t32 WHERE id = 1"
        )
        .await,
        vec![r#"[{"k":1},{}]"#]
    );
    // A lambda reads the row's other columns too.
    assert_eq!(
        rows(&e, "SELECT arrayMap(x -> id, nums) FROM t32 ORDER BY id").await,
        vec!["[[1,1,1]]", "[[]]"]
    );
    let err = e
        .query(
            "SELECT arrayMap((x, y) -> x, arr, nums) FROM t32 WHERE id = 1",
            &[],
            &[],
            "default",
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, 190, "{err}");
}

#[tokio::test]
async fn untuple_expands_into_columns() {
    let e = engine();
    let lines = answer(&e, "SELECT untuple(tup) FROM t32 WHERE id = 1").await;
    assert_eq!(
        lines[0],
        r#"["tupleElement(tup, '1')","tupleElement(tup, '2')"]"#
    );
    assert_eq!(lines[2], r#"[7,"x"]"#);
}

/// Every Tier 0 function of this family in `profile.toml` is registered,
/// as a scalar or a higher-order function, under its internal name.
#[test]
fn every_tier0_array_function_is_registered() {
    let profile = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../conformance/clickhouse/profile/profile.toml"
    ))
    .unwrap();
    let family = |f: &str| {
        f.starts_with("array")
            || f.starts_with("map")
            || f.starts_with("tuple")
            || [
                "has", "hasAll", "hasAny", "indexOf", "length", "empty", "notEmpty", "range",
                "untuple",
            ]
            .contains(&f)
    };
    let mut tier0 = Vec::new();
    let mut current: Option<String> = None;
    for line in profile.lines() {
        if let Some(name) = line.strip_prefix("name = \"function:") {
            current = Some(name.trim_end_matches('"').to_string());
        } else if line.trim() == "tier = 0"
            && let Some(f) = current.take()
            && family(&f)
        {
            tier0.push(f);
        } else if line.starts_with("[[feature]]") {
            current = None;
        }
    }
    assert!(tier0.len() >= 30, "{tier0:?}");
    let registry = loams_house_engine::registry::registry();
    for f in &tier0 {
        let known = loams_house_sql::functions::resolve(f).unwrap_or_else(|| panic!("{f}"));
        let internal = loams_house_sql::functions::internal_name(&known.canonical);
        assert!(
            registry.scalars.contains_key(&internal)
                || registry.higher_order.contains_key(&internal),
            "{f} ({internal}) is not registered"
        );
    }
}
