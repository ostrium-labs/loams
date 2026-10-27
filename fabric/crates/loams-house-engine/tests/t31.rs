//! The string and hash functions against the oracle (plan HE1 Task 31). The
//! expected answers are clickhouse-server 26.9.2.8's, captured by
//! `scripts/house/capture-t31-fixtures.sh` into `fixtures/t31_oracle.jsonl`;
//! the oracle job runs the full corpus (`corpus/he1/31`).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use loams_house_engine::Engine;
use loams_house_types::text::TextSettings;

const FIXTURE: &str = include_str!("fixtures/t31_oracle.jsonl");
const PROFILE: &str = include_str!("../../../../conformance/clickhouse/profile/profile.toml");

/// The lines of a `JSONCompactEachRowWithNamesAndTypes` answer as JSON
/// values (the server's text has spaces after commas, the engine's none).
fn rows(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect()
}

async fn answer(engine: &Engine, sql: &str) -> Vec<serde_json::Value> {
    let a = engine
        .query(sql, &[], &[], "default")
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    let columns = loams_house_formats::columns(&a.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(
        &columns,
        &a.batches,
        &TextSettings::default(),
        &mut out,
    )
    .unwrap();
    rows(&String::from_utf8(out).unwrap())
}

/// The fixture's cases whose query passes `keep`.
fn cases(keep: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    FIXTURE
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            (
                v["query"].as_str().unwrap().to_string(),
                v["output"].as_str().unwrap().to_string(),
            )
        })
        .filter(|(q, _)| keep(q))
        .collect()
}

fn is_hash(q: &str) -> bool {
    ["cityHash64(", "xxHash32(", "xxHash64(", "sipHash64("]
        .iter()
        .any(|f| q.starts_with(&format!("SELECT {f}")))
}

#[tokio::test]
async fn hashes_match_the_oracle() {
    let engine = Engine::new();
    let cases = cases(is_hash);
    assert!(cases.len() >= 12);
    for (q, want) in cases {
        assert_eq!(answer(&engine, &q).await, rows(&want), "{q}");
    }
}

#[tokio::test]
async fn string_functions_match_the_oracle() {
    let engine = Engine::new();
    let cases = cases(|q| !is_hash(q));
    assert!(cases.len() >= 10);
    for (q, want) in cases {
        assert_eq!(answer(&engine, &q).await, rows(&want), "{q}");
    }
}

/// Every Tier 0 function of the string, regular-expression, hash and UUID
/// family in `profile.toml` is registered under its ClickHouse name.
#[test]
fn every_tier0_string_function_is_registered() {
    let family: Vec<&str> = loams_house_exec::func::strings::NAMES
        .iter()
        .chain(loams_house_exec::func::hash::NAMES)
        .copied()
        .collect();
    // The profile's Tier 0 functions of this family, read from the file.
    let mut tier0 = Vec::new();
    let mut name = None;
    for line in PROFILE.lines() {
        if let Some(n) = line.strip_prefix("name = \"function:") {
            name = n.strip_suffix('"').map(str::to_string);
        } else if line == "tier = 0"
            && let Some(n) = name.take()
        {
            tier0.push(n);
        }
    }
    let ours = [
        "lower",
        "lowerUTF8",
        "length",
        "lengthUTF8",
        "concat",
        "leftUTF8",
        "startsWith",
        "positionCaseInsensitive",
        "like",
        "notLike",
        "match",
        "cityHash64",
        "xxHash32",
        "generateUUIDv4",
    ];
    for f in ours {
        assert!(tier0.iter().any(|t| t == f), "{f} is not Tier 0 any more");
    }
    let registry = loams_house_engine::registry::registry();
    for f in &tier0 {
        if family.contains(&f.as_str()) {
            assert!(registry.scalars.contains_key(f.as_str()), "{f}");
        }
    }
    for f in ours {
        assert!(registry.scalars.contains_key(f), "{f} is not registered");
    }
}
