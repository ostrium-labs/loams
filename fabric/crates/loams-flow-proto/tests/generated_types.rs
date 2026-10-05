//! CN1 plan Task 1: the `loams.flow.v1` manifest types as `build.rs` generates
//! them, so a change to `fabric/proto/loams/flow/v1/` that moves a field or
//! renames a value is caught here rather than in `loams-flow`'s loader.
//!
//! The names asserted below are the *generated* ones, read out of `$OUT_DIR`
//! rather than guessed from the proto: an enum value keeps its
//! UPPER_SNAKE_CASE name (`CATEGORY_MESSAGING`) and gains a CamelCase alias
//! constant beside it (`Category::Messaging`); a message field keeps its
//! lower_snake_case name (`spec_version`, with `ref` a raw identifier,
//! `r#ref`); every message also carries a `__buffa_unknown_fields` field that
//! the JSON mapping skips. The JSON keys are the YAML manifest keys of
//! `connectors/registry/*.yaml`, because D352 makes the YAML canonical and this
//! form mirrors it one to one — and an enum value serializes as its *proto*
//! name, not as the YAML spelling its `connector.proto` comment gives.

use std::collections::BTreeSet;

use buffa::{EnumValue, Enumeration, Message};
use buffa_types::google::protobuf::Struct;
use loams_flow_proto::loams::flow::v1::{
    __buffa::oneof::fabric_binding::Target, AuthMethod, Backpressure, Capabilities, Category,
    ConnectorSpec, Delivery, DeliveryCaps, Direction, EnvelopeDecl, FabricBinding, Format,
    Instance, Licence, Limits, Ordering, Priority, RuntimeKind, RuntimeRef, SinkCaps, SourceCaps,
    Status, Suite,
};
use serde_json::json;

