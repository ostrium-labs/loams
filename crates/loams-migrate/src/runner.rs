//! The migration runner (design §54 §6, UP1 Task 3): `plan`, `run` (with
//! resume) and `abandon`.
//!
//! `run` takes the migration lease, checks that the stored release upgrades
//! to the target in one step (`release/upgrade-path.toml`), runs every
//! pending migration in order, checkpoints each cursor in the ledger, verifies
//! each migration once it is done, and writes the stack record last (the
//! commit point, ruling T0-2). A failure marks the migration `Failed` with its
//! error and leaves the old data readable; running again resumes it from its
//! cursor.
//!
//! Crash testing: with the `failpoints` feature, `migrate::before_apply`,
//! `migrate::after_apply_before_checkpoint`, `migrate::after_checkpoint`,
//! `migrate::before_switch` (in [`Cx::before_switch`]) and
//! `migrate::after_switch_before_done` stop the run there as a process crash
//! would: nothing more is written.

use std::time::SystemTime;

use serde::Serialize;

use crate::formats::REGISTRY;
use crate::ledger::{
    self, LEASE_TTL, Ledger, LedgerEntry, LedgerError, LedgerState, MigrationLease,
};
use crate::migration::{Class, Cx, Estimate, Migration, MigrationError, Step};
use crate::path::UpgradePath;
use crate::stack::StackRecord;

/// The error code a failpoint's simulated crash carries.
pub const CRASH: &str = "failpoint_crash";

/// One migration `run` would apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannedStep {
    pub id: String,
    pub store: crate::formats::Store,
    pub class: Class,
    pub estimate: Estimate,
    /// Where it is: `pending`, or `running`/`failed` when a run stopped in it.
    pub state: LedgerState,
}

/// What `run` would do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The stored release; `None` for an unstamped store.
    pub from: Option<String>,
    pub to: String,
    pub steps: Vec<PlannedStep>,
    /// Whether any step is a point of no return.
    pub point_of_no_return: bool,
}

/// How to run.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The lease holder: unique per process incarnation.
    pub holder: String,
    pub image_digest: Option<String>,
    /// `--no-snapshot-i-accept-data-loss`: run a point-of-no-return
    /// migration without a recorded snapshot.
    pub no_snapshot_i_accept_data_loss: bool,
    /// The store is new and empty: stamp it at the target, migrate nothing.
    pub new_stack: bool,
}

impl RunOptions {
    pub fn new(holder: impl Into<String>) -> Self {
        RunOptions {
            holder: holder.into(),
            image_digest: None,
            no_snapshot_i_accept_data_loss: false,
            new_stack: false,
        }
    }
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub from: Option<String>,
    pub to: String,
    /// Migrations completed by this run, in order.
    pub applied: Vec<String>,
    /// Whether this run wrote the stack record.
    pub stack_written: bool,
    /// The system streams the run made sure of (`_system/_meter`), as
    /// `<namespace>/<stream>` ([`crate::system`]).
    pub system_streams: Vec<String>,
}

/// Why a run did not complete.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Refused before anything changed: `upgrade_step_not_adjacent`,
    /// `release_not_found`, `snapshot_required`, `migration_mismatch`,
    /// `stack_unrecognised`, `migration_unknown`.
    #[error("{code}: {detail}")]
    Refused { code: &'static str, detail: String },
    /// A migration failed; the ledger marks it `Failed`.
    #[error("migration {id} failed: {error}")]
    Failed { id: String, error: MigrationError },
    /// A failpoint stopped the run as a crash would.
    #[error("crashed at {0}")]
    Crashed(String),
    #[error(transparent)]
    Ledger(#[from] ledger::Error),
}

impl RunError {
    /// The stable error code (`upgrade_in_progress` for a held lease).
    pub fn code(&self) -> &str {
        match self {
            RunError::Refused { code, .. } => code,
            RunError::Failed { .. } => "migration_failed",
            RunError::Crashed(_) => CRASH,
            RunError::Ledger(ledger::Error::LeaseHeld { .. }) => "upgrade_in_progress",
            RunError::Ledger(_) => "ledger",
        }
    }
}

impl From<MigrationError> for RunError {
    fn from(error: MigrationError) -> Self {
        if error.code == CRASH {
            RunError::Crashed(error.message)
        } else {
            RunError::Failed {
                id: String::new(),
                error,
            }
        }
    }
}

fn refused(code: &'static str, detail: impl Into<String>) -> RunError {
    RunError::Refused {
        code,
        detail: detail.into(),
    }
}

/// Runs `migrations` (in order) against one stack.
pub struct Runner<'a> {
    migrations: &'a [&'a dyn Migration],
    path: &'a UpgradePath,
}

