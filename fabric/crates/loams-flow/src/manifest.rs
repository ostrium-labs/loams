//! The connector manifest: §33 §4's YAML as typed Rust, and the protobuf form
//! that mirrors it one to one.
//!
//! A manifest is one versioned YAML document per connector at
//! `connectors/registry/<id>.yaml` (design §33 D352). The YAML is the source of
//! truth — §33 §4's last paragraph says the protobuf form mirrors it one to one
//! and CI checks that both agree — so every field and every enum here carries
//! the **exact YAML slug** of the manifest, and `to_proto`/`from_proto` are the
//! only place a slug becomes a protobuf name such as `ORDERING_PER_KEY`.
//!
//! Three shapes are worth naming, because each of them is a place the design,
//! the JSON Schema and CN1 plan Task 1 did not agree:
//!
//! * **`RuntimeSpec` is a struct, not an enum.** §33 §4's `runtime` is
//!   `{kind, ref, version}` and its `ref` comment fixes one meaning per `kind`,
//!   so a five-variant Rust enum would have to restate that mapping and could
//!   drift from the YAML. D352 makes the YAML canonical, so the struct wins;
//!   `connector.proto`'s `RuntimeRef` is a message for the same reason.
//! * **`config_ref` is the path and `config_schema` is the resolved document.**
//!   §33 §4's `config: {$ref: …}` is a repository-relative path and D353's
//!   manifest declares the shape of an instance's settings, so the loader
//!   resolves the reference and keeps the document. `config_schema` is skipped
//!   when serializing — the path is what round-trips.
//! * **`apiVersion` and `kind` are not fields.** `connector.schema.json` makes
//!   them `const` and `connector.proto` gives them no field, whose own header
//!   comment says so. `to_json`/`to_yaml` write them and the schema checks them.
//!
//! [`ConnectorSpec::to_proto`] and [`ConnectorSpec::from_proto`] are the pair
//! `proto_and_yaml_agree` drives over all 21 ★ manifests.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use buffa::{EnumValue, Enumeration};
use serde::{Deserialize, Serialize};

use crate::validate::ManifestSchema;

/// §33 §4's `apiVersion`, `const` in `connectors/schema/connector.schema.json`.
pub const API_VERSION: &str = "loams.flow/v1";

/// §33 §4's `kind`, `const` in `connectors/schema/connector.schema.json`.
pub const KIND: &str = "Connector";

/// The generated `loams.flow.v1` messages, as `loams-flow-proto` emits them.
pub mod pb {
    pub use loams_flow_proto::loams::flow::v1::*;
}

/// Declares one manifest enum: the YAML slug on each variant, the protobuf value
/// it mirrors, and both translations.
///
/// The slug is written per variant rather than derived by a blanket `rename_all`,
/// because the slugs are not one convention: `object-storage`,
/// `push-with-ack` and `sasl-scram-256` are kebab, `per_key` and `at_least_once`
/// are snake, and `P1` is upper. `enum_slugs_match_the_json_schema` pins every one
/// of them against `connectors/schema/connector.schema.json`, so the two lists
/// cannot drift.
macro_rules! slug_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $proto_name:literal, $proto:ty,
        $( $variant:ident => $slug:literal => $proto_variant:ident ),+ $(,)?
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                #[doc = concat!("`", $slug, "`")]
                #[serde(rename = $slug)]
                $variant,
            )+
        }

        impl $name {
            /// Every value, in the order §33 §4 and `connector.schema.json` list them.
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// The exact YAML slug, which is also the `connector.schema.json` enum
            /// member and the comment `connector.proto` gives the protobuf value.
            pub const fn slug(self) -> &'static str {
                match self {
                    $( Self::$variant => $slug, )+
                }
            }

            /// The value a manifest slug names, or `None` when the slug is not one of
            /// them. `Deserialize` is the usual path; this is for the drift checks that
            /// read a slug out of the CSV or out of §33 Appendix A.
            pub fn from_slug(slug: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.slug() == slug)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.slug())
            }
        }

        /// A YAML slug becomes the protobuf value `connector.proto` names for it.
        impl From<$name> for EnumValue<$proto> {
            fn from(value: $name) -> Self {
                match value {
                    $( $name::$variant => <$proto>::$proto_variant.into(), )+
                }
            }
        }

        /// A protobuf value becomes the YAML slug, or an error: the `_UNSPECIFIED`
        /// zero every generated enum carries, and any value off the wire, have no
        /// member in `connector.schema.json` and so cannot name a manifest field.
        impl TryFrom<EnumValue<$proto>> for $name {
            type Error = ProtoError;

            fn try_from(value: EnumValue<$proto>) -> Result<Self, Self::Error> {
                match value {
                    EnumValue::Known(proto) => match proto {
                        $( <$proto>::$proto_variant => Ok(Self::$variant), )+
                        other => Err(ProtoError::NoYamlSpelling {
                            enum_name: $proto_name,
                            value: other.proto_name().to_string(),
                        }),
                    },
                    EnumValue::Unknown(number) => Err(ProtoError::UnknownEnumValue {
                        enum_name: $proto_name,
                        value: number,
                    }),
                }
            }
        }

        /// The same, by reference, for the repeated enum fields (`auth`, `formats`,
        /// `conformance`), which are read in place.
        impl TryFrom<&EnumValue<$proto>> for $name {
            type Error = ProtoError;

            fn try_from(value: &EnumValue<$proto>) -> Result<Self, Self::Error> {
                match value {
                    EnumValue::Known(proto) => match proto {
                        $( <$proto>::$proto_variant => Ok(Self::$variant), )+
                        other => Err(ProtoError::NoYamlSpelling {
                            enum_name: $proto_name,
                            value: other.proto_name().to_string(),
                        }),
                    },
                    EnumValue::Unknown(number) => Err(ProtoError::UnknownEnumValue {
                        enum_name: $proto_name,
                        value: *number,
                    }),
                }
            }
        }
    };
}

