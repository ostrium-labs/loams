//! Loams streams in and out of RisingWave through the Iggy Kafka gateway
//! (design 02 §7.3, design 32 §5.10; design 81 R11, and R5).
//!
//! A Loams stream is an Iggy topic; Kafka clients reach it through the
//! fork's `iggy-gateway-kafka` (compose service `kafka-gateway`, port 9093).
//! RisingWave's built-in `kafka` connector talks to that gateway as it would
//! to a broker. Transactions and exactly-once are not supported by the
//! gateway, so Kafka sinks are at-least-once (D355's idempotency keys or the
//! target's primary key dedupe); exactly-once end to end is Iceberg only.
//!
//! RisingWave signs in as its instance's own Iggy user, minted by the engines
//! facade and scoped to the namespace's topics (R5): never a user's API key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::format::{Encode, Format, FormatSpec};
use super::{
    Credential, Ddl, DdlError, ObjectName, Options, RenderContext, Schema, SinkFrom, quote_ident,
    quote_literal, sink_head,
};

/// The compose address of the Iggy Kafka gateway, on the stack's network.
pub const LOAMS_KAFKA_GATEWAY: &str = "kafka-gateway:9093";

/// The SASL mechanisms the gateway accepts or plans to (design 32 D1797:
/// PLAIN now, SCRAM and OAUTHBEARER planned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaslMechanism {
    #[serde(rename = "PLAIN")]
    Plain,
    #[serde(rename = "SCRAM-SHA-256")]
    ScramSha256,
    #[serde(rename = "SCRAM-SHA-512")]
    ScramSha512,
}

impl SaslMechanism {
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::ScramSha512 => "SCRAM-SHA-512",
        }
    }
}

/// How RisingWave signs in to the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum KafkaAuth {
    /// No authentication: only for a gateway with `KAFKA_SASL_ENABLED=false`
    /// on a private network.
    None,
    Sasl {
        mechanism: SaslMechanism,
        username: String,
        password: Credential,
        /// `SASL_SSL` rather than `SASL_PLAINTEXT`.
        #[serde(default)]
        tls: bool,
        /// A CA bundle path inside the RisingWave containers, for `tls`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_location: Option<String>,
    },
}

/// Where a Loams stream is read or written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaEndpoint {
    /// `host:port[,host:port…]`.
    pub bootstrap_servers: String,
    pub auth: KafkaAuth,
}

impl KafkaEndpoint {
    /// The stack's gateway, signed in as the instance's Iggy user (R5,
    /// R11): SASL/PLAIN (what the gateway checks today), no TLS on the
    /// private compose network. `password` is normally
    /// [`Credential::Env`], filled by the facade.
    pub fn loams_gateway(iggy_user: impl Into<String>, password: Credential) -> Self {
        Self {
            bootstrap_servers: LOAMS_KAFKA_GATEWAY.to_owned(),
            auth: KafkaAuth::Sasl {
                mechanism: SaslMechanism::Plain,
                username: iggy_user.into(),
                password,
                tls: false,
                ca_location: None,
            },
        }
    }

    /// A gateway without SASL (a local stack with `KAFKA_SASL_ENABLED=false`).
    pub fn unauthenticated(bootstrap_servers: impl Into<String>) -> Self {
        Self {
            bootstrap_servers: bootstrap_servers.into(),
            auth: KafkaAuth::None,
        }
    }

    pub fn with_bootstrap(mut self, bootstrap_servers: impl Into<String>) -> Self {
        self.bootstrap_servers = bootstrap_servers.into();
        self
    }

    /// `SASL_SSL` (behind loams-gateway or across the tailnet).
    pub fn with_tls(mut self, ca_location: Option<String>) -> Self {
        if let KafkaAuth::Sasl {
            tls,
            ca_location: ca,
            ..
        } = &mut self.auth
        {
            *tls = true;
            *ca = ca_location;
        }
        self
    }

