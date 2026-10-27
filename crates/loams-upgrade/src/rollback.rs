//! Restoring a stack from an upgrade snapshot (design §54 §8): stop
//! everything, put back the old bundle, `.env` and `secrets/`, restore TiKV
//! into its volumes at the snapshot's version (the cold copy unpacked, or
//! fresh volumes and `br restore txn`), restore the Loams-owned stores with
//! `loams migrate restore`, start the previous release, verify, open the
//! door. Automatic after a failed upgrade (Q823); `loams upgrade rollback`
//! (UP1 Task 14) runs it on request.

use std::path::Path;

use crate::compose;
use crate::door;
use crate::errors::{Result, UpgradeError};
use crate::helpers::{self, SNAPSHOTS, volume};
use crate::install;
use crate::runtime::Runtime;
use crate::stackdir::{StackDir, read_env};

/// What to restore.
#[derive(Clone, Debug)]
pub struct Restore {
    pub snapshot: String,
    /// How TiKV was copied (`br` or `cold`) and at which version.
    pub tikv: Option<(String, String)>,
    /// The image that runs `loams migrate restore`: the one that took the
    /// snapshot (the upgrade's target), which reads its format.
    pub image: String,
    pub br_image: String,
}

fn run(runtime: &dyn Runtime, spec: &helpers::HelperSpec, what: &str) -> Result<()> {
    let out = runtime.engine(&spec.render())?;
    if out.ok() {
        Ok(())
    } else {
        let last = out.stderr.lines().last().unwrap_or_default().trim();
        Err(UpgradeError::new(
            "rollback_failed",
            format!("{what} (exit {}): {last}", out.status),
        ))
    }
}

/// Whether the snapshot is complete (its manifest written last), before
/// anything is changed.
fn complete(runtime: &dyn Runtime, stack: &StackDir, image: &str, id: &str) -> Result<()> {
    let listing = crate::snapshots::list(runtime, stack, image)?;
    let found = listing
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"] == id);
    match found {
        Some(s) if s["complete"] == true => Ok(()),
        Some(_) => Err(UpgradeError::new(
            "snapshot_incomplete",
            format!("snapshot {id} has no manifest: it cannot be restored"),
        )),
        None => Err(UpgradeError::new(
            "snapshot_not_found",
            format!("no snapshot {id} in the snapshot volume"),
        )),
    }
}

/// Restores `stack` from `restore`. See the module documentation.
pub fn restore(runtime: &dyn Runtime, stack: &mut StackDir, restore: &Restore) -> Result<()> {
    let id = &restore.snapshot;
    complete(runtime, stack, &restore.image, id)?;
    let pd_tikv = vec!["pd".to_string(), "tikv".to_string()];
    // Everything stops (the door was closed by the upgrade, or is now).
    let config = compose::config(runtime, &stack.dir)?;
    door::close(runtime, &stack.dir, &config)?;
    compose::stop(runtime, &stack.dir, &compose::services(&config))?;
    install::restore_host(&stack.dir, id)?;
    stack.env = read_env(&stack.dir.join(".env"));
    let tikv_version = match &restore.tikv {
        Some((method, version)) if method == "cold" => {
            run(
                runtime,
                &helpers::cold_copy_helper(stack, &restore.image, true, id),
                "unpacking TiKV's cold copy",
            )?;
            compose::up(runtime, &stack.dir, &pd_tikv)?;
            Some(version.clone())
        }
        Some((_, version)) => {
            // BR restores into a fresh cluster at the snapshot's version.
            // PD's and TiKV's stopped containers still hold their volumes:
            // they go first, and `up` creates them again.
            compose::remove(runtime, &stack.dir, &pd_tikv)?;
            let out = runtime.engine(&[
                "volume".to_string(),
                "rm".to_string(),
                "-f".to_string(),
                volume(stack, "pd-data"),
                volume(stack, "tikv-data"),
            ])?;
            if !out.ok() {
                return Err(UpgradeError::new(
                    "rollback_failed",
                    format!("removing TiKV's volumes: {}", out.stderr.trim()),
                ));
            }
            compose::up(runtime, &stack.dir, &pd_tikv)?;
            run(
                runtime,
                &helpers::br_helper(stack, &restore.br_image, true, id),
                "BR's restore of TiKV",
            )?;
            Some(version.clone())
        }
        // No TiKV copy: a stack on the embedded stores (its metastore is in
        // the data directory `migrate restore` puts back).
        None => {
            if stack.tikv() {
                compose::up(runtime, &stack.dir, &pd_tikv)?;
            }
            None
        }
    };
    let mut sub = vec!["restore".to_string()];
    sub.extend(helpers::migrate_store_args(stack, false));
    sub.push(format!("--from={SNAPSHOTS}/{id}"));
    if let Some(version) = tikv_version {
        sub.push(format!("--tikv-version={version}"));
    }
    sub.push("--output=json".to_string());
    run(
        runtime,
        &helpers::migrate_helper(stack, &restore.image, &sub),
        "loams migrate restore",
    )?;
    // The previous release, then its health, then the door.
    let config = compose::config(runtime, &stack.dir)?;
    let bad = start_previous(runtime, &stack.dir, &config)?;
    if !bad.is_empty() {
        return Err(UpgradeError::new(
            "rollback_failed",
            format!("not healthy after the restore: {}", bad.join(", ")),
        ));
    }
    crate::run::engine_ready(runtime, &stack.dir)?;
    door::open(runtime, &stack.dir, &config)
}

