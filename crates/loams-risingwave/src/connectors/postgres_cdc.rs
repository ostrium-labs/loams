//! Loams Postgres change data capture into RisingWave (design 81 R12).
//!
//! RisingWave's built-in `postgres-cdc` connector (Debezium's embedded engine
//! inside RisingWave's connector node) reads logical replication from one
//! database through one slot and one publication. The pattern is one shared
//! source per database and one table per upstream table:
//!
//! ```sql
//! CREATE SOURCE "pg_shop" WITH (connector = 'postgres-cdc', …);
//! CREATE TABLE "orders" (*) FROM "pg_shop" TABLE 'public.orders';
//! ```
//!
//! The engines controller, not RisingWave, creates the publication, under a
//! CDC role per source (R12), so the builder sets
//! `publication.create.enable = 'false'`. Slots are named
//! `loams_rw_<instance26>_<n>` ([`cdc_slot_name`]) and dropped with the
//! source and the instance. Loams Postgres (design 46, the Neon fork) needs
//! logical replication on for the branch (`wal_level = logical`, verify).
//! Design 33 D357 keeps Loams' own bridges as the default CDC path into the
//! Fabric; this connector is for RisingWave's own use of a database.
//! RisingWave's automatic schema change for CDC is Premium (R4), so it is not
//! offered here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Credential, Ddl, DdlError, ObjectName, Options, RenderContext, Schema, quote_literal};

/// The port of a Loams Postgres compute on the stack's network (direct
/// endpoints, `compute1:55433` and `ep-<ulid>:55433`).
pub const LOAMS_COMPUTE_PORT: u16 = 55433;

/// TLS to the database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disabled,
    Preferred,
    #[default]
    Required,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    /// The spelling of RisingWave's `postgres-cdc` and `postgres` connectors.
    pub fn risingwave(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Preferred => "preferred",
            Self::Required => "required",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }

    /// The spelling of libpq and pgJDBC's `sslmode`.
    pub fn libpq(self) -> &'static str {
        match self {
            Self::Disabled => "disable",
            Self::Preferred => "prefer",
            Self::Required => "require",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }
}

/// A Loams Postgres endpoint and the role RisingWave signs in as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresEndpoint {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: Credential,
    #[serde(default)]
    pub ssl_mode: SslMode,
}

fn default_port() -> u16 {
    LOAMS_COMPUTE_PORT
}

impl PostgresEndpoint {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        database: impl Into<String>,
        user: impl Into<String>,
        password: Credential,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            database: database.into(),
            user: user.into(),
            password,
            ssl_mode: SslMode::Required,
        }
    }

    /// A Loams Postgres compute reached directly on the stack's network.
    pub fn loams_compute(
        host: impl Into<String>,
        database: impl Into<String>,
        user: impl Into<String>,
        password: Credential,
    ) -> Self {
        Self::new(host, LOAMS_COMPUTE_PORT, database, user, password)
    }

    pub fn ssl_mode(mut self, mode: SslMode) -> Self {
        self.ssl_mode = mode;
        self
    }

    pub(super) fn check(&self) -> Result<(), DdlError> {
        if self.host.is_empty() {
            return Err(DdlError::Missing("host"));
        }
        if self.database.is_empty() {
            return Err(DdlError::Missing("database"));
        }
        if self.user.is_empty() {
            return Err(DdlError::Missing("user"));
        }
        if self.port == 0 {
            return Err(DdlError::Invalid("port 0".to_owned()));
        }
        Ok(())
    }
}

/// `CREATE SOURCE … WITH (connector = 'postgres-cdc', …);`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresCdcSource {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub endpoint: PostgresEndpoint,
    /// The default upstream schema for the tables created from this source.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// The replication slot, `loams_rw_<instance26>_<n>` ([`cdc_slot_name`]).
    pub slot_name: String,
    /// The publication the controller created [default: the slot's name].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_name: Option<String>,
    /// Let RisingWave create the publication [default: false; the
    /// controller creates it, R12].
    #[serde(default)]
    pub create_publication: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

fn default_schema() -> String {
    "public".to_owned()
}

/// `loams_rw_<instance26>_<n>` (R12): the instance id without its `eng_`
/// prefix, lower-cased, with every character a slot name cannot hold
/// (`[a-z0-9_]`) replaced by `_`, cut so the whole name fits Postgres' 63
/// bytes.
pub fn cdc_slot_name(instance_id: &str, n: u32) -> String {
    let id = instance_id.strip_prefix("eng_").unwrap_or(instance_id);
    let suffix = format!("_{n}");
    let mut id: String = id
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    id.truncate(63 - "loams_rw_".len() - suffix.len());
    format!("loams_rw_{id}{suffix}")
}

