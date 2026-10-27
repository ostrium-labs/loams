//! `loams risingwave …`: the managed RisingWave's SQL client (plan RW1, lane
//! RW-b, assumption A11).
//!
//! ```text
//! loams risingwave sql "SELECT * FROM clicks_per_minute LIMIT 5"
//! loams risingwave sql --file pipeline.sql
//! loams risingwave list [mvs|sources|sinks|tables|all]
//! loams risingwave progress [<job-id>] [--wait] [--timeout 600]
//! loams risingwave cancel <job-id>
//! loams risingwave show-create <mv|source|sink|table> <name>
//! loams risingwave render <pipeline.toml|pipeline.json>
//! loams risingwave apply <pipeline.toml|pipeline.json> [--dry-run] [--wait]
//! ```
//!
//! The frontend is reached through loams-gateway (design 81 R7, R8): `--url`
//! or `LOAMS_RISINGWAVE_URL` [default: `postgres://loams@127.0.0.1:4566/dev`,
//! the compose `engine-gateway`]. The password is a Loams API key or token,
//! read from `LOAMS_RISINGWAVE_PASSWORD` (falling back to `LOAMS_API_KEY`),
//! never from the command line. `apply` reads each credential a pipeline
//! names (`{ env = "…" }`) from the environment. `--json` prints
//! results as JSON on stdout. Like `loams cloud`, it starts no server.
//! Status: preview.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};

use crate::connectors::pipeline::Pipeline;
use crate::sql_api::{
    DEFAULT_URL, JobProgress, Outcome, Relation, RelationKind, SqlApi, SqlApiError,
};

/// `loams risingwave`.
#[derive(Debug, Args)]
pub struct RisingWaveArgs {
    /// The RisingWave frontend (pgwire).
    #[arg(long, env = "LOAMS_RISINGWAVE_URL", default_value = DEFAULT_URL, global = true)]
    pub url: String,
    /// Print results as JSON.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: RisingWaveCommand,
}

#[derive(Debug, Subcommand)]
pub enum RisingWaveCommand {
    /// Run SQL statements (`;`-separated) and print their results.
    Sql {
        /// The statements. Leave out with --file.
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        sql: Option<String>,
        /// Read the statements from a file.
        #[arg(long, short)]
        file: Option<PathBuf>,
    },
    /// List materialized views, sources, sinks or tables.
    List {
        #[arg(value_enum, default_value = "all")]
        what: ListWhat,
        /// Include each statement's definition.
        #[arg(long)]
        definitions: bool,
    },
    /// Show the jobs being created and their backfill progress.
    Progress {
        /// One job.
        job: Option<i64>,
        /// Wait until the job (or every job) has finished.
        #[arg(long)]
        wait: bool,
        /// How long --wait waits, in seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Cancel a job that is being created (`CANCEL JOBS`).
    Cancel { job: i64 },
    /// Print the stored statement of a relation (`SHOW CREATE`).
    ShowCreate {
        #[arg(value_enum)]
        kind: Kind,
        name: String,
    },
    /// Print a pipeline's statements (secrets redacted) without connecting.
    Render { pipeline: PathBuf },
    /// Create a pipeline's secrets and objects, in order.
    Apply {
        pipeline: PathBuf,
        /// Print what would run (secrets redacted) and stop.
        #[arg(long)]
        dry_run: bool,
        /// Wait for the backfill of every job the pipeline started.
        #[arg(long)]
        wait: bool,
        /// How long --wait waits, in seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ListWhat {
    Mvs,
    Sources,
    Sinks,
    Tables,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    Mv,
    Source,
    Sink,
    Table,
}

impl From<Kind> for RelationKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Mv => Self::MaterializedView,
            Kind::Source => Self::Source,
            Kind::Sink => Self::Sink,
            Kind::Table => Self::Table,
        }
    }
}

impl ListWhat {
    fn kinds(self) -> &'static [RelationKind] {
        match self {
            Self::Mvs => &[RelationKind::MaterializedView],
            Self::Sources => &[RelationKind::Source],
            Self::Sinks => &[RelationKind::Sink],
            Self::Tables => &[RelationKind::Table],
            Self::All => &[
                RelationKind::Source,
                RelationKind::Table,
                RelationKind::MaterializedView,
                RelationKind::Sink,
            ],
        }
    }
}

