//! `loams upgrade --to` against a recording container runtime (UP1 Task 13).

mod support;

use std::sync::{Arc, Mutex};

use clap::Parser;
use loams_upgrade::bundle::Bundle;
use loams_upgrade::history::{Event, History};
use loams_upgrade::run::{self, Options, STEPS};
use loams_upgrade::{UpgradeArgs, upgrade_chain};
use support::world::*;
use support::{Fake, failed, ok};
#[test]
fn steps_run_in_documented_order() {
    let world = World::new(ENV);
    let fake = scripted(Script::new(), false);
    let report = upgrade(&world, &fake, &opts()).expect("upgrade");
    let names: Vec<&str> = report.steps.iter().map(|s| s.step.as_str()).collect();
    assert_eq!(names, STEPS);
    let k = kinds(&fake);
    in_order(
        &k,
        &[
            "migrate plan",
            "stop tailscale house-tailscale",
            "stop compute1 house loams tidb",
            "prepare",
            "stop pd tikv",
            "cold copy",
            "up pd tikv",
            "migrate snapshot",
            "up pd",
            "up tikv",
            "migrate run",
            "up tidb",
            "up loams",
            "up house",
            "up rustfs storage_broker pageserver safekeeper1 compute1",
            "migrate verify",
            "engine ready",
            "up tailscale house-tailscale",
        ],
    );
    // The snapshot drained Postgres and recorded TiKV's copy.
    let snap = fake
        .calls()
        .into_iter()
        .find(|c| kind(c) == "migrate snapshot")
        .expect("snapshot");
    for flag in [
        "--pageserver=http://pageserver:9898",
        "--tikv=cold",
        "--tikv-version=v8.5.8",
        "--target-release=0.1.0",
    ] {
        assert!(snap.contains(&flag.to_string()), "{flag}: {snap:?}");
    }
    // migrate ran from the target image, with the declared source.
    let migrate = fake
        .calls()
        .into_iter()
        .find(|c| kind(c) == "migrate run")
        .expect("run");
    assert!(migrate.contains(&"ghcr.io/ostrium-labs/loams:0.1.0".to_string()));
    assert!(migrate.contains(&"--declared-release=0.1.0-preview.2".to_string()));
    assert_eq!(report.host_changes[0]["service"], "safekeeper1");
    // The bundle is installed beside the user's files.
    let dir = world.stack_dir();
    assert!(
        std::fs::read_to_string(dir.join("compose.yaml"))
            .expect("c")
            .contains("# 0.1.0")
    );
    assert!(dir.join("compose.override.yaml").exists());
    assert!(
        std::fs::read_to_string(
            world
                .root
                .path()
                .join("stack/loams-postgres-dev/compose.yaml")
        )
        .expect("pg")
        .contains("# 0.1.0")
    );
    assert!(world.env().contains("LOAMS_VERSION=0.1.0"));
    assert_eq!(last_event(&world), Event::Done);
    // BR when TiKV mounts the snapshots (0.1.0 onwards).
    let world = World::new(ENV);
    let fake = fake_br();
    upgrade(&world, &fake, &opts()).expect("upgrade");
    let k = kinds(&fake);
    in_order(&k, &["prepare", "br backup", "migrate snapshot"]);
    assert!(!k.contains(&"cold copy".to_string()));
}

fn fake_br() -> Fake {
    scripted(Script::new(), true)
}

#[test]
fn pull_failure_changes_nothing() {
    // A downloaded bundle is pulled; a local one is not, so this test marks
    // the bundle verified.
    let world = World::new(ENV);
    let mut script = Script::new();
    script.insert("pull images".to_string(), failed(1, "manifest unknown"));
    let fake = scripted(script, false);
    let mut bundle = world.bundle();
    bundle.verified = true;
    let mut stack = world.stack();
    let err = run::upgrade_step(
        &fake,
        &mut stack,
        &bundle,
        "0.1.0-preview.2",
        "0.1.0",
        &opts(),
    )
    .unwrap_err();
    assert_eq!(err.code, "registry");
    assert_eq!(err.exit_code(), 7);
    let k = kinds(&fake);
    assert!(
        !k.iter()
            .any(|k| k.starts_with("stop") || k.starts_with("up")),
        "{k:?}"
    );
    assert_eq!(world.env(), ENV);
    assert!(History::of(&world.stack_dir()).entries().is_empty());
    assert!(!world.stack_dir().join(".loams-upgrade/next.env").exists());
}