/// Whether `name` is a valid slot name: `[a-z0-9_]`, 1 to 63 bytes.
fn check_slot_name(name: &str) -> Result<(), DdlError> {
    if name.is_empty()
        || name.len() > 63
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(DdlError::Invalid(format!(
            "{name:?} is not a replication slot name ([a-z0-9_], at most 63 bytes)"
        )));
    }
    Ok(())
}

impl PostgresCdcSource {
    pub fn new(
        name: impl Into<ObjectName>,
        endpoint: PostgresEndpoint,
        slot_name: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            endpoint,
            schema: default_schema(),
            slot_name: slot_name.into(),
            publication_name: None,
            create_publication: false,
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = schema.into();
        self
    }

    pub fn publication_name(mut self, publication: impl Into<String>) -> Self {
        self.publication_name = Some(publication.into());
        self
    }

    /// Let RisingWave create the publication (outside the managed
    /// controller, which creates it itself).
    pub fn create_publication(mut self, on: bool) -> Self {
        self.create_publication = on;
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

impl Ddl for PostgresCdcSource {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        self.endpoint.check()?;
        check_slot_name(&self.slot_name)?;
        let mut options = Options::new();
        options.set("connector", "postgres-cdc");
        options.set("hostname", self.endpoint.host.clone());
        options.set("port", self.endpoint.port.to_string());
        options.set("username", self.endpoint.user.clone());
        options.set_credential("password", &self.endpoint.password);
        options.set("database.name", self.endpoint.database.clone());
        options.set("schema.name", self.schema.clone());
        options.set("slot.name", self.slot_name.clone());
        options.set(
            "publication.name",
            self.publication_name
                .clone()
                .unwrap_or_else(|| self.slot_name.clone()),
        );
        options.set(
            "publication.create.enable",
            if self.create_publication {
                "true"
            } else {
                "false"
            },
        );
        options.set("ssl.mode", self.endpoint.ssl_mode.risingwave());
        options.extend(&self.extra);
        Ok(format!(
            "CREATE SOURCE {}{}\n{};",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name.render()?,
            options.render_with(ctx)?
        ))
    }
}

/// `CREATE TABLE name (…) [WITH (…)] FROM source TABLE 'schema.table';`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdcTable {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    /// Explicit columns with the upstream primary key, or [`Schema::inferred`]
    /// (`(*)`) to take the upstream table's columns.
    pub schema: Schema,
    pub source: ObjectName,
    /// The upstream table, `schema.table` (the source's schema when no
    /// schema is given).
    pub upstream_table: String,
    /// Backfill the existing rows first [default: RisingWave's, true].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl CdcTable {
    pub fn new(
        name: impl Into<ObjectName>,
        source: impl Into<ObjectName>,
        upstream_table: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            schema: Schema::inferred(),
            source: source.into(),
            upstream_table: upstream_table.into(),
            snapshot: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn schema(mut self, schema: Schema) -> Self {
        self.schema = schema;
        self
    }

    pub fn snapshot(mut self, on: bool) -> Self {
        self.snapshot = Some(on);
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

impl Ddl for CdcTable {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if self.upstream_table.is_empty() {
            return Err(DdlError::Missing("upstream_table"));
        }
        if !self.schema.infer && self.schema.primary_key.is_empty() {
            return Err(DdlError::Invalid(
                "a CDC table with explicit columns needs the upstream primary key".to_owned(),
            ));
        }
        if !self.schema.infer && self.schema.columns.is_empty() {
            return Err(DdlError::Missing("columns (or an inferred schema)"));
        }
        let mut options = Options::new();
        if let Some(snapshot) = self.snapshot {
            options.set("snapshot", if snapshot { "true" } else { "false" });
        }
        options.extend(&self.extra);
        let with = if options.is_empty() {
            String::new()
        } else {
            format!("\n{}", options.render_with(ctx)?)
        };
        Ok(format!(
            "CREATE TABLE {}{}{}{}\nFROM {} TABLE {};",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name.render()?,
            self.schema.render()?,
            with,
            self.source.render()?,
            quote_literal(&self.upstream_table)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Column;
    use super::*;
    use crate::connectors::testing::ctx;

    fn shop() -> PostgresEndpoint {
        PostgresEndpoint::loams_compute(
            "ep-01jb6x",
            "shop",
            "rw_cdc",
            Credential::env("RW_CDC_PASSWORD"),
        )
    }

    #[test]
    fn cdc_source_uses_the_controllers_slot_and_publication() {
        let slot = cdc_slot_name("eng_01JBZQ7M3K8X2V5N9C4T6W0R1Y", 1);
        assert_eq!(slot, "loams_rw_01jbzq7m3k8x2v5n9c4t6w0r1y_1");
        let source = PostgresCdcSource::new("pg_shop", shop(), slot).if_not_exists();
        assert_eq!(
            source.render(&ctx()).unwrap(),
            r#"CREATE SOURCE IF NOT EXISTS "pg_shop"
WITH (
    connector = 'postgres-cdc',
    hostname = 'ep-01jb6x',
    port = '55433',
    username = 'rw_cdc',
    password = '${RW_CDC_PASSWORD}',
    database.name = 'shop',
    schema.name = 'public',
    slot.name = 'loams_rw_01jbzq7m3k8x2v5n9c4t6w0r1y_1',
    publication.name = 'loams_rw_01jbzq7m3k8x2v5n9c4t6w0r1y_1',
    publication.create.enable = 'false',
    ssl.mode = 'required'
);"#
        );
    }

    #[test]
    fn cdc_source_with_explicit_names() {
        let source = PostgresCdcSource::new(
            "cdc.Pg-Shop",
            shop().ssl_mode(SslMode::VerifyFull),
            "loams_rw_x_2",
        )
        .schema("sales")
        .publication_name("rw_pub")
        .create_publication(true);
        let sql = source.render(&ctx()).unwrap();
        assert!(
            sql.starts_with("CREATE SOURCE \"cdc\".\"Pg-Shop\"\nWITH ("),
            "{sql}"
        );
        assert!(sql.contains("schema.name = 'sales'"), "{sql}");
        assert!(sql.contains("slot.name = 'loams_rw_x_2'"), "{sql}");
        assert!(sql.contains("publication.name = 'rw_pub'"), "{sql}");
        assert!(sql.contains("publication.create.enable = 'true'"), "{sql}");
        assert!(sql.contains("ssl.mode = 'verify-full'"), "{sql}");
        assert!(!sql.contains("auto.schema.change"), "{sql}");
    }

    #[test]
    fn slot_names_are_sanitised_and_bounded() {
        let slot = cdc_slot_name(&"X".repeat(100), 12);
        assert_eq!(slot.len(), 63);
        assert!(slot.starts_with("loams_rw_xxx"));
        assert!(slot.ends_with("x_12"));
        assert_eq!(cdc_slot_name("eng_a b-c", 0), "loams_rw_a_b_c_0");
        assert!(
            PostgresCdcSource::new("s", shop(), "Bad-Slot")
                .render(&ctx())
                .is_err()
        );
        assert!(
            PostgresCdcSource::new("s", shop(), "")
                .render(&ctx())
                .is_err()
        );
    }

    #[test]
    fn cdc_table_with_inferred_columns() {
        let table = CdcTable::new("orders", "pg_shop", "public.orders");
        assert_eq!(
            table.render(&ctx()).unwrap(),
            "CREATE TABLE \"orders\" (*)\nFROM \"pg_shop\" TABLE 'public.orders';"
        );
    }

    #[test]
    fn cdc_table_with_explicit_columns_and_no_snapshot() {
        let table = CdcTable::new("customers", "pg_shop", "public.customers")
            .if_not_exists()
            .schema(
                Schema::columns([Column::new("id", "INT"), Column::new("email", "VARCHAR")])
                    .with_primary_key(["id"]),
            )
            .snapshot(false);
        assert_eq!(
            table.render(&ctx()).unwrap(),
            r#"CREATE TABLE IF NOT EXISTS "customers" (
    "id" INT,
    "email" VARCHAR,
    PRIMARY KEY ("id")
)
WITH (
    snapshot = 'false'
)
FROM "pg_shop" TABLE 'public.customers';"#
        );
    }

    #[test]
    fn cdc_table_checks() {
        let keyless =
            CdcTable::new("t", "s", "public.t").schema(Schema::columns([Column::new("a", "INT")]));
        assert!(keyless.render(&ctx()).is_err());
        assert!(CdcTable::new("t", "s", "").render(&ctx()).is_err());
        let mut no_host = shop();
        no_host.host.clear();
        assert!(
            PostgresCdcSource::new("s", no_host, "loams_rw_x_1")
                .render(&ctx())
                .is_err()
        );
    }

    #[test]
    fn ssl_mode_spellings() {
        assert_eq!(SslMode::Required.risingwave(), "required");
        assert_eq!(SslMode::Required.libpq(), "require");
        assert_eq!(SslMode::Disabled.libpq(), "disable");
    }
}
