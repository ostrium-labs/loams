//! `loams-house-mine matrix` (HE0 Task 8): the function and engine matrix,
//! `docs/house/compat-matrix.md`. See [`crate::matrix`].

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, bail};

use crate::matrix::{self, Classes, Kind, Mined};

/// Arguments of `matrix`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The directory of chDB's dumps (`functions.tsv`, `table_engines.tsv`,
    /// `table_functions.tsv`, `source.txt`).
    #[arg(long, default_value = "conformance/clickhouse/matrix/chdb")]
    pub dumps: PathBuf,
    /// The hand-kept classes (§75's tiers and the security list).
    #[arg(long, default_value = "conformance/clickhouse/matrix/classes.toml")]
    pub classes: PathBuf,
    /// The mined profile (HE0 Tasks 2 and 5).
    #[arg(long, default_value = "conformance/clickhouse/profile/profile.toml")]
    pub profile: PathBuf,
    /// The directory of mined inventories (`<app>.json`).
    #[arg(long, default_value = "conformance/clickhouse/inventory")]
    pub inventories: PathBuf,
    /// Where the matrix is written.
    #[arg(long, default_value = "docs/house/compat-matrix.md")]
    pub out: PathBuf,
    /// Writes nothing; fails when the committed matrix is stale or a mined
    /// feature is unclassified (CI).
    #[arg(long)]
    pub check: bool,
    /// First refreshes the dumps from a chDB House over HTTP (`http://host:port/`).
    #[arg(long)]
    pub from_url: Option<String>,
    /// With --from-url: the user.
    #[arg(long, default_value = "default")]
    pub user: String,
    /// With --from-url: the environment variable holding the user's key.
    #[arg(long, default_value = "LOAMS_HOUSE_KEY")]
    pub password_env: String,
    /// With --from-url: the chDB release the House links (`source.txt`).
    #[arg(long)]
    pub chdb_release: Option<String>,
}

/// Runs `matrix`.
///
/// # Errors
///
/// Unreadable inputs; with `--check`, a stale matrix or an unclassified mined
/// feature.
pub fn run(args: &Args) -> anyhow::Result<()> {
    if let Some(url) = &args.from_url {
        if args.check {
            bail!("--check reads the committed dumps; --from-url refreshes them");
        }
        let release = args
            .chdb_release
            .as_deref()
            .context("--from-url needs --chdb-release (the chDB release the House links)")?;
        refresh_dumps(args, url, release)?;
    }
    let (items, source) = matrix::load_dumps(&args.dumps)?;
    let classes_text = std::fs::read_to_string(&args.classes)
        .with_context(|| format!("reading {}", args.classes.display()))?;
    let classes = Classes::parse(&classes_text)?;
    let mined = Mined::load(&args.profile, &args.inventories)?;
    let entries = matrix::classify(&items, &classes, &mined);
    let not_in_chdb = matrix::mined_not_in_chdb(&items, &mined);
    let md = matrix::render(&entries, &source, &not_in_chdb);
    let unclassified = matrix::unclassified_mined(&entries);
    for e in &unclassified {
        eprintln!(
            "  unclassified, mined in {}: {}:{}",
            e.apps.iter().cloned().collect::<Vec<_>>().join(", "),
            e.item.kind.prefix(),
            e.item.name
        );
    }
    if args.check {
        let committed = std::fs::read_to_string(&args.out).unwrap_or_default();
        let mut failures = Vec::new();
        if committed != md {
            failures.push(format!(
                "{} is stale: run `loams-house-mine matrix` and commit it",
                args.out.display()
            ));
        }
        if !unclassified.is_empty() {
            failures.push(format!(
                "{} mined features are unclassified: classify them in {}",
                unclassified.len(),
                args.classes.display()
            ));
        }
        if !failures.is_empty() {
            bail!("{}", failures.join("; "));
        }
        println!("matrix: {} entries, up to date", entries.len());
        return Ok(());
    }
    if let Some(dir) = args.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&args.out, md).with_context(|| format!("writing {}", args.out.display()))?;
    println!(
        "matrix: {} entries written to {} ({} mined features unclassified)",
        entries.len(),
        args.out.display(),
        unclassified.len()
    );
    Ok(())
}

/// Reads chDB's lists over HTTP into the dumps directory.
fn refresh_dumps(args: &Args, url: &str, release: &str) -> anyhow::Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let key = std::env::var(&args.password_env).unwrap_or_default();
    let ask = |sql: &str| -> anyhow::Result<String> {
        agent
            .post(url)
            .header("X-ClickHouse-User", &args.user)
            .header("X-ClickHouse-Key", &key)
            .send(sql)
            .with_context(|| format!("asking {url}"))?
            .body_mut()
            .read_to_string()
            .context("reading the answer")
    };
    std::fs::create_dir_all(&args.dumps)?;
    for kind in Kind::ALL {
        let text = ask(kind.dump_query())?;
        matrix::parse_dump(kind, &text)?;
        std::fs::write(args.dumps.join(kind.dump_file()), text)?;
    }
    let version = ask("SELECT version()")?;
    std::fs::write(
        args.dumps.join("source.txt"),
        format!("chDB {release}, ClickHouse {}\n", version.trim()),
    )?;
    Ok(())
}
