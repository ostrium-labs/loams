//! Helper containers (design §54 §10.1's boundary): every step that touches
//! data runs in one, never in this process. Only two images are used: the
//! target `loams` image (`loams migrate …`, and the cold volume copy) and
//! BR, both pinned. Each helper runs read-only with every capability
//! dropped and `no-new-privileges`, publishes no port, gets no container
//! socket, joins only the stack's default network, and mounts only the data
//! volumes of its step and the snapshots; its only credentials are the
//! engine's own (`secrets/engine.env`), passed as an env file.

use crate::stackdir::StackDir;

/// Where the snapshots are mounted in every helper (and in `tikv`, for BR).
pub const SNAPSHOTS: &str = "/var/lib/loams-upgrade-snapshots";

/// One mount of a helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

/// A helper container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperSpec {
    pub image: String,
    pub entrypoint: Option<String>,
    pub args: Vec<String>,
    pub network: String,
    pub mounts: Vec<Mount>,
    pub env_file: Option<String>,
    /// `--user` (the image's own user when `None`).
    pub user: Option<String>,
    /// Capabilities added back after `--cap-drop ALL`: only the cold copy,
    /// which must read and write TiKV's root-owned files.
    pub cap_add: Vec<String>,
}

impl HelperSpec {
    /// `run` arguments for the container engine.
    pub fn render(&self) -> Vec<String> {
        let mut out: Vec<String> = [
            "run",
            "--rm",
            "--read-only",
            "--tmpfs",
            "/tmp",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--network",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        out.push(self.network.clone());
        for cap in &self.cap_add {
            out.push("--cap-add".to_string());
            out.push(cap.clone());
        }
        if let Some(user) = &self.user {
            out.push("--user".to_string());
            out.push(user.clone());
        }
        for m in &self.mounts {
            out.push("-v".to_string());
            out.push(if m.read_only {
                format!("{}:{}:ro", m.source, m.target)
            } else {
                format!("{}:{}", m.source, m.target)
            });
        }
        if let Some(env) = &self.env_file {
            out.push("--env-file".to_string());
            out.push(env.clone());
        }
        if let Some(entrypoint) = &self.entrypoint {
            out.push("--entrypoint".to_string());
            out.push(entrypoint.clone());
        }
        out.push(self.image.clone());
        out.extend(self.args.iter().cloned());
        out
    }
}

/// The compose volume (or network) `name` of the stack.
pub fn volume(stack: &StackDir, name: &str) -> String {
    format!("{}_{name}", stack.project)
}

/// The snapshots' mount source: the host directory `.env` names, else the
/// `upgrade-snapshots` volume.
pub fn snapshots_source(stack: &StackDir) -> String {
    stack.snapshot_host_dir().map_or_else(
        || volume(stack, "upgrade-snapshots"),
        |d| d.display().to_string(),
    )
}

fn engine_env(stack: &StackDir) -> Option<String> {
    let path = stack.dir.join("secrets").join("engine.env");
    path.exists().then(|| path.display().to_string())
}

/// The stores' flags of `loams migrate`, as the compose `migrate` service
/// has them.
pub fn migrate_store_args(stack: &StackDir, postgres: bool) -> Vec<String> {
    let bucket = stack
        .env
        .get("LOAMS_BUCKET")
        .filter(|b| !b.is_empty())
        .cloned()
        .unwrap_or_else(|| "file:///var/lib/loams/bucket".to_string());
    let mut args = vec![
        "--meta=tikv://pd:2379/loams_meta".to_string(),
        "--live-store=tikv://pd:2379/loams_live_dev".to_string(),
        format!("--bucket={bucket}"),
        "--data-dir=/var/lib/loams".to_string(),
        "--house-dir=/var/lib/loams-house".to_string(),
        "--pg-control-dir=/var/lib/loams-pg-control".to_string(),
    ];
    if postgres {
        args.push("--pageserver=http://pageserver:9898".to_string());
        args.push("--safekeeper=http://safekeeper1:7676".to_string());
    }
    args
}

/// `loams migrate <subcommand…>` in a container of `image` over the stack's
/// Loams-owned volumes.
pub fn migrate_helper(stack: &StackDir, image: &str, subcommand: &[String]) -> HelperSpec {
    let mut args = vec!["migrate".to_string()];
    args.extend(subcommand.iter().cloned());
    HelperSpec {
        image: image.to_string(),
        entrypoint: None,
        args,
        network: volume(stack, "default"),
        mounts: vec![
            Mount {
                source: volume(stack, "loams-data"),
                target: "/var/lib/loams".to_string(),
                read_only: false,
            },
            Mount {
                source: volume(stack, "house-data"),
                target: "/var/lib/loams-house".to_string(),
                read_only: false,
            },
            Mount {
                source: volume(stack, "pg-control-data"),
                target: "/var/lib/loams-pg-control".to_string(),
                read_only: false,
            },
            Mount {
                source: snapshots_source(stack),
                target: SNAPSHOTS.to_string(),
                read_only: false,
            },
        ],
        env_file: engine_env(stack),
        user: None,
        cap_add: Vec::new(),
    }
}

/// BR (`br backup txn` or `br restore txn`) against the stack's PD, into or
/// from `<snapshots>/<id>/tikv`.
pub fn br_helper(stack: &StackDir, image: &str, restore: bool, id: &str) -> HelperSpec {
    HelperSpec {
        image: image.to_string(),
        entrypoint: Some("/br".to_string()),
        args: vec![
            if restore { "restore" } else { "backup" }.to_string(),
            "txn".to_string(),
            "--pd".to_string(),
            "pd:2379".to_string(),
            "--storage".to_string(),
            format!("local://{SNAPSHOTS}/{id}/tikv"),
            "--log-file".to_string(),
            "/tmp/br.log".to_string(),
        ],
        network: volume(stack, "default"),
        mounts: vec![Mount {
            source: snapshots_source(stack),
            target: SNAPSHOTS.to_string(),
            read_only: false,
        }],
        env_file: None,
        user: None,
        cap_add: Vec::new(),
    }
}

/// The uid and gid the `loams` image runs as.
pub const LOAMS_UID: &str = "10001";

/// A cold copy of `pd-data` and `tikv-data` (both stopped) as one archive,
/// `<snapshots>/<id>/tikv/volumes.tar`, that keeps their owners and modes;
/// or, for a restore, both volumes emptied and the archive unpacked into
/// them. Runs the target `loams` image's `tar` as root with only the
/// capabilities a copy of another user's files needs, on no network. A
/// stack whose `tikv` does not mount the snapshots (preview.2) takes this
/// path; BR needs that mount (UP1 Task 13 ruling).
pub fn cold_copy_helper(stack: &StackDir, image: &str, restore: bool, id: &str) -> HelperSpec {
    let dir = format!("{SNAPSHOTS}/{id}/tikv");
    let script = if restore {
        format!(
            "set -e; test -f {dir}/volumes.tar; \
             for v in pd tikv; do find /vol/$v -mindepth 1 -delete; done; \
             tar -C /vol -xpf {dir}/volumes.tar"
        )
    } else {
        format!(
            "set -e; mkdir -p {dir}; tar -C /vol -cpf {dir}/volumes.tar pd tikv; \
             chown -R {LOAMS_UID}:{LOAMS_UID} {dir}; chmod 0700 {dir}; chmod 0600 {dir}/volumes.tar"
        )
    };
    HelperSpec {
        image: image.to_string(),
        entrypoint: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), script],
        network: "none".to_string(),
        mounts: vec![
            Mount {
                source: volume(stack, "pd-data"),
                target: "/vol/pd".to_string(),
                read_only: !restore,
            },
            Mount {
                source: volume(stack, "tikv-data"),
                target: "/vol/tikv".to_string(),
                read_only: !restore,
            },
            Mount {
                source: snapshots_source(stack),
                target: SNAPSHOTS.to_string(),
                read_only: restore,
            },
        ],
        env_file: None,
        user: Some("0".to_string()),
        cap_add: ["CHOWN", "DAC_OVERRIDE", "FOWNER"]
            .iter()
            .map(|c| (*c).to_string())
            .collect(),
    }
}

