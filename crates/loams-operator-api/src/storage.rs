//! `StorageBinding`: a named storage system an install can use.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Condition, OperationStatus, SecretRef, Url};

/// The kind of storage. This is the CRD's own type; code that compares it
/// with the marketplace's `StorageKind` maps with an explicit match.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageKind {
    Object,
    SqlMysql,
    SqlPostgres,
    Kafka,
    Kv,
}

/// The object store provider.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObjectProvider {
    S3,
    Gcs,
    Azure,
    R2,
}

/// An object store bucket.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSpec {
    pub provider: ObjectProvider,
    pub bucket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Endpoint URL for S3-compatible stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Url>,
    /// Key prefix inside the bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// A MySQL-compatible database. Type only; no controller uses it yet.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SqlMysqlSpec {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// A Postgres database. Type only; no controller uses it yet.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SqlPostgresSpec {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// A Kafka-compatible cluster. Type only; no controller uses it yet.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KafkaSpec {
    pub bootstrap_servers: Vec<String>,
}

/// A key-value store. Type only; no controller uses it yet.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KvSpec {
    pub endpoint: Url,
}

/// Cloud workload identity: no key is stored anywhere.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadIdentity {
    /// AWS IAM role ARN (IRSA).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_role_arn: Option<String>,
    /// GCP service account email (GKE Workload Identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_service_account: Option<String>,
    /// Azure client id (Azure Workload Identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_client_id: Option<String>,
}

/// How the operator and the installs authenticate to the storage. Set at
/// most one of the two.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [{
    "rule": "!(has(self.workloadIdentity) && has(self.secretRef))",
    "message": "set only one of workloadIdentity and secretRef"
}]))]
pub struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_identity: Option<WorkloadIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretRef>,
}

/// A named storage system.
#[derive(CustomResource, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[kube(
    group = "loams.dev",
    version = "v1alpha1",
    kind = "StorageBinding",
    plural = "storagebindings",
    shortname = "sb",
    namespaced,
    status = "StorageBindingStatus",
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Summary","type":"string","jsonPath":".status.summary"}"#
)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [{
    "rule": "self.kind != 'object' || has(self.object)",
    "message": "kind object needs an object block"
}]))]
pub struct StorageBindingSpec {
    /// The storage kind. Cannot change after creation.
    #[schemars(extend("x-kubernetes-validations" = [{
        "rule": "self == oldSelf",
        "message": "kind is immutable"
    }]))]
    pub kind: StorageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<ObjectSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_mysql: Option<SqlMysqlSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_postgres: Option<SqlPostgresSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kafka: Option<KafkaSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv: Option<KvSpec>,
    /// Credentials; references only.
    #[serde(default)]
    pub credentials: Credentials,
}

/// What the probe found out about an object store.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectCapabilities {
    pub conditional_put: bool,
    pub range_read: bool,
    pub multipart: bool,
}

/// Observed state of a `StorageBinding`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageBindingStatus {
    /// The spec generation the conditions describe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Conditions; `Ready` is the one to wait for.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// A one-line summary for `kubectl get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The journal of the operation in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationStatus>,
    /// Capabilities the probe found (object kind only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ObjectCapabilities>,
}
