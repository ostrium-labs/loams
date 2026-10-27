//! `loams upgrade check` (design §54 §10.2): read-only. It says where the
//! stack is, where it would go and how, what each migration costs and
//! whether it can be undone, and what would make `upgrade` refuse.

use schemars::JsonSchema;
use serde::Serialize;

use crate::errors::{Result, UpgradeError};
use crate::helpers::migrate_helper;
use crate::path::{UpgradePath, compare_pins};
use crate::runtime::{Runtime, args};
use crate::stackdir::StackDir;

/// One migration of the step.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct CheckMigration {
    pub id: String,
    pub store: String,
    /// `reversible` or `point_of_no_return`.
    pub class: String,
    pub estimate: CheckEstimate,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, JsonSchema)]
pub struct CheckEstimate {
    pub seconds: u64,
}

/// A third-party component's pin, before and after.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct CheckComponent {
    pub name: String,
    pub from: Option<String>,
    pub to: String,
}

/// Something that makes `upgrade` refuse.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Blocker {
    pub code: String,
    pub message: String,
}

/// `check`'s JSON.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct CheckReport {
    pub current: String,
    pub target: String,
    /// The releases the upgrade walks, one step each.
    pub path: Vec<String>,
    pub migrations: Vec<CheckMigration>,
    pub components: Vec<CheckComponent>,
    pub snapshot_estimate_bytes: u64,
    /// Free bytes where the snapshot goes; `None` when it cannot be told.
    pub free_bytes: Option<u64>,
    pub downtime_estimate_seconds: u64,
    pub point_of_no_return: bool,
    pub blockers: Vec<Blocker>,
}

/// What `check` reads, gathered first so the decision is a pure function.
#[derive(Clone, Debug, Default)]
pub struct Observed {
    /// The current release's pins (from `.env` and the compose file).
    pub current_components: std::collections::BTreeMap<String, String>,
    /// `loams migrate plan`'s JSON from the target image, if it ran.
    pub plan: Option<serde_json::Value>,
    /// Services that are not running or not healthy.
    pub unhealthy: Vec<String>,
    pub snapshot_estimate_bytes: u64,
    pub free_bytes: Option<u64>,
}

/// The pins a stack's `.env` names.
pub fn current_components(stack: &StackDir) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    if !stack.tikv() {
        // The embedded stores: no PD, TiKV or TiDB runs.
        return out;
    }
    let tikv = stack
        .env
        .get("TIKV_TAG")
        .cloned()
        .unwrap_or_else(|| "v8.5.8".to_string());
    out.insert("pd".to_string(), tikv.clone());
    out.insert("tikv".to_string(), tikv);
    if let Some(tag) = stack
        .env
        .get("TIDB_IMAGE")
        .and_then(|image| image.rsplit_once(':').map(|(_, t)| t.to_string()))
    {
        out.insert("tidb".to_string(), tag);
    }
    out
}

/// The decision of `check` from what it observed.
pub fn report(
    current: &str,
    target: &str,
    path: &UpgradePath,
    observed: &Observed,
) -> Result<CheckReport> {
    let release = path.release(target).ok_or_else(|| {
        UpgradeError::new(
            "release_not_found",
            format!("{target} is not in the release train"),
        )
    })?;
    let mut blockers = Vec::new();
    let steps = match path.steps(current, target) {
        Ok(steps) => steps,
        Err(e) => {
            blockers.push(Blocker {
                code: e.code.to_string(),
                message: e.message,
            });
            vec![target.to_string()]
        }
    };
    let estimate_of = |id: &str| -> u64 {
        observed
            .plan
            .as_ref()
            .and_then(|p| p["steps"].as_array())
            .and_then(|steps| steps.iter().find(|s| s["id"] == id))
            .and_then(|s| s["estimate"]["seconds"].as_u64())
            .unwrap_or(0)
    };
    let mut migrations = Vec::new();
    for version in &steps {
        if let Some(r) = path.release(version) {
            for m in &r.migrations {
                migrations.push(CheckMigration {
                    id: m.id.clone(),
                    store: m.store.clone(),
                    class: m.class.clone(),
                    estimate: CheckEstimate {
                        seconds: estimate_of(&m.id),
                    },
                });
            }
        }
    }
    if let Some(failed) = observed
        .plan
        .as_ref()
        .and_then(|p| p["steps"].as_array())
        .and_then(|s| {
            s.iter()
                .find(|s| s["state"] == "failed" || s["state"] == "running")
        })
    {
        blockers.push(Blocker {
            code: "migration_incomplete".to_string(),
            message: format!(
                "migration {} is {}; run the upgrade again to resume it, or roll back",
                failed["id"].as_str().unwrap_or("?"),
                failed["state"].as_str().unwrap_or("?")
            ),
        });
    }
    let mut components = Vec::new();
    for (name, to) in &release.components {
        let from = observed.current_components.get(name).cloned();
        if let Some(from) = &from
            && compare_pins(from, to) == Some(std::cmp::Ordering::Less)
        {
            blockers.push(Blocker {
                code: "component_downgrade".to_string(),
                message: format!("{name} would go from {from} down to {to}"),
            });
        }
        components.push(CheckComponent {
            name: name.clone(),
            from,
            to: to.clone(),
        });
    }
    let factor = release.free_factor.unwrap_or(1.5);
    let needed = (observed.snapshot_estimate_bytes as f64 * factor) as u64;
    if let Some(free) = observed.free_bytes
        && free < needed
    {
        blockers.push(Blocker {
            code: "insufficient_space".to_string(),
            message: format!("{free} bytes free; the snapshot needs about {needed} (×{factor})"),
        });
    }
    if !observed.unhealthy.is_empty() {
        blockers.push(Blocker {
            code: "stack_unhealthy".to_string(),
            message: format!("not healthy: {}", observed.unhealthy.join(", ")),
        });
    }
    let downtime = steps
        .iter()
        .filter_map(|v| path.release(v))
        .map(|r| r.downtime_seconds.unwrap_or(300))
        .sum();
    Ok(CheckReport {
        current: current.to_string(),
        target: target.to_string(),
        path: steps,
        point_of_no_return: migrations.iter().any(|m| m.class == "point_of_no_return"),
        migrations,
        components,
        snapshot_estimate_bytes: observed.snapshot_estimate_bytes,
        free_bytes: observed.free_bytes,
        downtime_estimate_seconds: downtime,
        blockers,
    })
}