    fn apply(&self, options: &mut Options) -> Result<(), DdlError> {
        if self.bootstrap_servers.trim().is_empty() {
            return Err(DdlError::Missing("bootstrap_servers"));
        }
        options.set(
            "properties.bootstrap.server",
            self.bootstrap_servers.clone(),
        );
        match &self.auth {
            KafkaAuth::None => {}
            KafkaAuth::Sasl {
                mechanism,
                username,
                password,
                tls,
                ca_location,
            } => {
                if username.is_empty() {
                    return Err(DdlError::Missing("SASL username"));
                }
                options.set(
                    "properties.security.protocol",
                    if *tls { "SASL_SSL" } else { "SASL_PLAINTEXT" },
                );
                options.set("properties.sasl.mechanism", mechanism.name());
                options.set("properties.sasl.username", username.clone());
                options.set_credential("properties.sasl.password", password);
                if let Some(ca) = ca_location {
                    if !*tls {
                        return Err(DdlError::Invalid("ca_location needs tls = true".to_owned()));
                    }
                    options.set("properties.ssl.ca.location", ca.clone());
                }
            }
        }
        Ok(())
    }
}

/// Where a consumer starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    #[default]
    Earliest,
    Latest,
    /// Milliseconds since the Unix epoch.
    TimestampMillis(i64),
}

/// Metadata columns a Kafka source can add (`INCLUDE …`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "field")]
pub enum Include {
    Key {
        alias: Option<String>,
    },
    Timestamp {
        alias: Option<String>,
    },
    Partition {
        alias: Option<String>,
    },
    Offset {
        alias: Option<String>,
    },
    /// All headers, as `STRUCT<key VARCHAR, value BYTEA>[]`.
    Headers {
        alias: Option<String>,
    },
    /// One header's value, as `BYTEA` (or `data_type`, such as `VARCHAR`).
    Header {
        name: String,
        data_type: Option<String>,
        alias: Option<String>,
    },
}

impl Include {
    fn render(&self) -> Result<String, DdlError> {
        let (head, alias) = match self {
            Self::Key { alias } => ("INCLUDE key".to_owned(), alias),
            Self::Timestamp { alias } => ("INCLUDE timestamp".to_owned(), alias),
            Self::Partition { alias } => ("INCLUDE partition".to_owned(), alias),
            Self::Offset { alias } => ("INCLUDE offset".to_owned(), alias),
            Self::Headers { alias } => ("INCLUDE header".to_owned(), alias),
            Self::Header {
                name,
                data_type,
                alias,
            } => {
                let mut head = format!("INCLUDE header {}", quote_literal(name)?);
                if let Some(ty) = data_type {
                    super::quote::check_data_type(ty)?;
                    head.push(' ');
                    head.push_str(ty.trim());
                }
                (head, alias)
            }
        };
        match alias {
            Some(alias) => Ok(format!("{head} AS {}", quote_ident(alias)?)),
            None => Ok(head),
        }
    }
}

/// A source (not stored; read by the MVs over it) or a table (stored, and
/// required for `FORMAT UPSERT` and for primary keys).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestKind {
    #[default]
    Source,
    Table,
}

