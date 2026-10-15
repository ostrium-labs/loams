//! `loams upgrade --to <version>` (design §54 §10.3, D935): one step of the
//! release train, in the twelve steps of §10.3, with the door closed from
//! quiesce to verify and an automatic rollback on a failure in steps 8–11
//! (Q823). The chain walk (Q825) calls [`upgrade_step`] once per release.
//!
//! Each finished step is appended to the host history, so a rerun after a
//! killed CLI resumes the open attempt (`upgrade_in_progress` for any other
//! target).

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use crate::bundle::{Bundle, loams_image};
use crate::compose;
use crate::door;
use crate::errors::{Result, UpgradeError};
use crate::helpers::{self, SNAPSHOTS};
use crate::history::{self, Entry, Event, History};
use crate::install;
use crate::path::Release;
use crate::rollback;
use crate::runtime::{Output, Runtime};
use crate::stackdir::{StackDir, read_env};

/// §10.3's steps, in order.
pub const STEPS: [&str; 12] = [
    "preflight",
    "fetch",
    "pull",
    "close_door",
    "quiesce",
    "snapshot",
    "install",
    "third_party",
    "migrate",
    "start",
    "verify",
    "open_door",
];

/// The writers stopped before the snapshot, in this order (§10.3 step 5).
pub const QUIESCE: [&str; 10] = [
    "agentd",
    "console",
    "compute1",
    "compute2",
    "pg-runner",
    "endpoint-storage",
    "pg-control",
    "house",
    "loams",
    "tidb",
];

/// The ordered start (§10.3 step 10): each group waits for the previous
/// one's healthchecks; whatever else the stack enables comes last, then the
/// door.
pub const START: [&[&str]; 4] = [
    &["tidb"],
    &["loams"],
    &["house"],
    &[
        "rustfs",
        "storage_broker",
        "pageserver",
        "safekeeper1",
        "compute1",
        "compute2",
        "pg-runner",
        "pg-control",
        "endpoint-storage",
    ],
];

/// The version of TiKV a stack runs when `.env` does not say.
pub const DEFAULT_TIKV: &str = "v8.5.8";

/// How to upgrade.
#[derive(Clone, Debug)]
pub struct Options {
    /// Leave a failed upgrade as it stopped, with the door closed.
    pub no_auto_rollback: bool,
    /// Keep the earlier snapshots after this one verifies.
    pub keep_snapshot: bool,
    /// BR's image, pinned by digest (`release/companions.toml`).
    pub br_image: String,
}

/// One step's time.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct StepTime {
    pub step: String,
    pub seconds: u64,
    /// Done by an earlier run of the same attempt.
    pub skipped: bool,
}

/// The final report of one release step.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct UpgradeReport {
    pub from: String,
    pub to: String,
    pub snapshot: String,
    /// `verified` (checked against the release's checksums) or `local,
    /// unverified` (`--bundle`).
    pub bundle: String,
    /// Whether this run resumed an attempt a killed CLI left.
    pub resumed: bool,
    pub steps: Vec<StepTime>,
    /// What the migrations asked of the host (a container recreated).
    pub host_changes: Vec<serde_json::Value>,
    /// Snapshots removed after this one verified.
    pub pruned: Vec<String>,
}

/// A failed command as `code`, with the last lines of what it printed.
pub fn failed_output(code: &'static str, what: &str, out: &Output) -> UpgradeError {
    UpgradeError::new(
        code,
        format!("{what} (exit {}): {}", out.status, tail(&out.stderr)),
    )
}

/// The last three non-empty lines of `text`, joined.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" | ")
}

/// Runs a helper; a non-zero exit is `code`.
fn helper(
    runtime: &dyn Runtime,
    spec: &helpers::HelperSpec,
    code: &'static str,
    what: &str,
) -> Result<Output> {
    let out = runtime.engine(&spec.render())?;
    if out.ok() {
        Ok(out)
    } else {
        Err(failed_output(code, what, &out))
    }
}

/// `loams migrate run`'s exit code as an upgrade error (§6.4's codes).
pub fn migrate_error(out: &Output) -> UpgradeError {
    let code = match out.status {
        75 => "upgrade_in_progress",
        78 if out.stderr.contains("upgrade_step_not_adjacent") => "upgrade_step_not_adjacent",
        78 if out.stderr.contains("stack_unrecognised") => "stack_unrecognised",
        _ => "migration_incomplete",
    };
    failed_output(code, "loams migrate run", out)
}

