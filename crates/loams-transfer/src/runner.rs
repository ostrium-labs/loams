//! `loams-transfer runner` (§85 §11, D2439–D2442): claims runs of the jobs
//! its operator configured, runs them, and streams their logs and artifacts
//! back through the run's journal.
//!
//! - **Named jobs only** (D2440): `jobs.toml` maps a job name to its argv,
//!   working directory, timeout, artifact globs and the params it accepts.
//!   A run's params reach the process only as environment variables named in
//!   that allowlist; nothing from the request becomes argv.
//! - **The spool** is a local file: the child's stdout and stderr go there,
//!   and an appender commits it to the server in pieces of up to 1 MiB, at
//!   least every second. A 409 with the committed length moves the appender
//!   to that offset (D2441), so appends are idempotent and memory stays
//!   bounded whatever the log size.
//! - **The lease** is renewed every 10 s; a cancel flag kills the job.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use serde::Deserialize;
use tokio::process::Command;

use crate::api::{Client, ClientError};
use crate::client::local::{BLOCK, read_exact_at};
use crate::client::{StateDir, TransferError, UploadOpts, backoff};
use crate::run::{ClaimedRun, RunOutcome};

/// One configured job.
#[derive(Clone, Debug, Deserialize)]
pub struct JobSpec {
    /// The program and its arguments, exactly as run.
    pub argv: Vec<String>,
    /// The working directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The params a run may set, as environment variables.
    #[serde(default)]
    pub env_allow: Vec<String>,
    /// Fixed environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Seconds before the job is killed and marked `timed_out`.
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
    /// Globs, relative to `cwd`, of files to upload as artifacts.
    #[serde(default)]
    pub artifacts: Vec<String>,
}

fn default_timeout() -> u64 {
    3600
}

/// `jobs.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct JobsFile {
    pub jobs: BTreeMap<String, JobSpec>,
}

impl JobsFile {
    pub fn load(path: &Path) -> Result<Self, TransferError> {
        let text = std::fs::read_to_string(path)?;
        let f: JobsFile = toml::from_str(&text)
            .map_err(|e| TransferError::Local(format!("{}: {e}", path.display())))?;
        for (name, j) in &f.jobs {
            if j.argv.is_empty() {
                return Err(TransferError::Local(format!(
                    "job {name} has an empty argv"
                )));
            }
        }
        Ok(f)
    }
}

/// The environment a run gets: the job's fixed variables, plus the run's
/// params whose names the job allows. Anything else is refused (D2440).
pub fn run_env(
    job: &JobSpec,
    params: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = job.env.clone();
    for (k, v) in params {
        if !job.env_allow.iter().any(|a| a == k) {
            return Err(format!("param {k} is not in the job's env_allow"));
        }
        env.insert(k.clone(), v.clone());
    }
    Ok(env)
}

/// Runner settings.
#[derive(Clone, Debug)]
pub struct RunnerOpts {
    pub runner_id: String,
    /// How often to look for a queued run.
    pub poll: Duration,
    /// How often to renew the run's lease and check for a cancel.
    pub renew_every: Duration,
    /// The most bytes in one log append.
    pub max_append: usize,
    /// How often the appender commits what the spool holds.
    pub flush_every: Duration,
}

impl Default for RunnerOpts {
    fn default() -> Self {
        RunnerOpts {
            runner_id: format!("runner-{}", crate::keys::new_id()),
            poll: Duration::from_secs(2),
            renew_every: Duration::from_secs(10),
            max_append: BLOCK,
            flush_every: Duration::from_secs(1),
        }
    }
}

