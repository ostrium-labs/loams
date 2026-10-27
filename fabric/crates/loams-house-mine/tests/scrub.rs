//! Review Focus 2: no literal from a query reaches a committed file.

mod common;

use loams_house_mine::scrub::{is_literal_free, scrub};
use loams_house_mine::tokens::{is_literal, tokenize};
use proptest::prelude::*;

/// A piece of a generated statement.
#[derive(Debug, Clone)]
enum Piece {
    Sql(&'static str),
    Str(String, bool),
    Int(i64),
    Float(f64),
    LineComment(String),
    BlockComment(String),
}

const SQL: [&str; 16] = [
    "SELECT",
    "FROM t",
    "WHERE a =",
    "AND",
    "f(",
    ")",
    ",",
    "IN (",
    "LIKE",
    "toDate(",
    "[",
    "]",
    "INTERVAL",
    "DAY",
    "SETTINGS max_threads =",
    "{id:String}",
];

fn marker() -> impl Strategy<Value = String> {
    "[a-z0-9]{6}".prop_map(|s| format!("zq{s}"))
}

fn piece() -> impl Strategy<Value = Piece> {
    prop_oneof![
        prop::sample::select(&SQL[..]).prop_map(Piece::Sql),
        (marker(), "[^\u{0}]{0,8}", any::<bool>())
            .prop_map(|(m, rest, double)| Piece::Str(format!("{m}{rest}"), double)),
        any::<i64>().prop_map(Piece::Int),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Piece::Float),
        (marker(), "[^\n\u{0}]{0,8}")
            .prop_map(|(m, rest)| Piece::LineComment(format!("{m}{rest}"))),
        (marker(), "[^*\u{0}]{0,8}")
            .prop_map(|(m, rest)| Piece::BlockComment(format!("{m}{rest}"))),
    ]
}

/// Renders the pieces, quoting strings the ways ClickHouse accepts, and
/// returns the markers that must not survive.
fn render(pieces: &[Piece]) -> (String, Vec<String>) {
    let mut sql = String::new();
    let mut secrets = Vec::new();
    for p in pieces {
        match p {
            Piece::Sql(s) => sql.push_str(s),
            Piece::Str(s, double) => {
                let body = s.replace('\\', "\\\\");
                let body = if *double {
                    body.replace('\'', "''")
                } else {
                    body.replace('\'', "\\'")
                };
                sql.push('\'');
                sql.push_str(&body);
                sql.push('\'');
                secrets.push(s[..8].to_owned());
            }
            Piece::Int(i) => sql.push_str(&i.to_string()),
            Piece::Float(f) => sql.push_str(&format!("{f:e}")),
            Piece::LineComment(c) => {
                sql.push_str("-- ");
                sql.push_str(c);
                sql.push('\n');
                secrets.push(c[..8].to_owned());
            }
            Piece::BlockComment(c) => {
                sql.push_str("/* ");
                sql.push_str(c);
                sql.push_str(" */");
                secrets.push(c[..8].to_owned());
            }
        }
        sql.push(' ');
    }
    (sql, secrets)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn scrub_removes_every_literal(pieces in prop::collection::vec(piece(), 1..12)) {
        let (sql, secrets) = render(&pieces);
        let scrubbed = scrub(&sql).expect("generated SQL tokenizes");
        for s in &secrets {
            prop_assert!(!scrubbed.text.contains(s.as_str()), "{s} survived in {}", scrubbed.text);
        }
        let tokens = tokenize(&scrubbed.text).expect("scrubbed text tokenizes");
        prop_assert!(tokens.iter().all(|t| !is_literal(&t.token)), "a literal survived in {}", scrubbed.text);
        prop_assert!(is_literal_free(&scrubbed.text));
        let literals = pieces.iter().filter(|p| matches!(p, Piece::Str(..) | Piece::Int(_) | Piece::Float(_))).count();
        prop_assert_eq!(scrubbed.replaced, literals);
    }
}

/// And over the fixture corpus: every sample the inventory keeps is
/// literal-free and none of the fixture's secrets survive.
#[test]
fn scrub_removes_every_literal_from_the_fixture() {
    let inv = common::inventory();
    let json = serde_json::to_string(&inv).unwrap();
    assert!(
        !json.contains("zqsecret"),
        "a fixture secret reached the inventory"
    );
    assert!(!json.contains("'p1'") && !json.contains("2024-01-01"));
    for (id, sample) in &inv.samples {
        let q = sample
            .query
            .as_deref()
            .unwrap_or_else(|| panic!("{id} has no sample"));
        assert!(is_literal_free(q), "{id}: {q}");
    }
    for s in common::sources() {
        if let Ok(out) = scrub(&s.text) {
            assert!(is_literal_free(&out.text), "{}: {}", s.id, out.text);
        }
    }
}
