//! A typed client for the JobManager REST API (port 8081): the cluster
//! overview, jobs, cancel, stop-with-savepoint, savepoints and checkpoints.
//!
//! | Call | Endpoint |
//! |---|---|
//! | [`FlinkRestClient::overview`] | `GET /overview` |
//! | [`FlinkRestClient::list_jobs`] | `GET /jobs/overview` |
//! | [`FlinkRestClient::job`] | `GET /jobs/:id` |
//! | [`FlinkRestClient::cancel_job`] | `PATCH /jobs/:id?mode=cancel` |
//! | [`FlinkRestClient::stop_with_savepoint`] | `POST /jobs/:id/stop` |
//! | [`FlinkRestClient::trigger_savepoint`] | `POST /jobs/:id/savepoints` |
//! | [`FlinkRestClient::savepoint_status`] | `GET /jobs/:id/savepoints/:trigger` |
//! | [`FlinkRestClient::trigger_checkpoint`] | `POST /jobs/:id/checkpoints` |
//! | [`FlinkRestClient::checkpoint_status`] | `GET /jobs/:id/checkpoints/:trigger` |
//! | [`FlinkRestClient::checkpoints`] | `GET /jobs/:id/checkpoints` |
//!
//! Note the two spellings Flink uses: `stop` takes `targetDirectory`,
//! `savepoints` takes `target-directory` and `cancel-job`.

use std::fmt;
use std::time::{Duration, Instant};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::FlinkError;
use crate::http::{Http, Ignored};

/// A Flink job id: 32 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(String);

impl JobId {
    /// Checks the id's shape (32 hex digits).
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] for anything else.
    pub fn new(value: impl Into<String>) -> Result<Self, FlinkError> {
        let value = value.into();
        if value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(FlinkError::Invalid(format!(
                "a Flink job id is 32 hex digits, not {value:?}"
            )))
        }
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The id of an asynchronous operation (savepoint, stop, checkpoint).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TriggerId(String);

impl TriggerId {
    /// Checks the id's shape.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] for anything but letters, digits and `-`.
    pub fn new(value: impl Into<String>) -> Result<Self, FlinkError> {
        let value = value.into();
        crate::http::check_segment("trigger id", &value)?;
        Ok(Self(value))
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TriggerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A job's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    /// Being set up.
    Initializing,
    /// Created, not yet scheduled.
    Created,
    /// Running.
    Running,
    /// Failing, about to restart or fail.
    Failing,
    /// Failed for good.
    Failed,
    /// Being cancelled.
    Cancelling,
    /// Cancelled.
    Canceled,
    /// Finished (bounded input, or stopped with a savepoint).
    Finished,
    /// Restarting after a failure.
    Restarting,
    /// Suspended (the JobManager lost leadership).
    Suspended,
    /// Reconciling after a JobManager failover.
    Reconciling,
}

impl JobState {
    /// Whether the job can no longer change state (Flink's
    /// "globally terminal").
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Failed | JobState::Canceled | JobState::Finished
        )
    }
}

/// `GET /overview`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ClusterOverview {
    /// Registered TaskManagers.
    pub taskmanagers: u32,
    /// Task slots in all.
    pub slots_total: u32,
    /// Free task slots.
    pub slots_available: u32,
    /// Running jobs.
    pub jobs_running: u32,
    /// Finished jobs.
    #[serde(default)]
    pub jobs_finished: u32,
    /// Cancelled jobs.
    #[serde(default)]
    pub jobs_cancelled: u32,
    /// Failed jobs.
    #[serde(default)]
    pub jobs_failed: u32,
    /// The Flink version.
    pub flink_version: String,
    /// The Flink commit.
    #[serde(default)]
    pub flink_commit: String,
}

/// A row of `GET /jobs/overview`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct JobOverview {
    /// The job id.
    pub jid: JobId,
    /// The job name (the SQL statement's `pipeline.name`).
    pub name: String,
    /// The job state.
    pub state: JobState,
    /// Start time, ms since the epoch.
    pub start_time: i64,
    /// End time, ms since the epoch, or -1.
    pub end_time: i64,
    /// Duration in ms.
    pub duration: i64,
    /// Last state change, ms since the epoch.
    pub last_modification: i64,
    /// Task counts by state.
    #[serde(default)]
    pub tasks: Value,
}

#[derive(Deserialize)]
struct JobsOverviewBody {
    #[serde(default)]
    jobs: Vec<JobOverview>,
}