impl std::fmt::Debug for Runner<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.migrations.iter().map(|m| m.id()).collect();
        f.debug_struct("Runner")
            .field("migrations", &ids)
            .finish_non_exhaustive()
    }
}

impl<'a> Runner<'a> {
    pub fn new(migrations: &'a [&'a dyn Migration], path: &'a UpgradePath) -> Self {
        Runner { migrations, path }
    }

    /// What [`Runner::run`] would do, read-only.
    pub async fn plan(&self, cx: &Cx, target: &str) -> Result<Plan, RunError> {
        let (from, entries) = match Ledger::open_existing(cx.meta.clone()).await? {
            Some(ledger) => (
                ledger.stack().await?.map(|s| s.release),
                ledger.entries().await?,
            ),
            None => (None, Vec::new()),
        };
        let mut steps = Vec::new();
        for migration in self.pending(&entries, from.as_deref(), target) {
            let state = entries
                .iter()
                .rev()
                .find(|e| e.id == migration.id())
                .map_or(LedgerState::Pending, |e| e.state);
            steps.push(PlannedStep {
                id: migration.id().to_string(),
                store: migration.store(),
                class: migration.class(),
                estimate: migration.estimate(cx).map_err(|error| RunError::Failed {
                    id: migration.id().to_string(),
                    error,
                })?,
                state,
            });
        }
        Ok(Plan {
            from,
            to: target.to_string(),
            point_of_no_return: steps.iter().any(|s| s.class == Class::PointOfNoReturn),
            steps,
        })
    }

    /// The compiled migrations not yet `Done`, in order, leaving out those a
    /// release at or before `stored` introduced: a stack stamped fresh at a
    /// release never needs that release's own migrations.
    fn pending(
        &self,
        entries: &[LedgerEntry],
        stored: Option<&str>,
        target: &str,
    ) -> Vec<&'a dyn Migration> {
        self.migrations
            .iter()
            .copied()
            .filter(|m| {
                !entries
                    .iter()
                    .any(|e| e.id == m.id() && e.state == LedgerState::Done)
            })
            .filter(|m| !stored.is_some_and(|s| self.path.introduced_at_or_before(m.id(), s)))
            .filter(|m| !self.path.introduced_after(m.id(), target))
            .collect()
    }

    /// Migrates the stack to `target`.
    pub async fn run(
        &self,
        cx: &mut Cx,
        target: &str,
        opts: &RunOptions,
    ) -> Result<Report, RunError> {
        if self.path.release(target).is_none() {
            return Err(refused(
                "release_not_found",
                format!("{target} is not in this binary's release train"),
            ));
        }
        let ledger = Ledger::open(cx.meta.clone()).await?;
        let lease =
            MigrationLease::acquire(cx.meta.clone(), opts.holder.clone(), LEASE_TTL).await?;
        let result = match self.run_leased(cx, &ledger, &lease, target, opts).await {
            // The stack is at `target`: its system objects exist on a new
            // stack and an upgraded one alike (ME1 Task 4).
            Ok(mut report) => match crate::system::ensure_system_streams(&*cx.meta).await {
                Ok(streams) => {
                    report.system_streams = streams;
                    Ok(report)
                }
                Err(e) => Err(RunError::from(e)),
            },
            Err(e) => Err(e),
        };
        // A simulated crash leaves the lease as a dead process would.
        if !matches!(result, Err(RunError::Crashed(_))) {
            let _ = lease.release().await;
        }
        result
    }

    async fn run_leased(
        &self,
        cx: &mut Cx,
        ledger: &Ledger,
        lease: &MigrationLease,
        target: &str,
        opts: &RunOptions,
    ) -> Result<Report, RunError> {
        let mut stored = ledger.stack().await?.map(|s| s.release);
        let mut entries = ledger.entries().await?;
        let mut report = Report {
            from: stored.clone(),
            to: target.to_string(),
            ..Report::default()
        };
        // Refuse to resume a run whose migration this binary defines
        // differently.
        for entry in &entries {
            if entry.state == LedgerState::Done {
                continue;
            }
            match self.migrations.iter().find(|m| m.id() == entry.id) {
                Some(m) if m.code_digest() != entry.code_digest => {
                    return Err(refused(
                        "migration_mismatch",
                        format!(
                            "{} was started by a binary that defines it differently",
                            entry.id
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    return Err(refused(
                        "migration_unknown",
                        format!("{} is in the ledger but not in this binary", entry.id),
                    ));
                }
            }
        }
        if stored.is_none() && opts.new_stack {
            let record = StackRecord::current(
                target,
                REGISTRY,
                SystemTime::now(),
                opts.image_digest.clone(),
            );
            ledger.set_stack(lease, &record).await?;
            report.stack_written = true;
            return Ok(report);
        }
        let mut pending = self.pending(&entries, stored.as_deref(), target);
        // An unstamped store is migrated only by an adoption, which runs first
        // and records the release it found.
        if stored.is_none() {
            let Some(adopter) = pending.first().copied().filter(|m| m.adopts().is_some()) else {
                return Err(refused(
                    "stack_unrecognised",
                    "the store has no stack record and nothing adopts it; start a fresh stack",
                ));
            };
            let adopted = adopter.adopts().unwrap_or_default();
            self.apply_one(
                cx,
                ledger,
                lease,
                adopter,
                &mut entries,
                adopted,
                target,
                opts,
            )
            .await?;
            report.applied.push(adopter.id().to_string());
            let record = StackRecord::current(
                adopted,
                REGISTRY,
                SystemTime::now(),
                opts.image_digest.clone(),
            );
            ledger.set_stack(lease, &record).await?;
            stored = Some(adopted.to_string());
            pending.remove(0);
        }
        let stored = stored.unwrap_or_default();
        // An older release over data a newer one migrated (a rollback with
        // `--keep-writes`, or its compose `migrate` before it starts): there is
        // nothing to do when every migration newer than it is `Reversible`
        // and done, as the startup gate also lets it serve (§54 §8).
        if crate::path::compare_releases(&stored, target) == Some(std::cmp::Ordering::Greater) {
            if let Some(record) = ledger.stack().await?
                && let Err(unreadable) = record.stored().readable_by(REGISTRY)
            {
                return Err(refused(
                    "format_unreadable",
                    format!("the data is at {stored}, newer than {target}: {unreadable}"),
                ));
            }
            let newer: Vec<&LedgerEntry> = entries
                .iter()
                .filter(|e| {
                    crate::path::compare_releases(&e.release_to, target)
                        == Some(std::cmp::Ordering::Greater)
                })
                .collect();
            if let Some(entry) = newer
                .iter()
                .find(|e| e.class != Class::Reversible || e.state != LedgerState::Done)
            {
                return Err(refused(
                    "format_unreadable",
                    format!(
                        "the data is at {stored}, newer than {target}, and {} ({:?}, {:?}) cannot \
                         be read by {target}: restore the snapshot taken before the upgrade",
                        entry.id, entry.class, entry.state
                    ),
                ));
            }
            return Ok(report);
        }
        if stored == target {
            if pending.is_empty() {
                return Ok(report);
            }
        } else {
            self.path
                .check_adjacent(&stored, target)
                .map_err(|r| refused(r.code, r.detail))?;
        }
        if pending.iter().any(|m| m.class() == Class::PointOfNoReturn)
            && !opts.no_snapshot_i_accept_data_loss
            && !has_snapshot(ledger).await?
        {
            return Err(refused(
                "snapshot_required",
                "a point-of-no-return migration needs a snapshot first (loams migrate snapshot, \
                 or --no-snapshot-i-accept-data-loss)",
            ));
        }
        for migration in pending {
            self.apply_one(
                cx,
                ledger,
                lease,
                migration,
                &mut entries,
                &stored,
                target,
                opts,
            )
            .await?;
            report.applied.push(migration.id().to_string());
        }
        let record = StackRecord::current(
            target,
            REGISTRY,
            SystemTime::now(),
            opts.image_digest.clone(),
        );
        let already = ledger.stack().await?.is_some_and(|s| s.release == target);
        if !already {
            ledger.set_stack(lease, &record).await?;
            report.stack_written = true;
        }
        Ok(report)
    }

    /// Runs one migration to `Done`, resuming its ledger entry if it has one.
    #[allow(clippy::too_many_arguments)]
    async fn apply_one(
        &self,
        cx: &mut Cx,
        ledger: &Ledger,
        lease: &MigrationLease,
        migration: &dyn Migration,
        entries: &mut Vec<LedgerEntry>,
        from: &str,
        to: &str,
        opts: &RunOptions,
    ) -> Result<(), RunError> {
        let id = migration.id();
        let now = SystemTime::now();
        let existing = entries.iter().rev().find(|e| e.id == id).cloned();
        let (seq, mut cursor) = match existing {
            Some(entry) => {
                let resumed = ledger
                    .update(lease, entry.seq, |e| {
                        e.state = LedgerState::Running;
                        e.attempts += 1;
                        e.error = None;
                    })
                    .await?;
                (resumed.seq, resumed.cursor)
            }
            None => {
                let entry = LedgerEntry {
                    seq: ledger.next_seq().await?,
                    id: id.to_string(),
                    release_from: from.to_string(),
                    release_to: to.to_string(),
                    store: migration.store(),
                    class: migration.class(),
                    state: LedgerState::Running,
                    cursor: None,
                    attempts: 1,
                    started_at: Some(now),
                    finished_at: None,
                    error: None,
                    image_digest: opts.image_digest.clone(),
                    code_digest: migration.code_digest(),
                };
                ledger.append(lease, &entry).await?;
                entries.push(entry.clone());
                (entry.seq, None)
            }
        };
        let failed = |error: MigrationError| async move {
            if error.code == CRASH {
                return RunError::Crashed(error.message);
            }
            let stored = LedgerError::new(error.code.clone(), error.message.clone());
            if let Err(e) = ledger
                .update(lease, seq, |e| {
                    e.state = LedgerState::Failed;
                    e.error = Some(stored);
                })
                .await
            {
                return e.into();
            }
            RunError::Failed {
                id: id.to_string(),
                error,
            }
        };
        loop {
            crate::failpoint!("migrate::before_apply");
            let step = match migration.apply(cx, cursor.clone()).await {
                Ok(step) => step,
                Err(error) => return Err(failed(error).await),
            };
            match step {
                Step::Continue(next) => {
                    crate::failpoint!("migrate::after_apply_before_checkpoint");
                    ledger
                        .update(lease, seq, |e| e.cursor = Some(next.clone()))
                        .await?;
                    crate::failpoint!("migrate::after_checkpoint");
                    lease.renew(LEASE_TTL).await?;
                    cursor = Some(next);
                }
                Step::Done => break,
            }
        }
        crate::failpoint!("migrate::after_switch_before_done");
        if let Err(error) = migration.verify(cx).await {
            return Err(failed(error).await);
        }
        ledger
            .update(lease, seq, |e| {
                e.state = LedgerState::Done;
                e.cursor = None;
                e.finished_at = Some(SystemTime::now());
            })
            .await?;
        if let Some(entry) = entries.iter_mut().find(|e| e.seq == seq) {
            entry.state = LedgerState::Done;
        }
        Ok(())
    }

    /// `loams migrate run --abandon <id>`: removes a failed migration's
    /// partial writes, and resets its entry so a later run starts it over.
    pub async fn abandon(&self, cx: &mut Cx, id: &str, holder: &str) -> Result<(), RunError> {
        let ledger = Ledger::open(cx.meta.clone()).await?;
        let Some(entry) = ledger
            .entries()
            .await?
            .into_iter()
            .rev()
            .find(|e| e.id == id)
        else {
            return Err(refused(
                "migration_unknown",
                format!("{id} is not in the ledger"),
            ));
        };
        if entry.state == LedgerState::Done {
            return Err(refused(
                "migration_done",
                format!("{id} is done; restore the snapshot to undo it"),
            ));
        }
        let Some(migration) = self.migrations.iter().find(|m| m.id() == id) else {
            return Err(refused(
                "migration_unknown",
                format!("{id} is not in this binary"),
            ));
        };
        let lease = MigrationLease::acquire(cx.meta.clone(), holder, LEASE_TTL).await?;
        let result = async {
            migration
                .abandon(cx)
                .await
                .map_err(|error| RunError::Failed {
                    id: id.to_string(),
                    error,
                })?;
            ledger
                .update(&lease, entry.seq, |e| {
                    e.state = LedgerState::Failed;
                    e.cursor = None;
                    e.error = Some(LedgerError::new("abandoned", "partial writes removed"));
                })
                .await?;
            Ok(())
        }
        .await;
        let _ = lease.release().await;
        result
    }
}

async fn has_snapshot(ledger: &Ledger) -> Result<bool, RunError> {
    Ok(ledger.snapshot_roots().await? > 0)
}
