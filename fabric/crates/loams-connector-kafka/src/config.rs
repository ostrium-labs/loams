//! An instance's Kafka settings (`connectors/schemas/kafka.config.json`) and
//! the librdkafka client configurations built from them.
//!
//! The config names secrets, it never holds them: `sasl.password` and
//! `tls.key_pem` are secret *names*, resolved through the instance's
//! [`SecretResolver`] when a client is built (so the SDK's scrubber records
//! them), and never logged: no `debug` context is set (so librdkafka never
//! dumps its configuration), and [`Connection`]'s `Debug` shows no value.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use loams_connector_sdk::{
    ConnectorContext, ConnectorError, EgressCheck, EnvelopeBuilder, InstanceKey, Rate, RateLimit,
    SecretRef, SecretResolver,
};
use rdkafka::ClientConfig;
use serde::Deserialize;

use crate::registry::ConfluentRegistry;

/// The checkpoint topic of the transactional sink (plan Task 9).
pub const CHECKPOINT_TOPIC: &str = "__loams_checkpoints";

/// librdkafka's own default for `message.max.bytes`.
pub const DEFAULT_MAX_RECORD_BYTES: usize = 1_000_000;

/// How long a blocking librdkafka call (metadata, commit, a transaction
/// step) may take before it is reported as `Retryable`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The instance config, as `kafka.config.json` describes it. Unknown keys are
/// ignored here (the manifest's schema refuses them at validation; `pipeline`
/// is the SDK's).
#[derive(Debug, Clone, Deserialize)]
pub struct KafkaConfig {
    /// Bootstrap brokers, `host:port`, comma separated. A `SASL_SSL://`,
    /// `SSL://`, `PLAINTEXT://` or `SASL_PLAINTEXT://` scheme on an entry (as
    /// Confluent Cloud's `bootstrap_endpoint` sometimes has, CC0 R0.12) is
    /// removed by [`KafkaConfig::parse`].
    pub brokers: String,
    /// The topics a source reads; a sink writes the one topic listed.
    pub topics: Vec<String>,
    #[serde(default)]
    pub group_id_prefix: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    /// The transactional sink's `transactional.id` prefix: the id is
    /// `<prefix><instance>-<split>`. Unset, it is `loams-<ns>-`. A Confluent
    /// install sets the module's `transactional_id_prefix` output (default
    /// `loams-<name>-`), which its PREFIXED ACL allows (D1826).
    #[serde(default)]
    pub transactional_id_prefix: Option<String>,
    /// The idempotent producer; it may not be turned off (the manifest
    /// declares `sink.idempotent: true`).
    #[serde(default = "yes")]
    pub enable_idempotence: bool,
    #[serde(default = "acks_all")]
    pub acks: String,
    #[serde(default)]
    pub max_poll_records: Option<usize>,
    #[serde(default)]
    pub auto_offset_reset: OffsetReset,
    #[serde(default)]
    pub security_protocol: SecurityProtocol,
    #[serde(default)]
    pub sasl: Option<SaslConfig>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    /// The sink's delivery: `effectively_once` (the idempotent producer, the
    /// default) or `exactly_once` (Kafka transactions, `txn.rs`).
    #[serde(default)]
    pub delivery: SinkDelivery,
    /// Where the transactional sink keeps its checkpoint.
    #[serde(default = "checkpoint_topic")]
    pub checkpoint_topic: String,
    /// Create the checkpoint topic (compacted, one partition) when it is
    /// missing.
    #[serde(default = "yes")]
    pub create_checkpoint_topic: bool,
    /// The largest record the sink writes (`message.max.bytes`); a larger
    /// event is `Poison`.
    #[serde(default = "max_record_bytes")]
    pub max_record_bytes: usize,
    /// `transaction.timeout.ms` of the transactional producer.
    #[serde(default = "transaction_timeout_ms")]
    pub transaction_timeout_ms: u64,
    /// How long `recover()` may read the checkpoint topic.
    #[serde(default = "recover_timeout_ms")]
    pub recover_timeout_ms: u64,
    /// How long `next` waits for a first record before it reports nothing.
    #[serde(default = "poll_wait_ms")]
    pub poll_wait_ms: u64,
    /// The instance's byte and request rates (D1822, §80 §8.1: Loams' share
    /// of a cluster's throughput); [`KafkaConfig::rate_limit`] maps it to the
    /// SDK's [`RateLimit`], which the pipeline enforces.
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    /// A Confluent-compatible Schema Registry (R5.4, D356).
    #[serde(default)]
    pub schema_registry: Option<SchemaRegistryConfig>,
    /// How the source reads a record's value (CC1 Task 3, D1822): `plain`
    /// (CloudEvents pass-through or a wrapped record, `record.rs`), or a
    /// Debezium change event in JSON or Avro (`debezium.rs`). Source only.
    #[serde(default)]
    pub format: RecordFormat,
    /// Debezium decoding options (with `format: debezium-*`).
    #[serde(default)]
    pub debezium: DebeziumConfig,
}