/// A vertex of `GET /jobs/:id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct JobVertex {
    /// The vertex id.
    pub id: String,
    /// The vertex name.
    pub name: String,
    /// The vertex's parallelism.
    pub parallelism: u32,
    /// The vertex's state.
    pub status: String,
}

/// `GET /jobs/:id` (the fields the controller reads; the rest is in
/// `extra`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JobDetails {
    /// The job id.
    pub jid: JobId,
    /// The job name.
    pub name: String,
    /// Whether the job takes `stop` (stop-with-savepoint).
    #[serde(rename = "isStoppable", default)]
    pub is_stoppable: bool,
    /// The job state.
    pub state: JobState,
    /// Start time, ms since the epoch.
    #[serde(rename = "start-time")]
    pub start_time: i64,
    /// End time, ms since the epoch, or -1.
    #[serde(rename = "end-time")]
    pub end_time: i64,
    /// Duration in ms.
    pub duration: i64,
    /// The job's max parallelism.
    #[serde(rename = "maxParallelism", default)]
    pub max_parallelism: i64,
    /// The vertices.
    #[serde(default)]
    pub vertices: Vec<JobVertex>,
    /// Everything else (`plan`, `timestamps`, `status-counts`, ...).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// How a savepoint is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SavepointFormat {
    /// Portable across state backends and Flink versions (upgrades, D1975).
    #[default]
    Canonical,
    /// The state backend's own format: faster, not portable.
    Native,
}

/// The body of `POST /jobs/:id/stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StopRequest {
    /// Where the savepoint goes; the cluster's
    /// `execution.checkpointing.savepoint-dir` when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<String>,
    /// Emit `MAX_WATERMARK` first (flush event-time windows; for jobs that
    /// end for good, not for upgrades).
    pub drain: bool,
    /// The savepoint format.
    pub format_type: SavepointFormat,
    /// An idempotency id for the trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
}

/// The body of `POST /jobs/:id/savepoints`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct SavepointRequest {
    /// Where the savepoint goes.
    #[serde(rename = "target-directory", skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<String>,
    /// Cancel the job once the savepoint is taken (prefer
    /// [`FlinkRestClient::stop_with_savepoint`]).
    #[serde(rename = "cancel-job")]
    pub cancel_job: bool,
    /// The savepoint format.
    #[serde(rename = "formatType")]
    pub format_type: SavepointFormat,
    /// An idempotency id for the trigger.
    #[serde(rename = "triggerId", skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
}

/// The kind of checkpoint `POST /jobs/:id/checkpoints` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckpointType {
    /// As the job is configured.
    #[default]
    Configured,
    /// A full checkpoint.
    Full,
    /// An incremental one (Flink documents it as not yet supported).
    Incremental,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointTriggerBody {
    checkpoint_type: CheckpointType,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger_id: Option<String>,
}

#[derive(Deserialize)]
struct RequestIdBody {
    #[serde(rename = "request-id")]
    request_id: String,
}

/// `IN_PROGRESS` or `COMPLETED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AsyncState {
    /// Still running.
    InProgress,
    /// Done (successfully or not: see the operation).
    Completed,
}

#[derive(Debug, Clone, Deserialize)]
struct AsyncStatus {
    id: AsyncState,
}

/// The outcome of a savepoint or stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavepointOutcome {
    /// Not done yet.
    InProgress,
    /// Written at `location`.
    Completed {
        /// The savepoint's path (e.g. `s3://flink/flink/t1/savepoints/savepoint-…`).
        location: String,
    },
    /// Failed; the cause is Flink's serialized exception, shortened.
    Failed {
        /// Why.
        cause: String,
    },
}

#[derive(Deserialize)]
struct SavepointStatusBody {
    status: AsyncStatus,
    #[serde(default)]
    operation: Option<SavepointOperationBody>,
}

#[derive(Deserialize)]
struct SavepointOperationBody {
    #[serde(default)]
    location: Option<String>,
    #[serde(rename = "failure-cause", default)]
    failure_cause: Option<Value>,
}

/// The outcome of a triggered checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointOutcome {
    /// Not done yet.
    InProgress,
    /// Completed as checkpoint `id`.
    Completed {
        /// The checkpoint id.
        id: u64,
    },
    /// Failed.
    Failed {
        /// Why.
        cause: String,
    },
}

