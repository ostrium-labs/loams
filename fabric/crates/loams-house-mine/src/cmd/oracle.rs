//! `loams-house-mine oracle` (HE0 Task 6): runs the oracle's corpus on its
//! targets and writes the triage. See [`crate::oracle`].

use std::path::PathBuf;

use anyhow::Context as _;

use crate::oracle;

/// Arguments of `oracle`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The corpora to run.
    #[arg(long, default_value = "conformance/clickhouse/oracle/corpora.toml")]
    pub corpora: PathBuf,
    /// The targets, one of them the baseline.
    #[arg(long, default_value = "conformance/clickhouse/oracle/targets.toml")]
    pub targets: PathBuf,
    /// The triage file, read and rewritten.
    #[arg(long, default_value = "conformance/clickhouse/oracle/triage.toml")]
    pub triage: PathBuf,
    /// Overrides a target's URL: `NAME=URL` (repeatable).
    #[arg(long = "target-url", value_name = "NAME=URL")]
    pub target_url: Vec<String>,
    /// Runs only this target besides the baseline (repeatable).
    #[arg(long)]
    pub only: Vec<String>,
    /// Runs only the cases whose id starts with this (repeatable).
    #[arg(long = "case", value_name = "PREFIX")]
    pub cases: Vec<String>,
    /// Leaves the triage file unchanged; still fails on untriaged mismatches.
    #[arg(long)]
    pub check: bool,
    /// Skips the corpora's setup statements (the tables exist already).
    #[arg(long)]
    pub skip_setup: bool,
    /// Also fails while any `bug` entry names this plan task (D1804).
    #[arg(long)]
    pub task: Option<String>,
}

/// Runs `oracle`.
///
/// # Errors
///
/// An unreachable target, a failing baseline, or an untriaged mismatch.
pub fn run(args: &Args) -> anyhow::Result<()> {
    let urls = args
        .target_url
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(n, u)| (n.to_string(), u.to_string()))
                .with_context(|| format!("--target-url is NAME=URL, not {pair:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    oracle::run(&oracle::Run {
        corpora: args.corpora.clone(),
        targets: args.targets.clone(),
        triage: args.triage.clone(),
        urls,
        only: args.only.clone(),
        cases: args.cases.clone(),
        check: args.check,
        skip_setup: args.skip_setup,
        task: args.task.clone(),
    })
    .map(|_| ())
}
