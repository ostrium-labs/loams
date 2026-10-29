//! Loams streams as Flink tables, through the Iggy Kafka gateway (D1983).
//!
//! `kafka` for append streams and `upsert-kafka` for changelog streams keyed
//! by a primary key. The gateway (`ghcr.io/ostrium-labs/iggy`, the `kafka`
//! compose profile) authenticates with SASL/PLAIN as the instance's own Iggy
//! user, minted by the engines facade and scoped to the namespace's topics
//! (ENG81 R5): a user's API key is never written into engine SQL. The
//! gateway has no transactions, so a sink is at-least-once: the builders
//! pin `sink.delivery-guarantee` and refuse `exactly-once`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Column, TableDdl, Watermark};
use crate::catalog::quote::{Escape, OptValue, Part, Secret, SqlError, WithOptions};

/// The gateway on the compose network.
pub const DEFAULT_BOOTSTRAP: &str = "kafka-gateway:9093";
/// The PLAIN login module, as shaded inside `flink-sql-connector-kafka`.
pub const SHADED_PLAIN_LOGIN_MODULE: &str =
    "org.apache.flink.kafka.shaded.org.apache.kafka.common.security.plain.PlainLoginModule";

fn default_bootstrap() -> String {
    DEFAULT_BOOTSTRAP.to_string()
}

/// The client's security protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KafkaSecurity {
    /// No SASL (a local gateway with `KAFKA_SASL_ENABLED=false`).
    Plaintext,
    /// SASL/PLAIN on the private compose network.
    #[default]
    SaslPlaintext,
    /// SASL/PLAIN inside TLS, through loams-gateway.
    SaslSsl,
}

impl KafkaSecurity {
    fn protocol(self) -> &'static str {
        match self {
            Self::Plaintext => "PLAINTEXT",
            Self::SaslPlaintext => "SASL_PLAINTEXT",
            Self::SaslSsl => "SASL_SSL",
        }
    }
}

/// SASL/PLAIN credentials: the instance's scoped Iggy user (ENG81 R5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaAuth {
    pub username: String,
    pub password: Secret,
}

/// Where and how to reach the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaGateway {
    #[serde(default = "default_bootstrap")]
    pub bootstrap: String,
    #[serde(default)]
    pub security: KafkaSecurity,
    #[serde(default)]
    pub auth: Option<KafkaAuth>,
}

impl Default for KafkaGateway {
    fn default() -> Self {
        Self {
            bootstrap: default_bootstrap(),
            security: KafkaSecurity::default(),
            auth: None,
        }
    }
}

impl KafkaGateway {
    fn apply(&self, o: &mut WithOptions) -> Result<(), SqlError> {
        if self.bootstrap.is_empty()
            || self
                .bootstrap
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(SqlError::Invalid(format!(
                "kafka bootstrap {:?} is not host:port[,host:port]",
                self.bootstrap
            )));
        }
        o.set("properties.bootstrap.servers", self.bootstrap.clone());
        match (self.security, &self.auth) {
            (KafkaSecurity::Plaintext, None) => {}
            (KafkaSecurity::Plaintext, Some(_)) => {
                return Err(SqlError::Invalid(
                    "kafka credentials need security sasl_plaintext or sasl_ssl".into(),
                ));
            }
            (security, None) => {
                return Err(SqlError::Invalid(format!(
                    "kafka security {} needs credentials",
                    security.protocol()
                )));
            }
            (security, Some(auth)) => {
                if auth.username.is_empty() {
                    return Err(SqlError::Invalid("kafka username is empty".into()));
                }
                o.set("properties.security.protocol", security.protocol())
                    .set("properties.sasl.mechanism", "PLAIN")
                    .set_value("properties.sasl.jaas.config", jaas_plain(auth));
            }
        }
        Ok(())
    }
}

/// `…PlainLoginModule required username="u" password="p";`, with both
/// values escaped for JAAS's double-quoted strings.
pub fn jaas_plain(auth: &KafkaAuth) -> OptValue {
    OptValue(vec![
        Part::Text(format!(
            "{SHADED_PLAIN_LOGIN_MODULE} required username=\"{}\" password=\"",
            crate::catalog::quote::jaas_escape(&auth.username)
        )),
        Part::Secret {
            secret: auth.password.clone(),
            escape: Escape::JaasQuoted,
        },
        Part::Text("\";".to_string()),
    ])
}

/// Where a source starts reading.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum StartupMode {
    EarliestOffset,
    LatestOffset,
    /// The consumer group's committed offsets, else `reset`.
    #[default]
    GroupOffsets,
    /// From a time, in milliseconds since the epoch.
    Timestamp {
        millis: i64,
    },
}

