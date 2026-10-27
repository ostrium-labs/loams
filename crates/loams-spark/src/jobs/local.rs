//! The local runner (D2002): an attempt is a child process on this host, for
//! `loams dev`, compose and laptops.
//!
//! * PySpark and SQL jobs with a Spark Connect endpoint (the compose `spark`
//!   profile, SP-a's proxy, or any `sc://` URL) run the Loams driver,
//!   `python -m loams.spark.driver`, which needs only `loams[spark]` (the
//!   Connect-only PySpark client, no JVM).
//! * Everything else runs `$SPARK_HOME/bin/spark-submit --master local[*]`,
//!   with the same conf the Kubernetes backend renders, minus the Kubernetes
//!   keys.
//!
//! Each attempt's stdout and stderr go to `<runs>/<run>/attempt-<n>.log`,
//! which is the attempt's log link.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::JobsError;
use super::render::{
    Master, RenderContext, SQL_RUNNER, TOKEN_ENV, connect_driver_argv, connect_driver_env,
    spark_submit_argv,
};
use super::run::{AttemptOutcome, LogLink};
use super::runner::{Attempt, AttemptHandle, AttemptPhase, Runner};
use super::spec::{Backend, SparkApp};
use crate::catalog::CatalogConfig;

/// The most of a log [`LocalRunner::logs`] returns: its last 1 MiB.
pub const LOG_TAIL_BYTES: u64 = 1 << 20;

/// How the local runner runs jobs.
#[derive(Clone)]
pub struct LocalConfig {
    /// Run directories go under here (`<data_dir>/spark/runs`).
    pub runs_dir: PathBuf,
    /// Apache Spark's home, for `spark-submit` (default `$SPARK_HOME`).
    pub spark_home: Option<PathBuf>,
    /// The Python with `loams[spark]` installed (default `python3`).
    pub python: String,
    /// The Spark Connect endpoint for Connect-mode jobs (`sc://…`).
    pub connect_remote: Option<String>,
    /// The Loams HTTP endpoint the driver's SDK uses.
    pub loams_endpoint: Option<String>,
    /// The Iceberg catalog.
    pub catalog: Option<CatalogConfig>,
    /// The token the driver authenticates with; passed in the child's
    /// environment only.
    pub token: Option<String>,
    /// The namespace id.
    pub namespace_id: u64,
    /// `local[n]` threads for `spark-submit` (`None` = all cores).
    pub threads: Option<u32>,
}

impl std::fmt::Debug for LocalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConfig")
            .field("runs_dir", &self.runs_dir)
            .field("spark_home", &self.spark_home)
            .field("python", &self.python)
            .field("connect_remote", &self.connect_remote)
            .field("loams_endpoint", &self.loams_endpoint)
            .field("catalog", &self.catalog)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("namespace_id", &self.namespace_id)
            .field("threads", &self.threads)
            .finish()
    }
}

impl LocalConfig {
    /// Defaults from the environment: `SPARK_HOME`, `LOAMS_SPARK_REMOTE`,
    /// `LOAMS_ENDPOINT`, `LOAMS_TOKEN` (else `LOAMS_API_KEY`).
    pub fn from_env(runs_dir: impl Into<PathBuf>) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            runs_dir: runs_dir.into(),
            spark_home: var("SPARK_HOME").map(PathBuf::from),
            python: var("LOAMS_SPARK_PYTHON").unwrap_or_else(|| "python3".to_string()),
            connect_remote: var("LOAMS_SPARK_REMOTE"),
            loams_endpoint: var("LOAMS_ENDPOINT"),
            catalog: None,
            token: var(TOKEN_ENV).or_else(|| var("LOAMS_API_KEY")),
            namespace_id: var("LOAMS_NAMESPACE")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            threads: None,
        }
    }
}

/// How one attempt is started: the argv and the environment.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalCommand {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// Environment added to the runner's own.
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for LocalCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env: BTreeMap<&String, &str> = self
            .env
            .iter()
            .map(|(k, v)| {
                let shown = if k == TOKEN_ENV {
                    "<redacted>"
                } else {
                    v.as_str()
                };
                (k, shown)
            })
            .collect();
        f.debug_struct("LocalCommand")
            .field("argv", &self.argv)
            .field("env", &env)
            .finish()
    }
}

/// Runs attempts as child processes.
#[derive(Debug)]
pub struct LocalRunner {
    config: LocalConfig,
    children: Mutex<HashMap<String, Child>>,
}

impl LocalRunner {
    /// A runner with `config`.
    pub fn new(config: LocalConfig) -> Self {
        Self {
            config,
            children: Mutex::new(HashMap::new()),
        }
    }

