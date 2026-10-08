//! The licence gate input stays in step with the Grafeo crates actually linked (GR1 Task 1,
//! Global Constraints "Licences", §48 §5 finding 12).
//!
//! `connectors/licences.toml` is D359's gate input: every library a shipped component links must
//! have a stanza there. Grafeo is a set of sibling crates that grows between releases (0.5.43
//! added `grafeo-storage`, which the five-crate list of 2026-10-04 missed), so the list is
//! checked against the lockfiles rather than written down here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `grafeo*` package name in a `Cargo.lock`, or `None` when the lockfile is absent.
fn grafeo_packages(lock: &Path) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(lock).ok()?;
    let value: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|err| panic!("{} does not parse: {err}", lock.display()));
    let packages = value
        .get("package")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{} has no [[package]] array", lock.display()));
    Some(
        packages
            .iter()
            .filter_map(|p| p.get("name").and_then(toml::Value::as_str))
            .filter(|name| name.starts_with("grafeo"))
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn licences_cover_every_grafeo_crate() {
    let root = repo_root();
    let licences_path = root.join("connectors/licences.toml");
    let text = std::fs::read_to_string(&licences_path).expect("connectors/licences.toml");
    let licences: toml::Value = toml::from_str(&text).expect("connectors/licences.toml parses");
    let components = licences
        .get("components")
        .and_then(toml::Value::as_table)
        .expect("connectors/licences.toml has [components]");
    // A stanza covers a crate when its key or its `name` is the crate's name.
    let covered: BTreeSet<&str> = components
        .iter()
        .flat_map(|(key, stanza)| {
            let name = stanza.get("name").and_then(toml::Value::as_str);
            std::iter::once(key.as_str()).chain(name)
        })
        .collect();

    // The engine workspace's lockfile must link Grafeo now that `loams-graph` lives here; the
    // Fabric workspace's is checked too, so a Grafeo crate creeping back in there is caught.
    let root_lock = root.join("Cargo.lock");
    let in_root = grafeo_packages(&root_lock).expect("the root Cargo.lock exists");
    assert!(
        in_root.contains("grafeo"),
        "the root Cargo.lock links no `grafeo`, so this test would pass vacuously: {in_root:?}"
    );
    let mut missing = Vec::new();
    for (lock, crates) in [
        (root_lock.clone(), Some(in_root)),
        (
            root.join("fabric/Cargo.lock"),
            grafeo_packages(&root.join("fabric/Cargo.lock")),
        ),
    ] {
        for name in crates.into_iter().flatten() {
            if !covered.contains(name.as_str()) {
                missing.push(format!("{name} (in {})", lock.display()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "connectors/licences.toml has no [components.<crate>] stanza for: {missing:?}"
    );

    // Each Grafeo stanza is a linked Apache-2.0 library (R0.21).
    for (key, stanza) in components.iter().filter(|(k, _)| k.starts_with("grafeo")) {
        assert_eq!(
            stanza.get("spdx").and_then(toml::Value::as_str),
            Some("Apache-2.0"),
            "components.{key}.spdx"
        );
        assert_eq!(
            stanza.get("kind").and_then(toml::Value::as_str),
            Some("library"),
            "components.{key}.kind"
        );
    }
}