slug_enum!(
    /// §33 §4's `category`: the kind of system the connector speaks to.
    Category, "Category", pb::Category,
        Messaging => "messaging" => CATEGORY_MESSAGING,
        Relational => "relational" => CATEGORY_RELATIONAL,
        Nosql => "nosql" => CATEGORY_NOSQL,
        Search => "search" => CATEGORY_SEARCH,
        Vector => "vector" => CATEGORY_VECTOR,
        Graph => "graph" => CATEGORY_GRAPH,
        Warehouse => "warehouse" => CATEGORY_WAREHOUSE,
        Lakehouse => "lakehouse" => CATEGORY_LAKEHOUSE,
        ObjectStorage => "object-storage" => CATEGORY_OBJECT_STORAGE,
        Format => "format" => CATEGORY_FORMAT,
        Cdc => "cdc" => CATEGORY_CDC,
        Integration => "integration" => CATEGORY_INTEGRATION,
        Saas => "saas" => CATEGORY_SAAS,
        Observability => "observability" => CATEGORY_OBSERVABILITY,
        Infra => "infra" => CATEGORY_INFRA,
        Identity => "identity" => CATEGORY_IDENTITY,
        Ai => "ai" => CATEGORY_AI,
        Protocol => "protocol" => CATEGORY_PROTOCOL,
);

slug_enum!(
    /// §33 §4's `priority`: the rollout phase, which for CN1 is also the build order.
    Priority, "Priority", pb::Priority,
        P1 => "P1" => PRIORITY_P1,
        P2 => "P2" => PRIORITY_P2,
        P3 => "P3" => PRIORITY_P3,
);

slug_enum!(
    /// §33 §4's `status`.
    Status, "Status", pb::Status,
        Planned => "planned" => STATUS_PLANNED,
        Preview => "preview" => STATUS_PREVIEW,
        Stable => "stable" => STATUS_STABLE,
        Deprecated => "deprecated" => STATUS_DEPRECATED,
);

slug_enum!(
    /// §33 §5's runtimes (D354): `native`, `iggy`, `camel`, `debezium`, `openapi`.
    RuntimeKind, "RuntimeKind", pb::RuntimeKind,
        Native => "native" => RUNTIME_KIND_NATIVE,
        Iggy => "iggy" => RUNTIME_KIND_IGGY,
        Camel => "camel" => RUNTIME_KIND_CAMEL,
        Debezium => "debezium" => RUNTIME_KIND_DEBEZIUM,
        Openapi => "openapi" => RUNTIME_KIND_OPENAPI,
);

slug_enum!(
    /// §33 §4's `capabilities.ordering`.
    Ordering, "Ordering", pb::Ordering,
        None => "none" => ORDERING_NONE,
        PerKey => "per_key" => ORDERING_PER_KEY,
        PerPartition => "per_partition" => ORDERING_PER_PARTITION,
        Total => "total" => ORDERING_TOTAL,
);

slug_enum!(
    /// §33 §4's `capabilities.delivery`: one direction's guarantee (D355).
    Delivery, "Delivery", pb::Delivery,
        AtMostOnce => "at_most_once" => DELIVERY_AT_MOST_ONCE,
        AtLeastOnce => "at_least_once" => DELIVERY_AT_LEAST_ONCE,
        ExactlyOnce => "exactly_once" => DELIVERY_EXACTLY_ONCE,
);

slug_enum!(
    /// §33 §4's `capabilities.formats`: the wire and payload formats.
    Format, "Format", pb::Format,
        CloudeventsBinary => "cloudevents-binary" => FORMAT_CLOUDEVENTS_BINARY,
        CloudeventsStructured => "cloudevents-structured" => FORMAT_CLOUDEVENTS_STRUCTURED,
        Json => "json" => FORMAT_JSON,
        Avro => "avro" => FORMAT_AVRO,
        Protobuf => "protobuf" => FORMAT_PROTOBUF,
        Bytes => "bytes" => FORMAT_BYTES,
        Arrow => "arrow" => FORMAT_ARROW,
        Parquet => "parquet" => FORMAT_PARQUET,
        Csv => "csv" => FORMAT_CSV,
        Ndjson => "ndjson" => FORMAT_NDJSON,
);

slug_enum!(
    /// §33 §4's `capabilities.schema.registry`.
    RegistryRequirement, "RegistryRequirement", pb::RegistryRequirement,
        None => "none" => REGISTRY_REQUIREMENT_NONE,
        Optional => "optional" => REGISTRY_REQUIREMENT_OPTIONAL,
        Required => "required" => REGISTRY_REQUIREMENT_REQUIRED,
);

slug_enum!(
    /// §33 §4's `capabilities.schema.evolution`.
    Evolution, "Evolution", pb::Evolution,
        None => "none" => EVOLUTION_NONE,
        Backward => "backward" => EVOLUTION_BACKWARD,
        BackwardTransitive => "backward_transitive" => EVOLUTION_BACKWARD_TRANSITIVE,
        Full => "full" => EVOLUTION_FULL,
);

slug_enum!(
    /// §33 §4's `capabilities.backpressure`.
    Backpressure, "Backpressure", pb::Backpressure,
        Pull => "pull" => BACKPRESSURE_PULL,
        PushWithAck => "push-with-ack" => BACKPRESSURE_PUSH_WITH_ACK,
        PushRateLimited => "push-rate-limited" => BACKPRESSURE_PUSH_RATE_LIMITED,
);

