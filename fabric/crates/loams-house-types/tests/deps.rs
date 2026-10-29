//! `engine_crates_stay_apart` (plan HE1 Task 20, Review Focus 4): the Rust
//! engine's crates never reach the House front or libchdb, and the front never
//! reaches DataFusion. The two talk over `loams-house-ipc` only (D1578).
//!
//! It reads the `fabric/` workspace's resolved graph from `cargo metadata`, so
//! it holds for every dependency path, not only the direct ones.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// The engine's crates (plan HE1, "Architecture").
const ENGINE: &[&str] = &[
    "loams-house-types",
    "loams-house-sql",
    "loams-house-exec",
    "loams-house-iceberg",
    "loams-house-formats",
    "loams-house-engine",
];

/// What no engine crate may depend on: the front, the chDB worker and libchdb,
/// and the chDB path's lake writer (iceberg 0.10.1 on Arrow 58, until HE2
/// Task 60 folds it into `loams-house-iceberg`).
const NOT_IN_THE_ENGINE: &[&str] = &[
    "loams-house",
    "loams-house-worker",
    "loams-house-lake",
    "loams-chdb",
    "loams-chdb-sys",
];

/// The crates on the front's side of the worker protocol.
const FRONT: &[&str] = &["loams-house", "loams-house-ipc", "loams-house-worker"];

/// A package id to its name, and each package id to the ids it depends on
/// (normal and build dependencies; dev-dependencies never reach a binary).
struct Graph {
    names: BTreeMap<String, String>,
    deps: BTreeMap<String, Vec<String>>,
}

fn graph() -> Graph {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    let names = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            (
                p["id"].as_str().expect("id").to_string(),
                p["name"].as_str().expect("name").to_string(),
            )
        })
        .collect();
    let deps = metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolve.nodes")
        .iter()
        .map(|node| {
            let id = node["id"].as_str().expect("node id").to_string();
            let kept = node["deps"]
                .as_array()
                .expect("node deps")
                .iter()
                .filter(|dep| {
                    dep["dep_kinds"]
                        .as_array()
                        .expect("dep_kinds")
                        .iter()
                        .any(|k| k["kind"].as_str() != Some("dev"))
                })
                .map(|dep| dep["pkg"].as_str().expect("dep pkg").to_string())
                .collect();
            (id, kept)
        })
        .collect();
    Graph { names, deps }
}

impl Graph {
    fn id(&self, name: &str) -> String {
        let ids: Vec<_> = self
            .names
            .iter()
            .filter(|(id, n)| *n == name && id.starts_with("path+"))
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(ids.len(), 1, "one workspace crate named {name}: {ids:?}");
        ids[0].clone()
    }

    /// The names of every crate `name` reaches, itself excluded.
    fn reach(&self, name: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![self.id(name)];
        while let Some(id) = stack.pop() {
            for dep in self.deps.get(&id).into_iter().flatten() {
                if seen.insert(dep.clone()) {
                    stack.push(dep.clone());
                }
            }
        }
        seen.iter().map(|id| self.names[id].clone()).collect()
    }
}

#[test]
fn engine_crates_stay_apart() {
    let graph = graph();
    let mut problems = Vec::new();
    for engine in ENGINE {
        let reach = graph.reach(engine);
        for banned in NOT_IN_THE_ENGINE {
            if reach.contains(*banned) {
                problems.push(format!("{engine} depends on {banned}"));
            }
        }
    }
    for front in FRONT {
        let reach = graph.reach(front);
        for engine in ENGINE {
            if reach.contains(*engine) {
                problems.push(format!("{front} depends on {engine}"));
            }
        }
        for name in &reach {
            if name == "datafusion" || name.starts_with("datafusion-") {
                problems.push(format!("{front} depends on {name}"));
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    // The check is live: the engine crate itself reaches DataFusion.
    assert!(graph.reach("loams-house-engine").contains("datafusion"));
}