#[test]
fn door_closed_during_upgrade() {
    let world = World::new(ENV);
    let fake = scripted(Script::new(), false);
    upgrade(&world, &fake, &opts()).expect("upgrade");
    let k = kinds(&fake);
    let closed = k
        .iter()
        .position(|k| k == "stop tailscale house-tailscale")
        .expect("closed");
    let opened = k
        .iter()
        .rposition(|k| k == "up tailscale house-tailscale")
        .expect("opened");
    let verified = k
        .iter()
        .position(|k| k == "migrate verify")
        .expect("verify");
    assert!(verified < opened, "the door opens after the verification");
    for call in &k[opened + 1..] {
        assert!(
            !call.starts_with("up ") && !call.starts_with("stop "),
            "after the door opened: {call}"
        );
    }
    for call in &k[closed + 1..opened] {
        if call.starts_with("up ") {
            assert!(
                !call.contains("tailscale"),
                "a sidecar started with the door closed: {call}"
            );
        }
    }
    // Nothing that writes ran before the door closed.
    let quiesce = k
        .iter()
        .position(|k| k.starts_with("stop compute1"))
        .expect("quiesce");
    assert!(closed < quiesce);
}

fn assert_rolled_back(world: &World, fake: &Fake, err: &loams_upgrade::errors::UpgradeError) {
    assert_eq!(err.code, "upgrade_failed_rolled_back", "{err}");
    assert_eq!(err.exit_code(), 10);
    in_order(
        &kinds(fake),
        &[
            "migrate snapshot",
            "migrate snapshots list",
            "stop tailscale house-tailscale",
            "cold restore",
            "up pd tikv",
            "migrate restore",
            "engine ready",
            "up tailscale house-tailscale",
        ],
    );
    assert_eq!(world.env(), ENV, ".env is back");
    assert!(
        std::fs::read_to_string(world.stack_dir().join("compose.yaml"))
            .expect("c")
            .contains("preview.2")
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
    assert_eq!(last_event(world), Event::RolledBack);
}

#[test]
fn migrate_failure_rolls_back_automatically() {
    let world = World::new(ENV);
    let mut script = Script::new();
    script.insert(
        "migrate run".to_string(),
        failed(65, "loams migrate: migration_failed: 0000-adopt-preview-2"),
    );
    let fake = scripted(script, false);
    let err = upgrade(&world, &fake, &opts()).unwrap_err();
    assert!(err.message.contains("migration_incomplete"), "{err}");
    assert_rolled_back(&world, &fake, &err);
}

#[test]
fn verify_failure_rolls_back_automatically() {
    let world = World::new(ENV);
    let mut script = Script::new();
    script.insert(
        "migrate verify".to_string(),
        failed(65, "loams migrate: verify failed: collections"),
    );
    let fake = scripted(script, false);
    let err = upgrade(&world, &fake, &opts()).unwrap_err();
    assert!(err.message.contains("stack_unhealthy"), "{err}");
    assert_rolled_back(&world, &fake, &err);
}

#[test]
fn no_auto_rollback_leaves_failed_state_and_door_closed() {
    let world = World::new(ENV);
    let mut script = Script::new();
    script.insert("migrate run".to_string(), failed(65, "migration_failed"));
    let fake = scripted(script, false);
    let err = upgrade(
        &world,
        &fake,
        &Options {
            no_auto_rollback: true,
            ..opts()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "migration_incomplete");
    assert_eq!(err.exit_code(), 5);
    assert!(err.message.contains("rollback"), "{err}");
    let k = kinds(&fake);
    assert!(!k.contains(&"migrate restore".to_string()));
    assert!(
        !k.contains(&"up tailscale house-tailscale".to_string()),
        "{k:#?}"
    );
    assert_eq!(last_event(&world), Event::Failed);
    // Still at the target's files: a rerun resumes, a rollback restores.
    assert!(world.env().contains("LOAMS_VERSION=0.1.0"));
}

#[test]
fn env_changes_keep_user_values() {
    let world = World::new(&format!(
        "{ENV}# a comment\nLOAMS_UPGRADE_SNAPSHOT_DIR_X=1\n"
    ));
    let fake = scripted(Script::new(), false);
    upgrade(&world, &fake, &opts()).expect("upgrade");
    let env = world.env();
    assert!(env.contains("LOAMS_VERSION=0.1.0\n"), "{env}");
    assert!(env.contains("AWS_SECRET_ACCESS_KEY=s3cr3t-aws-key-value\n"));
    assert!(env.contains("LOAMS_AUTH_MODE=tailscale\n"));
    assert!(env.contains("# a comment\n"));
    assert!(env.contains("\nLOAMS_UPGRADE_SNAPSHOT_DIR=\n"), "{env}");
    let backup = world.stack_dir().join(".env.bak-0.1.0-preview.2");
    assert_eq!(
        std::fs::read_to_string(&backup).expect("backup"),
        format!("{ENV}# a comment\nLOAMS_UPGRADE_SNAPSHOT_DIR_X=1\n")
    );
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&backup)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // A value the user already set is kept.
    let release = world
        .bundle()
        .path
        .release("0.1.0")
        .cloned()
        .expect("p3");
    let text = loams_upgrade::install::env_with_changes(
        "LOAMS_UPGRADE_SNAPSHOT_DIR=/srv/snaps\nLOAMS_VERSION=0.1.0-preview.2\n",
        "0.1.0",
        &release,
    );
    assert_eq!(
        text,
        "LOAMS_UPGRADE_SNAPSHOT_DIR=/srv/snaps\nLOAMS_VERSION=0.1.0\n"
    );
}

#[test]
fn component_downgrade_refused() {
    let world = World::new(&format!("{ENV}TIKV_TAG=v8.5.9\n"));
    let fake = scripted(Script::new(), false);
    let err = upgrade(&world, &fake, &opts()).unwrap_err();
    assert_eq!(err.code, "component_downgrade");
    assert_eq!(err.exit_code(), 5);
    let k = kinds(&fake);
    assert!(!k.iter().any(|k| k.starts_with("stop")), "{k:?}");
    assert!(History::of(&world.stack_dir()).entries().is_empty());
}

/// A train of three releases: p2 -> p3 -> p4.
fn chain_bundle(world: &World, version: &str) -> Bundle {
    let dir = world.root.path().join(format!("bundle-{version}/release"));
    write(
        &dir.join("compose.yaml"),
        &format!("name: loams\n# {version}\nservices: {{}}\n"),
    );
    write(
        &dir.parent().expect("p").join("upgrade-path.toml"),
        r#"
[[release]]
version = "0.1.0-preview.4"
from = "0.1.0"
[[release]]
version = "0.1.0"
from = "0.1.0-preview.2"
[[release]]
version = "0.1.0-preview.2"
from = "fresh"
"#,
    );
    Bundle::local(&dir).expect("bundle")
}

#[test]
fn chain_walk_stops_at_first_failure() {
    let world = World::new(ENV);
    // The second step's migrate fails (the first's succeeds).
    let runs = Arc::new(Mutex::new(0));
    let counter = runs.clone();
    let mut script = Script::new();
    script.insert("unused".to_string(), ok(""));
    let base = scripted(script, false);
    let fake = Fake::new(move |args| {
        if kind(args) == "migrate run" {
            let mut n = counter.lock().expect("n");
            *n += 1;
            if *n == 2 {
                return failed(65, "migration_failed");
            }
        }
        loams_upgrade::runtime::Runtime::engine(&base, args).expect("base")
    });
    let mut stack = world.stack();
    let steps = vec!["0.1.0".to_string(), "0.1.0-preview.4".to_string()];
    let mut asked = Vec::new();
    let err = upgrade_chain(&fake, &mut stack, "0.1.0-preview.2", &steps, &opts(), |v| {
        asked.push(v.clone());
        Ok(chain_bundle(&world, v))
    })
    .unwrap_err();
    assert_eq!(err.code, "upgrade_failed_rolled_back");
    assert_eq!(
        asked, steps,
        "both steps were attempted, none after the failure"
    );
    let entries = History::of(&world.stack_dir()).entries();
    let ends: Vec<(String, Event)> = entries
        .iter()
        .filter(|e| matches!(e.event, Event::Done | Event::RolledBack))
        .map(|e| (e.to.clone(), e.event))
        .collect();
    assert_eq!(
        ends,
        [
            ("0.1.0".to_string(), Event::Done),
            ("0.1.0-preview.4".to_string(), Event::RolledBack)
        ]
    );
    // The first step's snapshot was kept (the chain had not verified).
    let k = kinds(&fake);
    assert!(
        !k.contains(&"migrate snapshots prune".to_string()),
        "{k:#?}"
    );
    // The stack is at 0.1.0: the verified step.
    assert!(world.env().contains("LOAMS_VERSION=0.1.0"));
}

#[test]
fn kill_cli_mid_upgrade_then_rerun_resumes() {
    let world = World::new(ENV);
    // The CLI dies while migrate runs (after the snapshot and the install).
    let fake_dying = Fake::new({
        let base = scripted(Script::new(), false);
        move |args| {
            assert!(kind(args) != "migrate run", "killed");
            loams_upgrade::runtime::Runtime::engine(&base, args).expect("base")
        }
    });
    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = upgrade(&world, &fake_dying, &opts());
    }));
    assert!(died.is_err());
    let open = History::of(&world.stack_dir())
        .open_attempt()
        .expect("open");
    let snapshot = loams_upgrade::history::snapshot_of(&open)
        .expect("snapshot")
        .0;
    // Another target is refused while it is open.
    let other = scripted(Script::new(), false);
    let mut stack = world.stack();
    let err = run::upgrade_step(
        &other,
        &mut stack,
        &world.bundle(),
        "0.1.0",
        "0.1.0-preview.4",
        &opts(),
    )
    .unwrap_err();
    assert_eq!(err.code, "upgrade_in_progress");
    assert_eq!(err.exit_code(), 5);
    // The rerun resumes: no second snapshot, migrate runs, the door opens.
    let fake = scripted(Script::new(), false);
    let mut stack = world.stack();
    // .env already says 0.1.0; the attempt says where it came from.
    let report = run::upgrade_step(
        &fake,
        &mut stack,
        &world.bundle(),
        "0.1.0",
        "0.1.0",
        &opts(),
    )
    .expect("resumed");
    assert!(report.resumed);
    assert_eq!(report.from, "0.1.0-preview.2");
    assert_eq!(report.snapshot, snapshot);
    let skipped: Vec<&str> = report
        .steps
        .iter()
        .filter(|s| s.skipped)
        .map(|s| s.step.as_str())
        .collect();
    assert_eq!(skipped, ["snapshot", "install", "third_party"]);
    let k = kinds(&fake);
    assert!(!k.contains(&"migrate snapshot".to_string()));
    assert!(
        !k.contains(&"migrate plan".to_string()),
        "no preflight on a resume"
    );
    in_order(
        &k,
        &[
            "migrate run",
            "migrate verify",
            "up tailscale house-tailscale",
        ],
    );
    assert_eq!(last_event(&world), Event::Done);
}

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    upgrade: UpgradeArgs,
}

