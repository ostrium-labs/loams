//! HE1 Task 33: conversions, conditionals, maths, JSON extraction and the
//! rest, end to end through the engine. The full check is the oracle job's
//! `corpus/he1/33/`; these pin answers measured on ClickHouse 26.9 that the
//! engine must keep.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use loams_house_engine::Engine;

/// Runs `SELECT <expr> AS v` and returns the column's type and value (as
/// JSON text).
async fn one(engine: &Engine, expr: &str) -> (String, String) {
    let sql = format!("SELECT {expr} AS v FORMAT JSONCompactEachRowWithNamesAndTypes");
    let answer = engine
        .query(&sql, &[], &[], "default")
        .await
        .unwrap_or_else(|e| panic!("{expr}: {e}"));
    let cols = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(&cols, &answer.batches, &Default::default(), &mut out)
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{expr}: {text}");
    let types: Vec<String> = serde_json::from_str(lines[1]).unwrap();
    let row: Vec<serde_json::Value> = serde_json::from_str(lines[2]).unwrap();
    (types[0].clone(), row[0].to_string())
}

#[tokio::test]
async fn conversion_functions_match_the_oracle() {
    let engine = Engine::new();
    for (expr, ty, value) in [
        ("toUInt8('42')", "UInt8", "42"),
        ("toUInt8(toInt16('300'))", "UInt8", "44"),
        ("toString(toUInt8('7'))", "String", "\"7\""),
        ("CAST('2026-01-02' AS Date)", "Date", "\"2026-01-02\""),
        (
            "CAST('2026-01-02 03:04:05', 'DateTime')",
            "DateTime",
            "\"2026-01-02 03:04:05\"",
        ),
        ("'5'::UInt16", "UInt16", "5"),
        ("toDecimal64('1.239', 2)", "Decimal(18, 2)", "1.23"),
        ("toFloat64OrZero('abc')", "Float64", "0"),
        ("toInt64OrNull('x')", "Nullable(Int64)", "null"),
        ("toInt32OrDefault('x', toInt32('5'))", "Int32", "5"),
        ("toBool('true')", "Bool", "true"),
        (
            "toTypeName(toNullable(toUInt8('1')))",
            "String",
            "\"Nullable(UInt8)\"",
        ),
        (
            "CAST('b' AS Enum8('a' = 1, 'b' = 2))",
            "Enum8('a' = 1, 'b' = 2)",
            "\"b\"",
        ),
        (
            "accurateCastOrNull(toInt16('-1'), 'UInt8')",
            "Nullable(UInt8)",
            "null",
        ),
    ] {
        let (t, v) = one(&engine, expr).await;
        assert_eq!((t.as_str(), v.as_str()), (ty, value), "{expr}");
    }
}

#[tokio::test]
async fn conditional_functions_match_the_oracle() {
    let engine = Engine::new();
    for (expr, ty, value) in [
        ("if(toUInt8('1'), 'a', 'b')", "String", "\"a\""),
        (
            "multiIf(toUInt8('0'), 'a', toUInt8('1'), 'b', 'c')",
            "String",
            "\"b\"",
        ),
        ("coalesce(NULL, 'x')", "String", "\"x\""),
        ("ifNull(NULL, 'y')", "String", "\"y\""),
        ("nullIf('a', 'a')", "Nullable(String)", "null"),
        ("greatest(toUInt8('1'), toUInt8('3'))", "UInt8", "3"),
        ("least(toUInt8('1'), toInt8('-3'))", "Int16", "-3"),
        ("isNull(NULL)", "UInt8", "1"),
        ("isNotNull(toUInt8('1'))", "UInt8", "1"),
        ("isNaN(toFloat64('nan'))", "UInt8", "1"),
        (
            "CASE toUInt8('2') WHEN toUInt8('1') THEN 'one' WHEN toUInt8('2') THEN 'two' END",
            "Nullable(String)",
            "\"two\"",
        ),
    ] {
        let (t, v) = one(&engine, expr).await;
        assert_eq!((t.as_str(), v.as_str()), (ty, value), "{expr}");
    }
}

