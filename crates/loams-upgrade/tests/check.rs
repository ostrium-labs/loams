//! `loams upgrade check` and the stack directory (UP1 Task 12).

mod support;

use std::path::Path;

use loams_upgrade::check::{self, CheckReport, Observed};
use loams_upgrade::helpers::{self, SNAPSHOTS};
use loams_upgrade::path::UpgradePath;
use loams_upgrade::stackdir::StackDir;
use serde_json::json;
use support::{Fake, failed, ok, stack, stack_dir};
use tempfile::TempDir;

/// The real release train.
fn train() -> UpgradePath {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/upgrade-path.toml"),
    )
    .expect("release/upgrade-path.toml");
    UpgradePath::parse(&text).expect("parses")
}

#[test]
fn stack_dir_discovery_order() {
    let flag = stack_dir("LOAMS_VERSION=0.1.0-preview.2\n");
    let env = stack_dir("LOAMS_VERSION=0.1.0-preview.1\n");
    let cwd = stack_dir("");
    let found = StackDir::discover(Some(flag.path()), Some(env.path()), cwd.path()).expect("flag");
    assert_eq!(found.dir, flag.path());
    let found = StackDir::discover(None, Some(env.path()), cwd.path()).expect("env");
    assert_eq!(found.dir, env.path());
    assert_eq!(found.release().as_deref(), Some("0.1.0-preview.1"));
    let found = StackDir::discover(None, None, cwd.path()).expect("cwd");
    assert_eq!(found.dir, cwd.path());
    // No LOAMS_VERSION: the compose file's default.
    assert_eq!(found.release().as_deref(), Some("0.1.0-preview.2"));
    // A flag that names no stack is not skipped for the next source.
    let empty = TempDir::new().expect("empty");
    let err = StackDir::discover(Some(empty.path()), Some(env.path()), cwd.path()).unwrap_err();
    assert_eq!(err.code, "stack_not_found");
    assert!(err.message.contains("--dir"), "{err}");
}

#[test]
fn non_loams_compose_dir_is_not_a_stack() {
    let dir = TempDir::new().expect("dir");
    std::fs::write(
        dir.path().join("compose.yaml"),
        "name: shop\nservices: {}\n",
    )
    .expect("w");
    let err = StackDir::discover(None, None, dir.path()).unwrap_err();
    assert_eq!(err.code, "stack_not_found");
    assert_eq!(err.exit_code(), 4);
    // A `name:` nested under a service is not the project's.
    std::fs::write(
        dir.path().join("compose.yaml"),
        "services:\n  x:\n    labels:\n      name: loams\n",
    )
    .expect("w");
    assert!(!loams_upgrade::stackdir::is_loams_stack(dir.path()));
    std::fs::write(dir.path().join("compose.yaml"), "name: \"loams\"\n").expect("w");
    assert!(loams_upgrade::stackdir::is_loams_stack(dir.path()));
}

fn observed() -> Observed {
    Observed {
        current_components: [("tikv", "v8.5.8"), ("pd", "v8.5.8")]
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        plan: Some(json!({
            "from": null, "to": "0.1.0", "point_of_no_return": false,
            "steps": [
                {"id": "0000-adopt-preview-2", "store": "meta", "class": "reversible",
                 "estimate": {"rows": 1, "bytes": 0, "seconds": 2}, "state": "pending"},
                {"id": "0001-safekeeper-volume", "store": "host", "class": "reversible",
                 "estimate": {"rows": 0, "bytes": 0, "seconds": 30}, "state": "pending"}
            ]
        })),
        unhealthy: Vec::new(),
        snapshot_estimate_bytes: 1_000,
        free_bytes: Some(1_000_000),
    }
}

#[test]
fn check_json_matches_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(CheckReport)).expect("schema");
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schemas/check.json");
    if std::env::var_os("LOAMS_UPDATE_SCHEMAS").is_some() {
        std::fs::write(
            &file,
            serde_json::to_string_pretty(&schema).expect("json") + "\n",
        )
        .expect("write");
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("tests/schemas/check.json"))
            .expect("parses");
    assert_eq!(
        schema, stored,
        "CheckReport changed: rerun with LOAMS_UPDATE_SCHEMAS=1 and review the diff"
    );
    // A real report has exactly the schema's required properties.
    let report =
        check::report("0.1.0-preview.2", "0.1.0", &train(), &observed()).expect("report");
    let value = serde_json::to_value(&report).expect("json");
    let mut keys: Vec<&String> = value.as_object().expect("object").keys().collect();
    keys.sort();
    let mut props: Vec<&String> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .collect();
    props.sort();
    assert_eq!(keys, props);
    // §54 §10.2's example.
    assert_eq!(value["path"], json!(["0.1.0"]));
    assert_eq!(
        value["migrations"][1],
        json!({"id": "0001-safekeeper-volume", "store": "host", "class": "reversible",
               "estimate": {"seconds": 30}})
    );
    assert_eq!(value["blockers"], json!([]));
    assert_eq!(value["point_of_no_return"], json!(false));
}