#[derive(Deserialize)]
struct CheckpointStatusBody {
    status: AsyncStatus,
    #[serde(default)]
    operation: Option<CheckpointOperationBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointOperationBody {
    #[serde(default)]
    checkpoint_id: Option<u64>,
    #[serde(default)]
    failure_cause: Option<Value>,
}

/// Checkpoint counts (`GET /jobs/:id/checkpoints`, `counts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct CheckpointCounts {
    /// Restored from a checkpoint or savepoint.
    #[serde(default)]
    pub restored: u64,
    /// All checkpoints.
    #[serde(default)]
    pub total: u64,
    /// In progress.
    #[serde(default)]
    pub in_progress: u64,
    /// Completed.
    #[serde(default)]
    pub completed: u64,
    /// Failed.
    #[serde(default)]
    pub failed: u64,
}

/// One checkpoint's statistics (the `latest` entries and `history`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CheckpointStats {
    /// The checkpoint id.
    pub id: u64,
    /// `IN_PROGRESS`, `COMPLETED` or `FAILED`.
    #[serde(default)]
    pub status: Option<String>,
    /// Whether this is a savepoint.
    #[serde(default)]
    pub is_savepoint: bool,
    /// When it was triggered, ms since the epoch.
    #[serde(default)]
    pub trigger_timestamp: i64,
    /// When the last subtask acknowledged, ms since the epoch.
    #[serde(default)]
    pub latest_ack_timestamp: i64,
    /// Bytes this checkpoint wrote (incremental size).
    #[serde(default)]
    pub checkpointed_size: u64,
    /// The full state size in bytes.
    #[serde(default)]
    pub state_size: u64,
    /// End-to-end duration in ms.
    #[serde(default)]
    pub end_to_end_duration: i64,
    /// Where a completed checkpoint is (externalized checkpoints).
    #[serde(default)]
    pub external_path: Option<String>,
    /// `CHECKPOINT`, `UNALIGNED_CHECKPOINT`, `SAVEPOINT`, `SYNC_SAVEPOINT`.
    #[serde(default)]
    pub checkpoint_type: Option<String>,
    /// Why a failed checkpoint failed.
    #[serde(default)]
    pub failure_message: Option<String>,
}

/// The restore a job started from (`latest.restored`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RestoredCheckpoint {
    /// The checkpoint id.
    pub id: u64,
    /// When it was restored, ms since the epoch.
    #[serde(default)]
    pub restore_timestamp: i64,
    /// Whether it was a savepoint.
    #[serde(default)]
    pub is_savepoint: bool,
    /// Where it was.
    #[serde(default)]
    pub external_path: Option<String>,
}

/// `latest` of `GET /jobs/:id/checkpoints`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LatestCheckpoints {
    /// The latest completed checkpoint.
    #[serde(default)]
    pub completed: Option<CheckpointStats>,
    /// The latest savepoint.
    #[serde(default)]
    pub savepoint: Option<CheckpointStats>,
    /// The latest failed checkpoint.
    #[serde(default)]
    pub failed: Option<CheckpointStats>,
    /// The restore the job started from.
    #[serde(default)]
    pub restored: Option<RestoredCheckpoint>,
}

/// `GET /jobs/:id/checkpoints`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CheckpointingStatistics {
    /// Counts.
    #[serde(default)]
    pub counts: CheckpointCounts,
    /// Size and duration summaries (min, max, avg, percentiles), as Flink
    /// sends them.
    #[serde(default)]
    pub summary: Value,
    /// The latest of each kind.
    #[serde(default)]
    pub latest: LatestCheckpoints,
    /// Recent checkpoints, newest first.
    #[serde(default)]
    pub history: Vec<CheckpointStats>,
}

impl CheckpointingStatistics {
    /// The path a restart should restore from: the newer of the latest
    /// completed checkpoint and the latest savepoint that have a path.
    pub fn restore_path(&self) -> Option<&str> {
        [&self.latest.completed, &self.latest.savepoint]
            .into_iter()
            .flatten()
            .filter(|c| c.external_path.is_some())
            .max_by_key(|c| (c.latest_ack_timestamp, c.id))
            .and_then(|c| c.external_path.as_deref())
    }
}

/// The JobManager REST client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct FlinkRestClient {
    http: Http,
}

fn cause_text(cause: Option<&Value>) -> String {
    let text = match cause {
        None | Some(Value::Null) => return "no failure cause given".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o
            .get("serialized-throwable")
            .or_else(|| o.get("class"))
            .and_then(Value::as_str)
            .map_or_else(|| Value::Object(o.clone()).to_string(), str::to_owned),
        Some(other) => other.to_string(),
    };
    crate::error::shorten(&text, crate::error::MAX_ERROR_LEN)
}

