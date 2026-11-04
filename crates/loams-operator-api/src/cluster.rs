//! `LoamsCluster`: a home cluster and its platform base (design 86 §4.1).

use std::time::Duration;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Condition, DbRef, OperationStatus, Replicas, Url, duration_string, one};

/// The role of a cluster. Only `home` exists in this version.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClusterRole {
    Home,
}

/// How the platform base is reachable. Never public.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExposureMode {
    /// Over the tailnet.
    #[default]
    Tailnet,
    /// Inside the cluster network only.
    Internal,
}

/// Exposure of the home.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Exposure {
    /// `tailnet` (default) or `internal`. `public` is not an option.
    #[serde(default)]
    pub mode: ExposureMode,
}

/// Who issues the internal certificates.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsIssuer {
    /// The operator's own CA.
    #[default]
    OperatorCa,
    /// cert-manager.
    CertManager,
}

/// Internal TLS.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tls {
    /// `operator-ca` (default) or `cert-manager`.
    #[serde(default)]
    pub issuer: TlsIssuer,
}

/// The object store the home keeps its state in.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterStorage {
    /// Name of an `object` StorageBinding in the same namespace.
    pub object: String,
}

/// Sandbox settings for installs.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Sandbox {
    /// A RuntimeClass to treat as gVisor, besides the ones the operator
    /// detects by handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_class_name: Option<String>,
}

fn default_refresh() -> Duration {
    Duration::from_secs(300)
}

/// Where the home reads the signed marketplace index from.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Marketplace {
    /// `https://` URL of the index, or `file:///etc/loams/marketplace/...`.
    #[schemars(extend("x-kubernetes-validations" = [{
        "rule": "self.startsWith('https://') || self.startsWith('file:///etc/loams/marketplace/')",
        "message": "index must be an https:// URL or a file:///etc/loams/marketplace/ path"
    }]))]
    pub index: Url,
    /// PEM public key the index is verified with. Empty means the official
    /// keyless trust anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// How often the index is fetched again, as a duration string.
    #[serde(default = "default_refresh", with = "duration_string")]
    #[schemars(schema_with = "refresh_schema")]
    pub refresh: Duration,
}

fn refresh_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "description": "How often the index is fetched again, as a duration string.",
        "default": "5m"
    })
}

/// Which identity provider Loams Access runs.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityProvider {
    /// Official Zitadel (default).
    #[default]
    Zitadel,
    /// `loams-identity`; renders only when its image is pinned.
    LoamsIdentity,
}

/// The identity provider of Loams Access.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessIdentity {
    /// `zitadel` (default) or `loams-identity`.
    #[serde(default)]
    pub provider: IdentityProvider,
    /// Number of replicas.
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub replicas: u32,
    /// The external domain the provider answers on.
    pub external_domain: String,
    /// The provider's Postgres database.
    pub database: DbRef,
}

/// OpenFGA, the permission store of Loams Access.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenFga {
    /// Number of replicas.
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub replicas: u32,
    /// OpenFGA's Postgres database.
    pub database: DbRef,
}

/// Where Access audit events (CloudEvents) go.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    /// Optional URL that receives the events over HTTP (binary mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sink: Option<Url>,
}

/// Loams Access: identity, permissions and usage (design 86 §4.3).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// The tenant tree's org id.
    pub org: String,
    /// The identity provider.
    pub identity: AccessIdentity,
    /// OpenFGA.
    pub openfga: OpenFga,
    /// The Meter.
    #[serde(default)]
    pub meter: Replicas,
    /// Audit delivery.
    #[serde(default)]
    pub audit: Audit,
}

/// The platform base every home runs.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBase {
    /// The gateway.
    #[serde(default)]
    pub gateway: Replicas,
    /// agentd.
    #[serde(default)]
    pub agentd: Replicas,
    /// Loams Access.
    pub access: Access,
}

/// A home cluster. Its platform base is rendered from this spec; engines
/// are `LoamsInstall`s.
#[derive(CustomResource, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[kube(
    group = "loams.dev",
    version = "v1alpha1",
    kind = "LoamsCluster",
    plural = "loamsclusters",
    shortname = "lc",
    namespaced,
    status = "LoamsClusterStatus",
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Summary","type":"string","jsonPath":".status.summary"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct LoamsClusterSpec {
    /// The cluster's role. Only `home`.
    pub role: ClusterRole,
    /// The Loams release, which resolves to a pinned component set.
    pub release: String,
    /// Exposure (default `tailnet`).
    #[serde(default)]
    pub exposure: Exposure,
    /// Internal TLS.
    #[serde(default)]
    pub tls: Tls,
    /// The object store of the home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<ClusterStorage>,
    /// Sandbox settings.
    #[serde(default)]
    pub sandbox: Sandbox,
    /// The marketplace index.
    pub marketplace: Marketplace,
    /// The platform base.
    pub base: PlatformBase,
}

/// Observed state of a `LoamsCluster`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoamsClusterStatus {
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
}
