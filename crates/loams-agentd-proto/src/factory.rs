//! The software factory's app connections (SF1 amendment F1-F6; design §50 §13, D795).
//!
//! The daemon holds the apps' credentials and runs the read-only op allowlist. No
//! reply carries a secret: [`FactoryConfigureParams`] is the only type that holds one,
//! and its `Debug` prints the field names only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// An app's state as the factory pages show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FactoryHealth {
    /// Its last check answered.
    Ok,
    /// It rejected the stored credentials.
    AuthFailed,
    /// It did not answer.
    Unreachable,
    /// Nothing is stored for it.
    Unconfigured,
    /// Its credentials are stored but cannot be read (the keyring changed).
    Locked,
}

/// One field of an app's configure form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryCredentialField {
    pub key: String,
    pub label: String,
    /// Kept in the keyring and never returned.
    pub secret: bool,
}

/// One factory app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryAppInfo {
    /// `forgejo`, `zulip`, `plane`, `glitchtip`, `openpanel`, `matomo`, `langfuse` or
    /// `signoz`.
    pub id: String,
    pub label: String,
    /// The configured URL (for Plane, its API's).
    pub url: Option<String>,
    pub health: FactoryHealth,
    /// Whether the factory has native panels for it (else its own UI only).
    pub has_panels: bool,
    pub credential_fields: Vec<FactoryCredentialField>,
    /// The non-secret fields' stored values (project key, App URL, SSO origin).
    pub fields: Option<BTreeMap<String, String>>,
    /// False when there is no keyring: secrets last until the daemon stops.
    pub persistent: bool,
    /// The honest status label while the app is a preview (SigNoz on Loams
    /// House, D1666); absent for an app that is not one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// `FactoryList`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryAppList {
    pub apps: Vec<FactoryAppInfo>,
}

/// One app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryAppParams {
    pub app: String,
}

/// `FactoryConfigure`'s params: write-only for secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryConfigureParams {
    pub app: String,
    pub url: String,
    /// By credential field key. A secret left out keeps its stored value while the
    /// URL's origin is unchanged.
    pub fields: BTreeMap<String, String>,
}

impl std::fmt::Debug for FactoryConfigureParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FactoryConfigureParams")
            .field("app", &self.app)
            .field("url", &self.url)
            .field("fields", &self.fields.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Why a factory call failed. `code` is one of `unknown_app`, `unknown_op`,
/// `bad_params`, `bad_fields`, `invalid_url`, `unconfigured`, `unsupported`,
/// `auth_failed`, `unreachable` or `upstream_error`; `message` is scrubbed of every
/// stored secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryError {
    pub code: String,
    pub message: String,
}

/// `FactoryConfigure`'s and `FactoryTest`'s reply: the app, or why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryAppReply {
    pub app: Option<FactoryAppInfo>,
    pub error: Option<FactoryError>,
}

/// One read from the op allowlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryQueryParams {
    pub app: String,
    pub op: String,
    /// The op's parameters, checked by its schema.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// `FactoryQuery`'s reply: the op's value, or why not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FactoryQueryReply {
    pub value: Option<serde_json::Value>,
    pub error: Option<FactoryError>,
}