/// `format`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordFormat {
    #[default]
    Plain,
    DebeziumJson,
    DebeziumAvro,
}

/// `debezium`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebeziumConfig {
    /// What a tombstone (a record with a key and no value, which Debezium
    /// writes after a delete for log compaction) becomes.
    #[serde(default)]
    pub tombstones: Tombstones,
    /// The row fields that make the change's key. Empty: the record key
    /// (Debezium's key struct, the table's primary key).
    #[serde(default)]
    pub key_fields: Vec<String>,
}

/// `debezium.tombstones`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tombstones {
    /// A delete (`loamsop = d`) for the record key.
    #[default]
    Delete,
    /// Nothing: the source moves past it.
    Drop,
}

/// `rate_limit`: per second, per instance (shared by its splits). An absent
/// rate is unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    #[serde(default)]
    pub bytes_per_sec: Option<u64>,
    #[serde(default)]
    pub requests_per_sec: Option<u64>,
}

/// `schema_registry`: the registry's URL and optional HTTP Basic credentials
/// (Confluent Cloud's Schema Registry API key and secret).
#[derive(Debug, Clone, Deserialize)]
pub struct SchemaRegistryConfig {
    pub url: String,
    #[serde(default)]
    pub subject_prefix: Option<String>,
    #[serde(default)]
    pub basic_auth: Option<BasicAuthConfig>,
}

/// HTTP Basic credentials: the username as a value (`username`) or as a
/// secret's name (`username_secret`), exactly one of them, and the password
/// as a secret's name.
#[derive(Debug, Clone, Deserialize)]
pub struct BasicAuthConfig {
    #[serde(default)]
    pub username: Option<String>,
    /// The secret's *name*.
    #[serde(default)]
    pub username_secret: Option<String>,
    /// The secret's *name*.
    pub password: String,
}

