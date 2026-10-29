//! The oracle runner (HE0 Task 6, design §75 §15 suites 2 to 4).
//!
//! It runs a corpus of `.sql` cases on a list of targets over the ClickHouse
//! HTTP interface (`clickhouse-server@26.9`, the baseline; `clickhouse-server@24.3`;
//! `house-chdb`; later the Rust engine), reads each answer as
//! `JSONCompactEachRowWithNamesAndTypes`, compares every target with the
//! baseline after [`normalise`], and merges the mismatches into
//! `conformance/clickhouse/oracle/triage.toml` ([`triage`]). A mismatch nobody
//! has triaged fails the run.

pub mod corpus;
pub mod normalise;
pub mod seed;
pub mod target;
pub mod triage;
pub mod types;

use std::path::PathBuf;

use anyhow::{Context as _, bail};

use self::corpus::Corpus;
use self::normalise::{Kind, compare, compare_bytes};
use self::target::{Client, Target, Targets};
use self::triage::{Found, Reconciled, Scope, Triage};

/// One run's inputs.
#[derive(Debug, Clone)]
pub struct Run {
    pub corpora: PathBuf,
    pub targets: PathBuf,
    pub triage: PathBuf,
    /// `name=url` overrides of `targets.toml`'s URLs.
    pub urls: Vec<(String, String)>,
    /// Run only these targets besides the baseline (all when empty).
    pub only: Vec<String>,
    /// Run only the cases whose id starts with one of these (all when empty).
    pub cases: Vec<String>,
    /// Leave the triage file as it is (CI): still fail on what it would say.
    pub check: bool,
    pub skip_setup: bool,
    /// Fails while any `bug` entry names this plan task (D1804).
    pub task: Option<String>,
}

/// What a run found.
#[derive(Debug)]
pub struct Summary {
    pub cases: usize,
    pub targets: Vec<String>,
    /// Each target's `SELECT version()`, in the order of `targets`.
    pub versions: Vec<String>,
    pub reconciled: Reconciled,
}

impl Summary {
    /// Prints the run's totals and its failures.
    pub fn print(&self) {
        println!(
            "oracle: {} cases on {} targets ({})",
            self.cases,
            self.targets.len(),
            self.targets.join(", ")
        );
        let entries = &self.reconciled.triage.entries;
        for (target, version) in self.targets.iter().zip(&self.versions) {
            let of = |status| {
                entries
                    .iter()
                    .filter(|e| &e.target == target && e.status == status)
                    .count()
            };
            println!(
                "  {target} ({version}): {} bug, {} incompatibility, {} untriaged",
                of(triage::Status::Bug),
                of(triage::Status::Incompatibility),
                of(triage::Status::Untriaged)
            );
        }
        for entry in &self.reconciled.resolved {
            println!(
                "  resolved: {} on {} ({}), was {:?}",
                entry.case,
                entry.target,
                entry.kind.as_str(),
                entry.status
            );
        }
    }
}

/// Runs the corpus, writes the triage (unless `check`), and fails on an
/// untriaged mismatch or an undocumented incompatibility.
///
/// # Errors
///
/// A target that cannot be reached, a baseline that fails a setup statement,
/// unreadable inputs, and the triage's failures.
pub fn run(run: &Run) -> anyhow::Result<Summary> {
    let summary = execute(run)?;
    summary.print();
    if !run.check {
        summary.reconciled.triage.write(&run.triage)?;
    }
    let mut failures = summary.reconciled.triage.failures();
    if let Some(task) = &run.task {
        for e in summary.reconciled.triage.bugs_of_task(task) {
            failures.push(format!(
                "a bug of task {task}: {} on {} ({}): {}",
                e.case,
                e.target,
                e.kind.as_str(),
                e.detail
            ));
        }
    }
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("  {failure}");
        }
        bail!(
            "{} triage failures (see {}); triage each mismatch as bug or incompatibility",
            failures.len(),
            run.triage.display()
        );
    }
    Ok(summary)
}

