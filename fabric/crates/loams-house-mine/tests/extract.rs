//! The extractor over the hand-written fixture (plan HE1 Task 2): one test
//! per feature kind, the trace test (Review Focus 1) and the unparsed test.

mod common;

use loams_house_mine::explain;
use loams_house_mine::explain::HttpExplainer;
use loams_house_mine::inventory::{self, Path, mine_statement};

use common::{inventory, sources, sources_of};

fn has(feature: &str, ids: &[&str]) {
    let inv = inventory();
    let got = sources_of(&inv, feature);
    for id in ids {
        assert!(
            got.iter().any(|g| g == id),
            "{feature}: {id} not in {got:?}"
        );
    }
}

#[test]
fn functions_include_operators_under_their_clickhouse_names() {
    has("function:count", &["q:q01", "q:q05"]);
    has("function:equals", &["q:q01", "q:q08"]);
    has("function:toStartOfHour", &["q:q02"]);
    has("function:JSONExtractString", &["q:q07"]);
    has("function:toIntervalDay", &["q:q07"]);
    has("function:tupleElement", &["q:q10"]);
    has("function:arrayMap", &["q:q10"]);
    has("function:lambda", &["q:q10"]);
    has("function:toYYYYMM", &["m:0001_traces.up.sql:2"]);
    has("function:windowFunnel", &["q:q05"]);
    has("function:like", &["q:q19"]);
    let inv = inventory();
    // Bound parameters and codecs are not functions.
    for not_a_function in [
        "function:_CAST",
        "function:_cast",
        "function:CODEC",
        "function:codec",
        "function:ZSTD",
    ] {
        assert!(
            !inv.features.contains_key(not_a_function),
            "{not_a_function}"
        );
    }
}

#[test]
fn combinators_and_their_base_functions() {
    has("function:countIf", &["q:q02"]);
    has("combinator:If", &["q:q02", "q:q18"]);
    has("combinator:Merge", &["q:q18"]);
    has("function:uniq", &["q:q18"]);
    has("combinator:State", &["m:clustered/0002_views.up.sql:3"]);
    has("syntax:parametric function", &["q:q02", "q:q05"]);
}

#[test]
fn table_functions() {
    has("table_function:numbers", &["q:q10"]);
    // `ARRAY JOIN f(x) AS a, g(y) AS b` holds functions, not table functions.
    has("function:mapKeys", &["q:q24", "m:0001_traces.up.sql:25"]);
    has("function:mapValues", &["q:q24"]);
    let inv = inventory();
    let table_functions: Vec<_> = inv
        .features
        .keys()
        .filter(|f| f.starts_with("table_function:"))
        .collect();
    assert_eq!(table_functions, ["table_function:numbers"]);
    // An index's expression is not its type.
    assert!(!inv.features.contains_key("syntax:skip index mapKeys"));
    has(
        "syntax:skip index bloom_filter",
        &["m:0001_traces.up.sql:25"],
    );
}

#[test]
fn templated_migrations_are_read_with_their_own_ids() {
    // Two statements on one line get two ids; a `{PLACEHOLDER}` template is
    // read with the placeholder removed.
    has(
        "syntax:template placeholder",
        &["m:0001_traces.up.sql:27", "m:0001_traces.up.sql:27.2"],
    );
    has("syntax:ALTER ADD COLUMN", &["m:0001_traces.up.sql:27.2"]);
    inventory::check(&inventory()).unwrap();
}

#[test]
fn down_migrations_are_not_read() {
    let inv = inventory();
    assert!(
        inv.samples.keys().all(|id| !id.contains(".down.sql")),
        "{:?}",
        inv.samples.keys()
    );
    assert!(!inv.features.contains_key("syntax:DROP TABLE"));
}

#[test]
fn engines() {
    has("engine:ReplacingMergeTree", &["m:0001_traces.up.sql:2"]);
    has("engine:MergeTree", &["m:0001_traces.up.sql:21"]);
    has(
        "engine:ReplicatedAggregatingMergeTree",
        &["m:clustered/0002_views.up.sql:1"],
    );
    let inv = inventory();
    // The partition key's function is not an engine.
    assert!(!inv.features.contains_key("engine:toYYYYMM"));
}

