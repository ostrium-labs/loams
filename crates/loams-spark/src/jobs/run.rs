//! A run's status, its state machine, and where statuses are kept (D2003).
//!
//! ```text
//! Pending ─▶ Submitted ─▶ Running ─▶ Succeeded
//!                │            │
//!                └────────────┴─▶ (attempt failed) ─▶ Retrying ─▶ Submitted …
//!                                        └─(no attempts left)─▶ Failed
//! any non-terminal state ─(cancel)─▶ Canceled
//! ```
//!
//! The transitions are pure functions of the status, the policy and the time,
//! so they are tested without a runner. Statuses are kept in a [`RunStore`]:
//! in memory, or one JSON file per run under the local runner's data
//! directory. The managed service keeps them in Loams Postgres (design 81's
//! common shape) through the same trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex;

use super::spec::{Backend, RetryPolicy, SparkJobSpec};
use super::{JobsError, RunId};

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Accepted, no attempt started yet.
    Pending,
    /// An attempt was handed to the backend (a pod created, a process
    /// spawned) and is not running yet.
    Submitted,
    /// An attempt is running.
    Running,
    /// An attempt failed; the next starts after the backoff.
    Retrying,
    /// An attempt succeeded.
    Succeeded,
    /// The last attempt failed.
    Failed,
    /// Canceled by a user.
    Canceled,
}

impl RunState {
    /// No more transitions.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }

    /// ENG-UI's `SparkJob.state` (D2015): `Queued`, `Running`, `Succeeded`,
    /// `Failed` or `Cancelled`. Retrying counts as running: the run is not over.
    pub fn facade_state(self) -> &'static str {
        match self {
            Self::Pending | Self::Submitted => "Queued",
            Self::Running | Self::Retrying => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Canceled => "Cancelled",
        }
    }

    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Submitted => "submitted",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

/// Where one attempt's logs are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLink {
    /// The attempt, 1-based.
    pub attempt: u32,
    /// A Kubernetes API path (`/api/v1/namespaces/<ns>/pods/<pod>/log`), a
    /// `file://` URL (local runner), or the facade's own logs URL.
    pub url: String,
}

/// How an attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Exit status 0.
    Succeeded,
    /// It failed.
    Failed {
        /// The driver's exit code, when there was one.
        exit_code: Option<i32>,
        /// What failed, for the status.
        message: String,
    },
    /// It was canceled (or killed for its timeout, with `timed_out`).
    Canceled {
        /// The run's `timeout_secs` passed.
        timed_out: bool,
    },
}

/// What the supervisor does after an attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Start the next attempt after this wait.
    Retry(Duration),
    /// The run is over.
    Done,
}

/// A run's status: the record the CLI, the SDK and the console show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatus {
    /// The run.
    pub run_id: RunId,
    /// The job's name.
    pub name: String,
    /// The state.
    pub state: RunState,
    /// The current (or last) attempt; 0 before the first.
    pub attempt: u32,
    /// The policy's attempts.
    pub max_attempts: u32,
    /// The resolved backend.
    pub backend: Backend,
    /// The last failure, or why it was canceled.
    #[serde(default)]
    pub message: Option<String>,
    /// The last attempt's exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// One link per attempt that started.
    #[serde(default)]
    pub logs: Vec<LogLink>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
    /// When the next attempt starts (RFC 3339), while `retrying`.
    #[serde(default)]
    pub next_attempt_at: Option<String>,
}

fn stamp(t: OffsetDateTime) -> String {
    t.format(&Rfc3339)
        .unwrap_or_else(|_| t.unix_timestamp().to_string())
}

impl RunStatus {
    /// A new, pending run of `spec`.
    pub fn new(run_id: RunId, spec: &SparkJobSpec, now: OffsetDateTime) -> Self {
        let t = stamp(now);
        Self {
            run_id,
            name: spec.name.clone(),
            state: RunState::Pending,
            attempt: 0,
            max_attempts: spec.retry.max_attempts,
            backend: spec.backend.resolve(&spec.app),
            message: None,
            exit_code: None,
            logs: Vec::new(),
            created_at: t.clone(),
            updated_at: t,
            next_attempt_at: None,
        }
    }

    /// Attempt `attempt` was handed to the backend.
    pub fn submitted(&mut self, attempt: u32, log: LogLink, now: OffsetDateTime) {
        self.state = RunState::Submitted;
        self.attempt = attempt;
        self.next_attempt_at = None;
        self.logs.retain(|l| l.attempt != attempt);
        self.logs.push(log);
        self.updated_at = stamp(now);
    }

    /// The current attempt is running.
    pub fn running(&mut self, now: OffsetDateTime) {
        if !self.state.is_terminal() && self.state != RunState::Running {
            self.state = RunState::Running;
            self.updated_at = stamp(now);
        }
    }