slug_enum!(
    /// §33 §4's `auth`: the slugs of Appendix A's legend.
    AuthMethod, "AuthMethod", pb::AuthMethod,
        None => "none" => AUTH_METHOD_NONE,
        Basic => "basic" => AUTH_METHOD_BASIC,
        Key => "key" => AUTH_METHOD_KEY,
        Oauth2 => "oauth2" => AUTH_METHOD_OAUTH2,
        Jwt => "jwt" => AUTH_METHOD_JWT,
        Hmac => "hmac" => AUTH_METHOD_HMAC,
        Mtls => "mtls" => AUTH_METHOD_MTLS,
        Sasl => "sasl" => AUTH_METHOD_SASL,
        SaslPlain => "sasl-plain" => AUTH_METHOD_SASL_PLAIN,
        SaslScram256 => "sasl-scram-256" => AUTH_METHOD_SASL_SCRAM_256,
        SaslScram512 => "sasl-scram-512" => AUTH_METHOD_SASL_SCRAM_512,
        AwsMskIam => "aws-msk-iam" => AUTH_METHOD_AWS_MSK_IAM,
        Iam => "iam" => AUTH_METHOD_IAM,
        Sa => "sa" => AUTH_METHOD_SA,
        Aad => "aad" => AUTH_METHOD_AAD,
        Ssh => "ssh" => AUTH_METHOD_SSH,
        Kerb => "kerb" => AUTH_METHOD_KERB,
        Cs => "cs" => AUTH_METHOD_CS,
        PerDriver => "per-driver" => AUTH_METHOD_PER_DRIVER,
        PerSpec => "per-spec" => AUTH_METHOD_PER_SPEC,
        PerComponent => "per-component" => AUTH_METHOD_PER_COMPONENT,
);

slug_enum!(
    /// §33 §4's `conformance`: the suites the connector passes (rule 3).
    Suite, "Suite", pb::Suite,
        Contract => "contract" => SUITE_CONTRACT,
        Roundtrip => "roundtrip" => SUITE_ROUNDTRIP,
        KillRestart => "kill-restart" => SUITE_KILL_RESTART,
        DupCheck => "dup-check" => SUITE_DUP_CHECK,
        Bulk => "bulk" => SUITE_BULK,
        Cdc => "cdc" => SUITE_CDC,
);

/// §33 §4's `runtime`: which runtime runs this connector, and what it points at.
///
/// A struct rather than CN1 plan Task 1's `RuntimeRef` enum, because §33 §4's YAML
/// is the source (D352) and its `ref` comment gives `kind` a per-value meaning — a
/// crate path, an Iggy plugin name, a Camel URI scheme, a Debezium connector class,
/// an OpenAPI spec URL — that an enum would restate and could disagree with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSpec {
    /// §33 §5's runtime (D354).
    pub kind: RuntimeKind,
    /// §33 §4's `runtime.ref`, the thing `kind` points at.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The pinned driver or library version behind `reference`, free-form: it spans
    /// crates and native builds. Absent on the generated stubs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// §33 §4's `config: {$ref: …}` — where the instance-config JSON Schema lives.
///
/// D353's manifest declares the shape of an instance's settings, so the reference
/// is a repository-relative path under `connectors/` (`schemas/kafka.config.json`)
/// that the loader resolves; `ConnectorSpec::config_schema` holds the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRef {
    /// A path under `connectors/`, for example `schemas/kafka.config.json`.
    #[serde(rename = "$ref")]
    pub reference: String,
}

/// §33 §4's `licence`, as D359's gate reads it: the runtime component's own SPDX
/// id, and the SPDX id of every library or driver it loads.
///
/// `dependencies` is sorted rather than insertion-ordered because the protobuf form
/// is a protobuf `map<string, string>`, which carries no order: a sorted map
/// compares equal after the round trip whatever order the YAML wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Licence {
    /// An SPDX id, for example `Apache-2.0`.
    pub component: String,
    /// Every loaded dependency by name, with its SPDX id. Empty for a component with
    /// none, and for a SaaS service reached through its public API (D359).
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// §33 §4's `capabilities.source`. Omitted from a sink-only connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCaps {
    /// Continuous, low-latency delivery.
    pub streaming: bool,
    /// Bulk or scheduled transfer.
    pub batch: bool,
    /// Row-level change capture (§33 §7, D357).
    pub cdc: bool,
    /// Receives webhooks as a source (CN1 Task 4's signed schemes).
    pub webhook: bool,
    /// Restarts from a committed position, which is what makes an at-least-once
    /// source safe to kill and restart (CN1 Ruling 4).
    pub resumable: bool,
    /// What the source checkpoints, for example `kafka-offsets`. Free-form, because
    /// each system names its own position; empty when it has none.
    pub position: String,
}

/// §33 §4's `capabilities.sink`. Omitted from a source-only connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkCaps {
    /// Continuous, low-latency delivery.
    pub streaming: bool,
    /// Bulk or scheduled writes.
    pub batch: bool,
    /// The sink writes inside the target system's transaction.
    pub transactional: bool,
    /// The sink upserts by a key it declares in its config schema. A route that
    /// upserts needs this (§33 §4 rule 1).
    pub upsert: bool,
    /// The sink applies deletes, which a CDC route needs.
    pub delete: bool,
    /// The sink itself dedupes retries, for example Kafka's idempotent producer.
    pub idempotent: bool,
}

/// §33 §4's `capabilities.delivery`, one guarantee per direction. Every runtime is
/// at-least-once underneath (§33 §6) and the target's own dedup key makes it
/// effectively once where the target dedupes (§33 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryCaps {
    /// The source's guarantee: re-delivery after a crash is expected.
    pub source: Delivery,
    /// The sink's guarantee. `exactly_once` is a claim only where the external system
    /// deduplicates (§33 §2.2).
    pub sink: Delivery,
}

/// §33 §4's `capabilities.schema`: whether a registry is involved, and how the
/// connector evolves a schema on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaCaps {
    /// `none`, `optional` or `required`.
    pub registry: RegistryRequirement,
    /// The compatibility rule the connector holds its schemas to.
    pub evolution: Evolution,
}

/// §33 §4's `capabilities.bulk`: bulk moves as Arrow, never row by row (D356).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkCaps {
    /// The connector takes or produces Arrow IPC batches (D356).
    pub arrow: bool,
    /// Rows per Arrow batch; CN1 Ruling 3's default is 65536.
    pub max_batch_rows: u64,
}

