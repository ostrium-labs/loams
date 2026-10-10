//! The CI fuzz job (.github/workflows/ci.yml, `sqlgate-fuzz`) runs every
//! cargo-fuzz target, pinned (R3.15).
#[test]
fn ci_fuzzes_every_target_pinned() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let ci = std::fs::read_to_string(format!("{root}/.github/workflows/ci.yml")).expect("ci.yml");
    let start = ci.find("\n  sqlgate-fuzz:\n").expect("sqlgate-fuzz job");
    let len = ci[start + 1..].find("\n  deny:\n").expect("the next job");
    let job = &ci[start..start + 1 + len];
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fuzz/Cargo.toml"))
        .expect("fuzz/Cargo.toml");
    let targets: Vec<&str> = manifest
        .lines()
        .filter_map(|l| l.strip_prefix("name = \""))
        .filter_map(|l| l.strip_suffix('"'))
        .filter(|n| *n != "loams-sqlgate-fuzz")
        .collect();
    assert_eq!(targets.len(), 5, "{targets:?}");
    for t in targets {
        assert!(job.contains(t), "CI does not fuzz {t}");
    }
    for needle in [
        "cargo-fuzz@0.13.2",
        "--locked",
        "timeout-minutes:",
        "fuzz/artifacts",
        "nightly-2026-09-24",
    ] {
        assert!(job.contains(needle), "sqlgate-fuzz lacks {needle}");
    }
    assert!(
        !job.contains("toolchain install nightly "),
        "floating nightly"
    );
}