    /// The current attempt ended: decides whether another one runs.
    pub fn attempt_ended(
        &mut self,
        outcome: AttemptOutcome,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Next {
        self.updated_at = stamp(now);
        match outcome {
            AttemptOutcome::Succeeded => {
                self.state = RunState::Succeeded;
                self.exit_code = Some(0);
                self.message = None;
                Next::Done
            }
            AttemptOutcome::Canceled { timed_out } => {
                if timed_out && self.attempt < policy.max_attempts {
                    let wait = policy.backoff.delay(self.attempt);
                    self.retry_after(wait, now, "attempt timed out".to_string());
                    return Next::Retry(wait);
                }
                self.state = if timed_out {
                    RunState::Failed
                } else {
                    RunState::Canceled
                };
                self.message = Some(if timed_out {
                    "timed out".to_string()
                } else {
                    "canceled".to_string()
                });
                Next::Done
            }
            AttemptOutcome::Failed { exit_code, message } => {
                self.exit_code = exit_code;
                if self.attempt < policy.max_attempts {
                    let wait = policy.backoff.delay(self.attempt);
                    self.retry_after(wait, now, message);
                    Next::Retry(wait)
                } else {
                    self.state = RunState::Failed;
                    self.message = Some(message);
                    Next::Done
                }
            }
        }
    }

    fn retry_after(&mut self, wait: Duration, now: OffsetDateTime, message: String) {
        self.state = RunState::Retrying;
        self.message = Some(message);
        let at = now + time::Duration::try_from(wait).unwrap_or(time::Duration::ZERO);
        self.next_attempt_at = Some(stamp(at));
    }

    /// Canceled before or between attempts.
    pub fn canceled(&mut self, now: OffsetDateTime) {
        if !self.state.is_terminal() {
            self.state = RunState::Canceled;
            self.message = Some("canceled".to_string());
            self.next_attempt_at = None;
            self.updated_at = stamp(now);
        }
    }

    /// A backend error that ends the run (the pod could not be created, the
    /// binary is missing): no retry, since another attempt would hit it too.
    pub fn errored(&mut self, err: &JobsError, now: OffsetDateTime) {
        self.state = RunState::Failed;
        self.message = Some(err.to_string());
        self.next_attempt_at = None;
        self.updated_at = stamp(now);
    }
}

/// Where statuses are kept.
#[async_trait]
pub trait RunStore: Send + Sync + std::fmt::Debug {
    /// Writes a status (insert or replace).
    async fn put(&self, status: &RunStatus) -> Result<(), JobsError>;
    /// A run's status.
    async fn get(&self, run: &RunId) -> Result<Option<RunStatus>, JobsError>;
    /// Every run, newest first, at most `limit`.
    async fn list(&self, limit: usize) -> Result<Vec<RunStatus>, JobsError>;
}

/// Statuses in memory (tests, and a host that keeps them elsewhere).
#[derive(Debug, Default, Clone)]
pub struct MemoryRunStore {
    runs: Arc<Mutex<HashMap<RunId, RunStatus>>>,
}

impl MemoryRunStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RunStore for MemoryRunStore {
    async fn put(&self, status: &RunStatus) -> Result<(), JobsError> {
        self.runs
            .lock()
            .await
            .insert(status.run_id.clone(), status.clone());
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunStatus>, JobsError> {
        Ok(self.runs.lock().await.get(run).cloned())
    }

    async fn list(&self, limit: usize) -> Result<Vec<RunStatus>, JobsError> {
        let mut all: Vec<RunStatus> = self.runs.lock().await.values().cloned().collect();
        // ULIDs sort by time.
        all.sort_by(|a, b| b.run_id.cmp(&a.run_id));
        all.truncate(limit);
        Ok(all)
    }
}

/// One `status.json` per run under `<dir>/<run>/` (the local runner's data
/// directory, `<data_dir>/spark/runs`). Writes go to a temporary file and are
/// renamed, so a reader never sees half a status.
#[derive(Debug, Clone)]
pub struct FileRunStore {
    dir: PathBuf,
}

impl FileRunStore {
    /// A store under `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The run's directory (its logs are there too).
    pub fn run_dir(&self, run: &RunId) -> PathBuf {
        self.dir.join(run.as_str())
    }

