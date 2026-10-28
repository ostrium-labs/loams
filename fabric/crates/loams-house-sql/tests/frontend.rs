//! The front end's core (plan HE1 Task 22): names against the oracle's
//! answers, the silent-drop guard, function names, and plans that run.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{Int64Array, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::TableSource;
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::ast::{SelectItem, SetExpr, Statement};
use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::parser::Parser;
use loams_house_sql::{Catalog, Registry, guard, lex, names, params, prepare};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Forms the pre-pass does not read yet, with the task that brings them
/// (the ternary came with Task 25).
const NAME_GAPS: &[(&str, u32)] = &[];

#[test]
fn names_match_the_oracle() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for line in fixture("names.tsv").lines() {
        let (expr, name) = line.split_once('\t').unwrap();
        let expected: String = serde_json::from_str(name).unwrap();
        if expected == "ERROR" || NAME_GAPS.iter().any(|(e, _)| *e == expr) {
            continue;
        }
        let sql = params::bind(
            &format!("SELECT {expr} FROM numbers(1)"),
            &[("p".into(), "7".into())],
        )
        .unwrap();
        let pre = lex::pre_pass(&sql).unwrap();
        let parsed = match Parser::parse_sql(&ClickHouseDialect {}, &pre.sql) {
            Ok(mut s) => s.pop().unwrap(),
            Err(e) => {
                problems.push(format!("{expr}: {e}"));
                continue;
            }
        };
        let Statement::Query(q) = parsed else {
            unreachable!()
        };
        let SetExpr::Select(select) = *q.body else {
            unreachable!()
        };
        let got = match &select.projection[0] {
            SelectItem::UnnamedExpr(e) => names::column_name(e),
            SelectItem::ExprWithAlias { alias, .. } => alias.value.clone(),
            other => format!("{other}"),
        };
        checked += 1;
        if got != expected {
            problems.push(format!("{expr}: {got:?}, the oracle {expected:?}"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(checked > 60, "only {checked} names checked");
}

/// Every construct DataFusion 54–55's `SqlToRel` ignores (dialect spike
/// §3.1) is refused, never planned without it.
#[test]
fn no_ignored_field_reaches_sqltorel() {
    let refused = [
        "SELECT x FROM t GROUP BY x WITH TOTALS",
        "SELECT x FROM t GROUP BY x WITH ROLLUP",
        "SELECT x FROM t GROUP BY x WITH CUBE",
        "SELECT x FROM t SAMPLE 0.1",
        "SELECT x FROM t FINAL",
        "SELECT x FROM db.t AS a FINAL WHERE x = 1",
        "SELECT x FROM t LIMIT 1 BY x",
        "SELECT x FROM t ORDER BY x WITH FILL",
        "SELECT x FROM t ARRAY JOIN a",
        "SELECT x FROM t LEFT ARRAY JOIN a AS y",
        "SELECT * FROM t ANY LEFT JOIN u USING (k)",
        "SELECT * FROM t SEMI LEFT JOIN u USING (k)",
        "SELECT x FROM t ORDER BY x INTERPOLATE",
        "SELECT x FROM (SELECT x FROM t SETTINGS max_threads = 1)",
    ];
    for sql in refused {
        match guard(sql, &[]) {
            Err(e) => assert_eq!(e.code, 48, "{sql}: {e}"),
            Ok(g) => panic!("{sql} was let through as {}", g.statement),
        }
    }
}

#[test]
fn prewhere_folds_into_where() {
    let g = guard("SELECT x FROM t PREWHERE a = 1 WHERE b = 2", &[]).unwrap();
    assert!(
        g.statement
            .to_string()
            .contains("WHERE (a = 1) AND (b = 2)"),
        "{}",
        g.statement
    );
    let g = guard("SELECT x FROM t PREWHERE a = 1", &[]).unwrap();
    assert!(
        g.statement.to_string().contains("WHERE a = 1"),
        "{}",
        g.statement
    );
}

#[test]
fn settings_and_format_belong_to_the_statement() {
    let g = guard(
        "SELECT 1 SETTINGS max_threads = 2, join_use_nulls = 1 FORMAT JSONEachRow",
        &[],
    )
    .unwrap();
    assert_eq!(
        g.settings,
        vec![
            ("max_threads".to_string(), "2".to_string()),
            ("join_use_nulls".to_string(), "1".to_string())
        ]
    );
    assert_eq!(g.format.as_deref(), Some("JSONEachRow"));
    assert!(!g.statement.to_string().contains("SETTINGS"));
}

#[test]
fn function_names_keep_their_case() {
    assert_eq!(
        guard("SELECT tostartofday(x) FROM t", &[])
            .unwrap_err()
            .code,
        46
    );
    let g = guard(
        "SELECT COUNT(*), toStartOfDay(x), DATE_DIFF('day', a, b), countIf(x > 1) FROM t",
        &[],
    )
    .unwrap();
    let f: Vec<&str> = g.resolution.functions.iter().map(String::as_str).collect();
    assert_eq!(f, vec!["count", "countIf", "dateDiff", "toStartOfDay"]);
    // Quoted, so DataFusion looks the name up as written.
    assert!(
        g.statement.to_string().contains("\"toStartOfDay\"(x)"),
        "{}",
        g.statement
    );
}

#[test]
fn final_is_never_an_alias() {
    // Refused for now (plan HE1 Task 27), never read as an alias.
    for sql in [
        "SELECT x FROM t FINAL",
        "SELECT x FROM t final",
        "SELECT x FROM t AS a FINAL",
    ] {
        let e = guard(sql, &[]).unwrap_err();
        assert!(e.message.contains("FINAL"), "{sql}: {e}");
    }
    let g = guard("SELECT x FROM t AS `FINAL`", &[]).unwrap();
    assert_eq!(g.resolution.tables[0].name, vec!["t".to_string()]);
}

#[test]
fn multi_statements_and_other_statements_are_refused() {
    assert_eq!(guard("SELECT 1; SELECT 2", &[]).unwrap_err().code, 62);
    assert_eq!(guard("INSERT INTO t VALUES (1)", &[]).unwrap_err().code, 48);
    assert_eq!(guard("SELEC 1", &[]).unwrap_err().code, 62);
}

/// The dialect spike's 20 queries (`spike_plan20.sql`, HE0 Task 10): each is
/// guarded or refused with a ClickHouse code; the ones whose construct a
/// later task brings are refused, never let through.
#[test]
fn spike_queries_are_guarded_or_refused() {
    let text = fixture("spike_plan20.sql");
    let mut id = String::new();
    let mut seen = 0;
    let refused_now = ["q03", "q04", "q05", "q06", "q07", "q08", "q11"];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("-- @id ") {
            id = rest.trim().to_string();
            continue;
        }
        if line.starts_with("--") || line.trim().is_empty() {
            continue;
        }
        seen += 1;
        let result = guard(
            line,
            &[
                ("p".into(), "x".into()),
                ("from".into(), "2026-01-01 00:00:00".into()),
            ],
        );
        let short = &id[..3];
        if refused_now.contains(&short) {
            let e = result.expect_err(&id);
            assert_eq!(e.code, 48, "{id}: {e}");
        } else if let Err(e) = result {
            assert!([46, 47, 48, 62, 456].contains(&e.code), "{id}: {e}");
        }
    }
    assert_eq!(seen, 20);
}

#[derive(Default)]
struct Mem(HashMap<(String, String), Arc<dyn TableSource>>);

impl std::fmt::Debug for Mem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Catalog for Mem {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        self.0
            .get(&(database.to_string(), name.to_string()))
            .cloned()
    }
}

fn catalog() -> (Mem, Arc<MemTable>) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Int64, false),
        Field::new("b", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 2])),
            Arc::new(StringArray::from(vec!["x", "x", "y"])),
        ],
    )
    .unwrap();
    let table = Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap());
    let mut m = Mem::default();
    m.0.insert(("db".into(), "t".into()), provider_as_source(table.clone()));
    (m, table)
}

