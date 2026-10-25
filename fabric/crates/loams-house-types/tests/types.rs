//! The type grammar, canonical names, Arrow layout and defaults against the
//! 26.9.2.8 oracle's answers (`tests/fixtures/*.tsv`, captured by
//! `scripts/house/capture-type-fixtures.sh`; plan HE1 Task 21).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use loams_house_types::arrow::{from_arrow, to_arrow};
use loams_house_types::default::default_array;
use loams_house_types::text::{Style, TextSettings, write_value};
use loams_house_types::{ChType, parse};

fn fixture(name: &str) -> String {
    // Lossy: an `AggregateFunction`'s default state is bytes, not text.
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `(input, canonical)` for every spelling the oracle accepted.
fn type_names() -> Vec<(String, String)> {
    fixture("type_names.tsv")
        .lines()
        .filter_map(|line| {
            let (input, out) = line.split_once('\t')?;
            (!out.starts_with("ERROR ")).then(|| (input.to_string(), out.replace("\\n", "\n")))
        })
        .collect()
}

#[test]
fn every_type_parses_and_prints_as_clickhouse() {
    let mut problems = Vec::new();
    let names = type_names();
    assert!(
        names.len() > 100,
        "the fixture holds the captured spellings"
    );
    for (input, canonical) in &names {
        match parse(input) {
            Ok(t) => {
                if t.to_string() != *canonical {
                    problems.push(format!(
                        "{input:?}: printed {:?}, the oracle {canonical:?}",
                        t.to_string()
                    ));
                }
                // The canonical text reads back as the same type.
                match parse(canonical) {
                    Ok(again) if again == t => {}
                    other => {
                        problems.push(format!("{canonical:?} reads back as {other:?}, not {t:?}"))
                    }
                }
            }
            Err(e) => problems.push(format!("{input:?}: {e}")),
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn compact_text_puts_named_tuples_on_one_line() {
    let t = parse("Tuple(a Tuple(b UInt8), c String)").unwrap();
    assert_eq!(t.to_compact_string(), "Tuple(a Tuple(b UInt8), c String)");
    assert!(t.to_string().contains('\n'));
}

#[test]
fn bad_types_are_refused_with_clickhouse_codes() {
    let cases = [
        ("NoSuchType", 50),
        ("Tuple(a UInt8, UInt8)", 36),
        ("Nullable(LowCardinality(String))", 43),
        ("Nullable(Array(UInt8))", 43),
        ("FixedString", 42),
        ("DateTime64(10)", 69),
        ("Decimal(80, 2)", 69),
        ("Map(String)", 42),
        ("UInt8(3)", 36),
        ("Array(String", 62),
    ];
    for (input, code) in cases {
        match parse(input) {
            Err(e) => assert_eq!(e.code, code, "{input}: {e}"),
            Ok(t) => panic!("{input} parsed as {t:?}"),
        }
    }
}

/// The types the engine lays out in Arrow round-trip through a field.
#[test]
fn arrow_round_trip_keeps_the_clickhouse_type() {
    let mut laid_out = 0;
    for (input, _) in type_names() {
        let t = parse(&input).unwrap();
        let Ok(field) = to_arrow("c", &t) else {
            continue;
        };
        laid_out += 1;
        assert_eq!(from_arrow(&field).unwrap(), t, "{input} as {field:?}");
        assert_eq!(field.is_nullable(), t.is_nullable(), "{input}");
    }
    assert!(laid_out > 80, "only {laid_out} types laid out");
}

#[test]
fn types_the_engine_does_not_lay_out_are_48() {
    for input in [
        "Int128",
        "UInt256",
        "BFloat16",
        "Dynamic",
        "Variant(String, UInt8)",
        "Time",
        "Nested(a UInt8)",
    ] {
        let t = parse(input).unwrap();
        let err = to_arrow("c", &t).expect_err(input);
        assert_eq!(err.code, 48, "{input}");
    }
}

#[test]
fn arrow_without_metadata_infers_a_type() {
    use arrow_schema::{DataType, Field, TimeUnit};
    let cases = [
        (Field::new("c", DataType::Int64, false), "Int64"),
        (Field::new("c", DataType::Utf8, true), "Nullable(String)"),
        (
            Field::new(
                "c",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
            "Nullable(DateTime64(6, 'UTC'))",
        ),
        (
            Field::new("c", DataType::Timestamp(TimeUnit::Second, None), false),
            "DateTime",
        ),
        (
            Field::new("c", DataType::Decimal128(18, 4), false),
            "Decimal(18, 4)",
        ),
        (Field::new("c", DataType::Date32, false), "Date32"),
        (
            Field::new_list("c", Field::new_list_field(DataType::Int32, true), false),
            "Array(Nullable(Int32))",
        ),
    ];
    for (field, expected) in cases {
        assert_eq!(
            from_arrow(&field).unwrap().to_string(),
            expected,
            "{field:?}"
        );
    }
}

/// `defaultValueOfTypeName` on the oracle, in TSV, for every type with a
/// default the engine can make.
#[test]
fn default_values_match_the_oracle() {
    let settings = TextSettings::default();
    let mut checked = 0;
    let mut problems = Vec::new();
    for line in fixture("defaults.tsv").lines() {
        let Some((input, expected)) = line.split_once('\t') else {
            continue;
        };
        let t = parse(input).unwrap();
        let Ok(array) = default_array(&t) else {
            continue;
        };
        let mut out = Vec::new();
        write_value(&t, array.as_ref(), 0, Style::Tsv, &settings, &mut out).unwrap();
        checked += 1;
        if String::from_utf8_lossy(&out) != expected {
            problems.push(format!(
                "{input}: {:?}, the oracle {expected:?}",
                String::from_utf8_lossy(&out)
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(checked > 70, "only {checked} defaults checked");
}

#[test]
fn every_profile_type_is_known() {
    let profile = std::fs::read_to_string(format!(
        "{}/../../../conformance/clickhouse/profile/profile.toml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("profile.toml");
    let families = include_str!("../src/data_type_families.tsv");
    for line in profile.lines() {
        let Some(name) = line
            .strip_prefix("name = \"type:")
            .and_then(|n| n.strip_suffix('"'))
        else {
            continue;
        };
        let known = families.lines().any(|f| {
            let fam = f.split('\t').next().unwrap_or_default();
            fam == name || fam.eq_ignore_ascii_case(name)
        });
        assert!(
            known,
            "type:{name} of the profile is not a ClickHouse family"
        );
    }
    // The Tier 0 families all lay out (with a parameter where they need one).
    for text in [
        "String",
        "Array(UInt8)",
        "LowCardinality(String)",
        "UInt64",
        "UInt32",
        "UInt8",
        "DateTime64(3)",
        "DateTime",
        "Nullable(Int32)",
        "Bool",
        "Map(String, String)",
        "Float64",
        "UInt16",
        "Int64",
        "UUID",
        "SimpleAggregateFunction(max, UInt64)",
        "AggregateFunction(uniq, String)",
        "FixedString(4)",
        "Float32",
        "Enum8('a' = 1)",
        "Int8",
        "Decimal(10, 2)",
        "Date",
        "Tuple(UInt8, String)",
        "Decimal(18, 4)",
        "JSON",
        "Int16",
        "Int32",
    ] {
        let t: ChType = parse(text).unwrap();
        to_arrow("c", &t).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}