#[test]
fn types() {
    for t in [
        "String",
        "DateTime64",
        "Array",
        "Map",
        "LowCardinality",
        "Enum8",
        "Nullable",
        "Decimal",
        "UInt8",
    ] {
        has(&format!("type:{t}"), &["m:0001_traces.up.sql:2"]);
    }
    has("type:UUID", &["m:0001_traces.up.sql:21", "q:q21"]);
    has(
        "type:AggregateFunction",
        &["m:clustered/0002_views.up.sql:1"],
    );
    // Query parameters and casts declare types too.
    has("type:DateTime64", &["q:q03"]);
    has("type:Nullable", &["q:q10"]);
    has("type:UInt8", &["q:q10"]);
}

#[test]
fn formats() {
    has("format:JSONEachRow", &["q:q03", "q:q04"]);
}

#[test]
fn settings_from_the_client_and_from_clauses() {
    has("setting:async_insert", &["q:q04"]);
    has("setting:wait_for_async_insert", &["q:q04"]);
    has(
        "setting:output_format_json_quote_64bit_integers",
        &["q:q03"],
    );
    has("setting:max_execution_time", &["q:q05"]);
    has("setting:index_granularity", &["m:0001_traces.up.sql:2"]);
    // The harness's own profile settings are never the application's.
    assert!(!inventory().features.contains_key("setting:log_queries"));
}

#[test]
fn syntax_forms() {
    for (form, ids) in [
        ("FINAL", &["q:q01"][..]),
        ("PREWHERE", &["q:q02"]),
        ("WITH TOTALS", &["q:q02"]),
        ("LIMIT BY", &["q:q03"]),
        ("query parameter", &["q:q03"]),
        ("ARRAY JOIN", &["q:q06"]),
        ("HAVING", &["q:q06"]),
        ("WITH", &["q:q07"]),
        ("window function", &["q:q07"]),
        ("LEFT JOIN", &["q:q07", "q:q11"]),
        ("UNION ALL", &["q:q07"]),
        ("ALTER DELETE", &["q:q08"]),
        ("ALTER UPDATE", &["q:q09"]),
        ("ON CLUSTER", &["q:q08", "m:clustered/0002_views.up.sql:1"]),
        ("GLOBAL JOIN", &["q:q11"]),
        ("ANY JOIN", &["q:q11"]),
        ("cast operator ::", &["q:q10"]),
        ("OPTIMIZE FINAL", &["q:q17"]),
        ("INSERT FORMAT", &["q:q04"]),
        ("CREATE TABLE", &["m:0001_traces.up.sql:2"]),
        ("PARTITION BY", &["m:0001_traces.up.sql:2"]),
        ("table ORDER BY", &["m:0001_traces.up.sql:2"]),
        ("table PRIMARY KEY", &["m:0001_traces.up.sql:2"]),
        ("TTL", &["m:0001_traces.up.sql:2"]),
        ("column DEFAULT", &["m:0001_traces.up.sql:2"]),
        ("skip index bloom_filter", &["m:0001_traces.up.sql:2"]),
        ("table SETTINGS", &["m:0001_traces.up.sql:2"]),
        (
            "CREATE MATERIALIZED VIEW",
            &["m:clustered/0002_views.up.sql:3"],
        ),
        ("MATERIALIZED VIEW TO", &["m:clustered/0002_views.up.sql:3"]),
        ("ALTER ADD COLUMN", &["m:clustered/0002_views.up.sql:5"]),
        ("ALTER MODIFY TTL", &["m:clustered/0002_views.up.sql:5"]),
        ("query parameter", &["q:q21"]),
        ("CREATE FUNCTION", &["m:clustered/0002_views.up.sql:7"]),
        ("user-defined function call", &["q:q22"]),
        ("ALTER UPDATE", &["q:q23"]),
        ("column CODEC", &["m:0001_traces.up.sql:23"]),
        ("codec ZSTD", &["m:0001_traces.up.sql:23"]),
    ] {
        has(&format!("syntax:{form}"), ids);
    }
}