/// Claims and runs jobs until `stop` is set.
pub async fn serve(
    client: &Client,
    state: &StateDir,
    jobs: &JobsFile,
    opts: &RunnerOpts,
    stop: Arc<AtomicBool>,
) -> Result<(), TransferError> {
    let names: Vec<String> = jobs.jobs.keys().cloned().collect();
    while !stop.load(Ordering::SeqCst) {
        match client.claim(&opts.runner_id, &names).await {
            Ok(Some(run)) => {
                if let Err(e) = run_one(client, state, jobs, opts, &run).await {
                    tracing::warn!(rid = %run.rid, error = %e, "run ended with an error");
                }
            }
            Ok(None) => tokio::time::sleep(opts.poll).await,
            Err(e) if e.is_transient() => {
                client.reset().await;
                tokio::time::sleep(opts.poll).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

async fn retry_client<T, F, Fut>(mut f: F) -> Result<T, TransferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ClientError>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_transient() && attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(backoff(attempt)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn finish(
    client: &Client,
    run: &ClaimedRun,
    outcome: RunOutcome,
    code: Option<i32>,
    msg: Option<String>,
) -> Result<(), TransferError> {
    retry_client(|| client.finish_run(&run.rid, run.epoch, outcome, code, msg.clone())).await?;
    Ok(())
}

/// Runs one claimed run to its end and returns how it ended.
pub async fn run_one(
    client: &Client,
    state: &StateDir,
    jobs: &JobsFile,
    opts: &RunnerOpts,
    run: &ClaimedRun,
) -> Result<RunOutcome, TransferError> {
    let Some(job) = jobs.jobs.get(&run.job) else {
        let msg = format!("this runner has no job {}", run.job);
        finish(client, run, RunOutcome::Failed, None, Some(msg)).await?;
        return Ok(RunOutcome::Failed);
    };
    let env = match run_env(job, &run.params) {
        Ok(e) => e,
        Err(msg) => {
            finish(client, run, RunOutcome::Failed, None, Some(msg)).await?;
            return Ok(RunOutcome::Failed);
        }
    };
    let spool_dir = state.root().join("runner");
    std::fs::create_dir_all(&spool_dir)?;
    let spool = spool_dir.join(format!("{}.log", run.rid));
    let out = File::create(&spool)?;
    let err = out.try_clone()?;
    let mut cmd = Command::new(&job.argv[0]);
    cmd.args(&job.argv[1..])
        .env_clear()
        .envs(base_env())
        .envs(&env)
        .env("LOAMS_RUN_ID", &run.rid)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .kill_on_drop(true);
    if let Some(cwd) = &job.cwd {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            finish(
                client,
                run,
                RunOutcome::Failed,
                None,
                Some(format!("spawn: {e}")),
            )
            .await?;
            return Ok(RunOutcome::Failed);
        }
    };

    let done = Arc::new(AtomicBool::new(false));
    let appender = tokio::spawn(append_spool(
        client.clone(),
        run.rid.clone(),
        run.epoch,
        run.log_bytes,
        spool.clone(),
        Arc::clone(&done),
        opts.clone(),
    ));
    let deadline = tokio::time::sleep(Duration::from_secs(job.timeout_s));
    tokio::pin!(deadline);
    let mut renew = tokio::time::interval(opts.renew_every);
    renew.tick().await;
    let ended = loop {
        tokio::select! {
            status = child.wait() => {
                let status = status?;
                let code = status.code();
                break if status.success() {
                    (RunOutcome::Succeeded, code, None)
                } else {
                    (RunOutcome::Failed, code, None)
                };
            }
            () = &mut deadline => {
                let _ = child.kill().await;
                break (RunOutcome::TimedOut, None, Some(format!("killed after {} s", job.timeout_s)));
            }
            _ = renew.tick() => {
                match client.renew_run(&run.rid, &opts.runner_id, run.epoch).await {
                    Ok(true) => {
                        let _ = child.kill().await;
                        break (RunOutcome::Cancelled, None, Some("cancelled".into()));
                    }
                    Ok(false) => {}
                    Err(e) if e.code() == Some("lease_lost") => {
                        let _ = child.kill().await;
                        appender.abort();
                        return Err(e.into());
                    }
                    Err(_) => {}
                }
            }
        }
    };
    // The appender commits the rest of the spool, then stops.
    done.store(true, Ordering::SeqCst);
    appender
        .await
        .map_err(|e| TransferError::Local(e.to_string()))??;
    let cwd = job.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
    upload_artifacts(client, state, run, job, &cwd).await?;
    let (outcome, code, msg) = ended;
    finish(client, run, outcome, code, msg).await?;
    let _ = std::fs::remove_file(&spool);
    Ok(outcome)
}

/// A minimal environment for jobs: a few variables from the runner and
/// nothing else of its own, so its token never reaches a job.
fn base_env() -> Vec<(String, String)> {
    ["PATH", "HOME", "LANG", "TMPDIR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}

async fn append_spool(
    client: Client,
    rid: String,
    epoch: u64,
    start: u64,
    spool: PathBuf,
    done: Arc<AtomicBool>,
    opts: RunnerOpts,
) -> Result<(), TransferError> {
    // Spool byte `n` is log byte `start + n`: a fresh claim starts at 0.
    let mut committed = start;
    let file = Arc::new(File::open(&spool)?);
    let mut attempt = 0u32;
    loop {
        // Read `done` before the length: once set, the child has exited, so
        // the length read after it is final.
        let finished = done.load(Ordering::SeqCst);
        let len = file.metadata()?.len() + start;
        if committed < len {
            let n = (len - committed).min(opts.max_append as u64) as usize;
            let f = Arc::clone(&file);
            let at = committed - start;
            let data = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0u8; n];
                read_exact_at(&f, &mut buf, at).map(|()| Bytes::from(buf))
            })
            .await
            .map_err(|e| TransferError::Local(e.to_string()))??;
            match client.append_log(&rid, epoch, committed, data).await {
                Ok(c) => {
                    committed = c;
                    attempt = 0;
                    continue;
                }
                Err(ClientError::Api { body, .. })
                    if body.code == "offset_mismatch" && body.committed.is_some() =>
                {
                    committed = body.committed.unwrap_or(committed);
                    continue;
                }
                Err(e) if e.is_transient() && attempt < 10 => {
                    attempt += 1;
                    client.reset().await;
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        if finished {
            return Ok(());
        }
        tokio::time::sleep(opts.flush_every).await;
    }
}

async fn upload_artifacts(
    client: &Client,
    state: &StateDir,
    run: &ClaimedRun,
    job: &JobSpec,
    cwd: &Path,
) -> Result<(), TransferError> {
    if job.artifacts.is_empty() {
        return Ok(());
    }
    let root = cwd.canonicalize()?;
    for pattern in &job.artifacts {
        let full = root.join(pattern);
        let paths = glob::glob(&full.to_string_lossy())
            .map_err(|e| TransferError::Local(format!("artifact glob {pattern}: {e}")))?;
        for p in paths.flatten() {
            // Artifacts stay inside the job's directory.
            let Ok(p) = p.canonicalize() else { continue };
            if !p.starts_with(&root) || !p.is_file() {
                continue;
            }
            let name = p
                .strip_prefix(&root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default();
            let opts = UploadOpts {
                min_chunk: 8 << 20,
                ..UploadOpts::default()
            };
            let v = crate::client::upload(client, state, &p, None, &opts, |_| {}).await?;
            retry_client(|| client.add_artifact(&run.rid, run.epoch, &name, &v.tid, v.size))
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(allow: &[&str]) -> JobSpec {
        JobSpec {
            argv: vec!["true".into()],
            cwd: None,
            env_allow: allow.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            timeout_s: 10,
            artifacts: vec![],
        }
    }

    #[test]
    fn params_only_via_env_allowlist() {
        let mut p = BTreeMap::new();
        p.insert("REF".to_string(), "main".to_string());
        assert_eq!(run_env(&job(&["REF"]), &p).expect("allowed")["REF"], "main");
        assert!(run_env(&job(&[]), &p).is_err());
    }

    #[test]
    fn jobs_file_parses_with_defaults() {
        let f: JobsFile = toml::from_str(
            "[jobs.ci]\nargv = [\"sh\", \"-c\", \"echo hi\"]\nenv_allow = [\"REF\"]\nartifacts = [\"out/*.txt\"]\n",
        )
        .expect("toml");
        assert_eq!(f.jobs["ci"].timeout_s, 3600);
        assert_eq!(f.jobs["ci"].env_allow, ["REF"]);
    }
}