/// §33 §4's `capabilities` (D353). Every field is declared, never inferred:
/// `ValidateRoute` refuses a route that uses an undeclared capability
/// (§33 §4 rule 1) and the contract tests prove each declared one (rule 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Omitted for a sink-only connector: a webhooks source, for example.
    ///
    /// `skip_serializing_if` because `connector.schema.json` types `source` and `sink`
    /// as objects and not as nullable ones: a manifest omits the key for a direction
    /// the connector does not serve, and writing an explicit `null` fails the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceCaps>,
    /// Omitted for a source-only connector: a JDBC sink, for example.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink: Option<SinkCaps>,
    /// The guarantee per direction.
    pub delivery: DeliveryCaps,
    /// The order events keep.
    pub ordering: Ordering,
    /// The wire and payload formats the connector emits or consumes.
    pub formats: Vec<Format>,
    /// Schema handling.
    pub schema: SchemaCaps,
    /// The bulk path.
    pub bulk: BulkCaps,
    /// How the sink tells the Fabric to slow down.
    pub backpressure: Backpressure,
}

/// §33 §4's `envelope`: the CloudEvents contract this connector emits and consumes
/// (D355).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeDecl {
    /// The type this connector emits, for example
    /// `io.loams.dev.flow.kafka.record.v1`; empty when it emits none. Never `*`.
    pub emits: String,
    /// The type it consumes, or `*` for any.
    pub consumes: String,
    /// Input that already is a valid CloudEvent keeps its producer's `type`,
    /// `source` and `id`, and the connector adds only `loamsconnector` and
    /// `loamsinstance` (§33 §6). False when the manifest omits it, which
    /// `connector.schema.json` allows.
    #[serde(default)]
    pub passthrough: bool,
}

/// §33 §4's `limits`: the bounds the runtime enforces. An absent bound leaves the
/// runtime's default (Ruling 3's `batch_bytes` is 8 MiB, Iggy's message size limit).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The largest single record, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_record_bytes: Option<u64>,
    /// Rows per batch for a batch connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_rows: Option<u64>,
    /// The largest batch the connector sends, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_bytes: Option<u64>,
}

/// §33 §4's manifest as typed Rust: one connector, what it can do (D353), which
/// runtime runs it (D354) and what it is licensed under (D359).
///
/// Every field name is its YAML key, and the rename rule is per block rather than
/// crate-wide because the manifest is not one convention: `specVersion` is the only
/// camelCase key in §33 §4, while every nested block spells its multi-word keys in
/// snake_case (`bulk.max_batch_rows`, `limits.max_record_bytes`, `limits.batch_bytes`).
/// So `camelCase` applies here and nowhere else, and `config_ref` is renamed again to
/// the one-key `config: {$ref: …}` object the JSON Schema requires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSpec {
    /// The registry key, `[a-z0-9-]{1,48}` and the manifest's file name.
    pub id: String,
    /// The display name, for example `Apache Kafka`.
    pub name: String,
    /// §33 §4 rule 4: a capability removed or narrowed bumps this major, and routes
    /// pinned to the old major keep running until migrated.
    pub spec_version: semver::Version,
    /// The kind of system the connector speaks to.
    pub category: Category,
    /// The rollout phase.
    pub priority: Priority,
    /// True for the 21 ★ connectors Loams owns and ships in CN1 (D358).
    pub starred: bool,
    /// The connector's maturity.
    pub status: Status,
    /// Which runtime runs it.
    pub runtime: RuntimeSpec,
    /// The licences D359's gate checks.
    pub licence: Licence,
    /// What the connector can do.
    pub capabilities: Capabilities,
    /// The auth methods it accepts.
    pub auth: Vec<AuthMethod>,
    /// Where the instance-config JSON Schema lives.
    #[serde(rename = "config")]
    pub config_ref: ConfigRef,
    /// The resolved instance-config JSON Schema, read from `config_ref` relative to
    /// the `connectors/` root. Not part of the document: the path is what
    /// round-trips, and this is the document it resolved to.
    #[serde(skip)]
    pub config_schema: serde_json::Value,
    /// The config fields that hold secrets, named as dotted paths (`sasl.password`).
    /// Resolved through the namespace's secret store (D189) and never stored in the
    /// instance.
    pub secrets: Vec<String>,
    /// The CloudEvents types emitted and consumed.
    pub envelope: EnvelopeDecl,
    /// The enforced bounds.
    pub limits: Limits,
    /// The conformance suites this connector passes (§33 §4 rule 3).
    pub conformance: Vec<Suite>,
    /// The guide page, or `None` when a generated stub has none (CN1 Ruling 1).
    pub docs: Option<String>,
}

/// One problem with one manifest: which field, and what is wrong with it.
///
/// The path is dotted rather than a JSON Pointer because that is how the manifest
/// reads (`capabilities.source.position`), and it is the file's own name when the
/// problem is with the file rather than with a field of it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ManifestError {
    /// The manifest field, or the manifest's file name.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
}

impl ManifestError {
    /// One error about `path`.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// One manifest file's errors, kept together so a load reports which file each came
/// from. `Display` renders all of them, one per line, under the file's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestErrors {
    /// The manifest's file name, for example `kafka.yaml`.
    pub file: String,
    /// Every problem found in it, not just the first.
    pub errors: Vec<ManifestError>,
}

