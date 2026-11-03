//! `LoamsInstall`: one listing installed into a home (design 86 §4.2).

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::{Condition, OperationStatus};

/// A listing id and version.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-kubernetes-validations" = [{
    "rule": "self.id == oldSelf.id",
    "message": "listing.id is immutable"
}]))]
pub struct ListingRef {
    /// The listing id, `<namespace>/<name>`. Cannot change after creation.
    pub id: String,
    /// The listing version. Changing it is an upgrade.
    pub version: String,
}

/// A storage need of the listing, wired to a StorageBinding.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageRef {
    /// The listing's name for this storage.
    pub name: String,
    /// Name of the StorageBinding in the same namespace.
    pub binding: String,
}

/// How the install is reachable.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InstallExposureMode {
    /// Not exposed outside its namespace (default).
    #[default]
    None,
    Tailnet,
    Internal,
    /// Accepted by the schema, refused by the controller in this version.
    GatewayEdge,
}

/// Exposure of an install.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallExposure {
    /// `none` (default), `tailnet`, `internal` or `gateway-edge`.
    #[serde(default)]
    pub mode: InstallExposureMode,
}

/// Sandbox choices of an install.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallSandbox {
    /// Allow a Community listing to run without gVisor. The install then
    /// reports `Sandboxed=False` and a Warning.
    #[serde(default)]
    pub allow_without_gvisor: bool,
}

fn default_project() -> String {
    "default".to_owned()
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn values_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true
    })
}

/// A listing installed into a home, in its own namespace.
#[derive(CustomResource, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[kube(
    group = "loams.dev",
    version = "v1alpha1",
    kind = "LoamsInstall",
    plural = "loamsinstalls",
    shortname = "li",
    namespaced,
    status = "LoamsInstallStatus",
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Summary","type":"string","jsonPath":".status.summary"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct LoamsInstallSpec {
    /// Name of the home `LoamsCluster` in the same namespace.
    pub cluster: String,
    /// The listing to install.
    pub listing: ListingRef,
    /// Values for the listing; checked against its `values_schema`.
    #[serde(default = "empty_object")]
    #[schemars(schema_with = "values_schema")]
    pub values: Value,
    /// Storage bindings the listing needs.
    #[serde(default)]
    pub storage: Vec<StorageRef>,
    /// Exposure (default `none`).
    #[serde(default)]
    pub exposure: InstallExposure,
    /// The Access project the install belongs to.
    #[serde(default = "default_project")]
    pub project: String,
    /// Sandbox choices.
    #[serde(default)]
    pub sandbox: InstallSandbox,
    /// The digest of the cluster-scoped manifest the user confirmed
    /// (`sha256:<hex>`, from `status.clusterScoped.digest`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_cluster_scoped: Option<String>,
}

/// The Access objects created for an install.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessStatus {
    /// Id of the machine identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,
    /// Whether the OpenFGA tuples are written.
    #[serde(default)]
    pub tuples_written: bool,
    /// The Meter subject (the Access subject id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter_subject: Option<String>,
}

/// The listing as the operator resolved it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedListing {
    /// The trust tier (`community`, `verified`, `first-party`).
    pub tier: String,
    /// `sha256:<hex>` of the verified listing.
    pub digest: String,
}

/// A cluster-scoped object an install needs.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectId {
    pub api_version: String,
    pub kind: String,
    pub name: String,
}

/// Cluster-scoped objects the user must apply and confirm.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterScoped {
    /// The objects, for review.
    #[serde(default)]
    pub objects: Vec<ObjectId>,
    /// `sha256:<hex>` of the manifest in ConfigMap `cluster-scoped`.
    pub digest: String,
}

/// A dependency the operator resolved.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedDependency {
    /// The dependency's listing id.
    pub id: String,
    /// The version chosen.
    pub version: String,
    /// Name of the `LoamsInstall` that provides it.
    pub install: String,
}

/// Observed state of a `LoamsInstall`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoamsInstallStatus {
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
    /// Access objects (identity, tuples, Meter subject).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<AccessStatus>,
    /// The install namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// The resolved listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<ResolvedListing>,
    /// Cluster-scoped objects awaiting confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_scoped: Option<ClusterScoped>,
    /// The dependencies and the installs that provide them.
    #[serde(default)]
    pub dependencies: Vec<ResolvedDependency>,
}
