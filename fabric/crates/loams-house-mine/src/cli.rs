//! The command line: one subcommand per module in [`crate::cmd`].

use clap::{Parser, Subcommand};

use crate::cmd;

/// Mine ClickHouse applications into the Loams House compatibility profile.
#[derive(Debug, Parser)]
#[command(name = "loams-house-mine", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extract one application's feature inventory from its query log and migrations.
    Extract(cmd::extract::Args),
    /// Combine inventories and rank features into the profile.
    Merge(cmd::merge::Args),
    /// Replace literals in kept query samples with typed placeholders.
    Scrub(cmd::scrub::Args),
    /// Write `profile.toml` and the compatibility report.
    Report(cmd::report::Args),
    /// Write the function and engine matrix from chDB's system tables.
    Matrix(Box<cmd::matrix::Args>),
    /// Run the oracle's corpus on its targets and write the triage.
    Oracle(Box<cmd::oracle::Args>),
}

impl Command {
    /// Runs the chosen subcommand.
    ///
    /// # Errors
    ///
    /// Whatever the subcommand reports.
    pub fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Extract(args) => cmd::extract::run(&args),
            Self::Merge(args) => cmd::merge::run(&args),
            Self::Scrub(args) => cmd::scrub::run(&args),
            Self::Report(args) => cmd::report::run(&args),
            Self::Matrix(args) => cmd::matrix::run(&args),
            Self::Oracle(args) => cmd::oracle::run(&args),
        }
    }
}