/// Starts the restored release's long-running services (the door aside) and
/// returns those of them not healthy. Containers of services the restored
/// release does not enable (the newer release added them, or moved them under
/// a profile) are removed first: they are not the previous release's, and a
/// stopped one would read as unhealthy.
fn start_previous(
    runtime: &dyn Runtime,
    dir: &Path,
    config: &serde_json::Value,
) -> Result<Vec<String>> {
    let enabled = compose::services(config);
    let mut stale: Vec<String> = compose::ps(runtime, dir)?
        .into_iter()
        .map(|r| r.service)
        .filter(|s| !s.is_empty() && !enabled.contains(s))
        .collect();
    stale.sort();
    stale.dedup();
    // Best effort: a service the compose file no longer defines at all is
    // an orphan compose cannot name; it is left, and not counted below.
    let _ = compose::remove(runtime, dir, &stale);
    let services: Vec<String> = compose::long_running(config)
        .into_iter()
        .filter(|s| !door::is_gate(config, s))
        .collect();
    compose::up(runtime, dir, &services)?;
    Ok(compose::unhealthy(&compose::ps(runtime, dir)?)
        .into_iter()
        .filter(|s| enabled.contains(s) && !door::is_gate(config, s))
        .collect())
}

/// `rollback_failed` (exit 11): the failure, the rollback's failure, where
/// the snapshot is and the manual steps.
pub fn failed(
    stack_dir: &Path,
    id: &str,
    cause: &UpgradeError,
    rollback: &UpgradeError,
) -> UpgradeError {
    let host = install::host_copy_dir(stack_dir, id);
    UpgradeError::new(
        "rollback_failed",
        format!(
            "{} ({}); the rollback failed too: {}. The snapshot is {id} (`loams upgrade \
             snapshots list`), its host files are in {}. By hand: `docker compose stop`; copy \
             the host files back into {}; restore TiKV (the cold copy's volumes.tar into \
             pd-data and tikv-data, or `br restore txn` into fresh volumes); run `loams migrate \
             restore --from {SNAPSHOTS}/{id}` in the loams image; `docker compose up -d`. See \
             docs/release/upgrades.md.",
            cause.message,
            cause.code,
            rollback.message,
            host.display(),
            stack_dir.display()
        ),
    )
}

/// `loams upgrade rollback`'s options.
#[derive(Clone, Debug, Default)]
pub struct RollbackOptions {
    /// The snapshot to restore (default: the last upgrade's).
    pub snapshot: Option<String>,
    /// Swap the images back and keep the data (Q824).
    pub keep_writes: bool,
    pub br_image: String,
}

