//! `loams upgrade rollback` against a recording container runtime (UP1
//! Task 14).

mod support;

use std::sync::Mutex;

use loams_upgrade::history::{Event, History};
use loams_upgrade::rollback::{self, RollbackOptions, utc};
use loams_upgrade::runtime::Output;
use serde_json::json;
use support::world::*;
use support::{Fake, failed, ok};

fn options(keep_writes: bool) -> RollbackOptions {
    RollbackOptions {
        snapshot: None,
        keep_writes,
        br_image: "docker.io/pingcap/br@sha256:cddec1ce".to_string(),
    }
}

/// A world upgraded to preview.3, and the snapshot its upgrade took.
fn upgraded(env: &str) -> (World, String) {
    let world = World::new(env);
    let fake = scripted(Script::new(), false);
    let mut stack = world.stack();
    let report = loams_upgrade::run::upgrade_step(
        &fake,
        &mut stack,
        &world.bundle(),
        "0.1.0-preview.2",
        "0.1.0-preview.3",
        &opts(),
    )
    .expect("upgrade");
    assert!(world.env().contains("LOAMS_VERSION=0.1.0-preview.3"));
    (world, report.snapshot)
}

/// The scripted runtime, plus the listing of `snapshot` (the upgrade's
/// fake is gone) and `migrate status` answering `status`.
fn after_upgrade(snapshot: &str, script: Script, status: Option<Output>) -> Fake {
    let base = scripted(script, false);
    let listing = json!([{ "id": snapshot, "complete": true }]).to_string();
    Fake::new(move |args| match kind(args).as_str() {
        "migrate snapshots list" => ok(&listing),
        "migrate status" if status.is_some() => status.clone().expect("status"),
        _ => loams_upgrade::runtime::Runtime::engine(&base, args).expect("base"),
    })
}

fn ledger(class: &str) -> Output {
    ok(&json!({
        "stack": {"release": "0.1.0-preview.3"},
        "ledger": [
            {"seq": 1, "id": "0000-adopt-preview-2", "from": "0.1.0-preview.2",
             "to": "0.1.0-preview.3", "store": "meta", "class": "reversible", "state": "done"},
            {"seq": 2, "id": "0001-safekeeper-volume", "from": "0.1.0-preview.2",
             "to": "0.1.0-preview.3", "store": "host", "class": class, "state": "done"}
        ]
    })
    .to_string())
}

#[test]
fn rollback_restores_every_component() {
    let (world, snapshot) = upgraded(ENV);
    let fake = after_upgrade(&snapshot, Script::new(), None);
    let mut stack = world.stack();
    let report =
        rollback::rollback(&fake, &mut stack, &options(false), &|_| true).expect("rollback");
    assert_eq!(report.from, "0.1.0-preview.3");
    assert_eq!(report.to, "0.1.0-preview.2");
    assert_eq!(report.snapshot, snapshot);
    let k = kinds(&fake);
    in_order(
        &k,
        &[
            "migrate snapshots list",
            "stop tailscale house-tailscale",
            "cold restore",
            "up pd tikv",
            "migrate restore",
            "engine ready",
            "up tailscale house-tailscale",
        ],
    );
    let restore = fake
        .calls()
        .into_iter()
        .find(|c| kind(c) == "migrate restore")
        .expect("restore");
    assert!(restore.contains(&format!(
        "--from=/var/lib/loams-upgrade-snapshots/{snapshot}"
    )));
    assert!(restore.contains(&"--tikv-version=v8.5.8".to_string()));
    // It ran the image that took the snapshot.
    assert!(restore.contains(&"ghcr.io/ostrium-labs/loams:0.1.0-preview.3".to_string()));
    // The previous bundle, .env and Postgres files are back.
    assert_eq!(world.env(), ENV);
    let dir = world.stack_dir();
    assert!(
        std::fs::read_to_string(dir.join("compose.yaml"))
            .expect("c")
            .contains("preview.2")
    );
    assert!(
        !dir.join(".env.example").exists(),
        "the target's files are gone"
    );
    assert!(
        dir.join("compose.override.yaml").exists(),
        "the user's file stays"
    );
    assert!(
        std::fs::read_to_string(
            world
                .root
                .path()
                .join("stack/loams-postgres-dev/compose.yaml")
        )
        .expect("pg")
        .contains("preview.2")
    );
    assert_eq!(
        History::of(&dir).entries().last().expect("entry").event,
        Event::RolledBack
    );
    // The files keep their modes (a service bind-mounts some of them and reads
    // them as another user), though the host copy is private.
    use std::os::unix::fs::PermissionsExt as _;
    let mode =
        |p: std::path::PathBuf| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    assert_eq!(
        mode(
            world
                .root
                .path()
                .join("stack/loams-postgres-dev/compose.yaml")
        ),
        0o644
    );
    assert_eq!(mode(dir.join("compose.yaml")), 0o644);
    // With BR, TiKV is restored into fresh volumes at the snapshot's version.
    let world = World::new(ENV);
    let br = scripted(Script::new(), true);
    let mut stack = world.stack();
    let report = loams_upgrade::run::upgrade_step(
        &br,
        &mut stack,
        &world.bundle(),
        "0.1.0-preview.2",
        "0.1.0-preview.3",
        &opts(),
    )
    .expect("upgrade");
    let fake = after_upgrade(&report.snapshot, Script::new(), None);
    let mut stack = world.stack();
    rollback::rollback(&fake, &mut stack, &options(false), &|_| true).expect("rollback");
    in_order(
        &kinds(&fake),
        &["volume rm", "up pd tikv", "br restore", "migrate restore"],
    );
}

