//! `loams-house-mine scrub`: literals out of query samples (HE0 Task 2).
//!
//! Reads a `system.query_log` dump (`.tsv`) or a file of `;`-separated
//! statements, and writes `TSVWithNames` with `id` and the scrubbed `query`.
//! A statement that cannot be scrubbed is left out and counted.

use std::path::PathBuf;

use anyhow::Context as _;

use crate::{querylog, scrub, tokens};

/// Arguments of `scrub`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The input: a query-log dump (`.tsv`) or a statements file.
    #[arg(long)]
    pub input: PathBuf,
    /// Where to write the scrubbed samples; standard output if absent.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

/// Runs `scrub`.
///
/// # Errors
///
/// An unreadable input or unwritable output.
pub fn run(args: &Args) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let statements: Vec<(String, String)> = if args.input.extension().is_some_and(|e| e == "tsv") {
        querylog::parse(&text)?
            .0
            .into_iter()
            .map(|r| (format!("q:{}", r.id), r.query))
            .collect()
    } else {
        tokens::split_statements(&text)
            .into_iter()
            .map(|s| (format!("line:{}", s.line), s.text))
            .collect()
    };
    let mut out = String::from("id\tquery\n");
    let mut dropped = 0usize;
    for (id, statement) in &statements {
        match scrub::scrub(statement) {
            Ok(s) if scrub::is_literal_free(&s.text) => {
                out.push_str(&escape(id));
                out.push('\t');
                out.push_str(&escape(&s.text));
                out.push('\n');
            }
            _ => dropped += 1,
        }
    }
    match &args.output {
        Some(path) => {
            std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?
        }
        None => print!("{out}"),
    }
    eprintln!(
        "scrubbed {} statements, left out {dropped} that could not be tokenized",
        statements.len() - dropped
    );
    Ok(())
}

/// ClickHouse TSV escaping.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