#[tokio::test]
async fn maths_json_and_the_rest_match_the_oracle() {
    let engine = Engine::new();
    for (expr, ty, value) in [
        ("round(toFloat64('2.5'))", "Float64", "2"),
        ("round(toFloat64('1.2345'), 2)", "Float64", "1.23"),
        ("floor(toFloat64('-1.5'))", "Float64", "-2"),
        ("negate(toUInt8('5'))", "Int16", "-5"),
        ("bitAnd(toUInt8('12'), toUInt16('10'))", "UInt16", "8"),
        ("JSONExtractString('{\"a\":\"x\"}', 'a')", "String", "\"x\""),
        (
            "JSONExtractInt('{\"a\":{\"b\":-3}}', 'a', 'b')",
            "Int64",
            "-3",
        ),
        (
            "JSONExtractRaw('{\"a\": [1, 2]}', 'a')",
            "String",
            "\"[1, 2]\"",
        ),
        (
            "simpleJSONExtractString('{\"a\":\"x\"}', 'a')",
            "String",
            "\"x\"",
        ),
        (
            "JSON_VALUE('{\"a\":{\"b\":1}}', '$.a.b')",
            "String",
            "\"1\"",
        ),
        ("currentDatabase()", "String", "\"default\""),
        ("timezone()", "String", "\"UTC\""),
    ] {
        let (t, v) = one(&engine, expr).await;
        assert_eq!((t.as_str(), v.as_str()), (ty, value), "{expr}");
    }
    let (t, _) = one(&engine, "rand()").await;
    assert_eq!(t, "UInt32");
}

#[tokio::test]
async fn casts_fail_as_clickhouse() {
    let engine = Engine::new();
    for (sql, code) in [
        ("SELECT CAST('abc' AS UInt8)", 6),
        ("SELECT toInt32('12abc')", 6),
        ("SELECT CAST(NULL AS UInt8)", 349),
        ("SELECT accurateCast(toInt16('-1'), 'UInt8')", 70),
        ("SELECT CAST('x' AS Date)", 38),
        ("SELECT CAST('x' AS DateTime)", 41),
        ("SELECT CAST('z' AS Enum8('a' = 1))", 691),
        ("SELECT toInt64OrZero(toUInt8('1'))", 43),
        ("SELECT CAST('1' AS NoSuchType)", 50),
        ("SELECT if('x', 1, 2)", 43),
        ("SELECT JSON_VALUE('{}', 'a.b')", 36),
    ] {
        let e = engine.query(sql, &[], &[], "default").await.expect_err(sql);
        assert_eq!(e.code, code, "{sql}: {e}");
    }
}

/// The Task 33 families' Tier 0 functions in `profile.toml` (the mined
/// applications' use), each registered.
#[test]
fn every_tier0_conversion_function_is_registered() {
    const SCOPE: &[&str] = &[
        "if",
        "multiIf",
        "CAST",
        "toString",
        "round",
        "toUInt64",
        "isNotNull",
        "greatest",
        "nullIf",
        "ifNull",
        "coalesce",
        "toBool",
        "toInt64",
        "toNullable",
        "isNaN",
        "toFloat64",
        "floor",
        "rand",
        "negate",
        "least",
        "simpleJSONExtractString",
        "bitAnd",
        "JSONExtractString",
        "isNull",
        "toUInt8",
        "toInt32",
        "toUInt32",
        "ifNotFinite",
        "toFloat64OrZero",
        "toInt64OrZero",
        "toInt32OrZero",
        "toFloat64OrDefault",
        "toDecimal64",
        "version",
    ];
    let profile = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../conformance/clickhouse/profile/profile.toml"
    ))
    .unwrap();
    let registry = loams_house_engine::registry::registry();
    let mut name = None;
    let mut checked = 0;
    for line in profile.lines() {
        if let Some(rest) = line.strip_prefix("name = \"function:") {
            name = rest.strip_suffix('"').map(str::to_string);
        } else if line.trim() == "tier = 0"
            && let Some(f) = name.take()
            && SCOPE.contains(&f.as_str())
        {
            assert!(registry.scalars.contains_key(&f), "{f} is not registered");
            checked += 1;
        }
    }
    assert!(checked >= 20, "only {checked} Tier 0 functions checked");
}
