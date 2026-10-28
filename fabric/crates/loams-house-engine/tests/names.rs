//! Name resolution on the engine (plan HE1 Task 25): aliases in every clause,
//! shadowing, scalar `WITH`, matchers and the ternary. The full check is the
//! oracle's `corpus/he1/25`; these tests pin the rules without it. Expected
//! values are the oracle's (clickhouse-server 26.9.2.8, analyzer on).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray, UInt64Array};
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::util::display::array_value_to_string;
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::Table;
use loams_house_types::arrow::to_arrow;
use loams_house_types::parse;

/// The corpus's `t25` (without its tuple column) and `u25`.
fn engine() -> Engine {
    let engine = Engine::new();
    add(
        &engine,
        "t25",
        vec![
            ("a", "Int64", Arc::new(Int64Array::from(vec![1, 2, 3]))),
            (
                "b",
                "String",
                Arc::new(StringArray::from(vec!["x", "y", "x"])),
            ),
            (
                "c",
                "Float64",
                Arc::new(Float64Array::from(vec![1.5, 2.5, 3.5])),
            ),
            (
                "number",
                "UInt64",
                Arc::new(UInt64Array::from(vec![10, 20, 30])),
            ),
        ],
    );
    add(
        &engine,
        "u25",
        vec![
            ("a", "Int64", Arc::new(Int64Array::from(vec![1, 2, 4]))),
            (
                "d",
                "String",
                Arc::new(StringArray::from(vec!["one", "two", "four"])),
            ),
            ("k", "Int64", Arc::new(Int64Array::from(vec![7, 8, 9]))),
        ],
    );
    engine
}

fn add(engine: &Engine, name: &str, columns: Vec<(&str, &str, ArrayRef)>) {
    let mut fields = Vec::new();
    let mut arrays = Vec::new();
    for (col, ty, values) in columns {
        fields.push(to_arrow(col, &parse(ty).unwrap()).unwrap());
        arrays.push(values);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    engine.catalog.insert(
        "default",
        name,
        Table {
            data: Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
            engine: "MergeTree".into(),
            sorting_key: "a".into(),
        },
    );
}

/// The answer's column names and its rows, each value as Arrow prints it.
async fn run(engine: &Engine, sql: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let a = engine
        .query(sql, &[], &[], "default")
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    let names = a.schema.fields().iter().map(|f| f.name().clone()).collect();
    let mut rows = Vec::new();
    for b in &a.batches {
        for r in 0..b.num_rows() {
            rows.push(
                b.columns()
                    .iter()
                    .map(|c| array_value_to_string(c, r).unwrap())
                    .collect(),
            );
        }
    }
    (names, rows)
}

fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|r| r.iter().map(|v| v.to_string()).collect())
        .collect()
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

#[tokio::test]
async fn aliases_resolve_as_clickhouse() {
    let e = engine();
    // In WHERE and ORDER BY.
    let (n, r) = run(&e, "SELECT a + 1 AS x FROM t25 WHERE x > 2 ORDER BY x DESC").await;
    assert_eq!(n, names(&["x"]));
    assert_eq!(r, rows(&[&["4"], &["3"]]));
    // In another select item, which keeps its name as written.
    let (n, r) = run(&e, "SELECT a * 2 AS d, d + 1 FROM t25 ORDER BY a").await;
    assert_eq!(n, names(&["d", "plus(d, 1)"]));
    assert_eq!(r, rows(&[&["2", "3"], &["4", "5"], &["6", "7"]]));
    // In GROUP BY.
    let (n, r) = run(&e, "SELECT b AS g, sum(a) FROM t25 GROUP BY g ORDER BY g").await;
    assert_eq!(n, names(&["g", "sum(a)"]));
    assert_eq!(r, rows(&[&["x", "4"], &["y", "2"]]));
    // In JOIN ON.
    let (_, r) = run(
        &e,
        "SELECT t25.a + 0 AS ka, d FROM t25 JOIN u25 ON ka = u25.a ORDER BY ka",
    )
    .await;
    assert_eq!(r, rows(&[&["1", "one"], &["2", "two"]]));
    // An alias inside an expression is visible to the whole query.
    let (n, r) = run(&e, "SELECT (a + 1 AS p) * 2, p FROM t25 ORDER BY a").await;
    assert_eq!(n, names(&["multiply(p, 2)", "p"]));
    assert_eq!(r, rows(&[&["4", "2"], &["6", "3"], &["8", "4"]]));
    // The ternary's name is `if(…)` (`names_match_the_oracle`); `if` itself
    // comes with Task 33.
    // Matchers.
    let (n, _) = run(&e, "SELECT * EXCEPT d FROM u25 ORDER BY a").await;
    assert_eq!(n, names(&["a", "k"]));
    let (n, r) = run(&e, "SELECT * REPLACE (k * 10 AS k) FROM u25 ORDER BY a").await;
    assert_eq!(n, names(&["a", "d", "k"]));
    assert_eq!(r[0], vec!["1", "one", "70"]);
    let (n, _) = run(&e, "SELECT COLUMNS('^[ab]$') FROM t25").await;
    assert_eq!(n, names(&["a", "b"]));
}

#[tokio::test]
async fn alias_shadows_column() {
    let e = engine();
    // `prefer_column_name_to_alias = 0`: `number` in the second item is the
    // alias; inside the alias's own definition it is the column.
    let (n, r) = run(
        &e,
        "SELECT number + 1 AS number, number * 10 FROM t25 ORDER BY a",
    )
    .await;
    assert_eq!(n, names(&["number", "multiply(number, 10)"]));
    assert_eq!(r, rows(&[&["11", "110"], &["21", "210"], &["31", "310"]]));
    // In WHERE too.
    let (n, r) = run(&e, "SELECT a + 10 AS a FROM t25 WHERE a > 12").await;
    assert_eq!(n, names(&["a"]));
    assert_eq!(r, rows(&[&["13"]]));
}

#[tokio::test]
async fn scalar_with_is_substituted() {
    let e = engine();
    let (n, r) = run(&e, "WITH 5 AS n SELECT a + n FROM t25 ORDER BY a").await;
    assert_eq!(n, names(&["plus(a, n)"]));
    assert_eq!(r, rows(&[&["6"], &["7"], &["8"]]));
    // A scalar subquery.
    let (n, r) = run(
        &e,
        "WITH (SELECT max(a) FROM t25) AS m SELECT a FROM t25 WHERE a = m",
    )
    .await;
    assert_eq!(n, names(&["a"]));
    assert_eq!(r, rows(&[&["3"]]));
    // At its own level a WITH name shadows a column.
    let (n, r) = run(&e, "WITH c + 1 AS a SELECT a, b FROM t25 ORDER BY b, a").await;
    assert_eq!(n, names(&["a", "b"]));
    assert_eq!(r, rows(&[&["2.5", "x"], &["4.5", "x"], &["3.5", "y"]]));
}