/// Makes `<snapshots>/<id>/tikv` (0700, the `loams` user's) before BR
/// writes into it, so `loams migrate snapshot` can still write the
/// snapshot's other files and its manifest beside it.
pub fn prepare_helper(stack: &StackDir, image: &str, id: &str) -> HelperSpec {
    HelperSpec {
        image: image.to_string(),
        entrypoint: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            format!(
                "set -e; mkdir -p {SNAPSHOTS}/{id}/tikv; chown {LOAMS_UID}:{LOAMS_UID} {SNAPSHOTS} \
                 {SNAPSHOTS}/{id} {SNAPSHOTS}/{id}/tikv; chmod 0700 {SNAPSHOTS} {SNAPSHOTS}/{id}"
            ),
        ],
        network: "none".to_string(),
        mounts: vec![Mount {
            source: snapshots_source(stack),
            target: SNAPSHOTS.to_string(),
            read_only: false,
        }],
        env_file: None,
        user: Some("0".to_string()),
        cap_add: ["CHOWN", "DAC_OVERRIDE", "FOWNER"]
            .iter()
            .map(|c| (*c).to_string())
            .collect(),
    }
}

/// Whether `tikv` mounts the snapshots at [`SNAPSHOTS`] in the stack's
/// resolved compose file (`compose config --format json`; 0.1.0 and
/// later): BR can then write its backup where `migrate` reads it.
pub fn tikv_mounts_snapshots(config: &serde_json::Value) -> bool {
    config["services"]["tikv"]["volumes"]
        .as_array()
        .is_some_and(|v| v.iter().any(|m| m["target"] == SNAPSHOTS))
}