fn codes(report: &CheckReport) -> Vec<&str> {
    report.blockers.iter().map(|b| b.code.as_str()).collect()
}

#[test]
fn check_reports_blockers() {
    let path = train();
    // Not adjacent: preview.1 is not 0.1.0's source, and the chain is named.
    let report =
        check::report("0.1.0-preview.1", "0.1.0", &path, &observed()).expect("report");
    assert_eq!(codes(&report), ["upgrade_step_not_adjacent"]);
    assert!(
        report.blockers[0]
            .message
            .contains("0.1.0-preview.2 -> 0.1.0"),
        "{:?}",
        report.blockers
    );
    // A downgrade of a pinned component.
    let mut newer = observed();
    newer
        .current_components
        .insert("tikv".to_string(), "v8.5.9".to_string());
    let report = check::report("0.1.0-preview.2", "0.1.0", &path, &newer).expect("r");
    assert_eq!(codes(&report), ["component_downgrade"]);
    // Too little space for 1.5× the estimate.
    let mut low = observed();
    low.free_bytes = Some(1_400);
    let report = check::report("0.1.0-preview.2", "0.1.0", &path, &low).expect("r");
    assert_eq!(codes(&report), ["insufficient_space"]);
    low.free_bytes = Some(1_500);
    let report = check::report("0.1.0-preview.2", "0.1.0", &path, &low).expect("r");
    assert!(report.blockers.is_empty(), "{:?}", report.blockers);
    // A migration a run left failed.
    let mut failed_run = observed();
    failed_run.plan.as_mut().expect("plan")["steps"][0]["state"] = json!("failed");
    let report =
        check::report("0.1.0-preview.2", "0.1.0", &path, &failed_run).expect("r");
    assert_eq!(codes(&report), ["migration_incomplete"]);
    assert!(report.blockers[0].message.contains("0000-adopt-preview-2"));
    // An unhealthy stack.
    let mut sick = observed();
    sick.unhealthy = vec!["tikv".to_string()];
    let report = check::report("0.1.0-preview.2", "0.1.0", &path, &sick).expect("r");
    assert_eq!(codes(&report), ["stack_unhealthy"]);
    // A release the train does not have.
    let err = check::report("0.1.0-preview.2", "9.9.9", &path, &observed()).unwrap_err();
    assert_eq!(err.code, "release_not_found");
}

/// The engine is down (its container exited): `check` still plans, through
/// `loams migrate plan` in a helper of the target image.
#[test]
fn check_reads_plan_when_engine_down() {
    let dir = stack_dir(
        "LOAMS_VERSION=0.1.0-preview.2\nLOAMS_BUCKET=s3://b\nAWS_SECRET_ACCESS_KEY=hunter2\n",
    );
    let stack = stack(&dir);
    let plan = observed().plan.expect("plan").to_string();
    let fake = Fake::new(move |args| {
        if args.first().map(String::as_str) == Some("compose") {
            return ok(concat!(
                r#"{"Service":"tikv","State":"running","Health":"healthy","ExitCode":0}"#,
                "\n",
                r#"{"Service":"loams","State":"exited","Health":"","ExitCode":137}"#,
                "\n",
                r#"{"Service":"migrate","State":"exited","Health":"","ExitCode":0}"#,
            ));
        }
        if args.iter().any(|a| a == "plan") {
            return ok(&plan);
        }
        if args.iter().any(|a| a.starts_with("du ")) {
            return ok("4096\n");
        }
        failed(1, "unexpected")
    });
    let report = check::check(&fake, &stack, &train(), "0.1.0").expect("check");
    assert_eq!(report.current, "0.1.0-preview.2");
    assert_eq!(report.migrations[0].estimate.seconds, 2);
    assert_eq!(report.snapshot_estimate_bytes, 4096);
    assert_eq!(codes(&report), ["stack_unhealthy"]);
    assert!(report.blockers[0].message.contains("loams"));
    let calls = fake.calls();
    let plan_call = calls
        .iter()
        .find(|c| c.iter().any(|a| a == "plan"))
        .expect("a plan helper ran");
    assert!(plan_call.contains(&"ghcr.io/ostrium-labs/loams:0.1.0".to_string()));
    assert!(plan_call.contains(&"--declared-release=0.1.0-preview.2".to_string()));
    assert!(plan_call.contains(&"--target-release=0.1.0".to_string()));
    assert!(plan_call.contains(&"--bucket=s3://b".to_string()));
    // No .env value other than the bucket's URL is put on a command line.
    for call in &calls {
        assert!(!call.iter().any(|a| a.contains("hunter2")), "{call:?}");
    }
}