/// A record format.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Format {
    /// JSON objects (Loams' CloudEvents are JSON, D355). Timestamps are
    /// ISO-8601.
    #[default]
    Json,
    /// JSON that skips records it cannot parse instead of failing the job.
    JsonLenient,
    Csv,
    /// One `STRING` or `BYTES` column holding the whole value.
    Raw,
    /// Debezium's JSON change events (a changelog source).
    DebeziumJson,
}

impl Format {
    /// Options for `format` (or `<prefix>format` with `key.`/`value.`).
    fn apply(&self, o: &mut WithOptions, prefix: &str) {
        let (name, extra): (&str, &[(&str, &str)]) = match self {
            Self::Json => ("json", &[("json.timestamp-format.standard", "ISO-8601")]),
            Self::JsonLenient => (
                "json",
                &[
                    ("json.timestamp-format.standard", "ISO-8601"),
                    ("json.ignore-parse-errors", "true"),
                    ("json.fail-on-missing-field", "false"),
                ],
            ),
            Self::Csv => ("csv", &[]),
            Self::Raw => ("raw", &[]),
            Self::DebeziumJson => ("debezium-json", &[]),
        };
        o.set(&format!("{prefix}format"), name);
        for (k, v) in extra {
            o.set(&format!("{prefix}{k}"), *v);
        }
    }
}

fn check_topic(topic: &str) -> Result<(), SqlError> {
    // Kafka's own rule: [a-zA-Z0-9._-]{1,249}.
    let ok = !topic.is_empty()
        && topic.len() <= 249
        && topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(SqlError::Invalid(format!(
            "topic {topic:?} is not [a-zA-Z0-9._-]{{1,249}}"
        )))
    }
}

fn refuse_exactly_once(extra: &BTreeMap<String, String>) -> Result<(), SqlError> {
    match extra.get("sink.delivery-guarantee").map(String::as_str) {
        Some("exactly-once") => Err(SqlError::Invalid(
            "the Iggy Kafka gateway has no transactions: sink.delivery-guarantee must be at-least-once or none".into(),
        )),
        _ => Ok(()),
    }
}

/// A Loams stream as an append-only Flink table (`'connector' = 'kafka'`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KafkaTable {
    /// The Flink table name.
    pub name: String,
    /// The Loams stream; the topic unless `topic` is set.
    pub stream: String,
    #[serde(default)]
    pub topic: Option<String>,
    pub columns: Vec<Column>,
    #[serde(default)]
    pub watermark: Option<Watermark>,
    #[serde(default)]
    pub gateway: KafkaGateway,
    #[serde(default)]
    pub startup: StartupMode,
    /// Defaults to `loams-flink-<name>`.
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl KafkaTable {
    pub fn new(name: &str, stream: &str, columns: Vec<Column>) -> Self {
        Self {
            name: name.to_string(),
            stream: stream.to_string(),
            topic: None,
            columns,
            watermark: None,
            gateway: KafkaGateway::default(),
            startup: StartupMode::default(),
            group_id: None,
            format: Format::default(),
            extra: BTreeMap::new(),
        }
    }

    pub fn topic(&self) -> &str {
        self.topic.as_deref().unwrap_or(&self.stream)
    }

    pub fn ddl(&self) -> Result<TableDdl, SqlError> {
        check_topic(self.topic())?;
        refuse_exactly_once(&self.extra)?;
        let mut o = WithOptions::new();
        o.set("connector", "kafka").set("topic", self.topic());
        self.gateway.apply(&mut o)?;
        let group = self
            .group_id
            .clone()
            .unwrap_or_else(|| format!("loams-flink-{}", self.name));
        o.set("properties.group.id", group);
        match &self.startup {
            StartupMode::EarliestOffset => {
                o.set("scan.startup.mode", "earliest-offset");
            }
            StartupMode::LatestOffset => {
                o.set("scan.startup.mode", "latest-offset");
            }
            StartupMode::GroupOffsets => {
                o.set("scan.startup.mode", "group-offsets")
                    .set("properties.auto.offset.reset", "earliest");
            }
            StartupMode::Timestamp { millis } => {
                o.set("scan.startup.mode", "timestamp")
                    .set("scan.startup.timestamp-millis", millis.to_string());
            }
        }
        self.format.apply(&mut o, "");
        o.set("sink.delivery-guarantee", "at-least-once");
        o.extend(&self.extra);
        Ok(TableDdl {
            name: vec![self.name.clone()],
            temporary: true,
            columns: self.columns.clone(),
            primary_key: Vec::new(),
            watermark: self.watermark.clone(),
            options: o,
        })
    }
}