impl FlinkRestClient {
    /// A client of the JobManager at `base_url` (e.g.
    /// `http://flink-jobmanager:8081`).
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] for a URL that is not `http(s)://`.
    pub fn new(base_url: &str) -> Result<Self, FlinkError> {
        Self::with_client(base_url, Http::default_client()?)
    }

    /// [`FlinkRestClient::new`] with a caller's HTTP client.
    ///
    /// # Errors
    ///
    /// As [`FlinkRestClient::new`].
    pub fn with_client(base_url: &str, client: reqwest::Client) -> Result<Self, FlinkError> {
        Ok(Self {
            http: Http::new(base_url, client)?,
        })
    }

    /// `GET /overview`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn overview(&self) -> Result<ClusterOverview, FlinkError> {
        self.http.get("overview").await
    }

    /// `GET /jobs/overview`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn list_jobs(&self) -> Result<Vec<JobOverview>, FlinkError> {
        let body: JobsOverviewBody = self.http.get("jobs/overview").await?;
        Ok(body.jobs)
    }

    /// `GET /jobs/:id`.
    ///
    /// # Errors
    ///
    /// The call's error (404 for an unknown job).
    pub async fn job(&self, id: &JobId) -> Result<JobDetails, FlinkError> {
        self.http.get(&format!("jobs/{id}")).await
    }

    /// `PATCH /jobs/:id?mode=cancel`: cancels without a savepoint.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn cancel_job(&self, id: &JobId) -> Result<(), FlinkError> {
        let mut url = self.http.url(&format!("jobs/{id}"))?;
        url.query_pairs_mut().append_pair("mode", "cancel");
        let _: Ignored = self.http.call::<(), _>(Method::PATCH, url, None).await?;
        Ok(())
    }

    /// `POST /jobs/:id/stop`: takes a savepoint and stops the job
    /// (FINISHED). Poll [`FlinkRestClient::savepoint_status`] with the
    /// trigger.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn stop_with_savepoint(
        &self,
        id: &JobId,
        req: &StopRequest,
    ) -> Result<TriggerId, FlinkError> {
        let body: RequestIdBody = self.http.post(&format!("jobs/{id}/stop"), req).await?;
        TriggerId::new(body.request_id)
    }

    /// `POST /jobs/:id/savepoints`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn trigger_savepoint(
        &self,
        id: &JobId,
        req: &SavepointRequest,
    ) -> Result<TriggerId, FlinkError> {
        let body: RequestIdBody = self
            .http
            .post(&format!("jobs/{id}/savepoints"), req)
            .await?;
        TriggerId::new(body.request_id)
    }

    /// `GET /jobs/:id/savepoints/:trigger` (also answers for `stop`).
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn savepoint_status(
        &self,
        id: &JobId,
        trigger: &TriggerId,
    ) -> Result<SavepointOutcome, FlinkError> {
        let body: SavepointStatusBody = self
            .http
            .get(&format!("jobs/{id}/savepoints/{trigger}"))
            .await?;
        Ok(match body.status.id {
            AsyncState::InProgress => SavepointOutcome::InProgress,
            AsyncState::Completed => {
                let op = body.operation;
                match op
                    .as_ref()
                    .and_then(|o| o.location.clone())
                    .filter(|l| !l.is_empty())
                {
                    Some(location) => SavepointOutcome::Completed { location },
                    None => SavepointOutcome::Failed {
                        cause: cause_text(op.as_ref().and_then(|o| o.failure_cause.as_ref())),
                    },
                }
            }
        })
    }

    /// Polls [`FlinkRestClient::savepoint_status`] every `poll` until the
    /// savepoint completes, and returns its location.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Failed`] with Flink's cause, [`FlinkError::Timeout`],
    /// or a call's error.
    pub async fn wait_savepoint(
        &self,
        id: &JobId,
        trigger: &TriggerId,
        poll: Duration,
        timeout: Duration,
    ) -> Result<String, FlinkError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.savepoint_status(id, trigger).await? {
                SavepointOutcome::Completed { location } => return Ok(location),
                SavepointOutcome::Failed { cause } => {
                    return Err(FlinkError::Failed(format!(
                        "savepoint {trigger} of job {id}: {cause}"
                    )));
                }
                SavepointOutcome::InProgress => {
                    if Instant::now() + poll > deadline {
                        return Err(FlinkError::Timeout(format!(
                            "savepoint {trigger} of job {id} after {timeout:?}"
                        )));
                    }
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    /// `POST /jobs/:id/checkpoints`: triggers a checkpoint now.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn trigger_checkpoint(
        &self,
        id: &JobId,
        kind: CheckpointType,
        trigger_id: Option<String>,
    ) -> Result<TriggerId, FlinkError> {
        let body: RequestIdBody = self
            .http
            .post(
                &format!("jobs/{id}/checkpoints"),
                &CheckpointTriggerBody {
                    checkpoint_type: kind,
                    trigger_id,
                },
            )
            .await?;
        TriggerId::new(body.request_id)
    }

    /// `GET /jobs/:id/checkpoints/:trigger`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn checkpoint_status(
        &self,
        id: &JobId,
        trigger: &TriggerId,
    ) -> Result<CheckpointOutcome, FlinkError> {
        let body: CheckpointStatusBody = self
            .http
            .get(&format!("jobs/{id}/checkpoints/{trigger}"))
            .await?;
        Ok(match body.status.id {
            AsyncState::InProgress => CheckpointOutcome::InProgress,
            AsyncState::Completed => match body.operation.as_ref().and_then(|o| o.checkpoint_id) {
                Some(id) => CheckpointOutcome::Completed { id },
                None => CheckpointOutcome::Failed {
                    cause: cause_text(
                        body.operation
                            .as_ref()
                            .and_then(|o| o.failure_cause.as_ref()),
                    ),
                },
            },
        })
    }

    /// `GET /jobs/:id/checkpoints`: counts, summaries, the latest of each
    /// kind and the history.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn checkpoints(&self, id: &JobId) -> Result<CheckpointingStatistics, FlinkError> {
        self.http.get(&format!("jobs/{id}/checkpoints")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_are_32_hex_digits() {
        assert!(JobId::new("fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10").is_ok());
        assert_eq!(
            JobId::new("FBF9AD9E7FB4C3C3A8BB3B4E3A1F2D10")
                .unwrap()
                .as_str(),
            "fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10"
        );
        assert!(JobId::new("../overview").is_err());
        assert!(JobId::new("abc").is_err());
    }

    #[test]
    fn stop_and_savepoint_bodies_use_flinks_two_spellings() {
        let stop = StopRequest {
            target_directory: Some("s3://b/sp".into()),
            drain: false,
            format_type: SavepointFormat::Canonical,
            trigger_id: None,
        };
        assert_eq!(
            serde_json::to_value(&stop).unwrap(),
            serde_json::json!({"targetDirectory": "s3://b/sp", "drain": false, "formatType": "CANONICAL"})
        );
        let sp = SavepointRequest {
            target_directory: Some("s3://b/sp".into()),
            cancel_job: false,
            format_type: SavepointFormat::Native,
            trigger_id: Some("t".into()),
        };
        assert_eq!(
            serde_json::to_value(&sp).unwrap(),
            serde_json::json!({"target-directory": "s3://b/sp", "cancel-job": false, "formatType": "NATIVE", "triggerId": "t"})
        );
    }

    #[test]
    fn restore_path_prefers_the_newest_with_a_path() {
        let stats: CheckpointingStatistics = serde_json::from_value(serde_json::json!({
            "counts": {"restored": 0, "total": 5, "in_progress": 0, "completed": 4, "failed": 1},
            "latest": {
                "completed": {"id": 9, "status": "COMPLETED", "is_savepoint": false, "latest_ack_timestamp": 2000, "external_path": "s3://b/cp/chk-9"},
                "savepoint": {"id": 7, "status": "COMPLETED", "is_savepoint": true, "latest_ack_timestamp": 1000, "external_path": "s3://b/sp/savepoint-7"},
                "failed": null,
                "restored": null
            },
            "history": []
        }))
        .unwrap();
        assert_eq!(stats.restore_path(), Some("s3://b/cp/chk-9"));
        assert_eq!(stats.counts.completed, 4);
    }

    #[test]
    fn failure_causes_read_the_serialized_throwable() {
        let cause =
            serde_json::json!({"class": "java.lang.Exception", "serialized-throwable": "boom"});
        assert_eq!(cause_text(Some(&cause)), "boom");
        assert_eq!(cause_text(None), "no failure cause given");
    }
}