/// UP1 Task 9's boundary (design §54 §10.1), on the rendered helper specs.
#[test]
fn helper_containers_follow_the_boundary() {
    let dir = stack_dir("LOAMS_VERSION=0.1.0-preview.2\n");
    std::fs::create_dir(dir.path().join("secrets")).expect("secrets");
    std::fs::write(
        dir.path().join("secrets/engine.env"),
        "AWS_SECRET_ACCESS_KEY=x\n",
    )
    .expect("w");
    let stack = stack(&dir);
    let image = "ghcr.io/ostrium-labs/loams:0.1.0";
    let br = "docker.io/pingcap/br@sha256:cddec1ce";
    let specs = [
        (
            "migrate",
            helpers::migrate_helper(&stack, image, &["plan".to_string()]),
        ),
        ("br", helpers::br_helper(&stack, br, false, "01J")),
        (
            "cold",
            helpers::cold_copy_helper(&stack, image, false, "01J"),
        ),
    ];
    let data_volumes = [
        "loams_loams-data",
        "loams_house-data",
        "loams_pg-control-data",
        "loams_pd-data",
        "loams_tikv-data",
        "loams_upgrade-snapshots",
    ];
    for (name, spec) in &specs {
        let args = spec.render();
        let joined = args.join(" ");
        assert!(
            args.contains(&"--read-only".to_string()),
            "{name}: {joined}"
        );
        let cap = args
            .iter()
            .position(|a| a == "--cap-drop")
            .expect("cap-drop");
        assert_eq!(args[cap + 1], "ALL", "{name}");
        assert!(
            joined.contains("--security-opt no-new-privileges"),
            "{name}"
        );
        let net = args.iter().position(|a| a == "--network").expect("network");
        assert!(
            ["loams_default", "none"].contains(&args[net + 1].as_str()),
            "{name}: only the stack's default network: {joined}"
        );
        for banned in ["-p", "--publish", "--privileged", "--pid", "--ipc"] {
            assert!(!args.contains(&banned.to_string()), "{name}: {banned}");
        }
        assert!(!joined.contains("docker.sock") && !joined.contains("podman.sock"));
        assert!(!joined.contains("house") || name == &"migrate", "{name}");
        for m in &spec.mounts {
            assert!(
                data_volumes.contains(&m.source.as_str()),
                "{name}: mounts only data volumes and the snapshots: {}",
                m.source
            );
        }
        // Only the migrate helper gets the engine's credentials, as a file.
        assert_eq!(spec.env_file.is_some(), *name == "migrate", "{name}");
        // Capabilities are added back only to the cold copy (TiKV's files).
        assert_eq!(spec.cap_add.is_empty(), *name != "cold", "{name}");
    }
    // The snapshots are always at the same path, so BR and TiKV agree.
    assert!(
        specs[1]
            .1
            .args
            .iter()
            .any(|a| a == &format!("local://{SNAPSHOTS}/01J/tikv"))
    );
    // A host snapshot directory is the only host path a helper mounts.
    let host = TempDir::new().expect("host");
    let dir = stack_dir(&format!(
        "LOAMS_UPGRADE_SNAPSHOT_DIR={}\n",
        host.path().display()
    ));
    let spec = helpers::migrate_helper(&support::stack(&dir), image, &[]);
    let host_mounts: Vec<_> = spec
        .mounts
        .iter()
        .filter(|m| m.source.starts_with('/'))
        .collect();
    assert_eq!(host_mounts.len(), 1);
    assert_eq!(host_mounts[0].target, SNAPSHOTS);
}