#[tokio::test]
async fn plans_name_columns_as_clickhouse_and_run() {
    let (cat, table) = catalog();
    let mut registry = Registry::default();
    registry.aggregates.insert(
        "count".into(),
        datafusion::functions_aggregate::count::count_udaf(),
    );
    let p = prepare(
        "SELECT a + 1, count() FROM t WHERE b = 'x' GROUP BY a ORDER BY a",
        &[],
        &cat,
        &registry,
        "db",
    )
    .unwrap();
    let names: Vec<String> = p
        .plan
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, vec!["plus(a, 1)", "count()"]);
    assert_eq!(p.resolution.tables[0].name, vec!["t".to_string()]);
    let ctx = SessionContext::new();
    ctx.register_table("t", table).unwrap();
    let batches = ctx
        .execute_logical_plan(p.plan)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
}

#[test]
fn unknown_tables_are_60() {
    let (cat, _) = catalog();
    let e = prepare("SELECT a FROM nope", &[], &cat, &Registry::default(), "db").unwrap_err();
    assert_eq!(e.code, 60, "{e}");
}

#[test]
fn casts_to_clickhouse_types_plan() {
    let (cat, _) = catalog();
    let p = prepare(
        "SELECT CAST(a, 'Int8') AS x, _CAST(b, 'LowCardinality(String)') AS y FROM t",
        &[],
        &cat,
        &Registry::default(),
        "db",
    )
    .unwrap();
    let schema = p.plan.schema();
    assert_eq!(schema.field(0).data_type(), &DataType::Int8);
    assert!(
        matches!(schema.field(1).data_type(), DataType::Dictionary(..)),
        "{:?}",
        schema.field(1)
    );
}

