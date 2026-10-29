//! MK1 Task 8: the LAPC Core runner against the Node fixture agents in
//! `testdata/conformance` (no npm dependencies). `agent/` is the package:
//! `good.js` conforms and each other entry file breaks one requirement; a
//! case is the good listing with that entry file. `c8-install-script/` is a
//! `package.json` laid over a copy of `agent/`. The tests need `node` and
//! skip without it.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use loams_agentd_marketplace::conformance::{
    self, CheckStatus, Limits, Options, Package, Report, Requirement, Target,
};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/conformance")
}

fn node() -> Option<PathBuf> {
    let found = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("node"))
        .find(|p| p.is_file());
    if found.is_none() {
        eprintln!("skipped: no node on PATH");
    }
    found
}

fn good_listing() -> Value {
    serde_json::from_slice(&std::fs::read(fixtures().join("agent/listing.json")).unwrap()).unwrap()
}

fn options(node: PathBuf) -> Options {
    Options {
        node: Some(node),
        limits: Limits {
            ready: Duration::from_secs(5),
            turn: Duration::from_secs(30),
            stop: Duration::from_secs(3),
        },
        ..Options::default()
    }
}

fn run_with(listing: Value, package: PathBuf, node: PathBuf) -> Report {
    let target = Target {
        listing,
        package: Package::Dir(package),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(conformance::run(&target, &options(node)))
        .unwrap()
}

/// The good listing with another entry file.
fn run_case(entry: &str) -> Option<Report> {
    let node = node()?;
    let mut listing = good_listing();
    listing["entrypoint"] = entry.into();
    Some(run_with(listing, fixtures().join("agent"), node))
}

/// The conforming fixture's report, run once for every pass test.
fn good() -> Option<&'static Report> {
    static GOOD: OnceLock<Option<Report>> = OnceLock::new();
    GOOD.get_or_init(|| run_case("good.js")).as_ref()
}

fn status(report: &Report, id: &str) -> CheckStatus {
    report
        .checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id} in the report"))
        .status
}

fn details(report: &Report, id: &str) -> String {
    report
        .checks
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.details.join("\n"))
        .unwrap_or_default()
}

fn passes(id: &str) {
    let Some(report) = good() else { return };
    assert_eq!(
        status(report, id),
        CheckStatus::Pass,
        "{id}: {}\nstderr: {}",
        details(report, id),
        report.measured.stderr_tail
    );
}

/// `id` fails, every other Core MUST passes, and the run is not Core.
fn fails_alone(report: &Report, id: &str) {
    assert_eq!(status(report, id), CheckStatus::Fail, "{id} should fail");
    for check in &report.checks {
        if check.id != id && check.requirement == Requirement::Must {
            assert_eq!(
                check.status,
                CheckStatus::Pass,
                "{} should pass beside a failing {id}: {:?}",
                check.id,
                check.details
            );
        }
    }
    assert_eq!(report.level, "none");
    assert!(!report.passed());
}

#[test]
fn the_conforming_fixture_reaches_core_and_its_report_matches_the_schema() {
    let Some(report) = good() else { return };
    assert_eq!(report.level, "core", "{}", report.to_json());
    assert!(report.passed());
    assert_eq!(
        status(report, "LAPC-C9"),
        CheckStatus::Pass,
        "{}",
        details(report, "LAPC-C9")
    );
    let json: Value = serde_json::from_str(&report.to_json()).unwrap();
    conformance::validate_report(&json).unwrap();
    assert_eq!(report.subject.id, "ostrium/lapc-fixture");
    assert_eq!(report.subject.package.origin, "dir");
    assert!(report.measured.model_requests >= 4, "{:?}", report.measured);
    if cfg!(target_os = "linux") {
        assert!(
            report
                .runner
                .egress_recording
                .contains(&"procfs".to_owned())
        );
    }
}

#[test]
fn lapc_c1_ready_passes() {
    passes("LAPC-C1");
}

#[test]
fn lapc_c1_no_initialize_fails() {
    let Some(report) = run_case("c1-no-initialize.js") else {
        return;
    };
    assert_eq!(status(&report, "LAPC-C1"), CheckStatus::Fail);
    // Nothing that needs a ready agent ran, and nothing skipped counts as a pass.
    for id in ["LAPC-C2", "LAPC-C3", "LAPC-C4", "LAPC-C7"] {
        assert_eq!(status(&report, id), CheckStatus::Skip, "{id}");
    }
    assert_eq!(report.level, "none");
    let json: Value = serde_json::from_str(&report.to_json()).unwrap();
    conformance::validate_report(&json).unwrap();
}

#[test]
fn lapc_c2_surfaces_pass() {
    passes("LAPC-C2");
}

#[test]
fn lapc_c2_protocol_mismatch_fails() {
    let Some(report) = run_case("c2-protocol-mismatch.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C2");
    assert!(details(&report, "LAPC-C2").contains("protocol version 2"));
}