/// A Loams stream as a changelog table keyed by `primary_key`
/// (`'connector' = 'upsert-kafka'`): the key is the record key, a null value
/// is a delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertKafkaTable {
    pub name: String,
    pub stream: String,
    #[serde(default)]
    pub topic: Option<String>,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub watermark: Option<Watermark>,
    #[serde(default)]
    pub gateway: KafkaGateway,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub key_format: Format,
    #[serde(default)]
    pub value_format: Format,
    /// `EXCEPT_KEY` leaves the key columns out of the value.
    #[serde(default)]
    pub value_except_key: bool,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl UpsertKafkaTable {
    pub fn new(name: &str, stream: &str, columns: Vec<Column>, primary_key: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            stream: stream.to_string(),
            topic: None,
            columns,
            primary_key: primary_key.iter().map(|s| s.to_string()).collect(),
            watermark: None,
            gateway: KafkaGateway::default(),
            group_id: None,
            key_format: Format::Json,
            value_format: Format::Json,
            value_except_key: false,
            extra: BTreeMap::new(),
        }
    }

    pub fn ddl(&self) -> Result<TableDdl, SqlError> {
        let topic = self.topic.as_deref().unwrap_or(&self.stream);
        check_topic(topic)?;
        refuse_exactly_once(&self.extra)?;
        if self.primary_key.is_empty() {
            return Err(SqlError::Invalid(format!(
                "upsert-kafka table {:?} needs a primary key",
                self.name
            )));
        }
        if matches!(self.key_format, Format::DebeziumJson)
            || matches!(self.value_format, Format::DebeziumJson)
        {
            return Err(SqlError::Invalid(
                "upsert-kafka takes insert-only formats (json, csv, raw)".into(),
            ));
        }
        let mut o = WithOptions::new();
        o.set("connector", "upsert-kafka").set("topic", topic);
        self.gateway.apply(&mut o)?;
        let group = self
            .group_id
            .clone()
            .unwrap_or_else(|| format!("loams-flink-{}", self.name));
        o.set("properties.group.id", group);
        self.key_format.apply(&mut o, "key.");
        self.value_format.apply(&mut o, "value.");
        if self.value_except_key {
            o.set("value.fields-include", "EXCEPT_KEY");
        }
        o.set("sink.delivery-guarantee", "at-least-once");
        o.extend(&self.extra);
        Ok(TableDdl {
            name: vec![self.name.clone()],
            temporary: true,
            columns: self.columns.clone(),
            primary_key: self.primary_key.clone(),
            watermark: self.watermark.clone(),
            options: o,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::quote::RenderCtx;

    fn auth() -> KafkaGateway {
        KafkaGateway {
            auth: Some(KafkaAuth {
                username: "key_01".into(),
                password: Secret::reference("loams-api-key"),
            }),
            ..KafkaGateway::default()
        }
    }

    #[test]
    fn exactly_once_is_refused() {
        let mut t = KafkaTable::new("t", "s", vec![Column::new("a", "INT")]);
        t.gateway = auth();
        t.extra
            .insert("sink.delivery-guarantee".into(), "exactly-once".into());
        assert!(t.ddl().is_err());
    }

    #[test]
    fn sasl_needs_credentials_and_plaintext_refuses_them() {
        let mut t = KafkaTable::new("t", "s", vec![Column::new("a", "INT")]);
        assert!(t.ddl().is_err(), "sasl_plaintext without credentials");
        t.gateway.security = KafkaSecurity::Plaintext;
        assert!(t.ddl().is_ok());
        t.gateway = auth();
        t.gateway.security = KafkaSecurity::Plaintext;
        assert!(t.ddl().is_err());
    }

    #[test]
    fn topic_names_are_checked() {
        let mut t = KafkaTable::new("t", "bad topic", vec![Column::new("a", "INT")]);
        t.gateway = auth();
        assert!(t.ddl().is_err());
        t.topic = Some("orders.v1".into());
        assert!(t.ddl().is_ok());
    }

    #[test]
    fn jaas_password_is_escaped_and_redacted() {
        let a = KafkaAuth {
            username: "u\"x".into(),
            password: Secret::inline("p\"w"),
        };
        let v = jaas_plain(&a);
        let mut t = KafkaTable::new("t", "s", vec![Column::new("a", "INT")]);
        t.gateway.auth = Some(a);
        let sql = t
            .ddl()
            .and_then(|d| d.render(&RenderCtx::redacted()))
            .unwrap_or_default();
        assert!(
            sql.contains("username=\"u\\\"x\" password=\"***\";"),
            "{sql}"
        );
        assert!(v.has_secret());
    }

    #[test]
    fn upsert_needs_a_key() {
        let mut t = UpsertKafkaTable::new("t", "s", vec![Column::new("a", "INT")], &[]);
        t.gateway = auth();
        assert!(t.ddl().is_err());
    }
}