/// The error a command ends with.
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Sql(#[from] SqlApiError),
    #[error(transparent)]
    Pipeline(#[from] crate::connectors::pipeline::PipelineError),
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Usage(String),
}

/// Runs `loams risingwave …` on its own current-thread runtime.
pub fn run(args: &RisingWaveArgs) -> ExitCode {
    // `render` and `apply --dry-run` never connect.
    let result = match &args.command {
        RisingWaveCommand::Render { pipeline } => render(pipeline, args.json),
        RisingWaveCommand::Apply {
            pipeline,
            dry_run: true,
            ..
        } => render(pipeline, args.json),
        _ => match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(connected(args)),
            Err(err) => Err(CliError::Usage(format!("starting the runtime: {err}"))),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("loams risingwave: {err}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|source| CliError::Read {
        path: path.display().to_string(),
        source,
    })
}

fn load_pipeline(path: &Path) -> Result<Pipeline, CliError> {
    let text = read(path)?;
    Ok(Pipeline::parse(&path.to_string_lossy(), &text)?)
}

fn render(path: &Path, json: bool) -> Result<(), CliError> {
    let statements = load_pipeline(path)?.render_redacted()?;
    if json {
        print_json(&statements);
    } else {
        for statement in statements {
            println!("{statement}\n");
        }
    }
    Ok(())
}

async fn connect(url: &str) -> Result<SqlApi, CliError> {
    let mut config: tokio_postgres::Config = url.parse().map_err(SqlApiError::Connect)?;
    let password = ["LOAMS_RISINGWAVE_PASSWORD", "LOAMS_API_KEY"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|value| !value.is_empty()));
    if let Some(password) = password {
        config.password(password);
    }
    config.application_name("loams-risingwave");
    config.connect_timeout(Duration::from_secs(10));
    Ok(SqlApi::connect_config(&config).await?)
}

async fn connected(args: &RisingWaveArgs) -> Result<(), CliError> {
    let api = connect(&args.url).await?;
    match &args.command {
        RisingWaveCommand::Sql { sql, file } => {
            let text = match (sql, file) {
                (Some(sql), None) => sql.clone(),
                (None, Some(path)) => read(path)?,
                _ => {
                    return Err(CliError::Usage(
                        "give the SQL or --file, not both".to_owned(),
                    ));
                }
            };
            let outcomes = api.submit(&text).await?;
            print_outcomes(&outcomes, args.json);
        }
        RisingWaveCommand::List { what, definitions } => {
            let mut all = Vec::new();
            for kind in what.kinds() {
                all.extend(api.list(*kind).await?);
            }
            print_relations(&all, *definitions, args.json);
        }
        RisingWaveCommand::Progress { job, wait, timeout } => {
            let timeout = Duration::from_secs(*timeout);
            if *wait {
                match job {
                    Some(id) => {
                        api.wait_for_job(*id, Duration::from_secs(2), timeout)
                            .await?
                    }
                    None => wait_all(&api, None, timeout).await?,
                }
            }
            let jobs = match job {
                Some(id) => api.job(*id).await?.into_iter().collect(),
                None => api.jobs().await?,
            };
            print_jobs(&jobs, args.json);
        }
        RisingWaveCommand::Cancel { job } => {
            api.cancel_job(*job).await?;
            if args.json {
                print_json(&serde_json::json!({ "cancelled": job }));
            } else {
                println!("cancelled job {job}");
            }
        }
        RisingWaveCommand::ShowCreate { kind, name } => {
            match api.show_create((*kind).into(), name).await? {
                Some(definition) if args.json => {
                    print_json(&serde_json::json!({ "name": name, "definition": definition }))
                }
                Some(definition) => println!("{definition}"),
                None => return Err(CliError::Usage(format!("{name}: not found"))),
            }
        }
        RisingWaveCommand::Apply {
            pipeline,
            wait,
            timeout,
            ..
        } => {
            let pipeline = load_pipeline(pipeline)?;
            let rendered = pipeline.render(&|name: &str| std::env::var(name).ok())?;
            let before: Vec<i64> = api.jobs().await?.into_iter().map(|job| job.id).collect();
            for statement in &rendered {
                eprintln!(
                    "-> {}",
                    statement.display.lines().next().unwrap_or_default()
                );
            }
            let outcomes = api.apply(&rendered).await?;
            if *wait {
                wait_all(&api, Some(&before), Duration::from_secs(*timeout)).await?;
            }
            if args.json {
                print_json(
                    &serde_json::json!({ "statements": rendered.len(), "outcomes": outcomes }),
                );
            } else {
                println!("applied {} statements", rendered.len());
            }
        }
        RisingWaveCommand::Render { .. } => unreachable!("render does not connect"),
    }
    Ok(())
}

/// Waits until every job not in `skip` has finished.
async fn wait_all(api: &SqlApi, skip: Option<&[i64]>, timeout: Duration) -> Result<(), CliError> {
    let start = std::time::Instant::now();
    loop {
        let pending: Vec<JobProgress> = api
            .jobs()
            .await?
            .into_iter()
            .filter(|job| skip.is_none_or(|skip| !skip.contains(&job.id)))
            .collect();
        let Some(first) = pending.first() else {
            return Ok(());
        };
        if start.elapsed() >= timeout {
            return Err(SqlApiError::Timeout {
                id: first.id,
                waited: start.elapsed(),
            }
            .into());
        }
        for job in &pending {
            eprintln!("job {}: {}", job.id, job.progress);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn print_json(value: &impl serde::Serialize) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("loams risingwave: printing JSON: {err}"),
    }
}

fn print_outcomes(outcomes: &[Outcome], json: bool) {
    if json {
        print_json(&outcomes);
        return;
    }
    for outcome in outcomes {
        match outcome {
            Outcome::Rows { columns, rows } => {
                let cells: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|c| c.clone().unwrap_or_else(|| "NULL".to_owned()))
                            .collect()
                    })
                    .collect();
                print!("{}", table(columns, &cells));
                println!("({} rows)", rows.len());
            }
            Outcome::Command { rows_affected } => println!("OK {rows_affected}"),
        }
    }
}

