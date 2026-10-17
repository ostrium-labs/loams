//! A stack directory and a target bundle on disk, and a scripted runtime
//! that answers like compose and the helpers (UP1 Tasks 13 and 14).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use loams_upgrade::bundle::Bundle;
use loams_upgrade::history::{Event, History};
use loams_upgrade::run::{self, Options};
use loams_upgrade::runtime::Output;
use loams_upgrade::stackdir::StackDir;
use serde_json::json;
use tempfile::TempDir;

use super::{Fake, ok};

pub const SECRET: &str = "s3cr3t-aws-key-value";

/// A stack directory (`<root>/stack/release`) at preview.2 and the target's
/// bundle (`<root>/bundle/release`) with the real release train.
pub struct World {
    pub root: TempDir,
}

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

impl World {
    pub fn new(env: &str) -> Self {
        let root = TempDir::new().expect("root");
        let stack = root.path().join("stack/release");
        write(
            &stack.join("compose.yaml"),
            "name: loams\n# preview.2\nservices: {}\n",
        );
        write(&stack.join(".env"), env);
        write(
            &stack.join("secrets/engine.env"),
            &format!("AWS_SECRET_ACCESS_KEY={SECRET}\n"),
        );
        write(&stack.join("compose.override.yaml"), "# the user's\n");
        write(
            &root.path().join("stack/loams-postgres-dev/compose.yaml"),
            "# preview.2 postgres\n",
        );
        let bundle = root.path().join("bundle/release");
        write(
            &bundle.join("compose.yaml"),
            "name: loams\n# 0.1.0\nservices: {}\n",
        );
        write(
            &bundle.join(".env.example"),
            "LOAMS_VERSION=0.1.0\n",
        );
        write(
            &root.path().join("bundle/loams-postgres-dev/compose.yaml"),
            "# 0.1.0 postgres\n",
        );
        let train = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/upgrade-path.toml"),
        )
        .expect("train");
        write(&root.path().join("bundle/upgrade-path.toml"), &train);
        World { root }
    }

    pub fn stack_dir(&self) -> PathBuf {
        self.root.path().join("stack/release")
    }

    pub fn stack(&self) -> StackDir {
        StackDir::discover(Some(&self.stack_dir()), None, self.root.path()).expect("stack")
    }

    pub fn bundle(&self) -> Bundle {
        Bundle::local(&self.root.path().join("bundle/release")).expect("bundle")
    }

    pub fn env(&self) -> String {
        std::fs::read_to_string(self.stack_dir().join(".env")).expect(".env")
    }
}

pub const ENV: &str = "LOAMS_VERSION=0.1.0-preview.2\nCOMPOSE_PROFILES=house\nAWS_SECRET_ACCESS_KEY=s3cr3t-aws-key-value\nLOAMS_AUTH_MODE=tailscale\n";

pub fn opts() -> Options {
    Options {
        no_auto_rollback: false,
        keep_snapshot: false,
        br_image: "docker.io/pingcap/br@sha256:cddec1ce".to_string(),
    }
}

