#![allow(clippy::unwrap_used)]

//! OP2-B Task 1: the listing v2 schema accepts its valid fixtures, rejects
//! each invalid one with an issue at the value that breaks the rule, and
//! leaves v1 as it was.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use loams_agentd_marketplace::listing::{self, SCHEMA_V2_ID, schema_v2};
use serde_json::Value;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/marketplace/testdata")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixtures(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
}

/// Splits a JSON Pointer into its unescaped tokens.
fn tokens(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// Applies one `set` or `remove` op, as `invalid.json` (MK0's form) writes them.
fn apply(doc: &mut Value, op: &Value) {
    let path = op["path"].as_str().unwrap();
    let tokens = tokens(path);
    let (last, parents) = tokens.split_last().expect("a non-root path");
    let mut target = &mut *doc;
    for token in parents {
        target = match target {
            Value::Array(items) => &mut items[token.parse::<usize>().unwrap()],
            Value::Object(map) => map.get_mut(token).unwrap_or_else(|| panic!("{path}")),
            _ => panic!("{path} goes through a scalar"),
        };
    }
    match (op["op"].as_str().unwrap(), target) {
        ("remove", Value::Object(map)) => {
            map.remove(last)
                .unwrap_or_else(|| panic!("{path} is absent"));
        }
        ("remove", Value::Array(items)) => {
            items.remove(last.parse::<usize>().unwrap());
        }
        ("set", Value::Object(map)) => {
            map.insert(last.clone(), op["value"].clone());
        }
        ("set", Value::Array(items)) => {
            items[last.parse::<usize>().unwrap()] = op["value"].clone();
        }
        (other, _) => panic!("unknown op {other} at {path}"),
    }
}

#[test]
fn v2_schema_accepts_fixtures() {
    let paths = fixtures(&testdata().join("v2/valid"));
    let mut kinds = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for path in paths {
        let doc = read_json(&path);
        kinds.insert(doc["kind"].as_str().unwrap().to_owned());
        if let Some(package) = doc["package"]["type"].as_str() {
            packages.insert(package.to_owned());
        }
        if let Err(issues) = listing::validate_v2(&doc) {
            panic!("{} is rejected: {issues:#?}", path.display());
        }
    }
    assert_eq!(
        kinds,
        ["agent", "app", "engine", "mcp-server"]
            .map(String::from)
            .into()
    );
    assert_eq!(
        packages,
        ["binary", "helm", "kustomize", "oci", "wasm"]
            .map(String::from)
            .into()
    );
}

#[test]
fn v2_schema_rejects_each_invalid_fixture() {
    let manifest = read_json(&testdata().join("v2/invalid.json"));
    let cases = manifest["cases"].as_array().unwrap();
    let mut names = BTreeSet::new();
    let mut problems = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert!(names.insert(name.to_owned()), "duplicate case {name}");
        let pointer = case["pointer"].as_str().unwrap();
        let base = testdata().join("v2").join(case["base"].as_str().unwrap());
        let mut doc = read_json(&base);
        assert!(
            listing::validate_v2(&doc).is_ok(),
            "{name}: base must be valid"
        );
        for op in case["patch"].as_array().unwrap() {
            apply(&mut doc, op);
        }
        match listing::validate_v2(&doc) {
            Ok(()) => problems.push(format!("{name}: accepted")),
            Err(issues) if !issues.iter().any(|i| i.instance_path == pointer) => {
                problems.push(format!("{name}: no issue at {pointer:?}: {issues:#?}"));
            }
            Err(_) => {}
        }
    }
    for want in [
        "wasm-with-cluster",
        "binary-with-cluster",
        "app-without-package",
        "engine-with-wasm-on-cluster",
        "oci-image-with-tag",
        "digest-not-sha256",
        "package-without-provenance",
        "app-without-cost-hint",
        "bad-storage-kind",
        "depends-on-bad-range",
        "pricing-not-free",
        "run-on-helm",
        "http-url",
    ] {
        assert!(names.contains(want), "no case {want}");
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn v1_fixtures_still_pass_v1() {
    let paths = fixtures(&testdata().join("valid"));
    assert!(paths.len() >= 6, "{paths:?}");
    for path in paths {
        let doc = read_json(&path);
        if let Err(issues) = listing::validate(&doc) {
            panic!("{} is rejected by v1: {issues:#?}", path.display());
        }
        // A v1 document is not a v2 one: v2 requires its own `$schema`.
        assert!(
            listing::validate_v2(&doc).is_err(),
            "{} passes v2",
            path.display()
        );
    }
}

#[test]
fn v2_schema_id_matches() {
    assert_eq!(schema_v2()["$id"], SCHEMA_V2_ID);
}