#[test]
fn lapc_c3_scripted_turn_passes() {
    passes("LAPC-C3");
}

#[test]
fn lapc_c3_no_model_fails() {
    let Some(report) = run_case("c3-no-model.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C3");
    assert!(details(&report, "LAPC-C3").contains("no request reached the model proxy"));
}

#[test]
fn lapc_c4_declared_tools_pass() {
    passes("LAPC-C4");
    let Some(report) = good() else { return };
    // Both always-ask tools ran and were asked about.
    let asked: Vec<&str> = report
        .measured
        .tool_calls
        .iter()
        .filter(|t| t.asked)
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        asked.contains(&"save_note") && asked.contains(&"create_issue"),
        "{asked:?}"
    );
}

#[test]
fn lapc_c4_undeclared_tool_fails() {
    let Some(report) = run_case("c4-undeclared-tool.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C4");
    assert!(details(&report, "LAPC-C4").contains("\"shell_exec\""));
}

#[test]
fn lapc_c4_unasked_tool_fails() {
    let Some(report) = run_case("c4-skips-ask.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C4");
    assert!(details(&report, "LAPC-C4").contains("without asking permission"));
}

#[test]
fn lapc_c5_declared_egress_passes() {
    passes("LAPC-C5");
    let Some(report) = good() else { return };
    assert!(
        report
            .measured
            .egress
            .iter()
            .any(|e| e.host == "api.example.com" && e.allowed),
        "{:?}",
        report.measured.egress
    );
}

#[test]
fn lapc_c5_egress_fails() {
    let Some(report) = run_case("c5-egress.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C5");
    assert!(details(&report, "LAPC-C5").contains("exfil.example:443"));
}

#[test]
fn lapc_c6_data_dir_writes_pass() {
    passes("LAPC-C6");
}

#[test]
fn lapc_c6_package_write_fails() {
    let Some(report) = run_case("c6-writes-package.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C6");
    assert!(details(&report, "LAPC-C6").contains("stray.txt"));
}

#[test]
fn lapc_c7_clean_stop_passes() {
    passes("LAPC-C7");
}

#[test]
fn lapc_c7_hang_fails() {
    let Some(report) = run_case("c7-hang.js") else {
        return;
    };
    fails_alone(&report, "LAPC-C7");
    let why = details(&report, "LAPC-C7");
    assert!(why.contains("session/cancel"), "{why}");
    assert!(why.contains("SIGTERM"), "{why}");
}

#[test]
fn lapc_c8_no_install_code_passes() {
    passes("LAPC-C8");
}

#[test]
fn lapc_c8_install_script_fails() {
    let Some(node) = node() else { return };
    let package = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(fixtures().join("agent"))
        .unwrap()
        .flatten()
    {
        std::fs::copy(entry.path(), package.path().join(entry.file_name())).unwrap();
    }
    std::fs::copy(
        fixtures().join("c8-install-script/package.json"),
        package.path().join("package.json"),
    )
    .unwrap();
    let report = run_with(good_listing(), package.path().to_owned(), node);
    fails_alone(&report, "LAPC-C8");
    assert!(details(&report, "LAPC-C8").contains("postinstall"));
    // The script never ran.
    assert!(!package.path().join("installed.txt").exists());
}

#[test]
fn lapc_c9_cost_profile_passes() {
    passes("LAPC-C9");
}

#[test]
fn lapc_c9_cost_profile_fails() {
    let Some(node) = node() else { return };
    let mut listing = good_listing();
    listing["cost"]["typical_tokens_per_run"] = 1_000_000.into();
    let report = run_with(listing, fixtures().join("agent"), node);
    assert_eq!(status(&report, "LAPC-C9"), CheckStatus::Fail);
    assert!(details(&report, "LAPC-C9").contains("not within 2x"));
    // A SHOULD: the run still reaches Core.
    assert_eq!(report.level, "core");
}

#[test]
fn a_directory_target_reads_its_listing_and_a_file_target_uses_the_registry() {
    let dir = fixtures().join("agent");
    let target = Target::resolve(&dir, None).unwrap();
    assert_eq!(target.package, Package::Dir(dir.clone()));
    assert_eq!(target.listing["id"], "ostrium/lapc-fixture");
    let file = Target::resolve(&dir.join("listing.json"), None).unwrap();
    assert_eq!(file.package, Package::Registry);
    let with = Target::resolve(&dir.join("listing.json"), Some(&dir)).unwrap();
    assert_eq!(with.package, Package::Dir(dir));
}

#[test]
fn an_invalid_listing_is_refused_before_anything_runs() {
    let target = Target {
        listing: serde_json::json!({ "id": "ostrium/x" }),
        package: Package::Registry,
    };
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(conformance::run(&target, &Options::default()))
        .unwrap_err();
    assert!(error.to_string().contains("listing schema v1"), "{error}");
}
