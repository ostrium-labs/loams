//! Spark job schedules on the embedded Resonate server (D2004; design 26
//! §8.1).
//!
//! A schedule is a Resonate schedule: a cron rule and a promise template.
//! When it fires, the server creates the promise
//! `spark-run-<schedule>-<timestamp>` once (`INSERT OR IGNORE`), tagged for
//! Loams' own in-process group, whose durable function
//! [`SCHEDULE_FUNCTION`] decodes the job with [`job_from_promise_param`] and
//! starts a run with the promise id as its idempotency key. So a tick starts
//! exactly one run, even if the schedule is processed twice, and a restart of
//! the node re-arms the timer from the durable store.
//!
//! This module builds the protocol requests and decodes the tick; the host
//! sends them through a [`DurableTransport`], which `loams` wires to
//! `loams_durable::DurableServer::process` (so this crate does not link the
//! Resonate server).

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::JobsError;
use super::spec::SparkJobSpec;

/// The Loams durable function a tick runs.
pub const SCHEDULE_FUNCTION: &str = "loams.spark.run";

/// The tick's target: Loams' own in-process group (design 26 §8.1).
pub const SCHEDULE_TARGET: &str = "inproc://any@loams";

/// How long a tick's promise may stay pending (a day, in ms): longer than any
/// run's retries, so a slow run is not cut off by its own promise.
pub const PROMISE_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// A Spark job on a cron rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparkSchedule {
    /// The schedule's id in the namespace (`[a-z0-9][a-z0-9._-]{0,62}`).
    pub id: String,
    /// A five-field cron rule, in UTC.
    pub cron: String,
    /// The job each tick runs.
    pub job: SparkJobSpec,
}

/// Sends protocol requests to the durable server.
#[async_trait]
pub trait DurableTransport: Send + Sync {
    /// One request envelope (`{"kind", "data"}`); answers the response
    /// envelope.
    async fn process(&self, request: Value) -> Result<Value, JobsError>;
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Five whitespace-separated fields of `0-9 * , - /` (Resonate's parser does
/// the rest; this refuses the obvious mistakes early, such as a six-field
/// Quartz rule or a `@daily` alias).
pub fn valid_cron(cron: &str) -> bool {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    fields.len() == 5
        && fields.iter().all(|f| {
            f.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'*' | b',' | b'-' | b'/'))
        })
}

impl SparkSchedule {
    /// Checks the id, the rule and the job.
    pub fn validate(&self) -> Result<(), JobsError> {
        if !valid_id(&self.id) {
            return Err(JobsError::InvalidSpec(format!(
                "schedule id {:?}: use [a-z0-9][a-z0-9._-]*, at most 63 characters",
                self.id
            )));
        }
        if !valid_cron(&self.cron) {
            return Err(JobsError::InvalidSpec(format!(
                "cron {:?}: five fields (minute hour day month weekday), UTC",
                self.cron
            )));
        }
        self.job.validate()
    }

    /// The Resonate schedule's id: namespaced, so two namespaces' schedules
    /// of the same name never collide on the shared server.
    pub fn durable_id(&self, namespace_id: u64) -> String {
        format!("ns{namespace_id}.spark.{}", self.id)
    }
}

/// The promise param of a tick: `{"headers": {}, "data": base64(json)}`, the
/// JSON being `{"func", "args": [namespace, job], "version": 1}`.
pub fn promise_param(namespace_id: u64, job: &SparkJobSpec) -> Result<Value, JobsError> {
    let body = json!({
        "func": SCHEDULE_FUNCTION,
        "args": [namespace_id, job],
        "version": 1
    });
    let bytes = serde_json::to_vec(&body).map_err(|e| JobsError::Backend(e.to_string()))?;
    Ok(json!({ "headers": {}, "data": STANDARD.encode(bytes) }))
}

/// Decodes a tick's promise param back into its namespace and job.
pub fn job_from_promise_param(param: &Value) -> Result<(u64, SparkJobSpec), JobsError> {
    let bad = |m: &str| JobsError::InvalidSpec(format!("schedule tick: {m}"));
    let data = param
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("no data"))?;
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| bad("data is not base64"))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| bad("data is not JSON"))?;
    if body.get("func").and_then(Value::as_str) != Some(SCHEDULE_FUNCTION) {
        return Err(bad("not a Spark tick"));
    }
    let args = body
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no args"))?;
    let ns = args
        .first()
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("no namespace"))?;
    let job: SparkJobSpec = args
        .get(1)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| bad(&e.to_string()))?
        .ok_or_else(|| bad("no job"))?;
    Ok((ns, job))
}

/// The `schedule.create` request.
pub fn create_request(namespace_id: u64, schedule: &SparkSchedule) -> Result<Value, JobsError> {
    schedule.validate()?;
    let id = schedule.durable_id(namespace_id);
    Ok(json!({
        "kind": "schedule.create",
        "data": {
            "id": id,
            "cron": schedule.cron,
            "promiseId": format!("spark-run-{id}-{{{{.timestamp}}}}"),
            "promiseTimeout": PROMISE_TIMEOUT_MS,
            "promiseParam": promise_param(namespace_id, &schedule.job)?,
            "promiseTags": {
                "resonate:target": SCHEDULE_TARGET,
                "loams:namespace": namespace_id.to_string(),
                "loams:engine": "spark",
                "loams:spark-job": schedule.job.name
            }
        }
    }))
}

