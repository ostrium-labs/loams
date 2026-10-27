//! `loams-house-mine extract`: one application's inventory (HE0 Task 2).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};

use crate::explain::{Explainer, HttpExplainer};
use crate::inventory::{self, Options, Source};
use crate::{querylog, tokens};

/// Arguments of `extract`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The application, as the apps manifest names it.
    #[arg(long)]
    pub app: String,
    /// The version or variant mined (`v3`).
    #[arg(long)]
    pub variant: Option<String>,
    /// A `system.query_log` dump in `TSVWithNames` (see the `querylog` module).
    #[arg(long)]
    pub query_log: Option<PathBuf>,
    /// A directory of migration files; every `*.sql` file under it is read.
    #[arg(long)]
    pub migrations: Option<PathBuf>,
    /// The pinned clickhouse-server's HTTP URL, for `EXPLAIN AST` of the
    /// statements sqlparser refuses. Without it they are counted unparsed.
    #[arg(long)]
    pub server: Option<String>,
    /// A setting name never to record (the harness's own); repeatable.
    #[arg(long = "ignore-setting")]
    pub ignore_settings: Vec<String>,
    /// Where to write the inventory.
    #[arg(long)]
    pub out: PathBuf,
}

/// Runs `extract`.
///
/// # Errors
///
/// Unreadable inputs, or an inventory that fails its own checks.
pub fn run(args: &Args) -> anyhow::Result<()> {
    ensure!(
        args.query_log.is_some() || args.migrations.is_some(),
        "give --query-log, --migrations or both"
    );
    let mut sources = Vec::new();
    let mut skipped = querylog::Skipped::default();
    if let Some(path) = &args.query_log {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let (rows, s) = querylog::parse(&text)?;
        skipped = s;
        sources.extend(rows.into_iter().map(|r| Source {
            id: format!("q:{}", r.id),
            text: r.query,
            settings: r.settings,
        }));
    }
    if let Some(dir) = &args.migrations {
        sources.extend(migration_sources(dir)?);
    }
    let mut http = args.server.as_deref().map(HttpExplainer::new);
    let explainer: Option<&mut dyn Explainer> = http.as_mut().map(|h| h as &mut dyn Explainer);
    let options = Options {
        ignore_settings: args.ignore_settings.iter().cloned().collect(),
    };
    let mut inv = inventory::build(
        &args.app,
        args.variant.as_deref(),
        &sources,
        explainer,
        &options,
    );
    inv.skipped = skipped.into();
    inventory::check(&inv)?;
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&inv)? + "\n")
        .with_context(|| format!("writing {}", args.out.display()))?;
    let c = inv.statements;
    eprintln!(
        "{}: {} statements ({} sqlparser, {} EXPLAIN AST, {} unparsed), {} features, {} samples",
        args.app,
        c.total,
        c.parsed,
        c.explained,
        c.unparsed,
        inv.features.len(),
        inv.samples.len()
    );
    Ok(())
}

/// Every statement of every `*.sql` file under `dir` except down migrations
/// (`*.down.sql`, which undo the schema rather than build it), with the id
/// `m:<path relative to dir>:<line>`, in path order.
///
/// # Errors
///
/// An unreadable directory or file.
pub fn migration_sources(dir: &Path) -> anyhow::Result<Vec<Source>> {
    let mut files = Vec::new();
    collect_sql(dir, &mut files)?;
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let rel = file
            .strip_prefix(dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        // Two statements can start on one line; the second is `<line>.2`, so
        // every id names one statement (HE0 Task 2 ruling R2.13).
        let mut on_line: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
        for st in tokens::split_statements(&text) {
            let n = on_line.entry(st.line).or_insert(0);
            *n += 1;
            let id = if *n == 1 {
                format!("m:{rel}:{}", st.line)
            } else {
                format!("m:{rel}:{}.{n}", st.line)
            };
            out.push(Source {
                id,
                text: st.text,
                settings: Vec::new(),
            });
        }
    }
    Ok(out)
}

fn collect_sql(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sql(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "sql")
            && !path.to_string_lossy().ends_with(".down.sql")
        {
            out.push(path);
        }
    }
    Ok(())
}
