//! `loams upgrade snapshots` and `status` (UP1 Task 12).

mod support;

use loams_upgrade::errors::exit_for;
use loams_upgrade::snapshots;
use loams_upgrade::status::{self, StatusReport};
use support::{Fake, failed, ok, stack, stack_dir};

#[test]
fn prune_requires_yes() {
    let dir = stack_dir("LOAMS_VERSION=0.1.0\n");
    let stack = stack(&dir);
    let fake = Fake::new(|_| ok(""));
    let image = "ghcr.io/ostrium-labs/loams:0.1.0";
    let err = snapshots::prune(&fake, &stack, image, "01J", false).unwrap_err();
    assert_eq!(err.code, "confirmation_required");
    assert_eq!(exit_for(err.code), 3);
    assert!(fake.calls().is_empty(), "nothing ran without --yes");
    snapshots::prune(&fake, &stack, image, "01J", true).expect("prune");
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    let at = call.iter().position(|a| a == image).expect("the image");
    assert_eq!(call[at + 1..at + 4], ["migrate", "snapshots", "prune"]);
    assert!(call.contains(&"--id=01J".to_string()));
    // An unknown id is snapshot_not_found (exit 4).
    let fake = Fake::new(|_| failed(4, "loams migrate: snapshot_not_found: 01K"));
    let err = snapshots::prune(&fake, &stack, image, "01K", true).unwrap_err();
    assert_eq!(err.code, "snapshot_not_found");
    assert_eq!(err.exit_code(), 4);
}

#[test]
fn snapshots_list_passes_the_listing_through() {
    let dir = stack_dir("LOAMS_VERSION=0.1.0\n");
    let listing = r#"[{"id":"01J","complete":true,"bytes":10}]"#;
    let fake = Fake::new(move |_| ok(listing));
    let value = snapshots::list(&fake, &stack(&dir), "img").expect("list");
    assert_eq!(value[0]["id"], "01J");
}

#[test]
fn status_reports_the_ledger_and_the_last_attempt() {
    let dir = stack_dir("LOAMS_VERSION=0.1.0-preview.2\n");
    std::fs::create_dir(dir.path().join(".loams-upgrade")).expect("mkdir");
    std::fs::write(
        dir.path().join(status::HISTORY),
        "{\"target\":\"0.1.0\",\"outcome\":\"rolled_back\"}\n{\"target\":\"0.1.0\",\"outcome\":\"done\"}\n",
    )
    .expect("w");
    // preview.2's image has no `migrate`: reported, not fatal.
    let fake = Fake::new(|_| failed(2, "error: unrecognized subcommand 'migrate'"));
    let report = status::status(&fake, &stack(&dir), "0.1.0-preview.2").expect("status");
    assert_eq!(report.migrate, None);
    assert!(
        report
            .migrate_error
            .as_deref()
            .is_some_and(|e| e.contains("unrecognized subcommand")),
        "{report:?}"
    );
    assert_eq!(
        report.last_attempt,
        Some(serde_json::json!({"target": "0.1.0", "outcome": "done"}))
    );
    let schema = serde_json::to_value(schemars::schema_for!(StatusReport)).expect("schema");
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schemas/status.json");
    if std::env::var_os("LOAMS_UPDATE_SCHEMAS").is_some() {
        std::fs::write(
            &file,
            serde_json::to_string_pretty(&schema).expect("json") + "\n",
        )
        .expect("write");
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("status.json")).expect("json");
    assert_eq!(
        schema, stored,
        "StatusReport changed: LOAMS_UPDATE_SCHEMAS=1"
    );
}

/// The engine's log lines can share a helper's stdout (the gate's first
/// `--keep-writes` run read nothing): the JSON is the last JSON line.
#[test]
fn helper_json_skips_log_lines() {
    use loams_upgrade::runtime::helper_json;
    let out = "\u{1b}[2m2026-10-16T00:09:58Z\u{1b}[0m INFO tikv_client: connect\n{\"stack\":null,\"ledger\":[]}\n";
    assert_eq!(
        helper_json(out).expect("json")["ledger"],
        serde_json::json!([])
    );
    assert_eq!(helper_json("[1,2]").expect("whole")[1], 2);
    assert!(helper_json("INFO only logs").is_err());
}
