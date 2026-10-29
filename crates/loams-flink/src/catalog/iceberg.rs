//! The Iceberg catalog on Lakekeeper (D1982).
//!
//! Flink's Iceberg catalog (`iceberg-flink-runtime`) against Lakekeeper's
//! REST endpoint, with S3FileIO on Loams object storage (RustFS in compose,
//! S3 in the cloud). `base-namespace = ns_<id>` makes the Flink database `d`
//! the Iceberg namespace `ns_<id>.d`, which is the House database `d`
//! (design 49, D771 and §5.3): a table Flink writes is a House external
//! table with no registration step.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::quote::{Secret, SqlError, WithOptions};
use super::{CatalogDdl, check_catalog_name, check_http_url};

/// The catalog name sessions get unless the spec says otherwise.
pub const DEFAULT_LAKE_CATALOG: &str = "loams";
/// Lakekeeper's REST catalog on the compose network.
pub const DEFAULT_LAKEKEEPER_URI: &str = "http://lakekeeper:8181/catalog";

fn default_name() -> String {
    DEFAULT_LAKE_CATALOG.to_string()
}

fn default_uri() -> String {
    DEFAULT_LAKEKEEPER_URI.to_string()
}

/// How the catalog authenticates to Lakekeeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IcebergAuth {
    /// No credentials (a local Lakekeeper with authentication off).
    None,
    /// OAuth2 client credentials against the identity provider's token
    /// endpoint (Zitadel, design 76): Iceberg's `credential` option is
    /// `<client_id>:<client_secret>`.
    #[serde(rename = "oauth2")]
    OAuth2 {
        server_uri: String,
        client_id: String,
        client_secret: Secret,
        #[serde(default)]
        scope: Option<String>,
    },
    /// A bearer token (a Loams token minted for the session).
    Token { token: Secret },
}

/// How the table files are reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum S3Access {
    /// Lakekeeper vends short-lived, prefix-scoped credentials per table
    /// (`X-Iceberg-Access-Delegation: vended-credentials`). The default.
    Vended,
    /// Static keys. For local development only.
    Static {
        access_key_id: String,
        secret_access_key: Secret,
    },
}

/// S3FileIO settings for Loams object storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Storage {
    /// `http://rustfs:9000` in compose; `None` for AWS S3.
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default = "default_region")]
    pub region: String,
    /// Path-style addressing: required for RustFS and MinIO.
    #[serde(default = "default_true")]
    pub path_style: bool,
    #[serde(default = "default_access")]
    pub access: S3Access,
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_true() -> bool {
    true
}

fn default_access() -> S3Access {
    S3Access::Vended
}

impl Default for S3Storage {
    fn default() -> Self {
        Self {
            endpoint: None,
            region: default_region(),
            path_style: true,
            access: S3Access::Vended,
        }
    }
}

/// The Iceberg catalog a session gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IcebergCatalog {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_uri")]
    pub uri: String,
    /// The Lakekeeper warehouse (one per cluster).
    pub warehouse: String,
    /// `ns_<id>`: the Loams namespace's Iceberg namespace. `None` lists the
    /// warehouse's top-level namespaces as databases.
    #[serde(default)]
    pub base_namespace: Option<String>,
    #[serde(default = "default_auth")]
    pub auth: IcebergAuth,
    #[serde(default)]
    pub storage: S3Storage,
    /// More catalog options, after the generated ones (a generated key
    /// given again is refused).
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

fn default_auth() -> IcebergAuth {
    IcebergAuth::None
}

impl IcebergCatalog {
    /// A catalog for Loams namespace `ns_id` in `warehouse`, with vended
    /// credentials and no authentication (fill `auth` before use outside a
    /// local stack).
    pub fn for_namespace(warehouse: impl Into<String>, ns_id: &str) -> Self {
        Self {
            name: default_name(),
            uri: default_uri(),
            warehouse: warehouse.into(),
            base_namespace: Some(format!("ns_{ns_id}")),
            auth: IcebergAuth::None,
            storage: S3Storage::default(),
            extra: BTreeMap::new(),
        }
    }

    pub fn ddl(&self) -> Result<CatalogDdl, SqlError> {
        check_catalog_name(&self.name)?;
        check_http_url("iceberg uri", &self.uri)?;
        if self.warehouse.trim().is_empty() {
            return Err(SqlError::Invalid("iceberg warehouse is empty".into()));
        }
        let mut o = WithOptions::new();
        o.set("type", "iceberg")
            .set("catalog-type", "rest")
            .set("uri", self.uri.clone())
            .set("warehouse", self.warehouse.clone());
        if let Some(ns) = &self.base_namespace {
            if ns.is_empty() || ns.contains('.') {
                return Err(SqlError::Invalid(format!(
                    "base namespace {ns:?} must be one non-empty level"
                )));
            }
            o.set("base-namespace", ns.clone());
        }
        match &self.auth {
            IcebergAuth::None => {}
            IcebergAuth::OAuth2 {
                server_uri,
                client_id,
                client_secret,
                scope,
            } => {
                check_http_url("oauth2 server uri", server_uri)?;
                if client_id.is_empty() || client_id.contains(':') {
                    return Err(SqlError::Invalid(
                        "oauth2 client id is empty or has a ':'".into(),
                    ));
                }
                let mut credential = super::quote::OptValue::text(format!("{client_id}:"));
                credential.0.push(super::quote::Part::Secret {
                    secret: client_secret.clone(),
                    escape: super::quote::Escape::None,
                });
                o.set("oauth2-server-uri", server_uri.clone())
                    .set_value("credential", credential)
                    .set_opt("scope", scope.clone());
            }
            IcebergAuth::Token { token } => {
                o.set_secret("token", token.clone());
            }
        }
        o.set("io-impl", "org.apache.iceberg.aws.s3.S3FileIO");
        if let Some(endpoint) = &self.storage.endpoint {
            check_http_url("s3 endpoint", endpoint)?;
            o.set("s3.endpoint", endpoint.clone());
        }
        o.set(
            "s3.path-style-access",
            if self.storage.path_style {
                "true"
            } else {
                "false"
            },
        )
        .set("client.region", self.storage.region.clone());
        match &self.storage.access {
            S3Access::Vended => {
                o.set("header.X-Iceberg-Access-Delegation", "vended-credentials");
            }
            S3Access::Static {
                access_key_id,
                secret_access_key,
            } => {
                o.set("s3.access-key-id", access_key_id.clone())
                    .set_secret("s3.secret-access-key", secret_access_key.clone());
            }
        }
        o.extend(&self.extra);
        Ok(CatalogDdl {
            name: self.name.clone(),
            options: o,
        })
    }
}
