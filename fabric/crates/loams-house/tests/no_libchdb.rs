//! `no_libchdb_in_front` (HS1 Task 2, D761, Global Constraint "libchdb only in the
//! worker"): the front's crates do not have `loams-chdb-sys` (and so
//! `libchdb.so`) in their normal or build dependency graph.
//!
//! `loams-fabric` is checked too once it exists (HS1 Task 7 creates it). The
//! `inproc-worker` feature is the one deliberate way libchdb enters `loams-house`,
//! and the last test shows the check sees it, so the check is not vacuous.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the fabric workspace")
}

/// `cargo tree -p <package> -e normal,build` as package names, one per line.
fn tree(package: &str, features: &[&str]) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace())
        .args(["tree", "--offline", "--locked", "-p", package])
        .args(["-e", "normal,build", "--prefix", "none", "--format", "{p}"]);
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    let output = command.output().expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree -p {package} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn links_libchdb(packages: &[String]) -> bool {
    packages
        .iter()
        .any(|p| p == "loams-chdb-sys" || p == "loams-chdb" || p == "loams-house-worker")
}

#[test]
fn no_libchdb_in_front() {
    let mut front = vec!["loams-house", "loams-house-ipc"];
    if workspace().join("crates/loams-fabric/Cargo.toml").is_file() {
        front.push("loams-fabric");
    }
    for package in front {
        let packages = tree(package, &[]);
        assert!(
            packages.contains(&package.to_string()),
            "{package}: {packages:?}"
        );
        assert!(
            !links_libchdb(&packages),
            "{package} reaches libchdb (D761 puts it in the worker only): {packages:?}"
        );
    }
}

#[test]
fn inproc_worker_feature_is_where_libchdb_comes_in() {
    assert!(
        links_libchdb(&tree("loams-house", &["inproc-worker"])),
        "the check must see libchdb when it is there"
    );
}