fn cli(args: &[&str]) -> UpgradeArgs {
    let mut all = vec!["upgrade"];
    all.extend_from_slice(args);
    Cli::try_parse_from(all).expect("parse").upgrade
}

#[test]
fn upgrade_output_has_no_env_values() {
    let world = World::new(ENV);
    let dir = world.stack_dir().display().to_string();
    let bundle = world
        .root
        .path()
        .join("bundle/release")
        .display()
        .to_string();
    let args = cli(&[
        "--dir",
        &dir,
        "--to",
        "0.1.0",
        "--bundle",
        &bundle,
        "--yes",
    ]);
    let fake = scripted(Script::new(), false);
    let value = loams_upgrade::run_redacted(&args, &fake).expect("upgrade");
    assert!(!value.to_string().contains(SECRET), "{value}");
    // A failure whose tool output echoes the secret.
    let world = World::new(ENV);
    let dir = world.stack_dir().display().to_string();
    let bundle = world
        .root
        .path()
        .join("bundle/release")
        .display()
        .to_string();
    let args = cli(&[
        "--dir",
        &dir,
        "--to",
        "0.1.0",
        "--bundle",
        &bundle,
        "--yes",
    ]);
    let mut script = Script::new();
    script.insert(
        "migrate run".to_string(),
        failed(65, &format!("could not sign in with key {SECRET}")),
    );
    let fake = scripted(script, false);
    let err = loams_upgrade::run_redacted(&args, &fake).unwrap_err();
    assert!(!err.message.contains(SECRET), "{err}");
    assert!(err.message.contains("<redacted>"), "{err}");
    // Neither the history nor a helper's command line holds it.
    let history = std::fs::read_to_string(world.stack_dir().join(".loams-upgrade/history.jsonl"))
        .expect("history");
    assert!(!history.contains(SECRET));
    for call in fake.calls() {
        assert!(!call.iter().any(|a| a.contains(SECRET)), "{call:?}");
    }
}

