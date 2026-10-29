//! `loams spark …` (design 81 SP-b). **Preview.**
//!
//! ```text
//! loams spark session-url                 the namespace's sc:// URL (no token in it)
//! loams spark submit etl --py s3://b/etl.py -- 2026-10-27
//! loams spark submit rollup --jar s3://b/r.jar --class dev.x.Rollup --executors 4
//! loams spark submit daily --sql q.sql --local --wait     run here (compose, laptop)
//! loams spark status <run> | logs <run> [--attempt n] | cancel <run> | list
//! loams spark render etl --py s3://b/etl.py               the driver pod, for kubectl
//! loams spark schedule nightly --cron "0 2 * * *" --sql s3://b/q.sql
//! loams spark catalog-check --database analytics --table events
//! ```
//!
//! Remote commands talk to the engines facade at `--endpoint`
//! ([`super::client`]); `--local` runs and reads jobs on this host
//! ([`super::local`]), keeping statuses under `--data-dir`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use tokio::sync::watch;
use url::Url;

use super::client::{SparkClient, session_url_for};
use super::local::{LocalConfig, LocalRunner, read_tail};
use super::render::{RenderContext, render_pod};
use super::run::{FileRunStore, RunStatus, RunStore};
use super::runner::Supervisor;
use super::spec::{Backend, Backoff, Resources, RetryPolicy, SparkApp, SparkJobSpec};
use super::{JobsError, RunId};
use crate::catalog::{CatalogConfig, LakekeeperClient, house_namespace, house_table};

/// Installs rustls' ring provider (the binary's choice), once.
pub fn install_crypto() {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}

/// `loams spark`.
#[derive(Debug, Args)]
pub struct SparkArgs {
    /// The Loams HTTP endpoint (the engines facade).
    #[arg(
        long,
        global = true,
        env = "LOAMS_ENDPOINT",
        default_value = "http://127.0.0.1:8080"
    )]
    pub endpoint: String,
    /// The token (an API key or an access token).
    #[arg(long, global = true, env = "LOAMS_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    /// The namespace.
    #[arg(long, global = true, env = "LOAMS_NAMESPACE")]
    pub namespace: Option<String>,
    /// The Spark engine instance (its id `eng_…` or its name).
    #[arg(long, global = true, env = "LOAMS_SPARK_INSTANCE")]
    pub instance: Option<String>,
    /// Print JSON.
    #[arg(long, global = true)]
    pub json: bool,
    /// The command.
    #[command(subcommand)]
    pub command: SparkCommand,
}

/// The `loams spark` commands.
#[derive(Debug, Subcommand)]
pub enum SparkCommand {
    /// Print the namespace's Spark Connect URL (`sc://…`). The token is never
    /// in it: clients send it as gRPC metadata (`loams.spark.session()` does).
    SessionUrl {
        /// Build it for this cloud base domain instead of asking the server
        /// (with --region).
        #[arg(long, requires = "region")]
        domain: Option<String>,
        /// The region, with --domain.
        #[arg(long)]
        region: Option<String>,
    },
    /// Submit a batch job.
    Submit {
        #[command(flatten)]
        job: JobArgs,
        #[command(flatten)]
        local: LocalArgs,
        /// Wait for the run to finish (always, with --local).
        #[arg(long)]
        wait: bool,
    },
    /// A run's status.
    Status {
        /// The run id.
        run: String,
        #[command(flatten)]
        local: LocalArgs,
    },
    /// A run's logs.
    Logs {
        /// The run id.
        run: String,
        /// The attempt (default: the last).
        #[arg(long)]
        attempt: Option<u32>,
        #[command(flatten)]
        local: LocalArgs,
    },
    /// Cancel a run.
    Cancel {
        /// The run id.
        run: String,
    },
    /// The newest runs.
    List {
        /// How many.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(flatten)]
        local: LocalArgs,
    },
    /// Print the Kubernetes objects of a job's first attempt (a `List`), for
    /// review or `kubectl apply -f -`.
    Render {
        #[command(flatten)]
        job: JobArgs,
        /// The Loams namespace id.
        #[arg(long, default_value_t = 0)]
        namespace_id: u64,
        /// The Kubernetes namespace [default: loams-ns-<id>].
        #[arg(long)]
        k8s_namespace: Option<String>,
        /// The Spark Connect URL, for Connect-backend jobs.
        #[arg(long, env = "LOAMS_SPARK_REMOTE")]
        remote: Option<String>,
        #[command(flatten)]
        catalog: CatalogArgs,
    },
    /// Create or replace a schedule: the job runs on a cron rule (UTC).
    Schedule {
        /// The schedule's id.
        id: String,
        /// Five-field cron, UTC.
        #[arg(long)]
        cron: String,
        #[command(flatten)]
        job: JobArgs,
    },
    /// Delete a schedule.
    Unschedule {
        /// The schedule's id.
        id: String,
    },
    /// Check the Iceberg catalog: load a table with vended credentials, and
    /// print its Spark and House names.
    CatalogCheck {
        /// The House database (the Iceberg namespace `ns_<id>.<database>`).
        #[arg(long, default_value = "default")]
        database: String,
        /// The table.
        #[arg(long)]
        table: String,
        /// The Loams namespace id.
        #[arg(long, default_value_t = 0)]
        namespace_id: u64,
        #[command(flatten)]
        catalog: CatalogArgs,
    },
}