/// Services of the stack that are not running, or not healthy (one-shot
/// services that exited 0 are fine).
pub fn unhealthy_services(runtime: &dyn Runtime, stack: &StackDir) -> Result<Vec<String>> {
    Ok(crate::compose::unhealthy(&crate::compose::ps(
        runtime, &stack.dir,
    )?))
}

/// `loams migrate plan` in a container of `image` (the target's), so a
/// stack whose engine is down, or older than the `migrate` command, is still
/// planned.
pub fn plan(
    runtime: &dyn Runtime,
    stack: &StackDir,
    image: &str,
    current: &str,
    target: &str,
) -> Result<serde_json::Value> {
    let mut sub = vec!["plan".to_string()];
    sub.extend(crate::helpers::migrate_store_args(stack, false));
    sub.push(format!("--target-release={target}"));
    sub.push(format!("--declared-release={current}"));
    sub.push("--output=json".to_string());
    let spec = migrate_helper(stack, image, &sub);
    let out = runtime.engine(&spec.render())?;
    if !out.ok() {
        return Err(UpgradeError::new(
            "migration_incomplete",
            format!("loams migrate plan failed: {}", out.stderr.trim()),
        ));
    }
    crate::runtime::helper_json(&out.stdout)
        .map_err(|e| UpgradeError::new("internal", format!("loams migrate plan's output: {e}")))
}

/// The bytes a snapshot would copy: the Loams-owned volumes and TiKV's (when
/// the stack runs it: mounting a volume that does not exist would create
/// it), as `du` sees them in a helper of `image` (read-only mounts).
pub fn snapshot_estimate(runtime: &dyn Runtime, stack: &StackDir, image: &str) -> Result<u64> {
    use crate::helpers::{HelperSpec, Mount, volume};
    let mut names = vec!["loams-data", "house-data", "pg-control-data"];
    if stack.tikv() {
        names.extend(["pd-data", "tikv-data"]);
    }
    let mounts = names
        .iter()
        .map(|v| Mount {
            source: volume(stack, v),
            target: format!("/vol/{v}"),
            read_only: true,
        })
        .collect();
    let spec = HelperSpec {
        image: image.to_string(),
        entrypoint: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "du -sb /vol/* | awk '{s+=$1} END {print s+0}'".to_string(),
        ],
        network: "none".to_string(),
        mounts,
        env_file: None,
        user: Some("0".to_string()),
        cap_add: vec!["DAC_READ_SEARCH".to_string()],
    };
    let out = runtime.engine(&spec.render())?;
    Ok(out.stdout.trim().parse().unwrap_or(0))
}

/// Free bytes where the snapshots go: the host directory `.env` names, or
/// the container runtime's storage.
pub fn free_bytes(runtime: &dyn Runtime, stack: &StackDir) -> Option<u64> {
    let path = match stack.snapshot_host_dir() {
        Some(dir) => dir.display().to_string(),
        None => {
            let info = runtime
                .engine(&args([
                    "info",
                    "--format",
                    "{{.DockerRootDir}}{{.Store.GraphRoot}}",
                ]))
                .ok()?;
            let root = info.stdout.trim().replace("<no value>", "");
            if root.is_empty() {
                return None;
            }
            root
        }
    };
    let out = std::process::Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(&path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)?
        .trim()
        .parse()
        .ok()
}

/// `check`: gathers what it needs through `runtime` and decides.
pub fn check(
    runtime: &dyn Runtime,
    stack: &StackDir,
    path: &UpgradePath,
    target: &str,
) -> Result<CheckReport> {
    if !stack.tikv() {
        // `loams migrate plan`, `verify` and `status` open the embedded
        // metastore, which the running engine holds locked (one writer).
        return Err(UpgradeError::new(
            "stack_unrecognised",
            "this stack runs on the embedded stores (COMPOSE_PROFILES without `tikv`), which \
             `loams upgrade` does not upgrade yet: back up the volumes, stop the stack, set \
             LOAMS_VERSION and start it again (`migrate` runs first; see docs/release/upgrades.md)",
        ));
    }
    let current = stack.release().ok_or_else(|| {
        UpgradeError::new(
            "stack_unrecognised",
            "the stack's .env names no LOAMS_VERSION",
        )
    })?;
    let image = crate::bundle::loams_image(&stack.env, target);
    let observed = Observed {
        current_components: current_components(stack),
        plan: plan(runtime, stack, &image, &current, target).ok(),
        unhealthy: unhealthy_services(runtime, stack)?,
        snapshot_estimate_bytes: snapshot_estimate(runtime, stack, &image).unwrap_or(0),
        free_bytes: free_bytes(runtime, stack),
    };
    report(&current, target, path, &observed)
}
