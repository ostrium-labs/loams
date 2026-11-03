//! `LoamsDevice` and `LoamsFleet`: edge devices and groups of them
//! (design 86, D2604). Types only; the edge controller (EDGE1) builds on
//! them later.

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, Time};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Condition;
use crate::install::ListingRef;

/// The kind of device.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceClass {
    Laptop,
    Gateway,
    Sensor,
}

/// How the device proves who it is. It holds ids and a fingerprint, never a
/// key or a token.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    /// Name of the Zitadel machine user, if one is bound to the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zitadel_machine_user: Option<String>,
    /// Fingerprint of the device's public key (`sha256:<hex>`).
    pub key_fingerprint: String,
}

/// One edge device.
#[derive(CustomResource, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[kube(
    group = "loams.dev",
    version = "v1alpha1",
    kind = "LoamsDevice",
    plural = "loamsdevices",
    shortname = "ldev",
    namespaced,
    status = "LoamsDeviceStatus",
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Summary","type":"string","jsonPath":".status.summary"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct LoamsDeviceSpec {
    /// Name of the home `LoamsCluster` in the same namespace.
    pub cluster: String,
    /// Name of the `LoamsFleet` the device belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<String>,
    /// How the device is identified.
    pub identity: DeviceIdentity,
    /// The kind of device.
    pub class: DeviceClass,
    /// The listings the device should run.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub desired: Vec<ListingRef>,
}

/// Observed state of a `LoamsDevice`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoamsDeviceStatus {
    /// The spec generation the conditions describe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Conditions; `Ready` is the one to wait for.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// A one-line summary for `kubectl get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// When the device last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Time>,
    /// The listings the device reported as running.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub reported: Vec<ListingRef>,
}

/// A group of devices chosen by labels, with the listings they should run.
#[derive(CustomResource, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[kube(
    group = "loams.dev",
    version = "v1alpha1",
    kind = "LoamsFleet",
    plural = "loamsfleets",
    shortname = "lfleet",
    namespaced,
    status = "LoamsFleetStatus",
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Summary","type":"string","jsonPath":".status.summary"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct LoamsFleetSpec {
    /// Name of the home `LoamsCluster` in the same namespace.
    pub cluster: String,
    /// Chooses the member devices by label.
    pub selector: LabelSelector,
    /// The listings every member should run.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub desired: Vec<ListingRef>,
}

/// Observed state of a `LoamsFleet`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoamsFleetStatus {
    /// The spec generation the conditions describe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Conditions; `Ready` is the one to wait for.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// A one-line summary for `kubectl get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