    /// The attempt's log file.
    pub fn log_path(&self, attempt: &Attempt) -> PathBuf {
        self.config
            .runs_dir
            .join(attempt.run_id.as_str())
            .join(format!("attempt-{}.log", attempt.attempt))
    }

    fn sql_runner_path(&self) -> PathBuf {
        self.config.runs_dir.join("sql_runner.py")
    }

    /// Whether `attempt` runs over Spark Connect here: a PySpark or SQL job,
    /// an endpoint configured, and the job not pinned to JVM Spark.
    pub fn uses_connect(&self, attempt: &Attempt) -> bool {
        let spec = &attempt.spec;
        self.config.connect_remote.is_some()
            && !matches!(spec.app, SparkApp::Jar { .. })
            && spec.backend != Backend::Kubernetes
    }

    /// The command for an attempt (pure, for tests and `--dry-run`).
    pub fn command(&self, attempt: &Attempt) -> Result<LocalCommand, JobsError> {
        let spec = &attempt.spec;
        let mut ctx = RenderContext::new(
            self.config.namespace_id,
            attempt.run_id.clone(),
            attempt.attempt,
        );
        ctx.connect_remote = self.config.connect_remote.clone();
        ctx.loams_endpoint = self.config.loams_endpoint.clone();
        ctx.catalog = self.config.catalog.clone();
        let mut env = BTreeMap::new();
        if let Some(t) = &self.config.token {
            env.insert(TOKEN_ENV.to_string(), t.clone());
        }
        let argv = if self.uses_connect(attempt) {
            env.extend(connect_driver_env(spec, &ctx));
            connect_driver_argv(spec, &self.config.python)
        } else {
            if spec.app.file().starts_with("s3://") && self.config.catalog.is_none() {
                tracing::debug!(
                    file = spec.app.file(),
                    "an s3:// app on local Spark needs hadoop-aws on its classpath"
                );
            }
            let home = self.config.spark_home.as_ref().ok_or_else(|| {
                JobsError::Unavailable(
                    "set SPARK_HOME to run JVM Spark locally, or LOAMS_SPARK_REMOTE for a \
                     Spark Connect endpoint"
                        .to_string(),
                )
            })?;
            let submit = home.join("bin").join("spark-submit");
            // The catalog token is literal here: the conf is in this
            // process's argv only, never in a stored spec. `$(LOAMS_TOKEN)`
            // is Kubernetes syntax and a shell would not expand it.
            let mut argv = spark_submit_argv(
                spec,
                &ctx,
                &Master::Local {
                    threads: self.config.threads,
                },
                &submit.to_string_lossy(),
                &self.sql_runner_path().to_string_lossy(),
            );
            if let Some(t) = &self.config.token {
                for a in &mut argv {
                    if a.contains("$(LOAMS_TOKEN)") {
                        *a = a.replace("$(LOAMS_TOKEN)", t);
                    }
                }
            }
            argv
        };
        env.extend(spec.env.clone());
        Ok(LocalCommand { argv, env })
    }
}

#[async_trait]
impl Runner for LocalRunner {
    async fn start(&self, attempt: &Attempt) -> Result<AttemptHandle, JobsError> {
        let cmd = self.command(attempt)?;
        let log_path = self.log_path(attempt);
        if let Some(dir) = log_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        if matches!(attempt.spec.app, SparkApp::Sql { .. }) && !self.uses_connect(attempt) {
            tokio::fs::write(self.sql_runner_path(), SQL_RUNNER).await?;
        }
        let log = std::fs::File::create(&log_path)?;
        let err = log.try_clone()?;
        let (program, args) = cmd
            .argv
            .split_first()
            .ok_or_else(|| JobsError::Backend("empty command".to_string()))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(&cmd.env)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|e| JobsError::Unavailable(format!("could not start {program}: {e}")))?;
        let id = format!("{}/{}", attempt.run_id, attempt.attempt);
        self.children.lock().await.insert(id.clone(), child);
        Ok(AttemptHandle {
            id,
            log: LogLink {
                attempt: attempt.attempt,
                url: format!("file://{}", log_path.display()),
            },
        })
    }

    async fn poll(&self, handle: &AttemptHandle) -> Result<AttemptPhase, JobsError> {
        let mut children = self.children.lock().await;
        let child = children
            .get_mut(&handle.id)
            .ok_or_else(|| JobsError::NotFound(handle.id.clone()))?;
        match child.try_wait()? {
            None => Ok(AttemptPhase::Running),
            Some(status) => {
                children.remove(&handle.id);
                Ok(AttemptPhase::Done(if status.success() {
                    AttemptOutcome::Succeeded
                } else {
                    AttemptOutcome::Failed {
                        exit_code: status.code(),
                        message: format!("the driver exited with {status}"),
                    }
                }))
            }
        }
    }

    async fn cancel(&self, handle: &AttemptHandle) -> Result<(), JobsError> {
        if let Some(mut child) = self.children.lock().await.remove(&handle.id) {
            child.start_kill()?;
            let _ = child.wait().await;
        }
        Ok(())
    }

    async fn logs(&self, handle: &AttemptHandle) -> Result<String, JobsError> {
        let path =
            handle.log.url.strip_prefix("file://").ok_or_else(|| {
                JobsError::Backend(format!("not a local log: {}", handle.log.url))
            })?;
        read_tail(std::path::Path::new(path), LOG_TAIL_BYTES).await
    }
}

