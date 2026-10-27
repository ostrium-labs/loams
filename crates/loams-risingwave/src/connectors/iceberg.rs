//! Iceberg sinks through Lakekeeper, Loams' one table catalog (design 02
//! §7.3, design 32 D333; design 81 R9, and R26).
//!
//! RisingWave's `iceberg` sink commits through the REST catalog. Lakekeeper
//! vends short-lived storage credentials for the table's location when the
//! client asks with `X-Iceberg-Access-Delegation: vended-credentials`, so
//! RisingWave holds no long-lived bucket keys.
//!
//! - Compose ([`RestCatalog::loams_compose`]): Lakekeeper on the internal
//!   `lake` network with auth off (single-tenant preview), warehouse `loams`
//!   on bucket `loams-lake`.
//! - Kubernetes ([`RestCatalog::oauth2`]): OIDC through Zitadel with the
//!   instance's own Lakekeeper client (R5).
//!
//! The catalog is called `loams` in every engine (R26), and a Loams
//! namespace's Iceberg namespace is `ns_<id>` (+ `.<db>`, [`iceberg_namespace`]).
//! Sinks into Iceberg are exactly-once (RisingWave commits on checkpoints).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    Credential, Ddl, DdlError, ObjectName, Options, RenderContext, SinkFrom, SinkMode, sink_head,
};

/// Lakekeeper's catalog root on the stack's network.
pub const LOAMS_LAKEKEEPER_CATALOG: &str = "http://lakekeeper:8181/catalog";

/// The Lakekeeper warehouse every engine writes (R9).
pub const LOAMS_WAREHOUSE: &str = "loams";

/// The catalog's name in every engine (R26).
pub const LOAMS_CATALOG_NAME: &str = "loams";

/// The Iceberg namespace of a Loams namespace: `ns_<id>`, or `ns_<id>.<db>`
/// (R9). The id is lower-cased, and every character Iceberg names should not
/// hold becomes `_`.
pub fn iceberg_namespace(namespace_id: &str, database: Option<&str>) -> String {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    match database {
        Some(db) => format!("ns_{}.{}", clean(namespace_id), clean(db)),
        None => format!("ns_{}", clean(namespace_id)),
    }
}

/// The REST header that asks the catalog to vend storage credentials.
pub const VENDED_CREDENTIALS_HEADER: &str = "X-Iceberg-Access-Delegation=vended-credentials";

/// How RisingWave authenticates to the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum CatalogAuth {
    /// No authentication (Lakekeeper's `allowall` authorizer in a local stack).
    None,
    /// OAuth2 client credentials. `credential` holds `client_id:client_secret`
    /// (as RisingWave's `catalog.credential` expects), normally as a secret.
    #[serde(rename = "oauth2")]
    OAuth2 {
        credential: Credential,
        /// The token endpoint (Zitadel's, design 76) [default: the catalog's
        /// own `/v1/oauth/tokens`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// A bearer token obtained elsewhere.
    Token { token: Credential },
}

/// Static S3 settings, for a catalog that does not vend credentials or for
/// the endpoint override RustFS needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default)]
    pub path_style_access: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key: Option<Credential>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<Credential>,
}

/// The REST catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestCatalog {
    pub uri: String,
    /// The Lakekeeper warehouse (its name, sent as the REST `warehouse`
    /// parameter).
    pub warehouse: String,
    /// RisingWave's name for the catalog (`catalog.name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub auth: CatalogAuth,
    /// Ask Lakekeeper for vended storage credentials [default: true].
    #[serde(default = "yes")]
    pub vended_credentials: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Settings>,
    /// Extra REST headers, `name=value`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
}

fn yes() -> bool {
    true
}

impl RestCatalog {
    /// The compose stack's Lakekeeper (R9): `http://lakekeeper:8181/catalog`,
    /// warehouse `loams`, catalog name `loams`, auth off on the internal
    /// `lake` network, credential vending on.
    pub fn loams_compose() -> Self {
        Self {
            uri: LOAMS_LAKEKEEPER_CATALOG.to_owned(),
            warehouse: LOAMS_WAREHOUSE.to_owned(),
            name: Some(LOAMS_CATALOG_NAME.to_owned()),
            auth: CatalogAuth::None,
            vended_credentials: true,
            s3: None,
            headers: Vec::new(),
        }
    }

