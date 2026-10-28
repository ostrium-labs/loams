//! Spark batch jobs on the Jobs API (design 26 §9.2 and §9.4; design 81 SP-b,
//! D2000–D2009). **Preview.**
//!
//! A [`SparkJobSpec`] names a PySpark file, a JAR and its main class, or a SQL
//! file, with conf, resources and a retry policy. It runs on one of three
//! backends:
//!
//! * **connect** (PySpark and SQL by default): a short-lived driver runs the
//!   script against the namespace's Spark Connect endpoint, which SP-a's proxy
//!   routes to Sail (design 26 §9.2);
//! * **kubernetes** (JARs by default): Apache Spark on Kubernetes, a driver pod
//!   running `spark-submit` in client mode that starts its own executors
//!   (design 26 §9.4), rendered by [`render`] and created by `K8sRunner`
//!   (feature `kubernetes`);
//! * **local**: a child process, for `loams dev` and compose ([`local`]).
//!
//! A [`Supervisor`] drives a run through its attempts with retries and records
//! a [`RunStatus`] (with log links) in a [`RunStore`]. Schedules are Resonate
//! schedules on the embedded durable server ([`schedule`], design 26 §8.1).

pub mod cli;
pub mod client;
#[cfg(feature = "kubernetes")]
pub mod k8s;
pub mod local;
pub mod render;
pub mod run;
pub mod runner;
pub mod schedule;
pub mod spec;

use std::fmt;

pub use render::{Master, RenderContext, RenderedJob, SecretRef};
pub use run::{
    AttemptOutcome, FileRunStore, LogLink, MemoryRunStore, RunState, RunStatus, RunStore,
};
pub use runner::{Attempt, AttemptHandle, AttemptPhase, Runner, Supervisor};
pub use spec::{Backend, Backoff, Resources, RetryPolicy, SparkApp, SparkJobSpec};

/// A run's id: a ULID, lower-cased so it fits Kubernetes names and labels.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RunId(pub String);

impl RunId {
    /// A new id.
    pub fn new() -> Self {
        Self(ulid::Ulid::generate().to_string().to_ascii_lowercase())
    }

    /// Parses an id given by a user: a ULID in either case.
    pub fn parse(s: &str) -> Result<Self, JobsError> {
        ulid::Ulid::from_string(s)
            .map(|u| Self(u.to_string().to_ascii_lowercase()))
            .map_err(|_| JobsError::InvalidSpec(format!("{s:?} is not a run id")))
    }

    /// The last ten characters (the random part), for pod names.
    pub fn short(&self) -> &str {
        let n = self.0.len();
        &self.0[n.saturating_sub(10)..]
    }

    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A jobs operation failed.
#[derive(Debug, thiserror::Error)]
pub enum JobsError {
    /// The spec is not valid; the message names the field.
    #[error("invalid Spark job: {0}")]
    InvalidSpec(String),
    /// No such run or schedule.
    #[error("not found: {0}")]
    NotFound(String),
    /// The backend is not configured here (no Kubernetes client, no
    /// `SPARK_HOME`, no Spark Connect endpoint).
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// The backend refused or failed the call.
    #[error("backend error: {0}")]
    Backend(String),
    /// Reading or writing local state failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The server answered with an error (the HTTP facade).
    #[error("server answered {status}: {message}")]
    Status {
        /// The HTTP status.
        status: u16,
        /// Its message.
        message: String,
    },
}