/// Run jobs on this host.
#[derive(Debug, Args)]
pub struct LocalArgs {
    /// Run (or read) the job on this host: a child process with the Loams
    /// driver (when --remote/LOAMS_SPARK_REMOTE is set) or $SPARK_HOME's
    /// spark-submit.
    #[arg(long)]
    pub local: bool,
    /// Where local runs are kept.
    #[arg(long, default_value = ".loams/spark/runs")]
    pub data_dir: PathBuf,
    /// The Spark Connect URL for local Connect-mode runs.
    #[arg(long, env = "LOAMS_SPARK_REMOTE")]
    pub remote: Option<String>,
}

/// The Iceberg catalog.
#[derive(Debug, Args)]
pub struct CatalogArgs {
    /// Lakekeeper's Iceberg REST URL.
    #[arg(long, env = "LOAMS_ICEBERG_URI")]
    pub catalog_uri: Option<Url>,
    /// The Lakekeeper warehouse.
    #[arg(long, env = "LOAMS_ICEBERG_WAREHOUSE")]
    pub warehouse: Option<String>,
    /// The S3 endpoint for vended credentials (RustFS).
    #[arg(long, env = "LOAMS_S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,
}

impl CatalogArgs {
    fn config(&self) -> Option<CatalogConfig> {
        let uri = self.catalog_uri.clone()?;
        let warehouse = self.warehouse.clone()?;
        let mut c = CatalogConfig::new(uri, warehouse);
        if let Some(e) = &self.s3_endpoint {
            c = c.with_s3_endpoint(e.clone());
        }
        Some(c)
    }
}

/// The backend, on the command line.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BackendArg {
    /// Connect for PySpark and SQL, Kubernetes for a JAR.
    Auto,
    /// The namespace's Spark Connect server.
    Connect,
    /// Spark on Kubernetes.
    Kubernetes,
    /// This host.
    Local,
}

impl From<BackendArg> for Backend {
    fn from(b: BackendArg) -> Self {
        match b {
            BackendArg::Auto => Self::Auto,
            BackendArg::Connect => Self::Connect,
            BackendArg::Kubernetes => Self::Kubernetes,
            BackendArg::Local => Self::Local,
        }
    }
}

