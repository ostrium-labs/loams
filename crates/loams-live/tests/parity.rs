//! LV1 plan Task 22: every Live suite runs on the embedded backend. The
//! meta-test reads this crate's test sources: a test that opens a store
//! must be a `live_test!` case (so it has `::embedded` and `::tikv`
//! variants), unless it is on the TiKV-only allowlist with a reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tests that cannot run on the embedded backend, and why. Each asserts a
/// TiKV-only behaviour (TSO-stream loss, region errors, a PD stall). None
/// of the Live suites does today: those behaviours are tested in
/// `loams-tikv` and `loams-kv`.
const TIKV_ONLY: &[(&str, &str)] = &[];

/// What a test that opens or reaches a store contains.
const STORE_MARKERS: &[&str] = &[
    "testing::cluster",
    "testing::tikv",
    "testing::stores",
    "Store::open",
    "Store::from",
    ".connect(TEST_LIVE)",
    "LiveServer::start",
    "Runner::open",
    "live()",
    "open()",
    "open_with(",
    "server(",
];

/// A test function found in a source file: its name and body.
#[derive(Debug)]
struct Found {
    file: String,
    name: String,
    body: String,
}

fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the tests directory")
        .map(|e| e.expect("an entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter(|p| p.file_name().is_some_and(|n| n != "parity.rs"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("a file name")
                .to_string();
            (name, std::fs::read_to_string(&p).expect("readable"))
        })
        .collect()
}

/// Every function of `text` (any indentation): its name, body, and whether
/// it is a `#[test]` or `#[tokio::test]`.
fn functions(file: &str, text: &str) -> Vec<(Found, bool)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (j, line) in lines.iter().enumerate() {
        let sig = line.trim_start();
        let sig = sig.strip_prefix("pub ").unwrap_or(sig);
        let sig = sig.strip_prefix("async ").unwrap_or(sig);
        let Some(rest) = sig.strip_prefix("fn ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // A test is marked by an attribute among the lines just above (doc
        // comments and other attributes may sit between).
        let is_test = lines[..j]
            .iter()
            .rev()
            .map(|l| l.trim())
            .take_while(|l| l.starts_with("#[") || l.starts_with("///"))
            .any(|l| l == "#[test]" || l.starts_with("#[tokio::test"));
        let mut depth = 0i32;
        let mut body = String::new();
        for l in &lines[j..] {
            body.push_str(l);
            body.push('\n');
            depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
            if depth <= 0 && l.contains('}') {
                break;
            }
        }
        found.push((
            Found {
                file: file.to_string(),
                name,
                body,
            },
            is_test,
        ));
    }
    found
}

/// The `#[test]` and `#[tokio::test]` functions of `text` that reach a
/// store, directly or through the file's helpers.
fn store_tests(file: &str, text: &str) -> Vec<Found> {
    let fns = functions(file, text);
    let mut markers: Vec<String> = STORE_MARKERS.iter().map(|m| (*m).to_string()).collect();
    // Helpers that reach a store are markers too, to a fixed point.
    loop {
        let mut grew = false;
        for (f, is_test) in &fns {
            let call = format!("{}(", f.name);
            if !*is_test
                && !markers.contains(&call)
                && markers
                    .iter()
                    .any(|m| f.body.lines().skip(1).any(|l| l.contains(m.as_str())))
            {
                markers.push(call);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    fns.into_iter()
        .filter(|(f, is_test)| {
            *is_test
                && markers
                    .iter()
                    .any(|m| f.body.lines().skip(1).any(|l| l.contains(m.as_str())))
        })
        .map(|(f, _)| f)
        .collect()
}

/// The names of the `live_test!` cases of `text`.
fn live_tests(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("live_test!(") {
        rest = &rest[at + "live_test!(".len()..];
        // Skip attributes and doc comments before the name.
        let mut tail = rest.trim_start();
        while tail.starts_with("#[") || tail.starts_with("///") {
            let end = tail.find('\n').map_or(tail.len(), |n| n + 1);
            tail = tail[end..].trim_start();
        }
        let name: String = tail
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

#[test]
fn every_live_suite_runs_on_embedded() {
    let allow: BTreeMap<&str, &str> = TIKV_ONLY.iter().copied().collect();
    let mut cases = BTreeSet::new();
    let mut missing = Vec::new();
    let mut allowed_seen = BTreeSet::new();
    let mut suites_with_store_tests = BTreeSet::new();
    let mut suites_on_the_macro = BTreeSet::new();
    for (file, text) in sources() {
        for name in live_tests(&text) {
            cases.insert(format!("{file}::{name}::embedded"));
            suites_on_the_macro.insert(file.clone());
        }
        for test in store_tests(&file, &text) {
            suites_with_store_tests.insert(test.file.clone());
            if allow.contains_key(test.name.as_str()) {
                allowed_seen.insert(test.name.clone());
            } else {
                missing.push(format!("{}::{}", test.file, test.name));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these tests open a store but have no ::embedded variant (move them onto \
         live_test!, or allowlist them in TIKV_ONLY with a reason): {missing:#?}"
    );
    let stale: Vec<_> = allow
        .keys()
        .filter(|n| !allowed_seen.contains(**n))
        .collect();
    assert!(stale.is_empty(), "allowlisted tests not found: {stale:?}");
    for suite in ["docs", "journal", "service", "session", "subs", "txn"] {
        assert!(
            suites_on_the_macro.contains(suite),
            "the {suite} suite has no live_test! case"
        );
    }
    assert!(
        cases.len() >= 60,
        "only {} ::embedded cases: {cases:#?}",
        cases.len()
    );
    eprintln!("{} ::embedded cases", cases.len());
}