    /// A Lakekeeper with OIDC (Kubernetes): the instance's OAuth2 client
    /// credentials (`client_id:client_secret`) and Zitadel's token endpoint.
    pub fn oauth2(
        uri: impl Into<String>,
        client_credentials: Credential,
        token_endpoint: impl Into<String>,
        scope: Option<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            auth: CatalogAuth::OAuth2 {
                credential: client_credentials,
                server_uri: Some(token_endpoint.into()),
                scope,
            },
            ..Self::loams_compose()
        }
    }

    pub fn with_warehouse(mut self, warehouse: impl Into<String>) -> Self {
        self.warehouse = warehouse.into();
        self
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = uri.into();
        self
    }

    pub fn with_token_endpoint(mut self, uri: impl Into<String>, scope: Option<String>) -> Self {
        if let CatalogAuth::OAuth2 {
            server_uri,
            scope: s,
            ..
        } = &mut self.auth
        {
            *server_uri = Some(uri.into());
            *s = scope;
        }
        self
    }

    pub fn with_s3(mut self, s3: S3Settings) -> Self {
        self.s3 = Some(s3);
        self
    }

    fn apply(&self, options: &mut Options) -> Result<(), DdlError> {
        if self.uri.is_empty() {
            return Err(DdlError::Missing("catalog uri"));
        }
        if self.warehouse.is_empty() {
            return Err(DdlError::Missing("warehouse"));
        }
        options.set("catalog.type", "rest");
        options.set("catalog.uri", self.uri.clone());
        options.set_opt("catalog.name", self.name.clone());
        options.set("warehouse.path", self.warehouse.clone());
        match &self.auth {
            CatalogAuth::None => {}
            CatalogAuth::OAuth2 {
                credential,
                server_uri,
                scope,
            } => {
                options.set_credential("catalog.credential", credential);
                options.set_opt("catalog.oauth2_server_uri", server_uri.clone());
                options.set_opt("catalog.scope", scope.clone());
            }
            CatalogAuth::Token { token } => {
                options.set_credential("catalog.token", token);
            }
        }
        let mut headers = Vec::new();
        if self.vended_credentials {
            headers.push(VENDED_CREDENTIALS_HEADER.to_owned());
        }
        for header in &self.headers {
            let Some((name, _)) = header.split_once('=') else {
                return Err(DdlError::Invalid(format!(
                    "header {header:?} is not name=value"
                )));
            };
            if name.is_empty() || header.contains(';') {
                return Err(DdlError::Invalid(format!(
                    "header {header:?} is not name=value"
                )));
            }
            headers.push(header.clone());
        }
        if !headers.is_empty() {
            options.set("catalog.header", headers.join(";"));
        }
        if let Some(s3) = &self.s3 {
            options.set_opt("s3.endpoint", s3.endpoint.clone());
            options.set_opt("s3.region", s3.region.clone());
            if s3.path_style_access {
                options.set("s3.path.style.access", "true");
            }
            match (&s3.access_key, &s3.secret_key) {
                (Some(access), Some(secret)) => {
                    if self.vended_credentials {
                        return Err(DdlError::Invalid(
                            "static S3 keys and vended credentials exclude each other".to_owned(),
                        ));
                    }
                    options.set_credential("s3.access.key", access);
                    options.set_credential("s3.secret.key", secret);
                }
                (None, None) => {}
                _ => {
                    return Err(DdlError::Invalid(
                        "S3 access_key and secret_key go together".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// `CREATE SINK … WITH (connector = 'iceberg', …);`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IcebergSink {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub from: SinkFrom,
    pub catalog: RestCatalog,
    /// The Iceberg namespace.
    pub database: String,
    pub table: String,
    pub mode: SinkMode,
    /// Create the Iceberg table from the sink's schema when it is missing
    /// [default: true].
    #[serde(default = "yes")]
    pub create_table_if_not_exists: bool,
    /// Commit every N checkpoints [default: 10, about every 10 s at
    /// RisingWave's default 1 s barrier].
    #[serde(default = "default_commit_interval")]
    pub commit_checkpoint_interval: u32,
    /// A partition spec for a table the sink creates: `day(ts), bucket(16, id)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_by: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

fn default_commit_interval() -> u32 {
    10
}

impl IcebergSink {
    pub fn new(
        name: impl Into<ObjectName>,
        from: SinkFrom,
        catalog: RestCatalog,
        database: impl Into<String>,
        table: impl Into<String>,
        mode: SinkMode,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            from,
            catalog,
            database: database.into(),
            table: table.into(),
            mode,
            create_table_if_not_exists: true,
            commit_checkpoint_interval: default_commit_interval(),
            partition_by: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn commit_every(mut self, checkpoints: u32) -> Self {
        self.commit_checkpoint_interval = checkpoints;
        self
    }

    pub fn partition_by(mut self, spec: impl Into<String>) -> Self {
        self.partition_by = Some(spec.into());
        self
    }

    pub fn create_table(mut self, on: bool) -> Self {
        self.create_table_if_not_exists = on;
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

impl Ddl for IcebergSink {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if self.database.is_empty() {
            return Err(DdlError::Missing("database"));
        }
        if self.table.is_empty() {
            return Err(DdlError::Missing("table"));
        }
        if self.commit_checkpoint_interval == 0 {
            return Err(DdlError::Invalid(
                "commit_checkpoint_interval must be at least 1".to_owned(),
            ));
        }
        let mut options = Options::new();
        options.set("connector", "iceberg");
        self.mode.apply(&mut options)?;
        self.catalog.apply(&mut options)?;
        options.set("database.name", self.database.clone());
        options.set("table.name", self.table.clone());
        if self.create_table_if_not_exists {
            options.set("create_table_if_not_exists", "true");
        }
        options.set(
            "commit_checkpoint_interval",
            self.commit_checkpoint_interval.to_string(),
        );
        options.set_opt("partition_by", self.partition_by.clone());
        options.extend(&self.extra);
        Ok(format!(
            "{}\n{};",
            sink_head(&self.name, self.if_not_exists, &self.from)?,
            options.render_with(ctx)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::testing::ctx;

    #[test]
    fn upsert_sink_through_lakekeeper_with_vended_credentials() {
        let sink = IcebergSink::new(
            "clicks_lake",
            SinkFrom::relation("clicks_per_minute"),
            RestCatalog::oauth2(
                "https://lakekeeper.loams.internal/catalog",
                Credential::env("RW_LAKEKEEPER_CLIENT"),
                "https://auth.loams.internal/oauth/v2/token",
                Some("lakekeeper".to_owned()),
            ),
            iceberg_namespace("01JBZQ", Some("web")),
            "clicks_per_minute",
            SinkMode::upsert(["window_start", "url"]),
        )
        .partition_by("day(window_start)");
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK "clicks_lake" FROM "clicks_per_minute"
WITH (
    connector = 'iceberg',
    type = 'upsert',
    primary_key = 'window_start,url',
    catalog.type = 'rest',
    catalog.uri = 'https://lakekeeper.loams.internal/catalog',
    catalog.name = 'loams',
    warehouse.path = 'loams',
    catalog.credential = '${RW_LAKEKEEPER_CLIENT}',
    catalog.oauth2_server_uri = 'https://auth.loams.internal/oauth/v2/token',
    catalog.scope = 'lakekeeper',
    catalog.header = 'X-Iceberg-Access-Delegation=vended-credentials',
    database.name = 'ns_01jbzq.web',
    table.name = 'clicks_per_minute',
    create_table_if_not_exists = 'true',
    commit_checkpoint_interval = '10',
    partition_by = 'day(window_start)'
);"#
        );
    }

    #[test]
    fn append_only_sink_with_static_keys_on_rustfs() {
        let mut catalog = RestCatalog::loams_compose().with_warehouse("dev");
        catalog.vended_credentials = false;
        let catalog = catalog.with_s3(S3Settings {
            endpoint: Some("http://rustfs:9000".to_owned()),
            region: Some("us-east-1".to_owned()),
            path_style_access: true,
            access_key: Some(Credential::env("RW_S3_ACCESS_KEY")),
            secret_key: Some(Credential::env("RW_S3_SECRET_KEY")),
        });
        let sink = IcebergSink::new(
            "events_lake",
            SinkFrom::relation("events"),
            catalog,
            "raw",
            "events",
            SinkMode::ForceAppendOnly,
        )
        .if_not_exists()
        .create_table(false)
        .commit_every(60);
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK IF NOT EXISTS "events_lake" FROM "events"
WITH (
    connector = 'iceberg',
    type = 'append-only',
    force_append_only = 'true',
    catalog.type = 'rest',
    catalog.uri = 'http://lakekeeper:8181/catalog',
    catalog.name = 'loams',
    warehouse.path = 'dev',
    s3.endpoint = 'http://rustfs:9000',
    s3.region = 'us-east-1',
    s3.path.style.access = 'true',
    s3.access.key = '${RW_S3_ACCESS_KEY}',
    s3.secret.key = '${RW_S3_SECRET_KEY}',
    database.name = 'raw',
    table.name = 'events',
    commit_checkpoint_interval = '60'
);"#
        );
    }

    #[test]
    fn compose_sink_has_no_catalog_credential() {
        let sink = IcebergSink::new(
            "clicks_lake",
            SinkFrom::relation("clicks_per_minute"),
            RestCatalog::loams_compose(),
            iceberg_namespace("default", None),
            "clicks_per_minute",
            SinkMode::AppendOnly,
        );
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK "clicks_lake" FROM "clicks_per_minute"
WITH (
    connector = 'iceberg',
    type = 'append-only',
    catalog.type = 'rest',
    catalog.uri = 'http://lakekeeper:8181/catalog',
    catalog.name = 'loams',
    warehouse.path = 'loams',
    catalog.header = 'X-Iceberg-Access-Delegation=vended-credentials',
    database.name = 'ns_default',
    table.name = 'clicks_per_minute',
    create_table_if_not_exists = 'true',
    commit_checkpoint_interval = '10'
);"#
        );
        assert_eq!(
            iceberg_namespace("A-b", Some("Sales Db")),
            "ns_a_b.sales_db"
        );
    }

    #[test]
    fn iceberg_checks() {
        let base = || {
            IcebergSink::new(
                "s",
                SinkFrom::relation("m"),
                RestCatalog::loams_compose(),
                "d",
                "t",
                SinkMode::AppendOnly,
            )
        };
        assert!(base().render(&ctx()).is_ok());
        let mut no_table = base();
        no_table.table.clear();
        assert!(no_table.render(&ctx()).is_err());
        assert!(base().commit_every(0).render(&ctx()).is_err());
        let mut mixed = base();
        mixed.catalog.s3 = Some(S3Settings {
            access_key: Some(Credential::env("a")),
            secret_key: Some(Credential::env("b")),
            ..S3Settings::default()
        });
        assert!(mixed.render(&ctx()).is_err(), "static keys with vending");
        let mut half = base();
        half.catalog.vended_credentials = false;
        half.catalog.s3 = Some(S3Settings {
            access_key: Some(Credential::env("a")),
            ..S3Settings::default()
        });
        assert!(half.render(&ctx()).is_err());
        let mut bad_header = base();
        bad_header.catalog.headers.push("no-equals".to_owned());
        assert!(bad_header.render(&ctx()).is_err());
        let mut headers = base();
        headers
            .catalog
            .headers
            .push("X-Project-Id=loams".to_owned());
        assert!(headers.render(&ctx()).unwrap().contains(
            "catalog.header = 'X-Iceberg-Access-Delegation=vended-credentials;X-Project-Id=loams'"
        ));
    }
}
