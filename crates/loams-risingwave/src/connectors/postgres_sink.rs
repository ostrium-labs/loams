//! Sinks into Loams Postgres (RW-b assumption A8).
//!
//! Two RisingWave connectors write Postgres: `jdbc` (the long-standing one,
//! through pgJDBC in RisingWave's connector node) and `postgres` (native,
//! newer). Both take `type = 'append-only' | 'upsert'` and a primary key for
//! upserts; the target table must exist with that key as its primary key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::postgres_cdc::PostgresEndpoint;
use super::{Ddl, DdlError, ObjectName, Options, RenderContext, SinkFrom, SinkMode, sink_head};

/// Which RisingWave connector writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostgresConnector {
    #[default]
    Jdbc,
    Native,
}

/// `CREATE SINK … WITH (connector = 'jdbc' | 'postgres', …);`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresSink {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub from: SinkFrom,
    pub endpoint: PostgresEndpoint,
    #[serde(default)]
    pub connector: PostgresConnector,
    /// The target schema [default: `public`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub table: String,
    pub mode: SinkMode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl PostgresSink {
    pub fn new(
        name: impl Into<ObjectName>,
        from: SinkFrom,
        endpoint: PostgresEndpoint,
        table: impl Into<String>,
        mode: SinkMode,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            from,
            endpoint,
            connector: PostgresConnector::Jdbc,
            schema: None,
            table: table.into(),
            mode,
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn connector(mut self, connector: PostgresConnector) -> Self {
        self.connector = connector;
        self
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// `jdbc:postgresql://host:port/db?sslmode=…`, with the host and the
    /// database percent-encoded where they would break the URL.
    fn jdbc_url(&self) -> String {
        let endpoint = &self.endpoint;
        let host = if endpoint.host.contains(':') && !endpoint.host.starts_with('[') {
            format!("[{}]", endpoint.host)
        } else {
            endpoint.host.clone()
        };
        format!(
            "jdbc:postgresql://{host}:{}/{}?sslmode={}",
            endpoint.port,
            encode_path(&endpoint.database),
            endpoint.ssl_mode.libpq()
        )
    }
}

/// Percent-encodes what a URL path segment cannot hold.
fn encode_path(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl Ddl for PostgresSink {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        self.endpoint.check()?;
        if self.table.is_empty() {
            return Err(DdlError::Missing("table"));
        }
        let mut options = Options::new();
        match self.connector {
            PostgresConnector::Jdbc => {
                options.set("connector", "jdbc");
                options.set("jdbc.url", self.jdbc_url());
                options.set("user", self.endpoint.user.clone());
                options.set_credential("password", &self.endpoint.password);
                options.set_opt("schema.name", self.schema.clone());
                options.set("table.name", self.table.clone());
            }
            PostgresConnector::Native => {
                options.set("connector", "postgres");
                options.set("host", self.endpoint.host.clone());
                options.set("port", self.endpoint.port.to_string());
                options.set("user", self.endpoint.user.clone());
                options.set_credential("password", &self.endpoint.password);
                options.set("database", self.endpoint.database.clone());
                options.set_opt("schema", self.schema.clone());
                options.set("table", self.table.clone());
                options.set("ssl_mode", self.endpoint.ssl_mode.risingwave());
            }
        }
        self.mode.apply(&mut options)?;
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
    use super::super::Credential;
    use super::super::postgres_cdc::SslMode;
    use super::*;
    use crate::connectors::testing::ctx;

    fn app_db() -> PostgresEndpoint {
        PostgresEndpoint::loams_compute(
            "compute1",
            "app",
            "rw_writer",
            Credential::env("RW_PG_PASSWORD"),
        )
    }

    #[test]
    fn jdbc_upsert_sink() {
        let sink = PostgresSink::new(
            "user_totals_pg",
            SinkFrom::relation("user_totals"),
            app_db(),
            "user_totals",
            SinkMode::upsert(["user_id"]),
        )
        .schema("analytics");
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK "user_totals_pg" FROM "user_totals"
WITH (
    connector = 'jdbc',
    jdbc.url = 'jdbc:postgresql://compute1:55433/app?sslmode=require',
    user = 'rw_writer',
    password = '${RW_PG_PASSWORD}',
    schema.name = 'analytics',
    table.name = 'user_totals',
    type = 'upsert',
    primary_key = 'user_id'
);"#
        );
    }

    #[test]
    fn native_append_only_sink() {
        let sink = PostgresSink::new(
            "audit_pg",
            SinkFrom::relation("audit_events"),
            app_db().ssl_mode(SslMode::VerifyFull),
            "audit_events",
            SinkMode::AppendOnly,
        )
        .connector(PostgresConnector::Native)
        .if_not_exists();
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK IF NOT EXISTS "audit_pg" FROM "audit_events"
WITH (
    connector = 'postgres',
    host = 'compute1',
    port = '55433',
    user = 'rw_writer',
    password = '${RW_PG_PASSWORD}',
    database = 'app',
    table = 'audit_events',
    ssl_mode = 'verify-full',
    type = 'append-only'
);"#
        );
    }

    #[test]
    fn jdbc_urls_encode_odd_names() {
        let mut endpoint = app_db();
        endpoint.database = "my db?x".to_owned();
        endpoint.host = "fd7a:115c::1".to_owned();
        let sink = PostgresSink::new(
            "s",
            SinkFrom::relation("m"),
            endpoint,
            "t",
            SinkMode::AppendOnly,
        );
        assert_eq!(
            sink.jdbc_url(),
            "jdbc:postgresql://[fd7a:115c::1]:55433/my%20db%3Fx?sslmode=require"
        );
    }

    #[test]
    fn postgres_sink_checks() {
        let mut sink = PostgresSink::new(
            "s",
            SinkFrom::relation("m"),
            app_db(),
            "",
            SinkMode::AppendOnly,
        );
        assert!(sink.render(&ctx()).is_err());
        sink.table = "t".to_owned();
        sink.mode = SinkMode::upsert(Vec::<String>::new());
        assert!(sink.render(&ctx()).is_err());
    }
}