/// The final report of a rollback.
#[derive(Clone, Debug, PartialEq, schemars::JsonSchema, serde::Serialize)]
pub struct RollbackReport {
    /// The release the stack was at.
    pub from: String,
    /// The release it is at now.
    pub to: String,
    pub snapshot: String,
    pub keep_writes: bool,
    /// What was lost: the writes since the snapshot (none with
    /// `--keep-writes`).
    pub lost_writes: String,
}

/// `secs` (Unix) as `YYYY-MM-DD HH:MM UTC`.
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// The upgrade a rollback undoes: `snapshot`'s, else the open attempt's (it
/// took a snapshot), else the last one that finished.
fn attempt_of(stack_dir: &Path, snapshot: Option<&str>) -> Result<Vec<crate::history::Entry>> {
    let history = crate::history::History::of(stack_dir);
    let entries = history.entries();
    let attempt = match snapshot {
        Some(id) => entries
            .iter()
            .find(|e| e.snapshot.as_deref() == Some(id))
            .map(|e| e.attempt.clone()),
        None => history
            .open_attempt()
            .filter(|a| crate::history::snapshot_of(a).is_some())
            .map(|a| a[0].attempt.clone())
            .or_else(|| history.last_done().map(|e| e.attempt)),
    };
    let Some(attempt) = attempt else {
        return Err(UpgradeError::new(
            "snapshot_not_found",
            match snapshot {
                Some(id) => format!("no upgrade in this stack's history took snapshot {id}"),
                None => "no upgrade in this stack's history took a snapshot".to_string(),
            },
        ));
    };
    Ok(entries
        .into_iter()
        .filter(|e| e.attempt == attempt)
        .collect())
}

/// Releases whose startup gate cannot serve data a later release stamped:
/// 0.1.0's gate counts its own migrations as pending under a stack record of
/// a release its train does not list (fixed after it, design §54 §17
/// T14-2), so it refuses to start on the writes `--keep-writes` keeps.
/// Earlier releases have no startup gate and later ones have the fix.
pub const KEEP_WRITES_UNSERVED: &[&str] = &["0.1.0"];

/// Why `--keep-writes` cannot be used, if it cannot: the previous release
/// (`from`) cannot start on a later release's data, a point-of-no-return
/// migration was applied for `to` (the ledger, read by `to`'s image), or a
/// third-party pin changed.
fn keep_writes_refusal(
    runtime: &dyn Runtime,
    stack: &StackDir,
    id: &str,
    from: &str,
    to: &str,
) -> Result<Option<String>> {
    if KEEP_WRITES_UNSERVED.contains(&from) {
        return Ok(Some(format!(
            "{from}'s startup gate refuses data that {to} stamped (it predates design §54 §17 \
             T14-2), so {from} would not start on the kept writes"
        )));
    }
    let status = crate::status::status(runtime, stack, to)?;
    let Some(ledger) = status
        .migrate
        .as_ref()
        .and_then(|m| m["ledger"].as_array().cloned())
    else {
        return Ok(Some(format!(
            "the ledger could not be read ({}), so no migration is known to be reversible",
            status.migrate_error.unwrap_or_default()
        )));
    };
    if let Some(entry) = ledger
        .iter()
        .find(|e| e["to"] == to && e["class"] == "point_of_no_return")
    {
        return Ok(Some(format!(
            "migration {} is a point of no return: the previous release cannot open its data",
            entry["id"].as_str().unwrap_or("?")
        )));
    }
    let before = read_env(&install::host_copy_dir(&stack.dir, id).join("release/.env"));
    let old = crate::check::current_components(&StackDir {
        dir: stack.dir.clone(),
        project: stack.project.clone(),
        env: before,
    });
    let new = crate::check::current_components(stack);
    if let Some((name, was)) = old
        .iter()
        .find(|(k, v)| new.get(*k).is_some_and(|n| n != *v))
    {
        return Ok(Some(format!(
            "{name} changed from {was} to {}: its files do not go back",
            new[name]
        )));
    }
    Ok(None)
}