/// `CREATE SOURCE` or `CREATE TABLE` over a Loams stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaIngest {
    #[serde(default)]
    pub kind: IngestKind,
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    #[serde(default)]
    pub schema: Schema,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<Include>,
    pub endpoint: KafkaEndpoint,
    /// The Loams stream (the Iggy topic, as the gateway names it).
    pub topic: String,
    #[serde(default)]
    pub startup: StartupMode,
    #[serde(default = "FormatSpec::plain_json")]
    pub format: FormatSpec,
    /// Options passed through as written, overriding the builder's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl KafkaIngest {
    pub fn source(
        name: impl Into<ObjectName>,
        topic: impl Into<String>,
        endpoint: KafkaEndpoint,
    ) -> Self {
        Self {
            kind: IngestKind::Source,
            name: name.into(),
            if_not_exists: false,
            schema: Schema::default(),
            include: Vec::new(),
            endpoint,
            topic: topic.into(),
            startup: StartupMode::Earliest,
            format: FormatSpec::plain_json(),
            extra: BTreeMap::new(),
        }
    }

    pub fn table(
        name: impl Into<ObjectName>,
        topic: impl Into<String>,
        endpoint: KafkaEndpoint,
    ) -> Self {
        Self {
            kind: IngestKind::Table,
            ..Self::source(name, topic, endpoint)
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

    pub fn include(mut self, include: Include) -> Self {
        self.include.push(include);
        self
    }

    pub fn startup(mut self, startup: StartupMode) -> Self {
        self.startup = startup;
        self
    }

    pub fn format(mut self, format: FormatSpec) -> Self {
        self.format = format;
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    fn check(&self) -> Result<(), DdlError> {
        if self.topic.is_empty() {
            return Err(DdlError::Missing("topic"));
        }
        if self.kind == IngestKind::Source && !self.schema.primary_key.is_empty() {
            return Err(DdlError::Invalid(
                "a primary key needs a table (kind = table), not a source".to_owned(),
            ));
        }
        if self.format.format == Format::Upsert {
            if self.kind != IngestKind::Table {
                return Err(DdlError::Invalid("FORMAT UPSERT needs a table".to_owned()));
            }
            if self.schema.primary_key.is_empty() {
                return Err(DdlError::Invalid(
                    "FORMAT UPSERT needs a primary key (usually the included key column)"
                        .to_owned(),
                ));
            }
        }
        if self.schema.columns.is_empty()
            && !self.schema.infer
            && !self.format.schema_from_registry()
        {
            return Err(DdlError::Invalid(format!(
                "ENCODE {:?} needs a column list (only a schema registry or file supplies one)",
                self.format.encode
            )));
        }
        if self.schema.infer {
            return Err(DdlError::Invalid(
                "(*) is for CDC tables; a Kafka schema comes from columns or a registry".to_owned(),
            ));
        }
        if self.format.encode == Encode::Bytes && self.schema.columns.len() != 1 {
            return Err(DdlError::Invalid(
                "ENCODE BYTES reads into exactly one BYTEA column".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Ddl for KafkaIngest {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        self.check()?;
        let mut options = Options::new();
        options.set("connector", "kafka");
        options.set("topic", self.topic.clone());
        self.endpoint.apply(&mut options)?;
        match self.startup {
            StartupMode::Earliest => {
                options.set("scan.startup.mode", "earliest");
            }
            StartupMode::Latest => {
                options.set("scan.startup.mode", "latest");
            }
            StartupMode::TimestampMillis(ms) => {
                options.set("scan.startup.timestamp.millis", ms.to_string());
            }
        }
        options.extend(&self.extra);
        let keyword = match self.kind {
            IngestKind::Source => "SOURCE",
            IngestKind::Table => "TABLE",
        };
        let mut sql = format!(
            "CREATE {keyword} {}{}{}",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name.render()?,
            self.schema.render()?
        );
        for include in &self.include {
            sql.push('\n');
            sql.push_str(&include.render()?);
        }
        sql.push('\n');
        sql.push_str(&options.render_with(ctx)?);
        sql.push('\n');
        sql.push_str(&self.format.render(ctx)?);
        sql.push(';');
        Ok(sql)
    }
}

/// `CREATE SINK … WITH (connector = 'kafka')` into a Loams stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaSink {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub from: SinkFrom,
    pub endpoint: KafkaEndpoint,
    pub topic: String,
    /// The record key's columns; required for `FORMAT UPSERT` and
    /// `FORMAT DEBEZIUM`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_key: Vec<String>,
    #[serde(default = "FormatSpec::plain_json")]
    pub format: FormatSpec,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl KafkaSink {
    pub fn new(
        name: impl Into<ObjectName>,
        from: SinkFrom,
        topic: impl Into<String>,
        endpoint: KafkaEndpoint,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            from,
            endpoint,
            topic: topic.into(),
            primary_key: Vec::new(),
            format: FormatSpec::plain_json(),
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn primary_key<S: Into<String>>(mut self, key: impl IntoIterator<Item = S>) -> Self {
        self.primary_key = key.into_iter().map(Into::into).collect();
        self
    }

    pub fn format(mut self, format: FormatSpec) -> Self {
        self.format = format;
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

impl Ddl for KafkaSink {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if self.topic.is_empty() {
            return Err(DdlError::Missing("topic"));
        }
        if matches!(self.format.format, Format::Upsert | Format::Debezium)
            && self.primary_key.is_empty()
        {
            return Err(DdlError::Invalid(format!(
                "FORMAT {:?} needs a primary key",
                self.format.format
            )));
        }
        if matches!(self.format.format, Format::Maxwell | Format::Canal) {
            return Err(DdlError::Invalid(
                "a Kafka sink writes PLAIN, UPSERT or DEBEZIUM".to_owned(),
            ));
        }
        let mut options = Options::new();
        options.set("connector", "kafka");
        self.endpoint.apply(&mut options)?;
        options.set("topic", self.topic.clone());
        if !self.primary_key.is_empty() {
            options.set(
                "primary_key",
                super::join_list(&self.primary_key, "primary_key")?,
            );
        }
        options.extend(&self.extra);
        Ok(format!(
            "{}\n{}\n{};",
            sink_head(&self.name, self.if_not_exists, &self.from)?,
            options.render_with(ctx)?,
            self.format.render(ctx)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::SchemaRegistry;
    use super::super::{Column, Watermark};
    use super::*;
    use crate::connectors::testing::ctx;

    fn gateway() -> KafkaEndpoint {
        KafkaEndpoint::loams_gateway(
            "eng_01jbzq7m3k8x2v5n9c4t6w0r1y",
            Credential::env("RW_KAFKA_PASSWORD"),
        )
    }

    #[test]
    fn json_source_from_the_gateway() {
        let source = KafkaIngest::source("clicks", "web.clicks", gateway())
            .if_not_exists()
            .schema(
                Schema::columns([
                    Column::new("user_id", "BIGINT"),
                    Column::new("url", "VARCHAR"),
                    Column::new("ts", "TIMESTAMPTZ"),
                ])
                .with_watermark(Watermark::new("ts", "5 seconds")),
            );
        assert_eq!(
            source.render(&ctx()).unwrap(),
            r#"CREATE SOURCE IF NOT EXISTS "clicks" (
    "user_id" BIGINT,
    "url" VARCHAR,
    "ts" TIMESTAMPTZ,
    WATERMARK FOR "ts" AS "ts" - INTERVAL '5 seconds'
)
WITH (
    connector = 'kafka',
    topic = 'web.clicks',
    properties.bootstrap.server = 'kafka-gateway:9093',
    properties.security.protocol = 'SASL_PLAINTEXT',
    properties.sasl.mechanism = 'PLAIN',
    properties.sasl.username = 'eng_01jbzq7m3k8x2v5n9c4t6w0r1y',
    properties.sasl.password = '${RW_KAFKA_PASSWORD}',
    scan.startup.mode = 'earliest'
)
FORMAT PLAIN ENCODE JSON;"#
        );
    }

    #[test]
    fn upsert_table_keyed_by_the_record_key() {
        let table = KafkaIngest::table(
            "users",
            "app.users",
            gateway().with_tls(Some("/etc/ssl/loams-ca.pem".to_owned())),
        )
        .schema(
            Schema::columns([
                Column::new("name", "VARCHAR"),
                Column::new("plan", "VARCHAR"),
            ])
            .with_primary_key(["user_key"]),
        )
        .include(Include::Key {
            alias: Some("user_key".to_owned()),
        })
        .startup(StartupMode::Latest)
        .format(FormatSpec::upsert_json());
        assert_eq!(
            table.render(&ctx()).unwrap(),
            r#"CREATE TABLE "users" (
    "name" VARCHAR,
    "plan" VARCHAR,
    PRIMARY KEY ("user_key")
)
INCLUDE key AS "user_key"
WITH (
    connector = 'kafka',
    topic = 'app.users',
    properties.bootstrap.server = 'kafka-gateway:9093',
    properties.security.protocol = 'SASL_SSL',
    properties.sasl.mechanism = 'PLAIN',
    properties.sasl.username = 'eng_01jbzq7m3k8x2v5n9c4t6w0r1y',
    properties.sasl.password = '${RW_KAFKA_PASSWORD}',
    properties.ssl.ca.location = '/etc/ssl/loams-ca.pem',
    scan.startup.mode = 'latest'
)
FORMAT UPSERT ENCODE JSON;"#
        );
    }

    #[test]
    fn avro_source_takes_its_columns_from_the_registry() {
        let source = KafkaIngest::source(
            "orders",
            "shop.orders",
            KafkaEndpoint::unauthenticated("127.0.0.1:9093"),
        )
        .format(
            FormatSpec::new(Format::Plain, Encode::Avro)
                .with_registry(SchemaRegistry::new("http://registry:8081")),
        )
        .startup(StartupMode::TimestampMillis(1_761_523_200_000))
        .include(Include::Header {
            name: "ce_type".to_owned(),
            data_type: Some("VARCHAR".to_owned()),
            alias: Some("ce_type".to_owned()),
        });
        assert_eq!(
            source.render(&ctx()).unwrap(),
            r#"CREATE SOURCE "orders"
INCLUDE header 'ce_type' VARCHAR AS "ce_type"
WITH (
    connector = 'kafka',
    topic = 'shop.orders',
    properties.bootstrap.server = '127.0.0.1:9093',
    scan.startup.timestamp.millis = '1761523200000'
)
FORMAT PLAIN ENCODE AVRO (schema.registry = 'http://registry:8081');"#
        );
    }

    #[test]
    fn ingest_checks() {
        let plain = KafkaIngest::source("s", "t", gateway());
        assert!(plain.render(&ctx()).is_err(), "JSON needs columns");
        let keyed_source = KafkaIngest::source("s", "t", gateway())
            .schema(Schema::columns([Column::new("a", "INT")]).with_primary_key(["a"]));
        assert!(
            keyed_source.render(&ctx()).is_err(),
            "sources have no primary key"
        );
        let upsert_no_key = KafkaIngest::table("s", "t", gateway())
            .schema(Schema::columns([Column::new("a", "INT")]))
            .format(FormatSpec::upsert_json());
        assert!(upsert_no_key.render(&ctx()).is_err());
        let no_topic = KafkaIngest::source("s", "", gateway())
            .schema(Schema::columns([Column::new("a", "INT")]));
        assert!(no_topic.render(&ctx()).is_err());
        let hostile = KafkaIngest::source("s", "t'; DROP SOURCE x; --", gateway())
            .schema(Schema::columns([Column::new("a", "INT")]));
        assert!(
            hostile
                .render(&ctx())
                .unwrap()
                .contains("topic = 't''; DROP SOURCE x; --'")
        );
    }

    #[test]
    fn upsert_sink_into_a_loams_stream() {
        let sink = KafkaSink::new(
            "user_totals_out",
            SinkFrom::relation("user_totals"),
            "analytics.user_totals",
            gateway(),
        )
        .primary_key(["user_id"])
        .format(FormatSpec::upsert_json());
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK "user_totals_out" FROM "user_totals"
WITH (
    connector = 'kafka',
    properties.bootstrap.server = 'kafka-gateway:9093',
    properties.security.protocol = 'SASL_PLAINTEXT',
    properties.sasl.mechanism = 'PLAIN',
    properties.sasl.username = 'eng_01jbzq7m3k8x2v5n9c4t6w0r1y',
    properties.sasl.password = '${RW_KAFKA_PASSWORD}',
    topic = 'analytics.user_totals',
    primary_key = 'user_id'
)
FORMAT UPSERT ENCODE JSON;"#
        );
    }

    #[test]
    fn append_only_sink_from_a_query() {
        let sink = KafkaSink::new(
            "alerts",
            SinkFrom::Query("SELECT * FROM big_orders WHERE amount > 1000;".to_owned()),
            "ops.alerts",
            gateway(),
        )
        .if_not_exists()
        .format(FormatSpec::plain_json().with_option("force_append_only", "true"));
        let sql = sink.render(&ctx()).unwrap();
        assert!(sql.starts_with(
            "CREATE SINK IF NOT EXISTS \"alerts\" AS SELECT * FROM big_orders WHERE amount > 1000\nWITH ("
        ));
        assert!(sql.ends_with("FORMAT PLAIN ENCODE JSON (force_append_only = 'true');"));
        let keyless = KafkaSink::new("x", SinkFrom::relation("y"), "z", gateway())
            .format(FormatSpec::upsert_json());
        assert!(keyless.render(&ctx()).is_err());
    }
}