/// Whether the stack directory is in a git checkout: then it must be at the
/// target's tag, clean (Q827), and it is its own bundle.
fn checkout_at(dir: &Path, to: &str) -> Result<bool> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if git(&["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return Ok(false);
    }
    let tag = git(&["describe", "--exact-match", "--tags", "HEAD"]);
    let clean = git(&["status", "--porcelain"]).is_some_and(|s| s.is_empty());
    if tag.as_deref() == Some(&format!("v{to}")) && clean {
        Ok(true)
    } else {
        Err(UpgradeError::new(
            "release_not_found",
            format!(
                "{} is a git checkout: check out the tag v{to} (clean) and pass --bundle {} \
                 (Q827)",
                dir.display(),
                dir.display()
            ),
        ))
    }
}

/// One release step's run.
struct Run<'a> {
    runtime: &'a dyn Runtime,
    stack: &'a mut StackDir,
    bundle: &'a Bundle,
    release: &'a Release,
    from: String,
    to: String,
    opts: &'a Options,
    history: History,
    attempt: String,
    done: Vec<String>,
    snapshot: Option<String>,
    tikv: Option<(String, String)>,
    times: Vec<StepTime>,
    host_changes: Vec<serde_json::Value>,
}

impl Run<'_> {
    fn image(&self) -> String {
        loams_image(&self.stack.env, &self.to)
    }

    fn entry(&self, event: Event, step: Option<&str>, error: Option<&str>) -> Entry {
        Entry {
            attempt: self.attempt.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            event,
            step: step.map(str::to_string),
            snapshot: self.snapshot.clone(),
            tikv: self.tikv.clone(),
            error: error.map(str::to_string),
            at: history::now(),
        }
    }

    /// Runs `step` unless an earlier run of this attempt finished it
    /// (closing the door and quiescing are repeated: they are idempotent).
    fn step(&mut self, name: &str, f: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let repeatable = matches!(name, "close_door" | "quiesce");
        if self.done.iter().any(|d| d == name) && !repeatable {
            self.times.push(StepTime {
                step: name.to_string(),
                seconds: 0,
                skipped: true,
            });
            return Ok(());
        }
        let started = std::time::Instant::now();
        f(self).map_err(|mut e| {
            e.message = format!("step {name}: {}", e.message);
            e
        })?;
        self.times.push(StepTime {
            step: name.to_string(),
            seconds: started.elapsed().as_secs(),
            skipped: false,
        });
        if STEPS.iter().position(|s| *s == name) >= STEPS.iter().position(|s| *s == "close_door") {
            self.history
                .append(&self.entry(Event::Step, Some(name), None))?;
        }
        self.done.push(name.to_string());
        Ok(())
    }

    fn enabled(&self) -> Result<(serde_json::Value, Vec<String>)> {
        let config = compose::config(self.runtime, &self.stack.dir)?;
        let services = compose::services(&config);
        Ok((config, services))
    }

    /// Whether Loams Postgres's storage is up: the snapshot drains it and
    /// `0001-safekeeper-volume` checks the drain.
    fn postgres_running(&self) -> Result<bool> {
        let rows = compose::ps(self.runtime, &self.stack.dir)?;
        let running = compose::running(&rows);
        Ok(["pageserver", "safekeeper1"]
            .iter()
            .all(|s| running.iter().any(|r| r == s)))
    }

    fn preflight(&mut self) -> Result<()> {
        if checkout_at(&self.stack.dir, &self.to)?
            && self.bundle.release_dir.canonicalize().ok() != self.stack.dir.canonicalize().ok()
        {
            return Err(UpgradeError::new(
                "release_not_found",
                "a git checkout upgrades from itself: pass --bundle with the stack's directory",
            ));
        }
        let report = crate::check::check(self.runtime, self.stack, &self.bundle.path, &self.to)?;
        if let Some(blocker) = report.blockers.first() {
            let code = crate::errors::CODES
                .iter()
                .find(|(c, _)| *c == blocker.code)
                .map_or("stack_unhealthy", |(c, _)| c);
            return Err(UpgradeError::new(code, blocker.message.clone()));
        }
        Ok(())
    }

    fn pull(&mut self) -> Result<()> {
        if !self.bundle.verified {
            // A local bundle's images are already present (§10.1).
            return Ok(());
        }
        let staged = history::private_dir(&self.stack.dir)?.join("next.env");
        let text = std::fs::read_to_string(self.stack.dir.join(".env")).unwrap_or_default();
        install::write_private(
            &staged,
            &install::env_with_changes(&text, &self.to, self.release),
        )?;
        let args: Vec<String> = [
            "--env-file",
            &staged.display().to_string(),
            "-p",
            &self.stack.project,
            "pull",
            "--quiet",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let out = self.runtime.compose(&self.bundle.release_dir, &args)?;
        let _ = std::fs::remove_file(&staged);
        if !out.ok() {
            return Err(failed_output(
                "registry",
                "pulling the target's images",
                &out,
            ));
        }
        let out = self
            .runtime
            .engine(&["pull".to_string(), self.opts.br_image.clone()])?;
        if !out.ok() {
            return Err(failed_output("registry", "pulling BR", &out));
        }
        Ok(())
    }

    fn quiesce(&mut self) -> Result<()> {
        let (_, enabled) = self.enabled()?;
        compose::stop(
            self.runtime,
            &self.stack.dir,
            &compose::present(&enabled, &QUIESCE),
        )
    }

    fn snapshot(&mut self) -> Result<()> {
        let id = ulid::Ulid::generate().to_string();
        install::save_host(&self.stack.dir, &id)?;
        let image = self.image();
        let stack = &*self.stack;
        helper(
            self.runtime,
            &helpers::prepare_helper(stack, &image, &id),
            "internal",
            "preparing the snapshot directory",
        )?;
        let (config, _) = self.enabled()?;
        let version = self
            .stack
            .env
            .get("TIKV_TAG")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| DEFAULT_TIKV.to_string());
        let method = if helpers::tikv_mounts_snapshots(&config) {
            helper(
                self.runtime,
                &helpers::br_helper(self.stack, &self.opts.br_image, false, &id),
                "internal",
                "BR's backup of TiKV",
            )?;
            "br"
        } else {
            let pd_tikv = vec!["pd".to_string(), "tikv".to_string()];
            compose::stop(self.runtime, &self.stack.dir, &pd_tikv)?;
            let copied = helper(
                self.runtime,
                &helpers::cold_copy_helper(self.stack, &image, false, &id),
                "internal",
                "the cold copy of TiKV",
            );
            compose::up(self.runtime, &self.stack.dir, &pd_tikv)?;
            copied?;
            "cold"
        };
        let mut sub = vec!["snapshot".to_string()];
        sub.extend(helpers::migrate_store_args(
            self.stack,
            self.postgres_running()?,
        ));
        sub.extend([
            format!("--to={SNAPSHOTS}"),
            format!("--id={id}"),
            format!("--tikv={method}"),
            format!("--tikv-version={version}"),
            format!("--target-release={}", self.to),
            "--output=json".to_string(),
        ]);
        let out = self
            .runtime
            .engine(&helpers::migrate_helper(self.stack, &image, &sub).render())?;
        if !out.ok() {
            let code = if out.stderr.contains("postgres_not_drained") {
                "timeout"
            } else {
                "snapshot_incomplete"
            };
            return Err(failed_output(code, "loams migrate snapshot", &out));
        }
        self.snapshot = Some(id);
        self.tikv = Some((method.to_string(), version));
        Ok(())
    }

    fn install(&mut self) -> Result<()> {
        install::install_bundle(&self.stack.dir, &self.bundle.release_dir)?;
        install::apply_env(&self.stack.dir, &self.from, &self.to, self.release)?;
        self.stack.env = read_env(&self.stack.dir.join(".env"));
        Ok(())
    }

    fn third_party(&mut self) -> Result<()> {
        // PD, then TiKV (§9.2 rule 5); TiDB starts in the ordered start.
        compose::up(self.runtime, &self.stack.dir, &["pd".to_string()])?;
        compose::up(self.runtime, &self.stack.dir, &["tikv".to_string()])
    }

    fn migrate(&mut self) -> Result<()> {
        let mut sub = vec!["run".to_string()];
        sub.extend(helpers::migrate_store_args(
            self.stack,
            self.postgres_running()?,
        ));
        sub.extend([
            format!("--target-release={}", self.to),
            format!("--declared-release={}", self.from),
            "--output=json".to_string(),
        ]);
        let image = self.image();
        let out = self
            .runtime
            .engine(&helpers::migrate_helper(self.stack, &image, &sub).render())?;
        if !out.ok() {
            return Err(migrate_error(&out));
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(out.stdout.trim())
            && let Some(changes) = value["host_changes"].as_array()
        {
            self.host_changes.clone_from(changes);
        }
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        let (config, enabled) = self.enabled()?;
        let long = compose::long_running(&config);
        let mut started: Vec<String> = vec!["pd".to_string(), "tikv".to_string()];
        for group in START {
            let services: Vec<String> = compose::present(&enabled, group)
                .into_iter()
                .filter(|s| long.contains(s))
                .collect();
            compose::up(self.runtime, &self.stack.dir, &services)?;
            started.extend(services);
        }
        let rest: Vec<String> = long
            .iter()
            .filter(|s| !started.contains(s) && !door::SIDECARS.contains(&s.as_str()))
            .cloned()
            .collect();
        compose::up(self.runtime, &self.stack.dir, &rest)
    }

    fn verify(&mut self) -> Result<()> {
        verify(self.runtime, self.stack, &self.image(), &self.to)
    }

    fn open_door(&mut self) -> Result<()> {
        let (_, enabled) = self.enabled()?;
        door::open(self.runtime, &self.stack.dir, &enabled)
    }

    /// Removes the earlier snapshots once this one verified (Q822), unless
    /// `--keep-snapshot`.
    fn prune_older(&self) -> Vec<String> {
        let (Some(current), false) = (&self.snapshot, self.opts.keep_snapshot) else {
            return Vec::new();
        };
        let image = self.image();
        let Ok(listing) = crate::snapshots::list(self.runtime, self.stack, &image) else {
            return Vec::new();
        };
        let mut pruned = Vec::new();
        for id in listing
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["id"].as_str())
            .filter(|id| id != current)
        {
            if crate::snapshots::prune(self.runtime, self.stack, &image, id, true).is_ok() {
                let host = install::host_copy_dir(&self.stack.dir, id);
                if let Some(dir) = host.parent() {
                    let _ = std::fs::remove_dir_all(dir);
                }
                pruned.push(id.to_string());
            }
        }
        pruned
    }
}