    fn status_path(&self, run: &RunId) -> PathBuf {
        self.run_dir(run).join("status.json")
    }
}

async fn read_status(path: &Path) -> Result<Option<RunStatus>, JobsError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| JobsError::Backend(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[async_trait]
impl RunStore for FileRunStore {
    async fn put(&self, status: &RunStatus) -> Result<(), JobsError> {
        let dir = self.run_dir(&status.run_id);
        tokio::fs::create_dir_all(&dir).await?;
        let body =
            serde_json::to_vec_pretty(status).map_err(|e| JobsError::Backend(e.to_string()))?;
        let tmp = dir.join("status.json.tmp");
        tokio::fs::write(&tmp, body).await?;
        tokio::fs::rename(&tmp, self.status_path(&status.run_id)).await?;
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunStatus>, JobsError> {
        read_status(&self.status_path(run)).await
    }

    async fn list(&self, limit: usize) -> Result<Vec<RunStatus>, JobsError> {
        let mut names = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = Vec::new();
        for name in names {
            if out.len() >= limit {
                break;
            }
            if let Some(s) = read_status(&self.dir.join(&name).join("status.json")).await? {
                out.push(s);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::spec::{Backoff, SparkApp};

    fn spec(attempts: u32) -> SparkJobSpec {
        let mut s = SparkJobSpec::new(
            "etl",
            SparkApp::Sql {
                file: "s3://b/q.sql".into(),
            },
        );
        s.retry = RetryPolicy {
            max_attempts: attempts,
            backoff: Backoff::Fixed { secs: 5 },
        };
        s
    }

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time")
    }

    fn link(n: u32) -> LogLink {
        LogLink {
            attempt: n,
            url: format!("file:///runs/x/attempt-{n}.log"),
        }
    }

    #[test]
    fn retries_then_fails() {
        let s = spec(2);
        let mut st = RunStatus::new(RunId::new(), &s, t0());
        assert_eq!(st.state, RunState::Pending);
        assert_eq!(st.backend, Backend::Connect);
        st.submitted(1, link(1), t0());
        st.running(t0());
        assert_eq!(st.state, RunState::Running);
        let next = st.attempt_ended(
            AttemptOutcome::Failed {
                exit_code: Some(1),
                message: "boom".into(),
            },
            &s.retry,
            t0(),
        );
        assert_eq!(next, Next::Retry(Duration::from_secs(5)));
        assert_eq!(st.state, RunState::Retrying);
        assert!(st.next_attempt_at.is_some());
        st.submitted(2, link(2), t0());
        assert_eq!(st.logs.len(), 2);
        let next = st.attempt_ended(
            AttemptOutcome::Failed {
                exit_code: Some(2),
                message: "boom again".into(),
            },
            &s.retry,
            t0(),
        );
        assert_eq!(next, Next::Done);
        assert_eq!(st.state, RunState::Failed);
        assert_eq!(st.exit_code, Some(2));
        assert_eq!(st.message.as_deref(), Some("boom again"));
    }

    #[test]
    fn success_and_cancel() {
        let s = spec(3);
        let mut st = RunStatus::new(RunId::new(), &s, t0());
        st.submitted(1, link(1), t0());
        assert_eq!(
            st.attempt_ended(AttemptOutcome::Succeeded, &s.retry, t0()),
            Next::Done
        );
        assert_eq!(st.state, RunState::Succeeded);
        st.canceled(t0());
        assert_eq!(st.state, RunState::Succeeded, "terminal states stay");

        let mut st = RunStatus::new(RunId::new(), &s, t0());
        st.submitted(1, link(1), t0());
        assert_eq!(
            st.attempt_ended(
                AttemptOutcome::Canceled { timed_out: false },
                &s.retry,
                t0()
            ),
            Next::Done
        );
        assert_eq!(st.state, RunState::Canceled);
    }

    #[test]
    fn a_timeout_is_retried_while_attempts_remain() {
        let s = spec(2);
        let mut st = RunStatus::new(RunId::new(), &s, t0());
        st.submitted(1, link(1), t0());
        let next = st.attempt_ended(AttemptOutcome::Canceled { timed_out: true }, &s.retry, t0());
        assert!(matches!(next, Next::Retry(_)));
        st.submitted(2, link(2), t0());
        let next = st.attempt_ended(AttemptOutcome::Canceled { timed_out: true }, &s.retry, t0());
        assert_eq!(next, Next::Done);
        assert_eq!(st.state, RunState::Failed);
        assert_eq!(st.message.as_deref(), Some("timed out"));
    }

    #[test]
    fn status_json_round_trips() {
        let s = spec(1);
        let st = RunStatus::new(RunId::new(), &s, t0());
        let json = serde_json::to_value(&st).expect("json");
        assert_eq!(json["state"], "pending");
        assert_eq!(json["backend"], "connect");
        let back: RunStatus = serde_json::from_value(json).expect("parse");
        assert_eq!(back, st);
    }

    #[tokio::test]
    async fn file_store_round_trips_and_lists_newest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileRunStore::new(dir.path());
        let s = spec(1);
        let a = RunStatus::new(RunId("01aaaaaaaaaaaaaaaaaaaaaaaa".into()), &s, t0());
        let b = RunStatus::new(RunId("01bbbbbbbbbbbbbbbbbbbbbbbb".into()), &s, t0());
        store.put(&a).await.expect("put");
        store.put(&b).await.expect("put");
        assert_eq!(store.get(&a.run_id).await.expect("get"), Some(a.clone()));
        let listed = store.list(10).await.expect("list");
        assert_eq!(listed, vec![b, a]);
        assert_eq!(
            store.get(&RunId("missing".into())).await.expect("get"),
            None
        );
    }
}