/// Runs the corpus and reconciles the triage, without writing or judging it.
///
/// # Errors
///
/// As [`run`], except the triage's failures.
pub fn execute(run: &Run) -> anyhow::Result<Summary> {
    let mut targets = Targets::load(&run.targets)?.targets;
    for (name, url) in &run.urls {
        let target = targets
            .iter_mut()
            .find(|t| &t.name == name)
            .with_context(|| format!("--target-url names no target {name}"))?;
        target.url.clone_from(url);
    }
    for name in &run.only {
        if !targets.iter().any(|t| &t.name == name) {
            bail!("--only names no target {name}");
        }
    }
    // A seeded target (the Rust engine's dev server) runs only when `--only`
    // names it, so a plain run needs only the servers it always needed.
    targets.retain(|t| {
        t.baseline
            || if run.only.is_empty() {
                !t.seed
            } else {
                run.only.contains(&t.name)
            }
    });
    let baseline_at = targets
        .iter()
        .position(|t| t.baseline)
        .context("a baseline")?;
    let baseline = targets.remove(baseline_at);

    let mut corpora = corpus::load(&run.corpora)?;
    for corpus in &mut corpora {
        corpus
            .cases
            .retain(|c| run.cases.is_empty() || run.cases.iter().any(|p| c.id.starts_with(p)));
    }
    corpora.retain(|c| !c.cases.is_empty());

    let client = Client::default();
    let mut versions = Vec::new();
    for target in std::iter::once(&baseline).chain(&targets) {
        versions.push(match client.query(target, "SELECT version()")? {
            normalise::Outcome::Rows(r) => r
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
            normalise::Outcome::Error { code, .. } => format!("version() failed: {code}"),
        });
    }
    let mut found = Vec::new();
    let mut scope = Scope {
        targets: targets.iter().map(|t| t.name.clone()).collect(),
        ..Scope::default()
    };
    let mut cases = 0;
    for corpus in &corpora {
        if !run.skip_setup {
            setup(&client, &baseline, &targets, corpus, &mut found, &mut scope)?;
        }
        for case in &corpus.cases {
            cases += 1;
            scope.cases.insert(case.id.clone());
            if let Some(format) = &case.format {
                let expected = client.query_bytes(&baseline, &case.sql, format)?;
                for target in &targets {
                    let got = client.query_bytes(target, &case.sql, format)?;
                    for m in compare_bytes(&expected, &got) {
                        found.push(Found {
                            case: case.id.clone(),
                            target: target.name.clone(),
                            kind: m.kind,
                            detail: m.detail,
                        });
                    }
                }
                continue;
            }
            let expected = client.query(&baseline, &case.sql)?;
            for target in &targets {
                let got = client.query(target, &case.sql)?;
                for m in compare(&expected, &got, case.order, case.precision) {
                    found.push(Found {
                        case: case.id.clone(),
                        target: target.name.clone(),
                        kind: m.kind,
                        detail: m.detail,
                    });
                }
            }
        }
    }
    let previous = Triage::load(&run.triage)?;
    Ok(Summary {
        cases,
        targets: std::iter::once(&baseline)
            .chain(&targets)
            .map(|t| t.name.clone())
            .collect(),
        versions,
        reconciled: triage::reconcile(&previous, found, &scope),
    })
}

/// Runs a corpus's setup on every target. The baseline must succeed; a target
/// that fails a statement the baseline ran is a mismatch of case
/// `<corpus>/@setup`, kind `error`.
fn setup(
    client: &Client,
    baseline: &Target,
    targets: &[Target],
    corpus: &Corpus,
    found: &mut Vec<Found>,
    scope: &mut Scope,
) -> anyhow::Result<()> {
    let case = format!("{}/@setup", corpus.name);
    scope.cases.insert(case.clone());
    for (i, statement) in corpus.setup.iter().enumerate() {
        if let Some(err) = client.execute(baseline, statement)? {
            bail!(
                "the baseline {} failed {}'s setup statement {}: {err:?}",
                baseline.name,
                corpus.name,
                i + 1
            );
        }
    }
    for target in targets {
        if target.seed {
            // A seeded target (the Rust engine's dev server) gets the
            // baseline's tables, not the statements (D1804, plan HE1 Task 23).
            if let Err(e) = seed::copy_tables(client, baseline, target) {
                found.push(Found {
                    case: case.clone(),
                    target: target.name.clone(),
                    kind: Kind::Error,
                    detail: format!("seeding: {e:#}"),
                });
            }
            continue;
        }
        for (i, statement) in corpus.setup.iter().enumerate() {
            if let Some(err) = client.execute(target, statement)? {
                let normalise::Outcome::Error { code, message } = err else {
                    continue;
                };
                found.push(Found {
                    case: case.clone(),
                    target: target.name.clone(),
                    kind: Kind::Error,
                    detail: format!("statement {}: error {code} ({message})", i + 1),
                });
                break;
            }
        }
    }
    Ok(())
}