fn print_relations(relations: &[Relation], definitions: bool, json: bool) {
    if json {
        print_json(&relations);
        return;
    }
    let mut columns = vec![
        "kind".to_owned(),
        "id".to_owned(),
        "name".to_owned(),
        "connector".to_owned(),
    ];
    if definitions {
        columns.push("definition".to_owned());
    }
    let rows: Vec<Vec<String>> = relations
        .iter()
        .map(|r| {
            let kind = match r.kind {
                RelationKind::MaterializedView => "mv",
                RelationKind::Source => "source",
                RelationKind::Sink => "sink",
                RelationKind::Table => "table",
            };
            let mut row = vec![
                kind.to_owned(),
                r.id.to_string(),
                format!("{}.{}", r.schema, r.name),
                r.connector.clone().unwrap_or_default(),
            ];
            if definitions {
                row.push(r.definition.replace('\n', " "));
            }
            row
        })
        .collect();
    print!("{}", table(&columns, &rows));
}

fn print_jobs(jobs: &[JobProgress], json: bool) {
    if json {
        print_json(&jobs);
        return;
    }
    if jobs.is_empty() {
        println!("no jobs in progress");
        return;
    }
    let columns = ["id", "progress", "started", "statement"].map(str::to_owned);
    let rows: Vec<Vec<String>> = jobs
        .iter()
        .map(|job| {
            vec![
                job.id.to_string(),
                job.progress.clone(),
                job.initialized_at.clone().unwrap_or_default(),
                job.statement.lines().next().unwrap_or_default().to_owned(),
            ]
        })
        .collect();
    print!("{}", table(&columns, &rows));
}

/// A plain text table: a header, a rule and left-aligned columns.
pub fn table(columns: &[String], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let line = |cells: &[String]| {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = widths.get(i).copied().unwrap_or(0);
                format!("{cell:<width$}")
            })
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    let mut out = line(columns);
    out.push_str(&format!(
        "{}\n",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    ));
    for row in rows {
        out.push_str(&line(row));
    }
    out
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        args: RisingWaveArgs,
    }

    fn parse(args: &[&str]) -> RisingWaveArgs {
        Cli::try_parse_from(std::iter::once("risingwave").chain(args.iter().copied()))
            .unwrap()
            .args
    }

    #[test]
    fn commands_parse() {
        let args = parse(&[
            "--url",
            "postgres://root@127.0.0.1:4566/dev",
            "list",
            "sinks",
            "--json",
        ]);
        assert_eq!(args.url, "postgres://root@127.0.0.1:4566/dev");
        assert!(args.json);
        assert!(matches!(
            args.command,
            RisingWaveCommand::List {
                what: ListWhat::Sinks,
                definitions: false
            }
        ));
        assert!(matches!(
            parse(&["progress", "42", "--wait"]).command,
            RisingWaveCommand::Progress {
                job: Some(42),
                wait: true,
                timeout: 600
            }
        ));
        assert!(matches!(
            parse(&["sql", "SELECT 1"]).command,
            RisingWaveCommand::Sql { .. }
        ));
        assert!(matches!(
            parse(&["apply", "p.toml", "--dry-run"]).command,
            RisingWaveCommand::Apply { dry_run: true, .. }
        ));
        assert!(matches!(
            parse(&["show-create", "mv", "clicks"]).command,
            RisingWaveCommand::ShowCreate { kind: Kind::Mv, .. }
        ));
        assert!(
            Cli::try_parse_from(["risingwave", "sql"]).is_err(),
            "SQL or --file is required"
        );
        assert!(Cli::try_parse_from(["risingwave", "sql", "x", "--file", "y"]).is_err());
    }

    #[test]
    fn tables_align() {
        let text = table(
            &["id".to_owned(), "name".to_owned()],
            &[
                vec!["1".to_owned(), "clicks".to_owned()],
                vec!["1001".to_owned(), "x".to_owned()],
            ],
        );
        assert_eq!(text, "id    name\n----  ------\n1     clicks\n1001  x\n");
    }

    #[test]
    fn list_all_covers_every_kind() {
        assert_eq!(ListWhat::All.kinds().len(), 4);
        assert_eq!(RelationKind::from(Kind::Mv), RelationKind::MaterializedView);
    }
}