/// The last `max` bytes of a file, as text.
pub async fn read_tail(path: &std::path::Path, max: u64) -> Result<String, JobsError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(JobsError::NotFound(path.display().to_string()));
        }
        Err(e) => return Err(e.into()),
    };
    let len = file.metadata().await?.len();
    if len > max {
        file.seek(std::io::SeekFrom::Start(len - max)).await?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::jobs::RunId;
    use crate::jobs::spec::SparkJobSpec;

    fn config(dir: &std::path::Path) -> LocalConfig {
        LocalConfig {
            runs_dir: dir.to_path_buf(),
            spark_home: Some(PathBuf::from("/opt/spark")),
            python: "python3".into(),
            connect_remote: None,
            loams_endpoint: None,
            catalog: None,
            token: Some("tok".into()),
            namespace_id: 7,
            threads: Some(2),
        }
    }

    fn attempt(app: SparkApp) -> Attempt {
        Attempt {
            run_id: RunId("01jbzq3w4y5x6v7t8s9r0pqmnk".into()),
            attempt: 1,
            spec: SparkJobSpec::new("etl", app),
        }
    }

    #[test]
    fn pyspark_with_a_remote_uses_the_connect_driver() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = config(dir.path());
        c.connect_remote = Some("sc://127.0.0.1:15002".into());
        let runner = LocalRunner::new(c);
        let a = attempt(SparkApp::Pyspark {
            file: "jobs/etl.py".into(),
            py_files: vec![],
        });
        let cmd = runner.command(&a).expect("command");
        assert_eq!(&cmd.argv[..3], &["python3", "-m", "loams.spark.driver"]);
        assert_eq!(cmd.env["LOAMS_SPARK_REMOTE"], "sc://127.0.0.1:15002");
        assert_eq!(cmd.env[TOKEN_ENV], "tok");
        assert!(!format!("{cmd:?}").contains("tok\""), "{cmd:?}");
    }

    #[test]
    fn jar_uses_local_spark_submit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = config(dir.path());
        c.connect_remote = Some("sc://127.0.0.1:15002".into());
        let runner = LocalRunner::new(c);
        let a = attempt(SparkApp::Jar {
            file: "a.jar".into(),
            main_class: "Main".into(),
            jars: vec![],
        });
        let cmd = runner.command(&a).expect("command");
        assert_eq!(cmd.argv[0], "/opt/spark/bin/spark-submit");
        assert_eq!(cmd.argv[2], "local[2]");
        assert!(cmd.argv.iter().all(|a| !a.contains("spark.kubernetes.")));
    }

    #[test]
    fn jvm_without_spark_home_is_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = config(dir.path());
        c.spark_home = None;
        let runner = LocalRunner::new(c);
        let a = attempt(SparkApp::Sql {
            file: "q.sql".into(),
        });
        assert!(matches!(runner.command(&a), Err(JobsError::Unavailable(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_process_and_reports_its_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A fake spark-submit: prints its arguments and exits with the code
        // its last argument names.
        let home = dir.path().join("spark");
        std::fs::create_dir_all(home.join("bin")).expect("mkdir");
        let script = home.join("bin").join("spark-submit");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"submit $*\"\neval \"exit \\${$#}\"\n",
        )
        .expect("write");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let mut c = config(dir.path());
        c.spark_home = Some(home);
        let runner = LocalRunner::new(c);
        for (code, ok) in [("0", true), ("3", false)] {
            let mut a = attempt(SparkApp::Jar {
                file: "a.jar".into(),
                main_class: "Main".into(),
                jars: vec![],
            });
            a.spec.args = vec![code.into()];
            let h = runner.start(&a).await.expect("start");
            let phase = loop {
                match runner.poll(&h).await.expect("poll") {
                    AttemptPhase::Done(o) => break o,
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            if ok {
                assert_eq!(phase, AttemptOutcome::Succeeded);
            } else {
                assert!(
                    matches!(
                        phase,
                        AttemptOutcome::Failed {
                            exit_code: Some(3),
                            ..
                        }
                    ),
                    "{phase:?}"
                );
            }
            let log = runner.logs(&h).await.expect("logs");
            assert!(log.contains("submit --master local[2]"), "{log}");
        }
    }
}