/// A job, on the command line.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("app").required(true).args(["py", "jar", "sql", "spec"])))]
pub struct JobArgs {
    /// The job's name ([a-z][a-z0-9-]*); taken from --spec when given.
    #[arg(required_unless_present = "spec")]
    pub name: Option<String>,
    /// A job spec as JSON (the SDK's shape); the other job flags are ignored.
    #[arg(long)]
    pub spec: Option<PathBuf>,
    /// A PySpark script.
    #[arg(long)]
    pub py: Option<String>,
    /// A JAR (with --class).
    #[arg(long, requires = "class")]
    pub jar: Option<String>,
    /// A SQL file.
    #[arg(long)]
    pub sql: Option<String>,
    /// The JAR's main class.
    #[arg(long)]
    pub class: Option<String>,
    /// An extra Python file (repeatable).
    #[arg(long = "py-file")]
    pub py_files: Vec<String>,
    /// An extra JAR (repeatable).
    #[arg(long = "with-jar")]
    pub jars: Vec<String>,
    /// Spark conf, key=value (repeatable).
    #[arg(long = "conf", value_parser = key_value)]
    pub conf: Vec<(String, String)>,
    /// Driver environment, NAME=value (repeatable).
    #[arg(long = "env", value_parser = key_value)]
    pub env: Vec<(String, String)>,
    /// Driver memory.
    #[arg(long, default_value = "1g")]
    pub driver_memory: String,
    /// Driver cores.
    #[arg(long, default_value_t = 1)]
    pub driver_cores: u32,
    /// Memory per executor.
    #[arg(long, default_value = "1g")]
    pub executor_memory: String,
    /// Cores per executor.
    #[arg(long, default_value_t = 1)]
    pub executor_cores: u32,
    /// Executors (Kubernetes backend).
    #[arg(long, default_value_t = 1)]
    pub executors: u32,
    /// Attempts in all (1 = no retry).
    #[arg(long, default_value_t = 1)]
    pub max_attempts: u32,
    /// The first retry's wait; doubles each time, up to 10 minutes.
    #[arg(long, default_value_t = 30)]
    pub backoff_secs: u64,
    /// Where it runs.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
    pub backend: BackendArg,
    /// The driver image.
    #[arg(long)]
    pub image: Option<String>,
    /// Kill an attempt after this many seconds.
    #[arg(long)]
    pub timeout_secs: Option<u64>,
    /// The application's arguments, after `--`.
    #[arg(last = true)]
    pub args: Vec<String>,
}

fn key_value(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((k, v)) if !k.trim().is_empty() => Ok((k.trim().to_string(), v.to_string())),
        _ => Err(format!("{s:?}: expected key=value")),
    }
}

impl JobArgs {
    /// The spec these flags describe (validated).
    pub fn spec(&self) -> Result<SparkJobSpec, JobsError> {
        if let Some(path) = &self.spec {
            let bytes = std::fs::read(path)?;
            let spec: SparkJobSpec = serde_json::from_slice(&bytes)
                .map_err(|e| JobsError::InvalidSpec(format!("{}: {e}", path.display())))?;
            spec.validate()?;
            return Ok(spec);
        }
        let app = match (&self.py, &self.jar, &self.sql) {
            (Some(file), None, None) => SparkApp::Pyspark {
                file: file.clone(),
                py_files: self.py_files.clone(),
            },
            (None, Some(file), None) => SparkApp::Jar {
                file: file.clone(),
                main_class: self.class.clone().unwrap_or_default(),
                jars: self.jars.clone(),
            },
            (None, None, Some(file)) => SparkApp::Sql { file: file.clone() },
            _ => {
                return Err(JobsError::InvalidSpec(
                    "give one of --py, --jar or --sql".to_string(),
                ));
            }
        };
        let spec = SparkJobSpec {
            name: self.name.clone().unwrap_or_default(),
            app,
            args: self.args.clone(),
            conf: self.conf.iter().cloned().collect::<BTreeMap<_, _>>(),
            resources: Resources {
                driver_cores: self.driver_cores,
                driver_memory: self.driver_memory.clone(),
                executor_cores: self.executor_cores,
                executor_memory: self.executor_memory.clone(),
                executors: self.executors,
            },
            retry: RetryPolicy {
                max_attempts: self.max_attempts,
                backoff: Backoff::Exponential {
                    base_secs: self.backoff_secs,
                    max_secs: 600,
                },
            },
            backend: self.backend.into(),
            image: self.image.clone(),
            env: self.env.iter().cloned().collect(),
            timeout_secs: self.timeout_secs,
        };
        spec.validate()?;
        Ok(spec)
    }
}

