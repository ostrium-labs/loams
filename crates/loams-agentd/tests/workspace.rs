//! Workspace-level checks for the move of the zeron fork (plan DD1 Tasks 1 and 5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `cargo metadata --no-deps` for the whole workspace.
fn workspace_metadata() -> Vec<serde_json::Value> {
    let root = workspace_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "{output:?}");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata JSON");
    metadata["packages"].as_array().expect("packages").clone()
}

fn workspace_packages() -> Vec<String> {
    workspace_metadata()
        .iter()
        .map(|p| p["name"].as_str().expect("name").to_owned())
        .collect()
}

#[test]
fn no_crate_named_loams_desktop() {
    let packages = workspace_packages();
    let stale: Vec<_> = packages
        .iter()
        .filter(|name| name.starts_with("loams-desktop"))
        .collect();
    assert!(
        stale.is_empty(),
        "packages still named loams-desktop*: {stale:?}"
    );
    for moved in [
        "loams-agentd",
        "loams-agentd-sessions",
        "loams-agentd-harness",
        "loams-agentd-proto",
        "loams-agentd-rpc",
        "loams-agentd-doc",
        "loams-agentd-store",
        "loams-agentd-mcp",
        "loams-agentd-link",
        "loams-agentd-preview",
    ] {
        assert!(
            packages.iter().any(|name| name == moved),
            "{moved} is not a workspace member"
        );
    }
}

/// Where a `loams-agentd*` crate's code comes from, as the README's licence table says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Inherited from zeron through the fork: MIT, "Copyright (c) 2026 Wing".
    Zeron,
    /// Written by the Loams Authors in the fork: Apache-2.0.
    LoamsDesktop,
    /// Written for the daemon (Tasks 14-23): the workspace licence.
    New,
}

#[derive(Debug)]
struct LicenceRow {
    licence: String,
    origin: Origin,
}

/// The licence table under `## Licences` in `crates/loams-agentd/README.md`: rows of
/// `` | `crate` | licence | origin | notes | ``.
fn readme_licence_table() -> BTreeMap<String, LicenceRow> {
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("crates/loams-agentd/README.md exists");
    let section = readme
        .split("\n## ")
        .find(|s| s.starts_with("Licences"))
        .expect("README.md has a `## Licences` section");
    let mut rows = BTreeMap::new();
    for line in section.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        assert!(cells.len() >= 3, "licence row has too few cells: {line}");
        let name = cells[0].trim_matches('`').to_owned();
        let origin = match cells[2] {
            "zeron" => Origin::Zeron,
            "Loams Desktop" => Origin::LoamsDesktop,
            "new" => Origin::New,
            other => panic!("{name}: unknown origin {other:?} (zeron, Loams Desktop or new)"),
        };
        let row = LicenceRow {
            licence: cells[1].to_owned(),
            origin,
        };
        assert!(
            rows.insert(name.clone(), row).is_none(),
            "{name} is listed twice"
        );
    }
    assert!(!rows.is_empty(), "the licence table has no rows");
    rows
}

/// Whether a manifest takes its licence from `[workspace.package]`.
fn inherits_workspace_licence(manifest: &str) -> bool {
    let text = std::fs::read_to_string(manifest).expect("read manifest");
    let manifest: toml::Value = toml::from_str(&text).expect("parse manifest");
    manifest
        .get("package")
        .and_then(|p| p.get("license"))
        .and_then(|l| l.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

#[test]
fn licence_fields_are_set() {
    let table = readme_licence_table();
    let packages: BTreeMap<String, serde_json::Value> = workspace_metadata()
        .into_iter()
        .filter_map(|p| {
            let name = p["name"].as_str().expect("name").to_owned();
            name.starts_with("loams-agentd").then_some((name, p))
        })
        .collect();
    let mut errors = Vec::new();
    for (name, package) in &packages {
        let licence = package["license"].as_str().unwrap_or("<none>");
        let Some(row) = table.get(name) else {
            errors.push(format!(
                "{name} ({licence}) is not in the README licence table"
            ));
            continue;
        };
        if licence != row.licence {
            errors.push(format!(
                "{name}: Cargo.toml says {licence}, the table says {}",
                row.licence
            ));
        }
        let manifest = package["manifest_path"].as_str().expect("manifest_path");
        if row.origin == Origin::New && !inherits_workspace_licence(manifest) {
            errors.push(format!(
                "{name}: a new crate must set `license.workspace = true`"
            ));
        }
    }
    for (name, row) in &table {
        let expected = match row.origin {
            Origin::Zeron => "MIT",
            Origin::LoamsDesktop | Origin::New => "Apache-2.0",
        };
        if row.licence != expected {
            errors.push(format!(
                "{name}: a {:?} crate is {expected}, not {}",
                row.origin, row.licence
            ));
        }
        // Only the new crates may be listed before they exist.
        if row.origin != Origin::New && !packages.contains_key(name) {
            errors.push(format!("{name} is in the table but not in the workspace"));
        }
    }
    assert!(errors.is_empty(), "licence fields:\n{}", errors.join("\n"));
}