impl fmt::Display for ManifestErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}:", self.file)?;
        for error in &self.errors {
            writeln!(f, "  {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ManifestErrors {}

/// A protobuf value that no YAML slug can name, or a manifest field the protobuf
/// form cannot carry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    /// A protobuf enum value with no `connector.schema.json` member: the
    /// `_UNSPECIFIED` zero every generated enum carries, or a value a newer
    /// `connector.proto` added.
    #[error("{enum_name}: the protobuf value {value} has no YAML slug")]
    NoYamlSpelling {
        /// The protobuf enum's name, for example `Category`.
        enum_name: &'static str,
        /// The protobuf value's proto name, for example `CATEGORY_UNSPECIFIED`.
        value: String,
    },
    /// A wire value outside the generated enum's known range.
    #[error("{enum_name}: the wire value {value} is not a known protobuf enum value")]
    UnknownEnumValue {
        /// The protobuf enum's name, for example `Category`.
        enum_name: &'static str,
        /// The wire value.
        value: i32,
    },
    /// A `specVersion` that is not semver, which `connector.schema.json` requires.
    #[error("specVersion: {value:?} is not a semver version")]
    InvalidSpecVersion {
        /// The offending text.
        value: String,
    },
    /// A required submessage the protobuf form does not carry.
    #[error("{field}: the protobuf message carries no value")]
    MissingMessage {
        /// The manifest field, named as the YAML key.
        field: &'static str,
    },
}

impl ConnectorSpec {
    /// The manifest as the document `connector.schema.json` validates: the typed
    /// spec plus the two `const` keys, `apiVersion` and `kind`, which
    /// `connector.proto` gives no field for.
    pub fn to_json(&self) -> Result<serde_json::Value, ManifestError> {
        let mut document = serde_json::to_value(self).map_err(|error| {
            ManifestError::new(&self.id, format!("cannot render as JSON: {error}"))
        })?;
        if let Some(object) = document.as_object_mut() {
            object.insert(
                "apiVersion".to_string(),
                serde_json::Value::String(API_VERSION.to_string()),
            );
            object.insert(
                "kind".to_string(),
                serde_json::Value::String(KIND.to_string()),
            );
        }
        Ok(document)
    }

    /// The manifest as YAML, the shape `connectors/registry/*.yaml` has, so
    /// [`load_manifest`] accepts what `to_yaml` writes.
    pub fn to_yaml(&self) -> Result<String, ManifestError> {
        serde_norway::to_string(&self.to_json()?).map_err(|error| {
            ManifestError::new(&self.id, format!("cannot render as YAML: {error}"))
        })
    }

    /// The protobuf form, which §33 §4's last paragraph says mirrors the YAML one to
    /// one: every field set, every YAML slug translated to the proto name
    /// `connector.proto` gives it.
    ///
    /// `config_schema` has no counterpart — the protobuf form carries the
    /// `config.$ref` path as `config_ref` — which is why [`Self::from_proto`] takes
    /// the resolved document back as an argument.
    pub fn to_proto(&self) -> Result<pb::ConnectorSpec, ProtoError> {
        Ok(pb::ConnectorSpec {
            id: self.id.clone(),
            name: self.name.clone(),
            spec_version: self.spec_version.to_string(),
            category: self.category.into(),
            priority: self.priority.into(),
            starred: self.starred,
            status: self.status.into(),
            runtime: pb::RuntimeRef::from(self.runtime.clone()).into(),
            licence: pb::Licence {
                component: self.licence.component.clone(),
                dependencies: self
                    .licence
                    .dependencies
                    .iter()
                    .map(|(name, spdx)| (name.clone(), spdx.clone()))
                    .collect(),
                ..Default::default()
            }
            .into(),
            capabilities: pb::Capabilities {
                source: self
                    .capabilities
                    .source
                    .clone()
                    .map(pb::SourceCaps::from)
                    .into(),
                sink: self
                    .capabilities
                    .sink
                    .clone()
                    .map(pb::SinkCaps::from)
                    .into(),
                delivery: pb::DeliveryCaps::from(self.capabilities.delivery.clone()).into(),
                ordering: self.capabilities.ordering.into(),
                formats: self
                    .capabilities
                    .formats
                    .iter()
                    .copied()
                    .map(Into::into)
                    .collect(),
                schema: pb::SchemaCaps {
                    registry: self.capabilities.schema.registry.into(),
                    evolution: self.capabilities.schema.evolution.into(),
                    ..Default::default()
                }
                .into(),
                bulk: pb::BulkCaps::from(self.capabilities.bulk.clone()).into(),
                backpressure: self.capabilities.backpressure.into(),
                ..Default::default()
            }
            .into(),
            auth: self.auth.iter().copied().map(Into::into).collect(),
            config_ref: self.config_ref.reference.clone(),
            secrets: self.secrets.clone(),
            envelope: pb::EnvelopeDecl::from(self.envelope.clone()).into(),
            limits: pb::Limits::from(self.limits.clone()).into(),
            conformance: self.conformance.iter().copied().map(Into::into).collect(),
            docs: self.docs.clone().unwrap_or_default(),
            ..Default::default()
        })
    }

    /// The manifest back from its protobuf form.
    ///
    /// `config_schema` is the resolved instance-config document, because the
    /// protobuf form carries only the path: the caller resolves it again, and the
    /// two documents must agree or the two forms have drifted.
    pub fn from_proto(
        proto: &pb::ConnectorSpec,
        config_schema: serde_json::Value,
    ) -> Result<Self, ProtoError> {
        Ok(Self {
            id: proto.id.clone(),
            name: proto.name.clone(),
            spec_version: semver::Version::parse(&proto.spec_version).map_err(|_| {
                ProtoError::InvalidSpecVersion {
                    value: proto.spec_version.clone(),
                }
            })?,
            category: proto.category.try_into()?,
            priority: proto.priority.try_into()?,
            starred: proto.starred,
            status: proto.status.try_into()?,
            runtime: {
                let runtime: pb::RuntimeRef = required("runtime", proto.runtime.clone())?;
                runtime.try_into()?
            },
            licence: {
                let licence: pb::Licence = required("licence", proto.licence.clone())?;
                licence.into()
            },
            capabilities: {
                let capabilities: pb::Capabilities =
                    required("capabilities", proto.capabilities.clone())?;
                capabilities.try_into()?
            },
            auth: proto
                .auth
                .iter()
                .map(|method| method.try_into())
                .collect::<Result<Vec<AuthMethod>, ProtoError>>()?,
            config_ref: ConfigRef {
                reference: proto.config_ref.clone(),
            },
            config_schema,
            secrets: proto.secrets.clone(),
            envelope: {
                let envelope: pb::EnvelopeDecl = required("envelope", proto.envelope.clone())?;
                envelope.into()
            },
            limits: {
                let limits: pb::Limits = required("limits", proto.limits.clone())?;
                limits.into()
            },
            conformance: proto
                .conformance
                .iter()
                .map(|suite| suite.try_into())
                .collect::<Result<Vec<Suite>, ProtoError>>()?,
            docs: (!proto.docs.is_empty()).then(|| proto.docs.clone()),
        })
    }
}

/// [`RuntimeSpec`] → `pb::RuntimeRef`. Infallible in this direction: every
/// `RuntimeKind` has a protobuf value, and it is the reverse direction that can meet
/// the `_UNSPECIFIED` zero and fail.
impl From<RuntimeSpec> for pb::RuntimeRef {
    fn from(spec: RuntimeSpec) -> Self {
        Self {
            kind: spec.kind.into(),
            r#ref: spec.reference,
            version: spec.version.unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// `pb::RuntimeRef` → [`RuntimeSpec`].
impl TryFrom<pb::RuntimeRef> for RuntimeSpec {
    type Error = ProtoError;

    fn try_from(proto: pb::RuntimeRef) -> Result<Self, Self::Error> {
        Ok(Self {
            kind: proto.kind.try_into()?,
            reference: proto.r#ref,
            version: (!proto.version.is_empty()).then_some(proto.version),
        })
    }
}

/// `pb::Licence` → [`Licence`].
impl From<pb::Licence> for Licence {
    fn from(proto: pb::Licence) -> Self {
        Self {
            component: proto.component,
            dependencies: proto.dependencies.into_iter().collect(),
        }
    }
}

/// `pb::SourceCaps` → [`SourceCaps`].
impl From<pb::SourceCaps> for SourceCaps {
    fn from(proto: pb::SourceCaps) -> Self {
        Self {
            streaming: proto.streaming,
            batch: proto.batch,
            cdc: proto.cdc,
            webhook: proto.webhook,
            resumable: proto.resumable,
            position: proto.position,
        }
    }
}

/// [`SourceCaps`] → `pb::SourceCaps`.
impl From<SourceCaps> for pb::SourceCaps {
    fn from(caps: SourceCaps) -> Self {
        Self {
            streaming: caps.streaming,
            batch: caps.batch,
            cdc: caps.cdc,
            webhook: caps.webhook,
            resumable: caps.resumable,
            position: caps.position,
            ..Default::default()
        }
    }
}

/// `pb::SinkCaps` → [`SinkCaps`].
impl From<pb::SinkCaps> for SinkCaps {
    fn from(proto: pb::SinkCaps) -> Self {
        Self {
            streaming: proto.streaming,
            batch: proto.batch,
            transactional: proto.transactional,
            upsert: proto.upsert,
            delete: proto.delete,
            idempotent: proto.idempotent,
        }
    }
}

/// [`SinkCaps`] → `pb::SinkCaps`.
impl From<SinkCaps> for pb::SinkCaps {
    fn from(caps: SinkCaps) -> Self {
        Self {
            streaming: caps.streaming,
            batch: caps.batch,
            transactional: caps.transactional,
            upsert: caps.upsert,
            delete: caps.delete,
            idempotent: caps.idempotent,
            ..Default::default()
        }
    }
}

/// `pb::DeliveryCaps` → [`DeliveryCaps`].
impl TryFrom<pb::DeliveryCaps> for DeliveryCaps {
    type Error = ProtoError;

    fn try_from(proto: pb::DeliveryCaps) -> Result<Self, Self::Error> {
        Ok(Self {
            source: proto.source.try_into()?,
            sink: proto.sink.try_into()?,
        })
    }
}

/// [`DeliveryCaps`] → `pb::DeliveryCaps`.
impl From<DeliveryCaps> for pb::DeliveryCaps {
    fn from(caps: DeliveryCaps) -> Self {
        Self {
            source: caps.source.into(),
            sink: caps.sink.into(),
            ..Default::default()
        }
    }
}

/// `pb::SchemaCaps` → [`SchemaCaps`].
impl TryFrom<pb::SchemaCaps> for SchemaCaps {
    type Error = ProtoError;

    fn try_from(proto: pb::SchemaCaps) -> Result<Self, Self::Error> {
        Ok(Self {
            registry: proto.registry.try_into()?,
            evolution: proto.evolution.try_into()?,
        })
    }
}

/// `pb::Capabilities` → [`Capabilities`], refusing a protobuf enum value that has no
/// YAML slug. `source` and `sink` stay optional, because their presence is the
/// declaration of the direction (§33 §4's capability schema).
impl TryFrom<pb::Capabilities> for Capabilities {
    type Error = ProtoError;

    fn try_from(proto: pb::Capabilities) -> Result<Self, Self::Error> {
        // `MessageField` derefs to its message, so each nested value is read off
        // the protobuf form and put back into a defaulted literal for the `From`
        // impls below; every field of those messages is `Copy` except `position`.
        let source: Option<pb::SourceCaps> = proto.source.clone().into();
        let sink: Option<pb::SinkCaps> = proto.sink.clone().into();
        Ok(Self {
            source: source.map(Into::into),
            sink: sink.map(Into::into),
            delivery: pb::DeliveryCaps {
                source: proto.delivery.source,
                sink: proto.delivery.sink,
                ..Default::default()
            }
            .try_into()?,
            ordering: proto.ordering.try_into()?,
            formats: proto
                .formats
                .iter()
                .map(|format| format.try_into())
                .collect::<Result<Vec<Format>, ProtoError>>()?,
            schema: pb::SchemaCaps {
                registry: proto.schema.registry,
                evolution: proto.schema.evolution,
                ..Default::default()
            }
            .try_into()?,
            bulk: pb::BulkCaps {
                arrow: proto.bulk.arrow,
                max_batch_rows: proto.bulk.max_batch_rows,
                ..Default::default()
            }
            .into(),
            backpressure: proto.backpressure.try_into()?,
        })
    }
}

/// `pb::BulkCaps` → [`BulkCaps`]. A negative wire value cannot be a row count, so
/// this saturates at zero rather than wrapping into a huge `u64`.
impl From<pb::BulkCaps> for BulkCaps {
    fn from(proto: pb::BulkCaps) -> Self {
        Self {
            arrow: proto.arrow,
            max_batch_rows: proto.max_batch_rows.max(0) as u64,
        }
    }
}

/// [`BulkCaps`] → `pb::BulkCaps`.
impl From<BulkCaps> for pb::BulkCaps {
    fn from(caps: BulkCaps) -> Self {
        Self {
            arrow: caps.arrow,
            max_batch_rows: to_i64(caps.max_batch_rows),
            ..Default::default()
        }
    }
}

/// `pb::EnvelopeDecl` → [`EnvelopeDecl`].
impl From<pb::EnvelopeDecl> for EnvelopeDecl {
    fn from(proto: pb::EnvelopeDecl) -> Self {
        Self {
            emits: proto.emits,
            consumes: proto.consumes,
            passthrough: proto.passthrough,
        }
    }
}

/// [`EnvelopeDecl`] → `pb::EnvelopeDecl`.
impl From<EnvelopeDecl> for pb::EnvelopeDecl {
    fn from(decl: EnvelopeDecl) -> Self {
        Self {
            emits: decl.emits,
            consumes: decl.consumes,
            passthrough: decl.passthrough,
            ..Default::default()
        }
    }
}

/// `pb::Limits` → [`Limits`]: protobuf has no optional scalars, so the wire's `0` is
/// this form's absent bound. `connector.schema.json` sets `minimum: 1` on all
/// three, so no manifest can write the `0` this collapses.
impl From<pb::Limits> for Limits {
    fn from(proto: pb::Limits) -> Self {
        Self {
            max_record_bytes: present(proto.max_record_bytes),
            batch_rows: present(proto.batch_rows),
            batch_bytes: present(proto.batch_bytes),
        }
    }
}

/// [`Limits`] → `pb::Limits`, an absent bound becoming the wire's `0`.
impl From<Limits> for pb::Limits {
    fn from(limits: Limits) -> Self {
        Self {
            max_record_bytes: to_i64(limits.max_record_bytes.unwrap_or_default()),
            batch_rows: to_i64(limits.batch_rows.unwrap_or_default()),
            batch_bytes: to_i64(limits.batch_bytes.unwrap_or_default()),
            ..Default::default()
        }
    }
}

/// The message of a required protobuf field, which `connector.schema.json` requires
/// of every manifest and which therefore has to be there for the round trip to mean
/// anything.
fn required<T>(field: &'static str, message: impl Into<Option<T>>) -> Result<T, ProtoError> {
    message.into().ok_or(ProtoError::MissingMessage { field })
}

/// The wire's `0` means absent; anything above zero is a real bound.
fn present(value: i64) -> Option<u64> {
    (value > 0).then_some(value as u64)
}

/// `u64` → protobuf `int64`. The JSON Schema puts no upper bound on a limit, so a
/// value above `i64::MAX` saturates rather than wrapping into a negative bound.
fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The manifest at `path`, with its `config.$ref` resolved against the
/// `connectors/` root `root`.
///
/// Reads the YAML, checks it against `connectors/schema/connector.schema.json`,
/// runs CN1 Task 1's semantic rules and loads the instance-config schema the
/// reference names. Every problem is returned, not just the first.
pub fn load_manifest(path: &Path, root: &Path) -> Result<ConnectorSpec, Vec<ManifestError>> {
    let schema = ManifestSchema::embedded().map_err(|error| {
        // A schema that will not compile is a problem with the loader's own input,
        // so it is reported against the schema's path; the message repeats it because
        // `SchemaError`'s `Display` is written to be read on its own.
        vec![ManifestError::new(
            error.to_string(),
            "the manifest schema does not load",
        )]
    })?;
    load_manifest_with_schema(path, root, &schema)
}

/// [`load_manifest`] against an already-compiled manifest schema, which is how
/// [`crate::registry::Registry::load`] compiles it once for 200 manifests.
pub fn load_manifest_with_schema(
    path: &Path,
    root: &Path,
    schema: &ManifestSchema,
) -> Result<ConnectorSpec, Vec<ManifestError>> {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let text = std::fs::read_to_string(path)
        .map_err(|error| vec![ManifestError::new(&file, format!("cannot read: {error}"))])?;
    let value: serde_json::Value = serde_norway::from_str(&text).map_err(|error| {
        vec![ManifestError::new(
            &file,
            format!("not valid YAML: {error}"),
        )]
    })?;
    load_manifest_value(&file, &value, root, schema)
}

/// The manifest already parsed into a JSON value, resolved against `root`.
pub fn load_manifest_value(
    file: &str,
    value: &serde_json::Value,
    root: &Path,
    schema: &ManifestSchema,
) -> Result<ConnectorSpec, Vec<ManifestError>> {
    // The shape first, so a malformed manifest is reported as malformed before the
    // loader reaches into it for a `$ref`.
    let shape = crate::validate::schema_violations(value, schema.validator());
    if !shape.is_empty() {
        return Err(shape);
    }
    let reference = value["config"]["$ref"].as_str().unwrap_or_default();
    let config_schema = load_config_schema(file, reference, root)?;
    let mut spec = crate::validate::validate_manifest_with_config_schema(
        value,
        schema.validator(),
        Some(&config_schema),
    )?;
    spec.config_schema = config_schema;
    Ok(spec)
}

/// The instance-config JSON Schema a manifest's `config.$ref` names, read relative to
/// the `connectors/` root.
///
/// D353 makes this part of loading — the manifest declares the shape of an
/// instance's settings — and CN1 "Rulings made during execution" row 5 killed the
/// exemption for `status: planned` rows, so a generated stub's `$ref` must resolve
/// too. A path that leaves `root` is refused: a manifest reaching out of the
/// `connectors/` tree cannot be shipped in the image.
fn load_config_schema(
    file: &str,
    reference: &str,
    root: &Path,
) -> Result<serde_json::Value, Vec<ManifestError>> {
    let path = Path::new(reference);
    if path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(vec![ManifestError::new(
            format!("{file}.config.$ref"),
            format!("{reference:?} is not a path under the registry root"),
        )]);
    }
    let target = root.join(path);
    let text = std::fs::read_to_string(&target).map_err(|error| {
        vec![ManifestError::new(
            format!("{file}.config.$ref"),
            format!("{reference} does not resolve: {error}"),
        )]
    })?;
    serde_json::from_str(&text).map_err(|error| {
        vec![ManifestError::new(
            format!("{file}.config.$ref"),
            format!("{reference} is not valid JSON: {error}"),
        )]
    })
}

/// This repository's root, derived from the crate's own location.
///
/// The crate is unpublished precisely because it reads `connectors/` from the
/// repository (D352, D359), so its compile-time location is the one path every
/// default can come from.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[cfg(test)]
mod tests {
    //! The enum slugs are the one thing in this crate written out by hand twice —
    //! once in Rust, once in `connectors/schema/connector.schema.json` — so the two
    //! are compared here, both ways, against the schema file itself.

    use super::*;

    /// §33 §4 and `connector.schema.json` are the authority; this crate is not.
    const SCHEMA_JSON: &str = include_str!("../../../../connectors/schema/connector.schema.json");

    /// The slugs of one enum's values, in declaration order.
    macro_rules! slugs {
        ($name:ty) => {
            <$name>::ALL
                .iter()
                .map(|value| value.slug())
                .collect::<Vec<_>>()
        };
    }

    fn schema() -> serde_json::Value {
        serde_json::from_str(SCHEMA_JSON).unwrap_or_else(|error| {
            panic!("connectors/schema/connector.schema.json is not valid JSON: {error}")
        })
    }

    /// One enum: the JSON pointer of its `connector.schema.json` list, and the slugs
    /// Rust writes for it.
    fn compare(pointer: &str, declared: &[&str]) {
        let mut node = &schema();
        for segment in pointer.split('/').skip(1) {
            node = &node[segment];
        }
        let listed: Vec<&str> = node["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{pointer} has no enum in the schema"))
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("{pointer} has a non-string member"))
            })
            .collect();
        assert_eq!(
            listed, declared,
            "{pointer}: the schema's enum and the Rust variants, in order"
        );
        assert!(
            listed.iter().all(|slug| !slug.is_empty()),
            "{pointer}: an empty slug would accept any string"
        );
    }

    #[test]
    fn enum_slugs_match_the_json_schema() {
        // Every enum list the schema holds, in the schema's own order: §33 §4's
        // category comment, priority and status, §33 §5's runtimes, the capability
        // enums, Appendix A's auth legend and §33 §4's conformance suites.
        compare("/properties/category", &slugs!(Category));
        compare("/properties/priority", &slugs!(Priority));
        compare("/properties/status", &slugs!(Status));
        compare("/properties/runtime/properties/kind", &slugs!(RuntimeKind));
        compare(
            "/properties/capabilities/properties/ordering",
            &slugs!(Ordering),
        );
        compare("/$defs/delivery", &slugs!(Delivery));
        compare(
            "/properties/capabilities/properties/formats/items",
            &slugs!(Format),
        );
        compare(
            "/properties/capabilities/properties/schema/properties/registry",
            &slugs!(RegistryRequirement),
        );
        compare(
            "/properties/capabilities/properties/schema/properties/evolution",
            &slugs!(Evolution),
        );
        compare(
            "/properties/capabilities/properties/backpressure",
            &slugs!(Backpressure),
        );
        compare("/properties/auth/items", &slugs!(AuthMethod));
        compare("/properties/conformance/items", &slugs!(Suite));
    }

    #[test]
    fn every_enum_value_survives_serde_and_the_proto_name() {
        for value in Category::ALL {
            assert_eq!(Category::from_slug(value.slug()), Some(*value));
            let proto: EnumValue<pb::Category> = (*value).into();
            assert_eq!(
                proto.to_string(),
                format!("CATEGORY_{}", proto_name_body(value.slug())),
                "the proto3 JSON name is the proto name, not the YAML slug"
            );
            let back: Category = proto
                .try_into()
                .unwrap_or_else(|error| panic!("{}: {error}", value.slug()));
            assert_eq!(back, *value, "{}", value.slug());
        }
        for value in AuthMethod::ALL {
            assert_eq!(AuthMethod::from_slug(value.slug()), Some(*value));
        }
        for value in Format::ALL {
            assert_eq!(Format::from_slug(value.slug()), Some(*value));
        }
        for value in Suite::ALL {
            assert_eq!(Suite::from_slug(value.slug()), Some(*value));
        }
    }

    /// `object-storage` and `at_least_once` become `OBJECT_STORAGE` and
    /// `AT_LEAST_ONCE`: the proto name is the slug upper-cased with its separators
    /// folded to `_`, which is exactly what `connector.proto` writes by hand.
    fn proto_name_body(slug: &str) -> String {
        slug.to_uppercase().replace(['-', '_'], "_")
    }

    #[test]
    fn a_proto_zero_value_has_no_yaml_slug() {
        // Every generated enum carries an `_UNSPECIFIED` zero, and no YAML slug
        // stands for it: `connector.schema.json` lists only the real members.
        let error = Category::try_from(EnumValue::from(pb::Category::CATEGORY_UNSPECIFIED)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some("Category: the protobuf value CATEGORY_UNSPECIFIED has no YAML slug".to_string())
        );
        let error = Suite::try_from(EnumValue::Unknown(99)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some("Suite: the wire value 99 is not a known protobuf enum value".to_string())
        );
    }
}
