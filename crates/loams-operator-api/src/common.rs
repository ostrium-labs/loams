//! Types shared by the kinds: references, replica counts, the status
//! journal and the duration encoding.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A status condition. Re-exported so every status uses one type.
pub use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

/// A URL, kept as text. Fields that take one carry a CEL rule for the
/// schemes they accept.
pub type Url = String;

/// A reference to a Secret in the same namespace. It names the Secret and
/// optionally the key inside it; it never holds a value.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretRef {
    /// The Secret's name.
    pub name: String,
    /// The key inside the Secret. Each user of a reference states its
    /// default (for a database reference, `dsn`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// A database the operator does not create: the Postgres DSN comes from a
/// Secret the user created, under key `dsn`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DbRef {
    /// Secret holding the DSN under key `dsn`.
    pub secret_ref: SecretRef,
}

/// A replica count for a platform-base component.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Replicas {
    /// Number of replicas (at least 1).
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub replicas: u32,
}

impl Default for Replicas {
    fn default() -> Self {
        Self { replicas: 1 }
    }
}

pub(crate) fn one() -> u32 {
    1
}

/// The step an operation is on, for crash-safe resume.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepStatus {
    /// The plan item being run.
    pub item: String,
    /// The phase inside the item (for example `prepare`, `apply`, `verify`).
    pub phase: String,
    /// When the step started.
    pub since: Time,
}

/// The journal of one multi-step operation. The operator writes it before
/// each step and reads it back after a restart, so a step is never repeated
/// by mistake. It holds names and ids, never secret values.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationStatus {
    /// Operation id, stable across restarts.
    pub id: String,
    /// Operation kind (for example `install`, `upgrade`, `delete`).
    pub kind: String,
    /// The ordered plan items.
    #[serde(default)]
    pub plan: Vec<String>,
    /// The step in progress, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepStatus>,
    /// The plan items already done.
    #[serde(default)]
    pub done: Vec<String>,
    /// Fence token of the leader that wrote this journal.
    #[serde(default)]
    pub fence: i32,
}

/// Serde for `std::time::Duration` as a duration string such as `5m`.
pub mod duration_string {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&humantime::format_duration(*d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let text = String::deserialize(d)?;
        humantime::parse_duration(&text).map_err(serde::de::Error::custom)
    }
}