fn yes() -> bool {
    true
}
fn acks_all() -> String {
    "all".to_owned()
}
fn checkpoint_topic() -> String {
    CHECKPOINT_TOPIC.to_owned()
}
fn max_record_bytes() -> usize {
    DEFAULT_MAX_RECORD_BYTES
}
fn transaction_timeout_ms() -> u64 {
    60_000
}
fn recover_timeout_ms() -> u64 {
    60_000
}
fn poll_wait_ms() -> u64 {
    100
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OffsetReset {
    Earliest,
    #[default]
    Latest,
    None,
}

impl OffsetReset {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            OffsetReset::Earliest => "earliest",
            OffsetReset::Latest => "latest",
            // librdkafka's spelling of "no reset": the consumer reports
            // `_AUTO_OFFSET_RESET`, which the source maps to `PositionLost`.
            OffsetReset::None => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityProtocol {
    Plaintext,
    #[default]
    Ssl,
    SaslPlaintext,
    SaslSsl,
}

impl SecurityProtocol {
    fn as_str(self) -> &'static str {
        match self {
            SecurityProtocol::Plaintext => "plaintext",
            SecurityProtocol::Ssl => "ssl",
            SecurityProtocol::SaslPlaintext => "sasl_plaintext",
            SecurityProtocol::SaslSsl => "sasl_ssl",
        }
    }
    fn sasl(self) -> bool {
        matches!(
            self,
            SecurityProtocol::SaslPlaintext | SecurityProtocol::SaslSsl
        )
    }
    fn tls(self) -> bool {
        matches!(self, SecurityProtocol::Ssl | SecurityProtocol::SaslSsl)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaslConfig {
    #[serde(default = "scram_512")]
    pub mechanism: String,
    /// The SASL username as a value. Exactly one of `username` and
    /// `username_secret` is set.
    #[serde(default)]
    pub username: Option<String>,
    /// The SASL username as a secret's *name*: a Confluent Cloud API key id
    /// written by `loams-byoc-secrets` beside its secret (D1827).
    #[serde(default)]
    pub username_secret: Option<String>,
    /// The secret's *name*.
    pub password: String,
}

/// Where a username comes from.
enum Username<'a> {
    Value(&'a str),
    Secret(&'a str),
}

/// Exactly one of a plain username and a username secret.
fn username<'a>(
    what: &str,
    value: Option<&'a str>,
    secret: Option<&'a str>,
) -> Result<Username<'a>, ConnectorError> {
    match (value, secret) {
        (Some(v), None) if !v.is_empty() => Ok(Username::Value(v)),
        (None, Some(s)) if !s.is_empty() => Ok(Username::Secret(s)),
        (Some(_), Some(_)) => Err(ConnectorError::fatal(format!(
            "kafka config: {what} sets both username and username_secret; set one"
        ))),
        _ => Err(ConnectorError::fatal(format!(
            "kafka config: {what} needs username or username_secret"
        ))),
    }
}

/// The URI schemes a bootstrap entry may carry, longest first.
const BROKER_SCHEMES: [&str; 4] = ["sasl_plaintext://", "sasl_ssl://", "plaintext://", "ssl://"];

/// `brokers` with a listener scheme removed from each comma-separated entry,
/// case-insensitively (Confluent Cloud's `bootstrap_endpoint` sometimes reads
/// `SASL_SSL://pkc-….confluent.cloud:9092`, CC0 R0.12). Any other scheme is
/// refused: it is not a Kafka bootstrap address.
pub fn normalise_brokers(brokers: &str) -> Result<String, ConnectorError> {
    let mut out = Vec::new();
    for entry in brokers.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((scheme, _)) = entry.split_once("://") else {
            out.push(entry);
            continue;
        };
        let with_sep = &entry[..scheme.len() + 3];
        if !BROKER_SCHEMES
            .iter()
            .any(|s| s.eq_ignore_ascii_case(with_sep))
        {
            return Err(ConnectorError::fatal(format!(
                "kafka config: broker {entry} has the scheme {scheme}://, which is not a Kafka listener's"
            )));
        }
        out.push(&entry[with_sep.len()..]);
    }
    Ok(out.join(","))
}

fn scram_512() -> String {
    "scram-sha-512".to_owned()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TlsConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The CA bundle, PEM (not a secret). Without it the system's roots are
    /// probed.
    #[serde(default)]
    pub ca_pem: Option<String>,
    /// The client certificate for mTLS, PEM (not a secret).
    #[serde(default)]
    pub cert_pem: Option<String>,
    /// The client key for mTLS: the secret's *name*.
    #[serde(default)]
    pub key_pem: Option<String>,
    #[serde(default = "yes")]
    pub verify_full: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkDelivery {
    #[default]
    EffectivelyOnce,
    ExactlyOnce,
}

impl KafkaConfig {
    /// Parse and check an instance config (profile defaults already merged).
    pub fn parse(config: &serde_json::Value) -> Result<Self, ConnectorError> {
        let mut cfg: KafkaConfig = serde_json::from_value(config.clone())
            .map_err(|e| ConnectorError::fatal(format!("kafka config: {e}")))?;
        cfg.brokers = normalise_brokers(&cfg.brokers)?;
        cfg.check()?;
        Ok(cfg)
    }

    fn check(&self) -> Result<(), ConnectorError> {
        if self.broker_list().is_empty() {
            return Err(ConnectorError::fatal("kafka config: brokers is empty"));
        }
        for b in self.broker_list() {
            split_host_port(b)?;
        }
        if self.topics.is_empty() || self.topics.iter().any(|t| t.trim().is_empty()) {
            return Err(ConnectorError::fatal(
                "kafka config: topics must name at least one topic",
            ));
        }
        if self
            .transactional_id_prefix
            .as_deref()
            .is_some_and(|p| p.trim().is_empty())
        {
            return Err(ConnectorError::fatal(
                "kafka config: transactional_id_prefix is empty",
            ));
        }
        if self.security_protocol.sasl() && self.sasl.is_none() {
            return Err(ConnectorError::fatal(format!(
                "kafka config: security_protocol {} needs a sasl block",
                self.security_protocol.as_str()
            )));
        }
        if let Some(sasl) = &self.sasl {
            sasl_mechanism(&sasl.mechanism)?;
            username(
                "sasl",
                sasl.username.as_deref(),
                sasl.username_secret.as_deref(),
            )?;
            if sasl.password.is_empty() {
                return Err(ConnectorError::fatal(
                    "kafka config: sasl.password must name a secret",
                ));
            }
        }
        if let Some(r) = &self.rate_limit
            && (r.bytes_per_sec == Some(0) || r.requests_per_sec == Some(0))
        {
            return Err(ConnectorError::fatal(
                "kafka config: a rate_limit rate must be positive (leave it out for no limit)",
            ));
        }
        if self.format == RecordFormat::DebeziumAvro && self.schema_registry.is_none() {
            return Err(ConnectorError::fatal(
                "kafka config: format debezium-avro needs a schema_registry block",
            ));
        }
        if self.debezium.key_fields.iter().any(|f| f.is_empty()) {
            return Err(ConnectorError::fatal(
                "kafka config: debezium.key_fields names an empty field",
            ));
        }
        if let Some(sr) = &self.schema_registry {
            if !(sr.url.starts_with("https://") || sr.url.starts_with("http://")) {
                return Err(ConnectorError::fatal(
                    "kafka config: schema_registry.url must be an http(s) URL",
                ));
            }
            if let Some(auth) = &sr.basic_auth {
                username(
                    "schema_registry.basic_auth",
                    auth.username.as_deref(),
                    auth.username_secret.as_deref(),
                )?;
                if auth.password.is_empty() {
                    return Err(ConnectorError::fatal(
                        "kafka config: schema_registry.basic_auth.password must name a secret",
                    ));
                }
            }
        }
        if !matches!(self.acks.as_str(), "all" | "-1" | "0" | "1") {
            return Err(ConnectorError::fatal(format!(
                "kafka config: acks {} is not one of all, -1, 0, 1",
                self.acks
            )));
        }
        if self.max_record_bytes == 0 {
            return Err(ConnectorError::fatal(
                "kafka config: max_record_bytes must be positive",
            ));
        }
        Ok(())
    }

    pub fn broker_list(&self) -> Vec<&str> {
        self.brokers
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// The consumer group: `group_id`, else `group_id_prefix`, else
    /// `loams-<ns>-<instance>` (CN1 Task 6). On Event Hubs this is the
    /// Event Hubs consumer group.
    pub fn group(&self, instance: &InstanceKey) -> String {
        self.group_id
            .clone()
            .or_else(|| self.group_id_prefix.clone())
            .unwrap_or_else(|| format!("loams-{}-{}", instance.namespace, instance.name))
    }

    /// The SDK's [`RateLimit`] for this instance's `rate_limit` (unlimited
    /// when there is none). The host passes it to the pipeline
    /// (`Pipeline::…::rate_limit`).
    pub fn rate_limit(&self) -> RateLimit {
        let r = self.rate_limit.unwrap_or_default();
        RateLimit {
            bytes: r.bytes_per_sec.map(|n| Rate::per_second(n as f64)),
            requests: r.requests_per_sec.map(|n| Rate::per_second(n as f64)),
        }
    }

    /// The one topic a sink writes.
    pub fn sink_topic(&self) -> Result<&str, ConnectorError> {
        match self.topics.as_slice() {
            [one] => Ok(one),
            _ => Err(ConnectorError::fatal(format!(
                "kafka config: a sink writes exactly one topic, and {} are listed",
                self.topics.len()
            ))),
        }
    }
}

/// librdkafka's name for a manifest SASL mechanism. OAUTHBEARER (and with it
/// `aws-msk-iam`) is not supported yet (R9.6).
fn sasl_mechanism(m: &str) -> Result<&'static str, ConnectorError> {
    Ok(match m.to_ascii_lowercase().as_str() {
        "plain" => "PLAIN",
        "scram-sha-256" => "SCRAM-SHA-256",
        "scram-sha-512" => "SCRAM-SHA-512",
        other => {
            return Err(ConnectorError::fatal(format!(
                "kafka config: SASL mechanism {other} is not supported by the native connector"
            )));
        }
    })
}

pub(crate) fn split_host_port(b: &str) -> Result<(&str, u16), ConnectorError> {
    let bad = || ConnectorError::fatal(format!("kafka config: broker {b} is not host:port"));
    let (host, port) = b.rsplit_once(':').ok_or_else(bad)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = port.parse::<u16>().map_err(|_| bad())?;
    if host.is_empty() {
        return Err(bad());
    }
    Ok((host, port))
}

/// What one instance's clients share: the parsed config, the secret resolver,
/// the egress check and the envelope builder.
pub struct Connection {
    pub connector: &'static str,
    pub instance: InstanceKey,
    pub cfg: KafkaConfig,
    pub envelope: EnvelopeBuilder,
    secrets: Arc<dyn SecretResolver>,
    egress: Arc<dyn EgressCheck>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("connector", &self.connector)
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub fn new(
        connector: &'static str,
        ctx: &ConnectorContext,
        config: &serde_json::Value,
    ) -> Result<Arc<Self>, ConnectorError> {
        let cfg = KafkaConfig::parse(config)?;
        Ok(Arc::new(Self {
            connector,
            instance: ctx.instance.clone(),
            envelope: EnvelopeBuilder::new(connector, &ctx.instance),
            cfg,
            secrets: ctx.secrets.clone(),
            egress: ctx.egress.clone(),
        }))
    }

    /// Check every bootstrap broker against the egress policy, before DNS by
    /// name and after DNS by address (Review Focus 9). librdkafka dials the
    /// brokers itself, so this runs before each client is built, and
    /// [`Connection::check_broker`] runs on every broker the cluster's
    /// metadata names.
    pub async fn check_egress(&self) -> Result<(), ConnectorError> {
        for b in self.cfg.broker_list() {
            let (host, port) = split_host_port(b)?;
            self.check_broker(host, port).await?;
        }
        Ok(())
    }

    pub async fn check_broker(&self, host: &str, port: u16) -> Result<(), ConnectorError> {
        self.egress
            .check_host(host, port)
            .map_err(|e| ConnectorError::fatal(format!("broker {host}:{port}: {e}")))?;
        let addrs: Vec<_> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| ConnectorError::retryable(format!("resolving broker {host}: {e}")))?
            .collect();
        for addr in &addrs {
            self.egress
                .check_addr(addr)
                .map_err(|e| ConnectorError::fatal(format!("broker {host}:{port}: {e}")))?;
        }
        Ok(())
    }

    async fn secret(&self, name: &str) -> Result<String, ConnectorError> {
        let s = self.secrets.resolve(&SecretRef::new(name)).await?;
        s.expose_str()
            .map(str::to_owned)
            .ok_or_else(|| ConnectorError::fatal(format!("secret {name} is not UTF-8")))
    }

    /// The settings every client shares: brokers, security and credentials.
    pub async fn base(&self) -> Result<ClientConfig, ConnectorError> {
        self.check_egress().await?;
        let mut c = ClientConfig::new();
        c.set("bootstrap.servers", &self.cfg.brokers)
            .set(
                "client.id",
                format!("loams-{}-{}", self.instance.namespace, self.instance.name),
            )
            .set("security.protocol", self.cfg.security_protocol.as_str());
        if self.cfg.security_protocol.sasl() {
            let sasl = self
                .cfg
                .sasl
                .as_ref()
                .ok_or_else(|| ConnectorError::fatal("kafka config: no sasl block"))?;
            let user = match username(
                "sasl",
                sasl.username.as_deref(),
                sasl.username_secret.as_deref(),
            )? {
                Username::Value(v) => v.to_owned(),
                Username::Secret(name) => self.secret(name).await?,
            };
            c.set("sasl.mechanism", sasl_mechanism(&sasl.mechanism)?)
                .set("sasl.username", user)
                .set("sasl.password", self.secret(&sasl.password).await?);
        }
        let tls = self.cfg.tls.clone().unwrap_or_default();
        if self.cfg.security_protocol.tls() && tls.enabled {
            match &tls.ca_pem {
                Some(ca) => c.set("ssl.ca.pem", ca),
                // The vendored OpenSSL has no compiled-in CA path: probe the
                // host's usual locations for the system roots.
                None => c.set("ssl.ca.location", "probe"),
            };
            if let Some(cert) = &tls.cert_pem {
                c.set("ssl.certificate.pem", cert);
            }
            if let Some(key) = &tls.key_pem {
                c.set("ssl.key.pem", self.secret(key).await?);
            }
            c.set(
                "ssl.endpoint.identification.algorithm",
                if tls.verify_full { "https" } else { "none" },
            );
        }
        Ok(c)
    }

    /// The configured Schema Registry client, credentials resolved, or
    /// `None` when there is no `schema_registry` block.
    pub async fn registry(&self) -> Result<Option<ConfluentRegistry>, ConnectorError> {
        let Some(sr) = &self.cfg.schema_registry else {
            return Ok(None);
        };
        let mut reg = ConfluentRegistry::new(&sr.url, self.egress.clone());
        if let Some(auth) = &sr.basic_auth {
            let user = match username(
                "schema_registry.basic_auth",
                auth.username.as_deref(),
                auth.username_secret.as_deref(),
            )? {
                Username::Value(v) => v.to_owned(),
                Username::Secret(name) => self.secret(name).await?,
            };
            let password = self
                .secrets
                .resolve(&SecretRef::new(&auth.password))
                .await?;
            reg = reg.with_basic_auth(user, password);
        }
        Ok(Some(reg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_checks() {
        let cfg = KafkaConfig::parse(&json!({
            "brokers": "a:9092, b:9093",
            "topics": ["orders"],
            "group_id_prefix": "loams-1-orders",
        }))
        .expect("parse");
        assert_eq!(cfg.broker_list(), vec!["a:9092", "b:9093"]);
        assert_eq!(cfg.security_protocol, SecurityProtocol::Ssl);
        assert_eq!(cfg.delivery, SinkDelivery::EffectivelyOnce);
        assert_eq!(cfg.checkpoint_topic, CHECKPOINT_TOPIC);
        assert_eq!(cfg.group(&InstanceKey::new(1, "x")), "loams-1-orders");
        assert_eq!(cfg.sink_topic().expect("one topic"), "orders");

        let no_group =
            KafkaConfig::parse(&json!({"brokers": "a:1", "topics": ["t"]})).expect("parse");
        assert_eq!(no_group.group(&InstanceKey::new(7, "inst")), "loams-7-inst");

        for bad in [
            json!({"brokers": "", "topics": ["t"]}),
            json!({"brokers": "nohost", "topics": ["t"]}),
            json!({"brokers": "a:1", "topics": []}),
            json!({"brokers": "a:1", "topics": ["t"], "security_protocol": "sasl_ssl"}),
            json!({"brokers": "a:1", "topics": ["t"], "security_protocol": "sasl_ssl",
                   "sasl": {"mechanism": "OAUTHBEARER", "username": "u", "password": "p"}}),
            json!({"brokers": "a:1", "topics": ["t"], "acks": "2"}),
        ] {
            assert!(KafkaConfig::parse(&bad).is_err(), "{bad} should be refused");
        }
        let two =
            KafkaConfig::parse(&json!({"brokers": "a:1", "topics": ["t", "u"]})).expect("parse");
        assert!(two.sink_topic().is_err());
    }

    #[test]
    fn bootstrap_scheme_is_stripped() {
        // Confluent Cloud's bootstrap_endpoint, with and without its scheme (CC0
        // R0.12), any case, per comma-separated entry.
        for (given, want) in [
            (
                "SASL_SSL://pkc-abc12.us-east-1.aws.confluent.cloud:9092",
                "pkc-abc12.us-east-1.aws.confluent.cloud:9092",
            ),
            (
                "pkc-abc12.us-east-1.aws.confluent.cloud:9092",
                "pkc-abc12.us-east-1.aws.confluent.cloud:9092",
            ),
            (
                "sasl_ssl://a:1, SSL://b:2,PLAINTEXT://c:3 , Sasl_Plaintext://d:4",
                "a:1,b:2,c:3,d:4",
            ),
            ("ssl://[::1]:9093", "[::1]:9093"),
        ] {
            assert_eq!(
                normalise_brokers(given).expect("normalised"),
                want,
                "{given}"
            );
            let cfg =
                KafkaConfig::parse(&json!({"brokers": given, "topics": ["t"]})).expect("parse");
            assert_eq!(cfg.brokers, want);
        }
        assert!(normalise_brokers("https://a:1").is_err());
        assert!(KafkaConfig::parse(&json!({"brokers": "http://a:1", "topics": ["t"]})).is_err());
        assert!(KafkaConfig::parse(&json!({"brokers": "SASL_SSL://", "topics": ["t"]})).is_err());
    }

    #[test]
    fn rate_limit_config_maps_to_sdk() {
        let none = KafkaConfig::parse(&json!({"brokers": "a:1", "topics": ["t"]})).expect("parse");
        assert_eq!(
            none.rate_limit(),
            RateLimit::default(),
            "no block: unlimited"
        );

        let both = KafkaConfig::parse(&json!({
            "brokers": "a:1", "topics": ["t"],
            "rate_limit": {"bytes_per_sec": 1_048_576, "requests_per_sec": 50},
        }))
        .expect("parse");
        assert_eq!(
            both.rate_limit(),
            RateLimit {
                bytes: Some(Rate::per_second(1_048_576.0)),
                requests: Some(Rate::per_second(50.0)),
            }
        );
        let bytes_only = KafkaConfig::parse(&json!({
            "brokers": "a:1", "topics": ["t"], "rate_limit": {"bytes_per_sec": 65_536},
        }))
        .expect("parse");
        assert_eq!(
            bytes_only.rate_limit(),
            RateLimit {
                bytes: Some(Rate::per_second(65_536.0)),
                requests: None,
            }
        );
        for bad in [
            json!({"bytes_per_sec": 0}),
            json!({"requests_per_sec": 0}),
            json!({"bytes_per_sec": -1}),
            json!({"bytes_per_second": 10}),
        ] {
            let cfg = json!({"brokers": "a:1", "topics": ["t"], "rate_limit": bad});
            assert!(KafkaConfig::parse(&cfg).is_err(), "{cfg} should be refused");
        }
    }

    #[test]
    fn sasl_username_value_or_secret() {
        let base = |sasl: serde_json::Value| json!({"brokers": "a:1", "topics": ["t"], "security_protocol": "sasl_ssl", "sasl": sasl});
        assert!(KafkaConfig::parse(&base(json!({"username": "u", "password": "p"}))).is_ok());
        assert!(
            KafkaConfig::parse(&base(
                json!({"username_secret": "c/username", "password": "c/password"})
            ))
            .is_ok()
        );
        for bad in [
            json!({"password": "p"}),
            json!({"username": "u", "username_secret": "s", "password": "p"}),
            json!({"username": "", "password": "p"}),
            json!({"username": "u", "password": ""}),
            json!({"username_secret": "s"}),
        ] {
            assert!(
                KafkaConfig::parse(&base(bad.clone())).is_err(),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn schema_registry_basic_auth() {
        let with = |sr: serde_json::Value| json!({"brokers": "a:1", "topics": ["t"], "schema_registry": sr});
        for ok in [
            json!({"url": "https://psrc-x.us-east-1.aws.confluent.cloud"}),
            json!({"url": "https://sr", "basic_auth": {"username": "k", "password": "c/sr-password"}}),
            json!({"url": "http://sr:8081", "basic_auth": {"username_secret": "c/sr-username", "password": "c/sr-password"}}),
        ] {
            assert!(KafkaConfig::parse(&with(ok.clone())).is_ok(), "{ok}");
        }
        for bad in [
            json!({"url": "sr:8081"}),
            json!({"url": "https://sr", "basic_auth": {"password": "p"}}),
            json!({"url": "https://sr", "basic_auth": {"username": "k", "username_secret": "s", "password": "p"}}),
            json!({"url": "https://sr", "basic_auth": {"username": "k"}}),
        ] {
            assert!(
                KafkaConfig::parse(&with(bad.clone())).is_err(),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn host_port() {
        assert_eq!(split_host_port("h:1").expect("ok"), ("h", 1));
        assert_eq!(split_host_port("[::1]:9092").expect("ok"), ("::1", 9092));
        assert!(split_host_port(":1").is_err());
        assert!(split_host_port("h:x").is_err());
    }
}