/// Review Focus 1: a feature is never counted for an application that did
/// not use it. Every feature's sources are statements of the fixture, each
/// of them uses the feature when read again on its own, and the count is the
/// executions of those sources.
#[test]
fn inventory_counts_trace_to_sources() {
    let inv = inventory();
    inventory::check(&inv).expect("the inventory keeps its invariants");
    let all = sources();
    let udfs = inventory::defined_functions(&all);
    let mut recorded = common::recorded();
    for (feature, uses) in &inv.features {
        let mut executions = 0;
        for id in &uses.sources {
            let source = all
                .iter()
                .find(|s| &s.id == id)
                .unwrap_or_else(|| panic!("{feature}: source {id} is not a fixture statement"));
            let (features, _) =
                mine_statement(source, Some(&mut recorded), &udfs, &Default::default());
            assert!(
                features.contains(feature),
                "{feature} is not in {id}: {}",
                source.text
            );
            executions += inv.samples[id].executions;
        }
        assert_eq!(executions, uses.count, "{feature}");
    }
    // q01 and q14 differ only in a literal: one sample, two executions.
    assert_eq!(inv.samples["q:q01"].executions, 2);
    assert!(!inv.samples.contains_key("q:q14"));
    assert_eq!(inv.features["syntax:FINAL"].count, 2);
}

#[test]
fn unparsed_statements_are_counted_not_dropped() {
    let inv = inventory();
    let c = inv.statements;
    assert_eq!(c.total, c.parsed + c.explained + c.unparsed);
    assert_eq!(inv.unparsed.sources, ["q:q12"]);
    assert_eq!(inv.unparsed.count, 1);
    assert_eq!(inv.samples["q:q12"].path, Path::Unparsed);
    assert!(inv.samples["q:q12"].query.is_some());
    // Without a server, every statement sqlparser refuses is unparsed, and
    // still counted.
    let offline = inventory::build("fixture", None, &sources(), None, &Default::default());
    let o = offline.statements;
    assert_eq!(o.explained, 0);
    assert_eq!(o.total, c.total);
    assert_eq!(o.unparsed, c.explained + c.unparsed);
    assert_eq!(offline.unparsed.count, o.unparsed);
    inventory::check(&offline).unwrap();
    // The query log's skipped rows are counted by reason.
    assert_eq!(inv.skipped.duplicate, 1);
    assert_eq!(inv.skipped.failed_before_start, 1);
    assert_eq!(inv.skipped.harness, 1);
    assert_eq!(inv.skipped.not_initial, 1);
}

#[test]
fn both_paths_are_exercised() {
    let inv = inventory();
    assert!(inv.statements.parsed > 0 && inv.statements.explained > 0);
}

/// Re-records `fixtures/explain.tsv` from a running clickhouse-server:
/// `LOAMS_HOUSE_MINE_SERVER=http://127.0.0.1:8123 cargo test -p
/// loams-house-mine --test extract -- --ignored record_explain_answers`.
#[test]
#[ignore = "needs a clickhouse-server; re-records the fixture"]
fn record_explain_answers() {
    let url = std::env::var("LOAMS_HOUSE_MINE_SERVER").expect("LOAMS_HOUSE_MINE_SERVER");
    let mut http = HttpExplainer::new(&url);
    // Record every question the extractor asks, as it asks it.
    struct Recording<'a>(&'a mut HttpExplainer, explain::Recorded);
    impl explain::Explainer for Recording<'_> {
        fn explain_ast(&mut self, statement: &str) -> Option<String> {
            let answer = self.0.explain_ast(statement);
            self.1.insert(statement, answer.clone());
            answer
        }
    }
    let mut recording = Recording(&mut http, explain::Recorded::default());
    for s in sources() {
        let _ = loams_house_mine::inventory::read_statement(&s.text, Some(&mut recording));
    }
    let recorded = recording.1;
    std::fs::write(common::fixtures().join("explain.tsv"), recorded.to_tsv()).unwrap();
}