/// A manifest with every field off its default, so the field assertions below
/// cannot pass on a `Default`.
fn spec() -> ConnectorSpec {
    let capabilities = Capabilities {
        source: SourceCaps {
            streaming: true,
            batch: true,
            cdc: false,
            webhook: false,
            resumable: true,
            position: "kafka-offsets".to_string(),
            ..Default::default()
        }
        .into(),
        sink: SinkCaps {
            streaming: true,
            batch: false,
            transactional: false,
            upsert: false,
            delete: true,
            idempotent: true,
            ..Default::default()
        }
        .into(),
        delivery: DeliveryCaps {
            source: Delivery::AtLeastOnce.into(),
            sink: Delivery::AtLeastOnce.into(),
            ..Default::default()
        }
        .into(),
        ordering: Ordering::PerKey.into(),
        formats: vec![Format::CloudeventsBinary.into()],
        schema: None.into(),
        bulk: None.into(),
        backpressure: Backpressure::Pull.into(),
        ..Default::default()
    };
    ConnectorSpec {
        id: "kafka".to_string(),
        name: "Apache Kafka".to_string(),
        spec_version: "1.2.0".to_string(),
        category: Category::Messaging.into(),
        priority: Priority::P1.into(),
        starred: true,
        status: Status::Stable.into(),
        runtime: RuntimeRef {
            kind: RuntimeKind::Native.into(),
            r#ref: "loams_flow::connectors::kafka".to_string(),
            version: "rdkafka 0.39 / librdkafka 2.x".to_string(),
            ..Default::default()
        }
        .into(),
        licence: Licence {
            component: "Apache-2.0".to_string(),
            dependencies: [("librdkafka".to_string(), "BSD-2-Clause".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
        .into(),
        capabilities: capabilities.into(),
        auth: vec![AuthMethod::SaslScram256.into(), AuthMethod::Mtls.into()],
        config_ref: "connectors/schemas/kafka.config.json".to_string(),
        secrets: vec!["sasl.password".to_string()],
        envelope: EnvelopeDecl {
            emits: "io.loams.dev.flow.kafka.record.v1".to_string(),
            consumes: String::new(),
            passthrough: false,
            ..Default::default()
        }
        .into(),
        limits: Limits {
            max_record_bytes: 1_000_000,
            batch_rows: 65_536,
            batch_bytes: 8 << 20,
            ..Default::default()
        }
        .into(),
        conformance: vec![Suite::Contract.into(), Suite::KillRestart.into()],
        docs: "connectors/kafka.md".to_string(),
        ..Default::default()
    }
}

/// The JSON keys of a serialized message, for the one-to-one comparison with
/// §33 §4's YAML keys.
fn json_keys<T: serde::Serialize>(message: &T) -> BTreeSet<String> {
    let json =
        serde_json::to_value(message).unwrap_or_else(|e| panic!("serializing to JSON failed: {e}"));
    let object = json
        .as_object()
        .unwrap_or_else(|| panic!("a message must serialize as a JSON object, got {json}"));
    object.keys().cloned().collect()
}

/// The three ways one enum value is named: the CamelCase alias constant the
/// generator puts in an `impl` block, the UPPER_SNAKE_CASE variant itself, and
/// the proto name the wire and the proto3 JSON mapping both carry.
fn enum_value<E: Enumeration>(alias: E, variant: E, proto_name: &str, number: i32) {
    assert_eq!(alias, variant, "{proto_name}");
    assert_eq!(variant.proto_name(), proto_name);
    assert_eq!(variant.to_i32(), number, "{proto_name}");
}

#[test]
fn connector_spec_has_every_manifest_field() {
    let spec = spec();
    assert_eq!(spec.id, "kafka");
    assert_eq!(spec.name, "Apache Kafka");
    assert_eq!(spec.spec_version, "1.2.0");
    assert_eq!(spec.category, EnumValue::Known(Category::Messaging));
    assert_eq!(spec.priority, EnumValue::Known(Priority::P1));
    assert!(spec.starred);
    assert_eq!(spec.status, EnumValue::Known(Status::Stable));
    assert_eq!(spec.runtime.kind, EnumValue::Known(RuntimeKind::Native));
    assert_eq!(spec.licence.component, "Apache-2.0");
    assert_eq!(spec.licence.dependencies["librdkafka"], "BSD-2-Clause");
    assert_eq!(
        spec.capabilities.ordering,
        EnumValue::Known(Ordering::PerKey)
    );
    assert_eq!(
        spec.capabilities.delivery.sink,
        EnumValue::Known(Delivery::AtLeastOnce)
    );
    let source: Option<SourceCaps> = spec.capabilities.source.clone().into();
    assert_eq!(
        source.map(|s| s.position),
        Some("kafka-offsets".to_string())
    );
    assert_eq!(
        spec.auth,
        [
            EnumValue::Known(AuthMethod::SaslScram256),
            EnumValue::Known(AuthMethod::Mtls),
        ],
    );
    assert_eq!(spec.config_ref, "connectors/schemas/kafka.config.json");
    assert_eq!(spec.secrets, ["sasl.password".to_string()]);
    assert_eq!(spec.envelope.emits, "io.loams.dev.flow.kafka.record.v1");
    assert_eq!(spec.limits.batch_rows, 65_536);
    assert_eq!(
        spec.conformance,
        [
            EnumValue::Known(Suite::Contract),
            EnumValue::Known(Suite::KillRestart)
        ],
    );
    assert_eq!(spec.docs, "connectors/kafka.md");
}

#[test]
fn connector_spec_json_keys_are_the_yaml_manifest_keys() {
    // §33 §4's YAML keys, one per field, no more and no fewer: the generator
    // adds `__buffa_unknown_fields`, but that field is `#[serde(skip)]`, so it
    // cannot widen this set.
    assert_eq!(
        json_keys(&spec()),
        BTreeSet::from(
            [
                "auth",
                "capabilities",
                "category",
                "conformance",
                "configRef",
                "docs",
                "envelope",
                "id",
                "licence",
                "limits",
                "name",
                "priority",
                "runtime",
                "secrets",
                "specVersion",
                "starred",
                "status",
            ]
            .map(String::from),
        ),
    );
}

#[test]
fn runtime_ref_has_kind_ref_and_version() {
    let runtime = RuntimeRef {
        kind: RuntimeKind::Native.into(),
        r#ref: "loams_flow::connectors::kafka".to_string(),
        version: "rdkafka 0.39 / librdkafka 2.x".to_string(),
        ..Default::default()
    };
    assert_eq!(runtime.kind, EnumValue::Known(RuntimeKind::Native));
    // `ref` is a Rust keyword, so the field is a raw identifier — the name
    // `connector.proto` gives it, with `r#` in front.
    assert_eq!(runtime.r#ref, "loams_flow::connectors::kafka");
    assert_eq!(runtime.version, "rdkafka 0.39 / librdkafka 2.x");
    assert_eq!(
        json_keys(&runtime),
        BTreeSet::from(["kind", "ref", "version"].map(String::from)),
    );

    // `RuntimeRef` is a message, not the plan's five-variant enum: §33 §4's
    // comment says so, and one shape on the wire keeps `ref` a plain string.
    assert_eq!(
        serde_json::to_value(&runtime).unwrap_or_else(|e| panic!("to JSON: {e}")),
        json!({
            "kind": "RUNTIME_KIND_NATIVE",
            "ref": "loams_flow::connectors::kafka",
            "version": "rdkafka 0.39 / librdkafka 2.x",
        }),
    );
}

#[test]
fn enum_values_carry_their_prefixed_proto_names() {
    // One value per enum of §33 §4, with the number its YAML declaration order
    // gives it (`connector.proto`'s header comment).
    enum_value(
        Category::Messaging,
        Category::CATEGORY_MESSAGING,
        "CATEGORY_MESSAGING",
        1,
    );
    enum_value(Priority::P1, Priority::PRIORITY_P1, "PRIORITY_P1", 1);
    enum_value(Status::Planned, Status::STATUS_PLANNED, "STATUS_PLANNED", 1);
    enum_value(
        RuntimeKind::Native,
        RuntimeKind::RUNTIME_KIND_NATIVE,
        "RUNTIME_KIND_NATIVE",
        1,
    );
    enum_value(Suite::Contract, Suite::SUITE_CONTRACT, "SUITE_CONTRACT", 1);
    enum_value(
        AuthMethod::Mtls,
        AuthMethod::AUTH_METHOD_MTLS,
        "AUTH_METHOD_MTLS",
        7,
    );
    enum_value(
        Ordering::PerKey,
        Ordering::ORDERING_PER_KEY,
        "ORDERING_PER_KEY",
        2,
    );
    enum_value(
        Delivery::AtLeastOnce,
        Delivery::DELIVERY_AT_LEAST_ONCE,
        "DELIVERY_AT_LEAST_ONCE",
        2,
    );

    // Every value of these enums carries its enum's name in full, as
    // `connector.proto`'s header comment says, with an `_UNSPECIFIED` zero.
    enum_value(
        Category::Unspecified,
        Category::CATEGORY_UNSPECIFIED,
        "CATEGORY_UNSPECIFIED",
        0,
    );
    enum_value(Priority::P3, Priority::PRIORITY_P3, "PRIORITY_P3", 3);
    enum_value(
        Status::Deprecated,
        Status::STATUS_DEPRECATED,
        "STATUS_DEPRECATED",
        4,
    );
    enum_value(
        RuntimeKind::Openapi,
        RuntimeKind::RUNTIME_KIND_OPENAPI,
        "RUNTIME_KIND_OPENAPI",
        5,
    );
    enum_value(Suite::Cdc, Suite::SUITE_CDC, "SUITE_CDC", 6);
    enum_value(
        AuthMethod::None,
        AuthMethod::AUTH_METHOD_NONE,
        "AUTH_METHOD_NONE",
        1,
    );
    enum_value(
        Ordering::Total,
        Ordering::ORDERING_TOTAL,
        "ORDERING_TOTAL",
        4,
    );
    enum_value(
        Delivery::ExactlyOnce,
        Delivery::DELIVERY_EXACTLY_ONCE,
        "DELIVERY_EXACTLY_ONCE",
        3,
    );
}

#[test]
fn instance_has_every_field_and_binds_to_the_fabric() {
    let mut config = Struct::new();
    config.insert("bootstrap.servers", "localhost:9092");
    let instance = Instance {
        namespace: 7,
        name: "kafka-main".to_string(),
        connector: "kafka".to_string(),
        connector_major: 1,
        config: Some(config).into(),
        secrets: [("sasl.password".to_string(), Default::default())]
            .into_iter()
            .collect(),
        direction: Direction::Source.into(),
        fabric: FabricBinding {
            target: Some(Target::Topic("warehouse.public.orders".to_string())),
            partitions: 8,
            ..Default::default()
        }
        .into(),
        enabled: true,
        version: 3,
        ..Default::default()
    };
    assert_eq!(instance.namespace, 7);
    assert_eq!(instance.name, "kafka-main");
    assert_eq!(instance.connector, "kafka");
    assert_eq!(instance.connector_major, 1);
    assert_eq!(instance.direction, EnumValue::Known(Direction::Source));
    assert!(instance.enabled);
    assert_eq!(instance.version, 3);
    assert_eq!(
        json_keys(&instance),
        BTreeSet::from(
            [
                "config",
                "connector",
                "connectorMajor",
                "direction",
                "enabled",
                "fabric",
                "name",
                "namespace",
                "secrets",
                "version",
            ]
            .map(String::from),
        ),
    );
    // The oneof flattens into the binding, with `partitions` as its sibling.
    assert_eq!(
        serde_json::to_value(&instance.fabric).unwrap_or_else(|e| panic!("to JSON: {e}")),
        json!({ "partitions": 8, "topic": "warehouse.public.orders" }),
    );
}

#[test]
fn connector_spec_survives_the_binary_encoding() {
    // The encoding the `FlowService` API and the system table store use: the
    // generated code has to be wired to a codec, not just present.
    let spec = spec();
    let back = ConnectorSpec::decode_from_slice(&spec.encode_to_vec())
        .unwrap_or_else(|e| panic!("decoding the encoded manifest failed: {e}"));
    assert_eq!(back, spec);
}