/// `loams upgrade rollback` (design §54 §8). `confirm` gets the question,
/// which names the writes that will be lost.
pub fn rollback(
    runtime: &dyn Runtime,
    stack: &mut StackDir,
    opts: &RollbackOptions,
    confirm: &dyn Fn(&str) -> bool,
) -> Result<RollbackReport> {
    use crate::history::{Entry, Event, History};
    let entries = attempt_of(&stack.dir, opts.snapshot.as_deref())?;
    let first = entries[0].clone();
    let (id, tikv) = crate::history::snapshot_of(&entries).ok_or_else(|| {
        UpgradeError::new(
            "rollback_needs_snapshot",
            "the upgrade stopped before its snapshot: nothing to roll back",
        )
    })?;
    let taken_at = entries
        .iter()
        .find(|e| e.snapshot.as_deref() == Some(&id))
        .map_or(first.at, |e| e.at);
    let image = crate::bundle::loams_image(&stack.env, &first.to);
    let now = crate::history::now();
    let lost_writes = if opts.keep_writes {
        "none: the data is kept".to_string()
    } else {
        format!(
            "writes between {} and {} will be lost",
            utc(taken_at),
            utc(now)
        )
    };
    if opts.keep_writes
        && let Some(why) = keep_writes_refusal(runtime, stack, &id, &first.from, &first.to)?
    {
        return Err(UpgradeError::new(
            "rollback_needs_snapshot",
            format!("--keep-writes cannot be used: {why}; roll back from the snapshot instead"),
        ));
    }
    let question = format!(
        "Roll the stack in {} back from {} to {} with snapshot {id}? {lost_writes}.",
        stack.dir.display(),
        first.to,
        first.from
    );
    eprintln!("loams upgrade: {lost_writes}");
    if !confirm(&question) {
        return Err(UpgradeError::new(
            "confirmation_required",
            format!("a rollback stops the stack and {lost_writes}: pass --yes"),
        ));
    }
    let history = History::of(&stack.dir);
    let entry = |event: Event, error: Option<&str>| Entry {
        attempt: first.attempt.clone(),
        from: first.from.clone(),
        to: first.to.clone(),
        event,
        step: Some("rollback".to_string()),
        snapshot: Some(id.clone()),
        tikv: tikv.clone(),
        error: error.map(str::to_string),
        at: crate::history::now(),
    };
    let result = if opts.keep_writes {
        swap_images_back(runtime, stack, &id)
    } else {
        restore(
            runtime,
            stack,
            &Restore {
                snapshot: id.clone(),
                tikv: tikv.clone(),
                image,
                br_image: opts.br_image.clone(),
            },
        )
    };
    match result {
        Ok(()) => {
            history.append(&entry(Event::RolledBack, None))?;
            Ok(RollbackReport {
                from: first.to.clone(),
                to: first.from.clone(),
                snapshot: id,
                keep_writes: opts.keep_writes,
                lost_writes,
            })
        }
        Err(e) => {
            let _ = history.append(&entry(Event::RollbackFailed, Some(e.code)));
            let cause =
                UpgradeError::new("rollback_failed", format!("rolling back to {}", first.from));
            Err(failed(&stack.dir, &id, &cause, &e))
        }
    }
}

/// `--keep-writes`: the previous release's bundle and `.env` come back and
/// start on the data as it is.
fn swap_images_back(runtime: &dyn Runtime, stack: &mut StackDir, id: &str) -> Result<()> {
    let config = compose::config(runtime, &stack.dir)?;
    door::close(runtime, &stack.dir, &config)?;
    compose::stop(runtime, &stack.dir, &compose::services(&config))?;
    install::restore_host(&stack.dir, id)?;
    stack.env = read_env(&stack.dir.join(".env"));
    let config = compose::config(runtime, &stack.dir)?;
    let bad = start_previous(runtime, &stack.dir, &config)?;
    if !bad.is_empty() {
        return Err(UpgradeError::new(
            "rollback_failed",
            format!("not healthy after the swap: {}", bad.join(", ")),
        ));
    }
    crate::run::engine_ready(runtime, &stack.dir)?;
    door::open(runtime, &stack.dir, &config)
}