/// §10.3 step 11 (§10.4): the stores' probes in a helper of `image`, every
/// service running and healthy, and the engine's `/ready` from inside its
/// container (it listens on loopback).
pub fn verify(runtime: &dyn Runtime, stack: &StackDir, image: &str, to: &str) -> Result<()> {
    let mut sub = vec!["verify".to_string()];
    sub.extend(helpers::migrate_store_args(stack, false));
    sub.extend([
        format!("--target-release={to}"),
        "--output=json".to_string(),
    ]);
    helper(
        runtime,
        &helpers::migrate_helper(stack, image, &sub),
        "stack_unhealthy",
        "loams migrate verify",
    )?;
    let bad: Vec<String> = compose::unhealthy(&compose::ps(runtime, &stack.dir)?)
        .into_iter()
        .filter(|s| !door::SIDECARS.contains(&s.as_str()))
        .collect();
    if !bad.is_empty() {
        return Err(UpgradeError::new(
            "stack_unhealthy",
            format!("not healthy after the start: {}", bad.join(", ")),
        ));
    }
    engine_ready(runtime, &stack.dir)
}

/// The engine's `/ready`, from inside its container.
pub fn engine_ready(runtime: &dyn Runtime, dir: &Path) -> Result<()> {
    let args: Vec<String> = [
        "exec",
        "-T",
        "loams",
        "curl",
        "-fsS",
        "http://127.0.0.1:8080/ready",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let out = runtime.compose(dir, &args)?;
    if out.ok() {
        Ok(())
    } else {
        Err(failed_output(
            "stack_unhealthy",
            "the engine's /ready",
            &out,
        ))
    }
}

/// Upgrades `stack` from `from` to `to` (one release step) with the target's
/// `bundle`. See the module documentation.
pub fn upgrade_step(
    runtime: &dyn Runtime,
    stack: &mut StackDir,
    bundle: &Bundle,
    from: &str,
    to: &str,
    opts: &Options,
) -> Result<UpgradeReport> {
    let history = History::of(&stack.dir);
    let (attempt, done, snapshot, tikv) = match history.open_attempt() {
        Some(entries) if entries[0].to == to => (
            entries[0].attempt.clone(),
            history::finished_steps(&entries),
            history::snapshot_of(&entries).map(|(s, _)| s),
            history::snapshot_of(&entries).and_then(|(_, t)| t),
        ),
        Some(entries) => {
            return Err(UpgradeError::new(
                "upgrade_in_progress",
                format!(
                    "an upgrade from {} to {} has not finished: run `loams upgrade --to {}` to \
                     resume it, or `loams upgrade rollback`",
                    entries[0].from, entries[0].to, entries[0].to
                ),
            ));
        }
        None => (ulid::Ulid::generate().to_string(), Vec::new(), None, None),
    };
    let release = bundle.path.release(to).ok_or_else(|| {
        UpgradeError::new(
            "release_not_found",
            format!("{to} is not in the bundle's release train"),
        )
    })?;
    // A resumed attempt runs from the release it started from.
    let from = history
        .open_attempt()
        .map_or_else(|| from.to_string(), |e| e[0].from.clone());
    let resumed = !done.is_empty() || snapshot.is_some();
    let mut run = Run {
        runtime,
        stack,
        bundle,
        release,
        from,
        to: to.to_string(),
        opts,
        history,
        attempt,
        done,
        snapshot,
        tikv,
        times: Vec::new(),
        host_changes: Vec::new(),
    };
    if !resumed {
        run.step("preflight", Run::preflight)?;
    }
    run.step("fetch", |_| Ok(()))?;
    run.step("pull", Run::pull)?;
    if !resumed {
        let started = run.entry(Event::Started, None, None);
        run.history.append(&started)?;
    }
    // Steps 4–7: a failure restarts the old release on its unchanged data.
    let before = (|| {
        run.step("close_door", |r| {
            let (_, enabled) = r.enabled()?;
            door::close(r.runtime, &r.stack.dir, &enabled)
        })?;
        run.step("quiesce", Run::quiesce)?;
        run.step("snapshot", Run::snapshot)?;
        run.step("install", Run::install)
    })();
    if let Err(err) = before {
        return Err(restart_old(&mut run, err));
    }
    // Steps 8–11: a failure rolls back from the snapshot.
    let after = (|| {
        run.step("third_party", Run::third_party)?;
        run.step("migrate", Run::migrate)?;
        run.step("start", Run::start)?;
        run.step("verify", Run::verify)
    })();
    if let Err(err) = after {
        return Err(roll_back(&mut run, err));
    }
    run.step("open_door", Run::open_door)?;
    let pruned = run.prune_older();
    let done = run.entry(Event::Done, None, None);
    run.history.append(&done)?;
    Ok(UpgradeReport {
        from: run.from.clone(),
        to: run.to.clone(),
        snapshot: run.snapshot.clone().unwrap_or_default(),
        bundle: if bundle.verified {
            "verified".to_string()
        } else {
            "local, unverified".to_string()
        },
        resumed,
        steps: run.times,
        host_changes: run.host_changes,
        pruned,
    })
}

/// A failure in steps 4–7: nothing of the data changed, so the old bundle
/// (if step 7 began) comes back and the old release starts again.
fn restart_old(run: &mut Run<'_>, err: UpgradeError) -> UpgradeError {
    let restarted = (|| {
        if run.done.iter().any(|d| d == "snapshot")
            && let Some(id) = &run.snapshot
        {
            install::restore_host(&run.stack.dir, id)?;
            run.stack.env = read_env(&run.stack.dir.join(".env"));
        }
        compose::up_all(run.runtime, &run.stack.dir)
    })();
    let event = if restarted.is_ok() {
        Event::RolledBack
    } else {
        Event::Failed
    };
    let entry = run.entry(event, None, Some(err.code));
    let _ = run.history.append(&entry);
    match restarted {
        Ok(()) => err,
        Err(e) => UpgradeError::new(
            err.code,
            format!(
                "{}; restarting the old release also failed: {}",
                err.message, e.message
            ),
        ),
    }
}

/// A failure in steps 8–11: roll back from the snapshot (exit 10), unless
/// `--no-auto-rollback` (the door stays closed; exit with the failure).
fn roll_back(run: &mut Run<'_>, err: UpgradeError) -> UpgradeError {
    if run.opts.no_auto_rollback {
        let entry = run.entry(Event::Failed, None, Some(err.code));
        let _ = run.history.append(&entry);
        return UpgradeError::new(
            err.code,
            format!(
                "{}; the stack is stopped with the door closed: rerun `loams upgrade --to {}` to \
                 resume, or `loams upgrade rollback`",
                err.message, run.to
            ),
        );
    }
    let Some(id) = run.snapshot.clone() else {
        return err;
    };
    let restore = rollback::Restore {
        snapshot: id.clone(),
        tikv: run.tikv.clone(),
        image: run.image(),
        br_image: run.opts.br_image.clone(),
    };
    match rollback::restore(run.runtime, run.stack, &restore) {
        Ok(()) => {
            let entry = run.entry(Event::RolledBack, None, Some(err.code));
            let _ = run.history.append(&entry);
            UpgradeError::new(
                "upgrade_failed_rolled_back",
                format!(
                    "{} ({}); the stack is back at {} from snapshot {id}",
                    err.message, err.code, run.from
                ),
            )
        }
        Err(e) => {
            let entry = run.entry(Event::RollbackFailed, None, Some(e.code));
            let _ = run.history.append(&entry);
            rollback::failed(&run.stack.dir, &id, &err, &e)
        }
    }
}