#[test]
fn rollback_prints_lost_writes_window() {
    assert_eq!(utc(0), "1970-01-01 00:00 UTC");
    assert_eq!(utc(1_760_000_000), "2025-10-09 08:53 UTC");
    assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
    let (world, snapshot) = upgraded(ENV);
    let fake = after_upgrade(&snapshot, Script::new(), None);
    let asked = Mutex::new(String::new());
    let mut stack = world.stack();
    let err = rollback::rollback(&fake, &mut stack, &options(false), &|q| {
        *asked.lock().expect("asked") = q.to_string();
        false
    })
    .unwrap_err();
    assert_eq!(err.code, "confirmation_required");
    assert_eq!(err.exit_code(), 3);
    let question = asked.lock().expect("asked").clone();
    assert!(question.contains("writes between "), "{question}");
    assert!(question.contains(" UTC will be lost"), "{question}");
    assert!(question.contains(&snapshot), "{question}");
    assert!(err.message.contains("will be lost"), "{err}");
    // Nothing was stopped or restored.
    let k = kinds(&fake);
    assert!(
        !k.iter()
            .any(|k| k.starts_with("stop") || k.contains("restore")),
        "{k:?}"
    );
    assert!(world.env().contains("LOAMS_VERSION=0.1.0-preview.3"));
}

#[test]
fn keep_writes_refused_with_point_of_no_return() {
    let (world, snapshot) = upgraded(ENV);
    let fake = after_upgrade(&snapshot, Script::new(), Some(ledger("point_of_no_return")));
    let mut stack = world.stack();
    let err = rollback::rollback(&fake, &mut stack, &options(true), &|_| true).unwrap_err();
    assert_eq!(err.code, "rollback_needs_snapshot");
    assert_eq!(err.exit_code(), 6);
    assert!(err.message.contains("0001-safekeeper-volume"), "{err}");
    assert!(!kinds(&fake).iter().any(|k| k.starts_with("stop")));
    // All reversible and no pin changed: the images are swapped back and
    // the data is kept (no restore).
    let fake = after_upgrade(&snapshot, Script::new(), Some(ledger("reversible")));
    let mut stack = world.stack();
    let report = rollback::rollback(&fake, &mut stack, &options(true), &|_| true).expect("keep");
    assert!(report.keep_writes);
    assert!(report.lost_writes.starts_with("none"));
    let k = kinds(&fake);
    assert!(!k.iter().any(|k| k.contains("restore")), "{k:?}");
    in_order(
        &k,
        &[
            "stop tailscale house-tailscale",
            "engine ready",
            "up tailscale house-tailscale",
        ],
    );
    assert_eq!(world.env(), ENV);
}

#[test]
fn keep_writes_refused_after_pin_change() {
    let (world, snapshot) = upgraded(&format!("{ENV}TIKV_TAG=v8.5.8\n"));
    // The upgrade moved TiKV (as a later release's pin bump would).
    let env = world.env().replace("TIKV_TAG=v8.5.8", "TIKV_TAG=v8.5.9");
    std::fs::write(world.stack_dir().join(".env"), env).expect("w");
    let fake = after_upgrade(&snapshot, Script::new(), Some(ledger("reversible")));
    let mut stack = world.stack();
    let err = rollback::rollback(&fake, &mut stack, &options(true), &|_| true).unwrap_err();
    assert_eq!(err.code, "rollback_needs_snapshot");
    assert!(
        err.message.contains("changed from v8.5.8 to v8.5.9"),
        "{err}"
    );
}

#[test]
fn rollback_failure_exits_11_with_steps() {
    let (world, snapshot) = upgraded(ENV);
    let mut script = Script::new();
    script.insert(
        "migrate restore".to_string(),
        failed(78, "snapshot_incomplete"),
    );
    let fake = after_upgrade(&snapshot, script, None);
    let mut stack = world.stack();
    let err = rollback::rollback(&fake, &mut stack, &options(false), &|_| true).unwrap_err();
    assert_eq!(err.code, "rollback_failed");
    assert_eq!(err.exit_code(), 11);
    for needle in [
        snapshot.as_str(),
        "By hand",
        "loams migrate restore",
        ".loams-upgrade/snapshots",
    ] {
        assert!(err.message.contains(needle), "{needle}: {err}");
    }
    assert_eq!(
        History::of(&world.stack_dir())
            .entries()
            .last()
            .expect("entry")
            .event,
        Event::RollbackFailed
    );
    // No snapshot in the history: snapshot_not_found (exit 4).
    let fresh = World::new(ENV);
    let mut stack = fresh.stack();
    let err = rollback::rollback(&fake, &mut stack, &options(false), &|_| true).unwrap_err();
    assert_eq!(err.code, "snapshot_not_found");
    assert_eq!(err.exit_code(), 4);
}