#[test]
fn local_bundle_is_reported_unverified() {
    let world = World::new(ENV);
    let dir = world.stack_dir().display().to_string();
    let bundle = world
        .root
        .path()
        .join("bundle/release")
        .display()
        .to_string();
    let args = cli(&[
        "--dir",
        &dir,
        "--to",
        "0.1.0",
        "--bundle",
        &bundle,
        "--yes",
    ]);
    let fake = scripted(Script::new(), false);
    let value = loams_upgrade::run_redacted(&args, &fake).expect("upgrade");
    assert_eq!(value["upgraded"][0]["bundle"], "local, unverified");
    // Its images are already present: nothing is pulled.
    let k = kinds(&fake);
    assert!(!k.iter().any(|k| k.starts_with("pull")), "{k:?}");
    // Without --yes and without a terminal: confirmation_required.
    let world = World::new(ENV);
    let dir = world.stack_dir().display().to_string();
    let args = cli(&[
        "--dir",
        &dir,
        "--to",
        "0.1.0",
        "--bundle",
        &bundle,
    ]);
    let fake = scripted(Script::new(), false);
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let err = loams_upgrade::run_redacted(&args, &fake).unwrap_err();
        assert_eq!(err.code, "confirmation_required");
        assert_eq!(err.exit_code(), 3);
    }
}