/// The `schedule.delete` request.
pub fn delete_request(namespace_id: u64, id: &str) -> Value {
    json!({ "kind": "schedule.delete", "data": { "id": format!("ns{namespace_id}.spark.{id}") } })
}

fn status_of(response: &Value) -> i64 {
    response["head"]["status"].as_i64().unwrap_or_default()
}

/// Creates or replaces a schedule (design 26 §8.1 "Upsert": delete, then
/// create). Answers the durable schedule id.
pub async fn upsert(
    transport: &dyn DurableTransport,
    namespace_id: u64,
    schedule: &SparkSchedule,
) -> Result<String, JobsError> {
    let create = create_request(namespace_id, schedule)?;
    match transport
        .process(delete_request(namespace_id, &schedule.id))
        .await
    {
        Ok(_) | Err(JobsError::NotFound(_)) => {}
        Err(JobsError::Status { status: 404, .. }) => {}
        Err(e) => return Err(e),
    }
    let answer = transport.process(create).await?;
    match status_of(&answer) {
        200 | 201 => Ok(schedule.durable_id(namespace_id)),
        other => Err(JobsError::Backend(format!(
            "schedule.create answered {other}: {}",
            answer["data"]
        ))),
    }
}

/// Deletes a schedule; a missing one is not an error.
pub async fn remove(
    transport: &dyn DurableTransport,
    namespace_id: u64,
    id: &str,
) -> Result<(), JobsError> {
    match transport.process(delete_request(namespace_id, id)).await {
        Ok(_) | Err(JobsError::NotFound(_)) => Ok(()),
        Err(JobsError::Status { status: 404, .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::jobs::spec::SparkApp;

    fn schedule() -> SparkSchedule {
        SparkSchedule {
            id: "nightly-rollup".into(),
            cron: "0 2 * * *".into(),
            job: SparkJobSpec::new(
                "rollup",
                SparkApp::Sql {
                    file: "s3://jobs/rollup.sql".into(),
                },
            ),
        }
    }

    #[test]
    fn create_request_shape() {
        let req = create_request(42, &schedule()).expect("request");
        assert_eq!(req["kind"], "schedule.create");
        let d = &req["data"];
        assert_eq!(d["id"], "ns42.spark.nightly-rollup");
        assert_eq!(d["cron"], "0 2 * * *");
        assert_eq!(
            d["promiseId"],
            "spark-run-ns42.spark.nightly-rollup-{{.timestamp}}"
        );
        assert_eq!(d["promiseTags"]["resonate:target"], SCHEDULE_TARGET);
        let (ns, job) = job_from_promise_param(&d["promiseParam"]).expect("decode");
        assert_eq!(ns, 42);
        assert_eq!(job, schedule().job);
    }

    #[test]
    fn validation() {
        let mut s = schedule();
        s.cron = "0 0 2 * * *".into();
        assert!(create_request(1, &s).is_err());
        let mut s = schedule();
        s.cron = "@daily".into();
        assert!(create_request(1, &s).is_err());
        let mut s = schedule();
        s.id = "Nightly".into();
        assert!(create_request(1, &s).is_err());
        assert!(valid_cron("*/15 0-6 1,15 * 1-5"));
    }

    #[test]
    fn foreign_ticks_are_refused() {
        let other = json!({"headers": {}, "data": STANDARD.encode(br#"{"func":"x","args":[]}"#)});
        assert!(job_from_promise_param(&other).is_err());
        assert!(job_from_promise_param(&json!({})).is_err());
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Value>>);

    #[async_trait]
    impl DurableTransport for Recorder {
        async fn process(&self, request: Value) -> Result<Value, JobsError> {
            let kind = request["kind"].as_str().unwrap_or_default().to_string();
            self.0.lock().expect("lock").push(request);
            if kind == "schedule.delete" {
                return Err(JobsError::Status {
                    status: 404,
                    message: "not found".into(),
                });
            }
            Ok(json!({"kind": kind, "head": {"status": 201}, "data": {}}))
        }
    }

    #[tokio::test]
    async fn upsert_deletes_then_creates() {
        let t = Recorder::default();
        let id = upsert(&t, 42, &schedule()).await.expect("upsert");
        assert_eq!(id, "ns42.spark.nightly-rollup");
        let kinds: Vec<String> =
            t.0.lock()
                .expect("lock")
                .iter()
                .map(|r| r["kind"].as_str().unwrap_or_default().to_string())
                .collect();
        assert_eq!(kinds, vec!["schedule.delete", "schedule.create"]);
        remove(&t, 42, "nightly-rollup").await.expect("remove");
    }
}
