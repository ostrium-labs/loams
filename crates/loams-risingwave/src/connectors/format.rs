//! `FORMAT … ENCODE … (…)`: how RisingWave reads or writes a Kafka record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Credential, DdlError, Options, RenderContext};

/// The envelope of a record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// One record is one row (append-only).
    #[default]
    Plain,
    /// The record key is the row key; a null value deletes.
    Upsert,
    /// Debezium's change envelope (`before`, `after`, `op`).
    Debezium,
    /// Maxwell's change envelope.
    Maxwell,
    /// Canal's change envelope.
    Canal,
}

impl Format {
    fn keyword(self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::Upsert => "UPSERT",
            Self::Debezium => "DEBEZIUM",
            Self::Maxwell => "MAXWELL",
            Self::Canal => "CANAL",
        }
    }
}

/// The encoding of a record's value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encode {
    #[default]
    Json,
    Avro,
    Protobuf,
    Csv,
    Bytes,
}

impl Encode {
    fn keyword(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Avro => "AVRO",
            Self::Protobuf => "PROTOBUF",
            Self::Csv => "CSV",
            Self::Bytes => "BYTES",
        }
    }
}

/// A Confluent-compatible schema registry (RW-b assumption A4).
///
/// Design 32 D335 keeps schemas as documents in the bucket and leaves a
/// registry REST subset for later (Q344); when one exists, Avro and Protobuf
/// read their schemas from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaRegistry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Credential>,
}

impl SchemaRegistry {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            username: None,
            password: None,
        }
    }

    pub fn with_basic_auth(mut self, username: impl Into<String>, password: Credential) -> Self {
        self.username = Some(username.into());
        self.password = Some(password);
        self
    }
}

/// The whole clause.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatSpec {
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub encode: Encode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_registry: Option<SchemaRegistry>,
    /// Protobuf's message, `package.Message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// A schema file instead of a registry (`schema.location`), for Avro
    /// and Protobuf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_location: Option<String>,
    /// Options passed through as written (`force_append_only`,
    /// `timestamptz.handling.mode`, `without_header` for CSV, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

impl FormatSpec {
    pub fn new(format: Format, encode: Encode) -> Self {
        Self {
            format,
            encode,
            ..Self::default()
        }
    }

    pub fn plain_json() -> Self {
        Self::new(Format::Plain, Encode::Json)
    }

    pub fn upsert_json() -> Self {
        Self::new(Format::Upsert, Encode::Json)
    }

    pub fn with_registry(mut self, registry: SchemaRegistry) -> Self {
        self.schema_registry = Some(registry);
        self
    }

    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }

    /// Whether the encoding needs a schema from somewhere other than the
    /// column list.
    fn needs_schema(&self) -> bool {
        matches!(self.encode, Encode::Avro | Encode::Protobuf)
    }

    /// Whether the column list may be left out (the schema comes from the
    /// registry or a file).
    pub fn schema_from_registry(&self) -> bool {
        self.needs_schema() && (self.schema_registry.is_some() || self.schema_location.is_some())
    }

    /// `FORMAT PLAIN ENCODE JSON` or `FORMAT PLAIN ENCODE AVRO (schema.registry = '…')`.
    pub fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        let mut options = Options::new();
        if let Some(registry) = &self.schema_registry {
            if !matches!(self.encode, Encode::Avro | Encode::Protobuf | Encode::Json) {
                return Err(DdlError::Invalid(format!(
                    "ENCODE {} cannot use a schema registry",
                    self.encode.keyword()
                )));
            }
            options.set("schema.registry", registry.url.clone());
            options.set_opt("schema.registry.username", registry.username.clone());
            if let Some(password) = &registry.password {
                options.set_credential("schema.registry.password", password);
            }
        }
        if let Some(location) = &self.schema_location {
            if self.schema_registry.is_some() {
                return Err(DdlError::Invalid(
                    "a schema comes from a registry or a location, not both".to_owned(),
                ));
            }
            options.set("schema.location", location.clone());
        }
        if self.encode == Encode::Protobuf {
            let message = self
                .message
                .as_deref()
                .ok_or(DdlError::Missing("message"))?;
            options.set("message", message);
            if self.schema_registry.is_none() && self.schema_location.is_none() {
                return Err(DdlError::Missing("schema_registry or schema_location"));
            }
        } else if self.message.is_some() {
            return Err(DdlError::Invalid(
                "message is for ENCODE PROTOBUF only".to_owned(),
            ));
        }
        if self.encode == Encode::Avro
            && self.schema_registry.is_none()
            && self.schema_location.is_none()
        {
            return Err(DdlError::Missing("schema_registry or schema_location"));
        }
        options.extend(&self.options);
        let head = format!(
            "FORMAT {} ENCODE {}",
            self.format.keyword(),
            self.encode.keyword()
        );
        if options.is_empty() {
            Ok(head)
        } else {
            Ok(format!("{head} {}", options.render_inline(ctx)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RenderContext<'static> {
        RenderContext::redacted()
    }

    #[test]
    fn plain_json_has_no_options() {
        assert_eq!(
            FormatSpec::plain_json().render(&ctx()).unwrap(),
            "FORMAT PLAIN ENCODE JSON"
        );
        assert_eq!(
            FormatSpec::upsert_json().render(&ctx()).unwrap(),
            "FORMAT UPSERT ENCODE JSON"
        );
    }

    #[test]
    fn avro_reads_the_registry_with_a_password_from_the_environment() {
        let spec = FormatSpec::new(Format::Plain, Encode::Avro).with_registry(
            SchemaRegistry::new("http://registry:8081")
                .with_basic_auth("svc", Credential::env("RW_REGISTRY_PASSWORD")),
        );
        assert!(spec.schema_from_registry());
        let env = |name: &str| (name == "RW_REGISTRY_PASSWORD").then(|| "pw".to_owned());
        assert_eq!(
            spec.render(&RenderContext::with_env(&env)).unwrap(),
            "FORMAT PLAIN ENCODE AVRO (schema.registry = 'http://registry:8081', \
             schema.registry.username = 'svc', schema.registry.password = 'pw')"
        );
        assert!(
            spec.render(&ctx())
                .unwrap()
                .ends_with("schema.registry.password = '<redacted>')")
        );
    }

    #[test]
    fn avro_and_protobuf_need_a_schema() {
        assert!(
            FormatSpec::new(Format::Plain, Encode::Avro)
                .render(&ctx())
                .is_err()
        );
        let mut proto = FormatSpec::new(Format::Plain, Encode::Protobuf);
        proto.schema_location = Some("s3://bucket/schemas/order.pb".to_owned());
        assert!(proto.render(&ctx()).is_err(), "message is required");
        proto.message = Some("shop.Order".to_owned());
        assert_eq!(
            proto.render(&ctx()).unwrap(),
            "FORMAT PLAIN ENCODE PROTOBUF (schema.location = 's3://bucket/schemas/order.pb', message = 'shop.Order')"
        );
    }

    #[test]
    fn sink_options_pass_through() {
        let spec = FormatSpec::plain_json().with_option("force_append_only", "true");
        assert_eq!(
            spec.render(&ctx()).unwrap(),
            "FORMAT PLAIN ENCODE JSON (force_append_only = 'true')"
        );
    }
}
