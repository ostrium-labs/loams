//! The hand-written fixture: `fixtures/queries.tsv`, `fixtures/migrations/`
//! and the server's recorded `EXPLAIN AST` answers, `fixtures/explain.tsv`.

#![allow(dead_code)]

use std::path::PathBuf;

use loams_house_mine::cmd::extract::migration_sources;
use loams_house_mine::explain::Recorded;
use loams_house_mine::inventory::{self, Inventory, Options, Source};
use loams_house_mine::querylog;

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

pub fn query_log_text() -> String {
    std::fs::read_to_string(fixtures().join("queries.tsv")).expect("queries.tsv")
}

/// Every source of the fixture, query log first.
pub fn sources() -> Vec<Source> {
    let (rows, _) = querylog::parse(&query_log_text()).expect("query log parses");
    let mut out: Vec<Source> = rows
        .into_iter()
        .map(|r| Source {
            id: format!("q:{}", r.id),
            text: r.query,
            settings: r.settings,
        })
        .collect();
    out.extend(migration_sources(&fixtures().join("migrations")).expect("migrations"));
    out
}

pub fn recorded() -> Recorded {
    Recorded::parse(&std::fs::read_to_string(fixtures().join("explain.tsv")).unwrap_or_default())
}

/// The fixture's inventory, built with the recorded answers.
pub fn inventory() -> Inventory {
    let mut recorded = recorded();
    let mut inv = inventory::build(
        "fixture",
        None,
        &sources(),
        Some(&mut recorded),
        &Options::default(),
    );
    let (_, skipped) = querylog::parse(&query_log_text()).expect("query log parses");
    inv.skipped = skipped.into();
    assert!(
        recorded.missing.is_empty(),
        "fixtures/explain.tsv has no answer for {:?}; run the ignored `record_explain_answers` test against a server",
        recorded.missing
    );
    inv
}

/// The features of the sample whose id is `id`.
pub fn sources_of(inv: &Inventory, feature: &str) -> Vec<String> {
    inv.features
        .get(feature)
        .unwrap_or_else(|| {
            panic!(
                "{feature} not mined; have {:?}",
                inv.features.keys().collect::<Vec<_>>()
            )
        })
        .sources
        .clone()
}
