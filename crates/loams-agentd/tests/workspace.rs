//! Workspace-level checks for the move of the zeron fork (plan DD1 Task 1).

use std::path::Path;
use std::process::Command;

fn workspace_packages() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
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
    metadata["packages"]
        .as_array()
        .expect("packages")
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