/// Runs `loams spark …`.
pub fn run(args: &SparkArgs) -> ExitCode {
    install_crypto();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("loams spark: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(dispatch(args)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("loams spark: {e}");
            ExitCode::FAILURE
        }
    }
}

fn client(args: &SparkArgs) -> Result<SparkClient, JobsError> {
    SparkClient::new(
        &args.endpoint,
        args.token.clone(),
        args.namespace.as_deref().unwrap_or_default(),
        args.instance.as_deref().unwrap_or_default(),
    )
}

fn print_status(args: &SparkArgs, s: &RunStatus) {
    if args.json {
        match serde_json::to_string_pretty(s) {
            Ok(j) => println!("{j}"),
            Err(e) => eprintln!("loams spark: {e}"),
        }
        return;
    }
    println!(
        "{}  {}  {}  attempt {}/{}  {}",
        s.run_id,
        s.name,
        s.state.as_str(),
        s.attempt,
        s.max_attempts,
        s.backend.as_str()
    );
    if let Some(m) = &s.message {
        println!("  {m}");
    }
    for l in &s.logs {
        println!("  logs (attempt {}): {}", l.attempt, l.url);
    }
}

fn exit_for(s: &RunStatus) -> ExitCode {
    match s.state {
        super::RunState::Succeeded => ExitCode::SUCCESS,
        super::RunState::Failed | super::RunState::Canceled => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}

fn local_runner(args: &SparkArgs, local: &LocalArgs) -> LocalRunner {
    let mut config = LocalConfig::from_env(&local.data_dir);
    if local.remote.is_some() {
        config.connect_remote = local.remote.clone();
    }
    if args.token.is_some() {
        config.token = args.token.clone();
    }
    config.loams_endpoint = Some(args.endpoint.clone());
    LocalRunner::new(config)
}

async fn dispatch(args: &SparkArgs) -> Result<ExitCode, JobsError> {
    match &args.command {
        SparkCommand::SessionUrl { domain, region } => {
            let url = match (domain, region, &args.namespace, &args.instance) {
                (Some(d), Some(r), Some(ns), Some(inst)) => {
                    session_url_for(inst, ns, Some((r.as_str(), d.as_str())))
                }
                (Some(_), _, _, _) => {
                    return Err(JobsError::InvalidSpec(
                        "--domain needs --region, --namespace and --instance".to_string(),
                    ));
                }
                _ => client(args)?.session_url().await?,
            };
            if url.contains("token=") {
                return Err(JobsError::Backend(
                    "the server returned a Spark Connect URL with a token in it; refusing to \
                     print it"
                        .to_string(),
                ));
            }
            println!("{url}");
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Submit { job, local, wait } => {
            let spec = job.spec()?;
            if local.local {
                let store = Arc::new(FileRunStore::new(&local.data_dir));
                let runner = Arc::new(local_runner(args, local));
                let sup = Supervisor::new(runner, store, Duration::from_millis(500));
                let status = sup.accept(&spec).await?;
                eprintln!("loams spark: run {} started", status.run_id);
                let (tx, rx) = watch::channel(false);
                let drive = sup.drive(status, &spec, rx);
                tokio::pin!(drive);
                let done = tokio::select! {
                    done = &mut drive => done,
                    _ = tokio::signal::ctrl_c() => {
                        let _ = tx.send(true);
                        drive.await
                    }
                };
                print_status(args, &done);
                return Ok(exit_for(&done));
            }
            let c = client(args)?;
            let mut status = c.submit(&spec).await?;
            if *wait {
                while !status.state.is_terminal() {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    status = c.status(&status.run_id).await?;
                }
                print_status(args, &status);
                return Ok(exit_for(&status));
            }
            print_status(args, &status);
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Status { run, local } => {
            let run = RunId::parse(run)?;
            let status = if local.local {
                FileRunStore::new(&local.data_dir)
                    .get(&run)
                    .await?
                    .ok_or_else(|| JobsError::NotFound(run.to_string()))?
            } else {
                client(args)?.status(&run).await?
            };
            print_status(args, &status);
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Logs {
            run,
            attempt,
            local,
        } => {
            let run = RunId::parse(run)?;
            let text = if local.local {
                let store = FileRunStore::new(&local.data_dir);
                let status = store
                    .get(&run)
                    .await?
                    .ok_or_else(|| JobsError::NotFound(run.to_string()))?;
                let n = attempt.unwrap_or(status.attempt);
                let path = store.run_dir(&run).join(format!("attempt-{n}.log"));
                read_tail(&path, super::local::LOG_TAIL_BYTES).await?
            } else {
                client(args)?.logs(&run, *attempt).await?
            };
            print!("{text}");
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Cancel { run } => {
            let run = RunId::parse(run)?;
            let status = client(args)?.cancel(&run).await?;
            print_status(args, &status);
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::List { limit, local } => {
            let runs = if local.local {
                FileRunStore::new(&local.data_dir).list(*limit).await?
            } else {
                client(args)?.list(*limit).await?
            };
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&runs)
                        .map_err(|e| JobsError::Backend(e.to_string()))?
                );
            } else {
                for s in &runs {
                    println!(
                        "{}  {:<30}  {:<9}  attempt {}/{}  {}",
                        s.run_id,
                        s.name,
                        s.state.as_str(),
                        s.attempt,
                        s.max_attempts,
                        s.updated_at
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Render {
            job,
            namespace_id,
            k8s_namespace,
            remote,
            catalog,
        } => {
            let spec = job.spec()?;
            let mut ctx = RenderContext::new(*namespace_id, RunId::new(), 1);
            if let Some(ns) = k8s_namespace {
                ctx.k8s_namespace = ns.clone();
            }
            if let Some(image) = &spec.image {
                ctx.spark_image = image.clone();
                ctx.driver_image = image.clone();
            }
            ctx.connect_remote = remote.clone();
            ctx.catalog = catalog.config();
            ctx.token_secret = Some(super::SecretRef {
                name: "loams-spark-token".to_string(),
                key: "token".to_string(),
            });
            let out = render_pod(&spec, &ctx);
            let mut items = Vec::new();
            if let Some(cm) = out.config_map {
                items.push(cm);
            }
            items.push(out.pod);
            let list = serde_json::json!({ "apiVersion": "v1", "kind": "List", "items": items });
            println!(
                "{}",
                serde_json::to_string_pretty(&list)
                    .map_err(|e| JobsError::Backend(e.to_string()))?
            );
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Schedule { id, cron, job } => {
            let spec = job.spec()?;
            let schedule = super::schedule::SparkSchedule {
                id: id.clone(),
                cron: cron.clone(),
                job: spec,
            };
            schedule.validate()?;
            let answer = client(args)?
                .schedule(&schedule.id, &schedule.cron, &schedule.job)
                .await?;
            if args.json {
                println!("{answer}");
            } else {
                println!("schedule {id}: {cron} (UTC)");
            }
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::Unschedule { id } => {
            client(args)?.unschedule(id).await?;
            println!("schedule {id} deleted");
            Ok(ExitCode::SUCCESS)
        }
        SparkCommand::CatalogCheck {
            database,
            table,
            namespace_id,
            catalog,
        } => {
            let config = catalog.config().ok_or_else(|| {
                JobsError::InvalidSpec(
                    "set --catalog-uri and --warehouse (or LOAMS_ICEBERG_URI and \
                     LOAMS_ICEBERG_WAREHOUSE)"
                        .to_string(),
                )
            })?;
            let name = config.name.clone();
            let lk = LakekeeperClient::new(config, args.token.clone())
                .map_err(|e| JobsError::Unavailable(e.to_string()))?;
            let ns = house_namespace(*namespace_id, database);
            let creds = lk
                .vended_credentials(&ns, table)
                .await
                .map_err(|e| JobsError::Backend(e.to_string()))?;
            let house = crate::catalog::HouseRef {
                database: database.clone(),
                table: table.clone(),
            };
            println!(
                "spark name:   {}",
                house_table(&name, *namespace_id, database, table)
            );
            println!("house name:   {}", house.clickhouse_name());
            println!("house check:  {}", house.count_sql());
            println!(
                "vended:       {} (scoped to {})",
                if creds.has_s3_session() {
                    "an S3 session"
                } else {
                    "no S3 session (is vending on for this warehouse?)"
                },
                creds.prefix.as_deref().unwrap_or("the table")
            );
            Ok(if creds.has_s3_session() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        spark: SparkArgs,
    }

    fn parse(args: &[&str]) -> SparkArgs {
        let mut all = vec!["loams-spark"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).expect("parse").spark
    }

    #[test]
    fn submit_pyspark_with_args() {
        let a = parse(&[
            "submit",
            "etl",
            "--py",
            "s3://b/etl.py",
            "--conf",
            "spark.sql.shuffle.partitions=8",
            "--max-attempts",
            "3",
            "--",
            "--day",
            "2026-10-27",
        ]);
        let SparkCommand::Submit { job, local, wait } = &a.command else {
            panic!("not submit");
        };
        assert!(!local.local);
        assert!(!wait);
        let spec = job.spec().expect("spec");
        assert_eq!(spec.name, "etl");
        assert_eq!(spec.args, vec!["--day", "2026-10-27"]);
        assert_eq!(spec.conf["spark.sql.shuffle.partitions"], "8");
        assert_eq!(spec.retry.max_attempts, 3);
        assert_eq!(spec.backend.resolve(&spec.app), Backend::Connect);
    }

    #[test]
    fn jar_needs_a_class() {
        let mut all = vec!["loams-spark", "submit", "r", "--jar", "a.jar"];
        assert!(Cli::try_parse_from(all.clone()).is_err());
        all.extend_from_slice(&["--class", "dev.x.Main"]);
        let a = Cli::try_parse_from(all).expect("parse").spark;
        let SparkCommand::Submit { job, .. } = &a.command else {
            panic!("not submit");
        };
        assert_eq!(
            job.spec()
                .expect("spec")
                .backend
                .resolve(&job.spec().expect("spec").app),
            Backend::Kubernetes
        );
    }

    #[test]
    fn one_app_only() {
        assert!(
            Cli::try_parse_from(["x", "submit", "a", "--py", "a.py", "--sql", "a.sql"]).is_err()
        );
        assert!(Cli::try_parse_from(["x", "submit", "a"]).is_err());
    }

    #[test]
    fn spec_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("job.json");
        std::fs::write(
            &path,
            r#"{"name": "daily", "app": {"kind": "sql", "file": "s3://b/q.sql"}}"#,
        )
        .expect("write");
        let a = parse(&["submit", "--spec", path.to_str().expect("utf-8")]);
        let SparkCommand::Submit { job, .. } = &a.command else {
            panic!("not submit");
        };
        assert_eq!(job.spec().expect("spec").name, "daily");
    }

    #[test]
    fn bad_conf_is_refused() {
        assert!(
            Cli::try_parse_from(["x", "submit", "a", "--py", "a.py", "--conf", "novalue"]).is_err()
        );
        let a = parse(&[
            "submit",
            "a",
            "--py",
            "a.py",
            "--conf",
            "spark.master=local",
        ]);
        let SparkCommand::Submit { job, .. } = &a.command else {
            panic!("not submit");
        };
        assert!(matches!(job.spec(), Err(JobsError::InvalidSpec(_))));
    }

    #[test]
    fn local_flags() {
        let a = parse(&[
            "submit",
            "a",
            "--sql",
            "q.sql",
            "--local",
            "--data-dir",
            "/d",
            "--remote",
            "sc://127.0.0.1:15002",
        ]);
        let SparkCommand::Submit { local, .. } = &a.command else {
            panic!("not submit");
        };
        assert!(local.local);
        assert_eq!(local.data_dir, PathBuf::from("/d"));
        assert_eq!(local.remote.as_deref(), Some("sc://127.0.0.1:15002"));
    }
}