#[test]
fn show_lists_are_guarded_as_their_selects() {
    let g =
        loams_house_sql::guard_in("SHOW TABLES LIKE 'a%' FORMAT JSONEachRow", &[], "db").unwrap();
    assert_eq!(
        g.resolution.tables[0].name,
        vec!["system".to_string(), "tables".to_string()]
    );
    assert_eq!(g.format.as_deref(), Some("JSONEachRow"));
    assert_eq!(guard("DESCRIBE TABLE t", &[]).unwrap_err().code, 48);
}

#[test]
fn table_functions_are_safe_or_refused() {
    let g = guard("SELECT number FROM numbers(10)", &[]).unwrap();
    assert!(g.resolution.table_functions.contains("numbers"));
    assert!(g.resolution.tables.is_empty());
    for sql in [
        "SELECT * FROM url('http://x/', CSV, 'a String')",
        "SELECT * FROM remote('h', system.one)",
        "SELECT * FROM deltaLake('s3://x')",
        "SELECT * FROM clusterAllReplicas('c', db.t)",
    ] {
        assert_eq!(guard(sql, &[]).unwrap_err().code, 344, "{sql}");
    }
    let g = guard("SELECT * FROM clusterAllReplicas('c', system.tables)", &[]).unwrap();
    assert_eq!(
        g.resolution.tables[0].name,
        vec!["system".to_string(), "tables".to_string()]
    );
    assert_eq!(guard("SELECT * FROM numbers(x)", &[]).unwrap_err().code, 48);
    assert_eq!(
        guard("SELECT * FROM numbers(1, 2, 3, 4)", &[])
            .unwrap_err()
            .code,
        42
    );
    assert_eq!(
        guard("SELECT * FROM generateRandom('a Int8')", &[])
            .unwrap_err()
            .code,
        48
    );
}

/// Array, tuple and map literals and subscripts become the House's own
/// functions (plan HE1 Task 32), and lambda parameters keep their case.
#[test]
fn composite_literals_and_subscripts_become_functions() {
    let g = guard(
        "SELECT [1, 2][1], {'a': 1}['a'], (1, 'x'), arrayMap(X -> X + 1, a), mapFilter(kv -> kv.1 = 'a', m), mapFilter((k, v) -> v > 0, m) FROM t",
        &[],
    )
    .unwrap();
    let text = g.statement.to_string();
    for want in [
        r#""arrayElement"("array"(1, 2), 1)"#,
        r#""arrayElement"("__ch_map"('a', 1), 'a')"#,
        r#""tuple"(1, 'x')"#,
        r#""X" -> X + 1"#,
        r#""__ch_mapFilter_tuple"("kv" -> "tupleElement"(kv, 1) = 'a', m)"#,
        r#""mapFilter"(("k", "v") -> v > 0, m)"#,
        r#"AS "arrayElement([1, 2], 1)""#,
        r#"AS "arrayElement(map('a', 1), 'a')""#,
    ] {
        assert!(text.contains(want), "{want} not in {text}");
    }
    let f: Vec<&str> = g.resolution.functions.iter().map(String::as_str).collect();
    for name in [
        "array",
        "arrayElement",
        "map",
        "tuple",
        "mapFilter",
        "tupleElement",
    ] {
        assert!(f.contains(&name), "{name} not in {f:?}");
    }
    assert!(guard("SELECT untuple(t) FROM x", &[]).is_ok());
}