/// A short name for a call.
pub fn kind(args: &[String]) -> String {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.first() {
        Some(&"compose") => {
            let sub = a.get(1).copied().unwrap_or_default();
            match sub {
                "up" => format!(
                    "up {}",
                    a[2..]
                        .iter()
                        .filter(|x| !x.starts_with('-'))
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                "stop" => format!("stop {}", a[2..].join(" ")),
                "exec" => "engine ready".to_string(),
                _ if a.contains(&"pull") => "pull images".to_string(),
                other => other.to_string(),
            }
        }
        Some(&"run") => {
            let joined = a.join(" ");
            if let Some(i) = a.iter().position(|x| *x == "migrate") {
                let sub = a.get(i + 1).copied().unwrap_or_default();
                if sub == "snapshots" {
                    return format!(
                        "migrate snapshots {}",
                        a.get(i + 2).copied().unwrap_or_default()
                    );
                }
                return format!("migrate {sub}");
            }
            if joined.contains("/br") {
                return if a.contains(&"restore") {
                    "br restore"
                } else {
                    "br backup"
                }
                .to_string();
            }
            if joined.contains("tar -C /vol -cpf") {
                return "cold copy".to_string();
            }
            if joined.contains("tar -C /vol -xpf") {
                return "cold restore".to_string();
            }
            if joined.contains("du -sb") {
                return "du".to_string();
            }
            if joined.contains("mkdir -p") {
                return "prepare".to_string();
            }
            "run ?".to_string()
        }
        Some(&"pull") => "pull br".to_string(),
        Some(&"volume") => "volume rm".to_string(),
        Some(other) => (*other).to_string(),
        None => String::new(),
    }
}

/// What the fake answers, by call kind; anything not listed is `ok("")`.
pub type Script = BTreeMap<String, Output>;

pub fn config(tikv_mount: bool) -> String {
    let mut services = serde_json::Map::new();
    for (name, restart) in [
        ("pd", "unless-stopped"),
        ("tikv", "unless-stopped"),
        ("tidb", "unless-stopped"),
        ("migrate", "no"),
        ("loams", "unless-stopped"),
        ("house", "unless-stopped"),
        ("house-tailscale", "unless-stopped"),
        ("tailscale", "unless-stopped"),
        ("rustfs", ""),
        ("create-bucket", "no"),
        ("storage_broker", ""),
        ("pageserver", ""),
        ("safekeeper1", ""),
        ("compute1", ""),
    ] {
        let mut svc = serde_json::Map::new();
        if !restart.is_empty() {
            svc.insert("restart".into(), json!(restart));
        }
        if name == "tikv" && tikv_mount {
            svc.insert(
                "volumes".into(),
                json!([{"source": "upgrade-snapshots", "target": "/var/lib/loams-upgrade-snapshots"}]),
            );
        }
        services.insert(name.to_string(), serde_json::Value::Object(svc));
    }
    json!({ "name": "loams", "services": services }).to_string()
}

pub const PS: &str = concat!(
    r#"{"Service":"pd","State":"running","Health":"healthy","ExitCode":0}"#,
    "\n",
    r#"{"Service":"tikv","State":"running","Health":"healthy","ExitCode":0}"#,
    "\n",
    r#"{"Service":"loams","State":"running","Health":"healthy","ExitCode":0}"#,
    "\n",
    r#"{"Service":"pageserver","State":"running","Health":"","ExitCode":0}"#,
    "\n",
    r#"{"Service":"safekeeper1","State":"running","Health":"","ExitCode":0}"#,
    "\n",
    r#"{"Service":"migrate","State":"exited","Health":"","ExitCode":0}"#,
);

/// The snapshot ids `migrate snapshot` was given, for the listing.
pub fn snapshot_ids(calls: &[Vec<String>]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| kind(c) == "migrate snapshot")
        .filter_map(|c| {
            c.iter()
                .find_map(|a| a.strip_prefix("--id=").map(str::to_string))
        })
        .collect()
}

pub fn scripted(script: Script, tikv_mount: bool) -> Fake {
    let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let seen = calls.clone();
    let plan = json!({"from": null, "to": "0.1.0", "point_of_no_return": false,
        "steps": [{"id": "0000-adopt-preview-2", "store": "meta", "class": "reversible",
                   "estimate": {"rows": 0, "bytes": 0, "seconds": 1}, "state": "pending"}]})
    .to_string();
    let config = config(tikv_mount);
    Fake::new(move |args| {
        let k = kind(args);
        seen.lock().expect("seen").push(args.to_vec());
        if let Some(out) = script.get(&k) {
            return out.clone();
        }
        match k.as_str() {
            "config" => ok(&config),
            "ps" => ok(PS),
            "migrate plan" => ok(&plan),
            "du" => ok("1000\n"),
            "migrate snapshots list" => {
                let ids = snapshot_ids(&seen.lock().expect("seen"));
                let listing: Vec<_> = ids
                    .iter()
                    .map(|id| json!({"id": id, "complete": true}))
                    .collect();
                ok(&serde_json::Value::Array(listing).to_string())
            }
            "migrate run" => ok(
                r#"{"applied":["0000-adopt-preview-2"],"host_changes":[{"kind":"recreate","service":"safekeeper1","reason":"r"}]}"#,
            ),
            _ => ok(""),
        }
    })
}

pub fn kinds(fake: &Fake) -> Vec<String> {
    fake.calls().iter().map(|c| kind(c)).collect()
}

/// `needles` appear in `haystack` in this order.
pub fn in_order(haystack: &[String], needles: &[&str]) {
    let mut at = 0;
    for needle in needles {
        match haystack[at..].iter().position(|k| k == needle) {
            Some(i) => at += i + 1,
            None => panic!("{needle:?} not found in order after {at} in {haystack:#?}"),
        }
    }
}

pub fn upgrade(
    world: &World,
    fake: &Fake,
    opts: &Options,
) -> loams_upgrade::errors::Result<run::UpgradeReport> {
    let mut stack = world.stack();
    run::upgrade_step(
        fake,
        &mut stack,
        &world.bundle(),
        "0.1.0-preview.2",
        "0.1.0",
        opts,
    )
}

pub fn last_event(world: &World) -> Event {
    History::of(&world.stack_dir())
        .entries()
        .last()
        .expect("an entry")
        .event
}
