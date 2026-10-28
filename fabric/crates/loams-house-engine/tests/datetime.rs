//! The date and time functions (plan HE1 Task 30). The full check is the
//! oracle job over `corpus/he1/30`; these tests pin the functions without
//! the oracle, against its answers captured in `fixtures/datetime.tsv`
//! (`scripts/house/capture-datetime-fixtures.sh`).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use loams_house_engine::Engine;
use loams_house_sql::functions::{internal_name, resolve};
use loams_house_types::arrow::from_arrow;
use loams_house_types::text::{Style, TextSettings, write_value};

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
}

/// Every expression of the fixture gives the oracle's type and value.
#[tokio::test]
async fn datetime_functions_match_the_oracle() {
    let fixtures = include_str!("fixtures/datetime.tsv");
    let engine = Engine::new();
    let mut failures = Vec::new();
    for line in fixtures.lines() {
        let mut cols = line.splitn(3, '\t');
        let (expr, want_type, want_value) = (
            cols.next().unwrap(),
            cols.next().unwrap().replace("\\'", "'"),
            cols.next().unwrap(),
        );
        let answer = match engine
            .query(&format!("SELECT {expr}"), &[], &[], "default")
            .await
        {
            Ok(a) => a,
            Err(e) => {
                failures.push(format!("{expr}: {e}"));
                continue;
            }
        };
        let field = answer.schema.field(0);
        let t = from_arrow(field).unwrap();
        let mut text = Vec::new();
        write_value(
            &t,
            answer.batches[0].column(0).as_ref(),
            0,
            Style::Tsv,
            &TextSettings::default(),
            &mut text,
        )
        .unwrap();
        let got = (t.to_compact_string(), String::from_utf8(text).unwrap());
        if got != (want_type.clone(), want_value.to_string()) {
            failures.push(format!(
                "{expr}: got {got:?}, the oracle {:?}",
                (want_type, want_value)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Whether a profile function belongs to the date and time family.
fn is_datetime(name: &str) -> bool {
    name.contains("Date")
        || name.contains("Time")
        || name.contains("Interval")
        || name.contains("Unix")
        || name.starts_with("toStartOf")
        || name.starts_with("toYYYY")
        || name.starts_with("now")
        || name.starts_with("toDayOf")
        || name.starts_with("subtract")
        || (name.starts_with("add") && name.ends_with('s'))
        || name.eq_ignore_ascii_case("date_trunc")
        || name == "dateDiff"
}

/// Every Tier 0 date and time function of the profile is registered, and
/// so is every Tier 1 one Task 30 took on.
#[test]
fn every_tier0_datetime_function_is_registered() {
    let profile =
        std::fs::read_to_string(repo().join("conformance/clickhouse/profile/profile.toml"))
            .unwrap();
    let registry = loams_house_engine::registry::registry();
    let mut missing = Vec::new();
    let mut seen = 0;
    let (mut name, mut tier) = (None::<String>, None::<i64>);
    for line in profile.lines().chain(std::iter::once("[[feature]]")) {
        let line = line.trim();
        if line == "[[feature]]" {
            if let (Some(n), Some(t)) = (name.take(), tier.take())
                && let Some(f) = n.strip_prefix("function:")
                && is_datetime(f)
            {
                let tier1_taken = matches!(
                    f,
                    "parseDateTimeBestEffort"
                        | "formatDateTime"
                        | "date_trunc"
                        | "toUnixTimestamp64Nano"
                        | "fromUnixTimestamp64Milli"
                        | "toTimeZone"
                        | "toTimezone"
                        | "toStartOfFiveMinutes"
                        | "toStartOfMonth"
                        | "timeDiff"
                );
                if t == 0 || tier1_taken {
                    seen += 1;
                    let canonical = resolve(f).unwrap().canonical;
                    if !registry.scalars.contains_key(&internal_name(&canonical)) {
                        missing.push(f.to_string());
                    }
                }
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("tier = ") {
            tier = v.parse().ok();
        }
    }
    assert!(
        seen >= 30,
        "only {seen} date and time functions in the profile"
    );
    assert!(missing.is_empty(), "not registered: {missing:?}");
}

/// The interval and `EXTRACT` syntax reaches the engine as ClickHouse's
/// calls, and the names stay ClickHouse's.
#[tokio::test]
async fn interval_and_extract_syntax_plan() {
    let engine = Engine::new();
    let a = engine
        .query(
            "SELECT toDate('2024-03-05') - INTERVAL 1 MONTH, EXTRACT(YEAR FROM toDate('2024-03-05'))",
            &[],
            &[],
            "default",
        )
        .await
        .unwrap();
    let names: Vec<&str> = a
        .schema
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();
    assert_eq!(
        names,
        [
            "minus(toDate('2024-03-05'), toIntervalMonth(1))",
            "toYear(toDate('2024-03-05'))"
        ]
    );
    let e = engine
        .query("SELECT INTERVAL 1 DAY * 2", &[], &[], "default")
        .await
        .unwrap_err();
    assert_eq!(e.code, 43);
}
