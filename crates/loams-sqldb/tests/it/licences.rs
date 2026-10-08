use std::path::Path;
use std::process::Command;

/// D11, D126: no GPL (or other non-permissive) crate in this crate's graph.
/// Runs `cargo deny check licenses` scoped to loams-sqldb with the
/// workspace's deny.toml. Skips, loudly, where cargo-deny is not installed;
/// CI installs it, and `LOAMS_REQUIRE_CARGO_DENY=1` turns the skip into a
/// failure.
#[test]
fn no_gpl_crate_in_dependency_graph() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../..");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let have = Command::new(&cargo)
        .args(["deny", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !have {
        assert!(
            std::env::var("LOAMS_REQUIRE_CARGO_DENY").as_deref() != Ok("1"),
            "cargo-deny is required"
        );
        println!("skipped: no_gpl_crate_in_dependency_graph needs cargo-deny");
        return;
    }
    let out = Command::new(&cargo)
        .current_dir(&root)
        .args(["deny", "--offline", "--manifest-path"])
        .arg(manifest.join("Cargo.toml"))
        .arg("--config")
        .arg(root.join("deny.toml"))
        .args(["check", "licenses"])
        .output()
        .expect("run cargo deny");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cargo deny check licenses failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("GPL"),
        "a GPL licence is in the graph:\n{stderr}"
    );
}
