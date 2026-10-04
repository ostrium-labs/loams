//! CN1 plan Task 1: the manifest schema, CN1's six semantic rules and §33 §4 rule 1's
//! refusal.
//!
//! `schema_validates_kafka_example` uses design §33 §4's Kafka YAML **verbatim**,
//! embedded with `include_str!` from `tests/fixtures/kafka-section4.yaml`, which is
//! a byte-for-byte copy of the fenced block in `docs/design/33-connectors.md` lines
//! 59–106. It is a fixture rather than an inline literal so the copy can be diffed
//! against the design document, and so the file keeps the inline comments that make
//! the example readable.
//!
//! The rest build their manifests on the same YAML and change exactly one field per
//! case, so a failure names the rule that broke rather than a large diff.

// The rule this crate is held to is "no `unwrap()` outside tests"; clippy's
// `unwrap_used` is a warning and CI runs `-D warnings`, so a test file says so once
// and may then panic freely: a failing assertion is the point of a test.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use loams_flow::manifest::{ConnectorSpec, ManifestError, load_manifest, load_manifest_value, pb};
use loams_flow::validate::{
    Direction, ManifestSchema, Mode, SinkRequirement, Uses, check_use, validate_manifest,
    validate_manifest_with_config_schema,
};
use serde_json::Value;

use loams_flow::manifest::{Category, Delivery, Ordering, Priority};

/// Design §33 §4's canonical Kafka manifest, verbatim.
const KAFKA_SECTION_4: &str = include_str!("fixtures/kafka-section4.yaml");

/// This repository's root, from the crate's own manifest directory: the tests read
/// `connectors/` from the repository, never from the current directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `connectors/`, the root every `config.$ref` is relative to.
fn connectors_root() -> PathBuf {
    repo_root().join("connectors")
}

/// §33 §4's example as a JSON value, ready to be checked and then broken in one
/// place at a time.
fn kafka_example() -> Value {
    serde_norway::from_str(KAFKA_SECTION_4)
        .unwrap_or_else(|error| panic!("the fixture is not valid YAML: {error}"))
}

/// §33 §4's example validated and parsed: the JSON Schema first, then CN1's rules
/// with the real `connectors/schemas/kafka.config.json` as the resolved
/// instance-config schema.
#[test]
fn schema_validates_kafka_example() {
    let schema = ManifestSchema::embedded().expect("the repository's manifest schema compiles");
    let value = kafka_example();
    let spec = validate_manifest_with_config_schema(
        &value,
        schema.validator(),
        Some(&kafka_config_schema()),
    )
    .unwrap_or_else(|errors| {
        panic!(
            "§33 §4's Kafka example must validate and parse; it did not:\n{}",
            loams_flow::validate::render_errors(&errors)
        )
    });

    // Every value §33 §4's example fixes is read back as that value, and no
    // capability is inferred: the manifest declares, the loader reads.
    assert_eq!(spec.id, "kafka");
    assert_eq!(spec.name, "Apache Kafka");
    assert_eq!(spec.spec_version.to_string(), "1.0.0");
    assert_eq!(spec.category, Category::Messaging);
    assert_eq!(spec.priority, Priority::P1);
    assert!(spec.starred);
    assert_eq!(spec.runtime.kind.slug(), "native");
    assert_eq!(spec.runtime.reference, "loams_flow::connectors::kafka");
    assert_eq!(
        spec.runtime.version.as_deref(),
        Some("rdkafka 0.39 / librdkafka 2.x (verify)")
    );
    assert_eq!(spec.licence.component, "Apache-2.0");
    assert_eq!(spec.licence.dependencies["librdkafka"], "BSD-2-Clause");

    let source = spec
        .capabilities
        .source
        .as_ref()
        .expect("§33 §4 declares a source");
    assert!(source.streaming && !source.batch && !source.cdc && !source.webhook);
    assert!(source.resumable);
    assert_eq!(source.position, "kafka-offsets");
    let sink = spec
        .capabilities
        .sink
        .as_ref()
        .expect("§33 §4 declares a sink");
    assert!(sink.streaming && sink.batch && sink.idempotent);
    assert!(!sink.transactional && !sink.upsert && !sink.delete);
    assert_eq!(spec.capabilities.delivery.source, Delivery::AtLeastOnce);
    assert_eq!(spec.capabilities.delivery.sink, Delivery::AtLeastOnce);
    assert_eq!(spec.capabilities.ordering, Ordering::PerPartition);
    assert_eq!(
        spec.capabilities
            .formats
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "cloudevents-binary",
            "cloudevents-structured",
            "json",
            "avro",
            "protobuf",
            "bytes"
        ]
    );
    assert!(!spec.capabilities.bulk.arrow);
    assert_eq!(spec.capabilities.bulk.max_batch_rows, 65536);
    assert_eq!(spec.capabilities.backpressure.slug(), "pull");
    assert_eq!(
        spec.auth
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "none",
            "sasl-plain",
            "sasl-scram-256",
            "sasl-scram-512",
            "mtls",
            "aws-msk-iam"
        ]
    );
    assert_eq!(spec.config_ref.reference, "schemas/kafka.config.json");
    assert_eq!(spec.secrets, ["sasl.password", "tls.key_pem"]);
    assert_eq!(spec.envelope.emits, "io.loams.dev.flow.kafka.record.v1");
    assert_eq!(spec.envelope.consumes, "*");
    assert!(
        !spec.envelope.passthrough,
        "§33 §4's example has no passthrough"
    );
    assert_eq!(spec.limits.max_record_bytes, Some(16777216));
    assert_eq!(spec.limits.batch_rows, None);
    assert_eq!(
        spec.conformance
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["contract", "roundtrip", "kill-restart", "dup-check"]
    );
    assert_eq!(
        spec.docs.as_deref(),
        Some("docs/guides/connectors/kafka.md")
    );

    // The two `const` keys the example carries and `connector.proto` has no field
    // for, checked by the schema and by the loader alike.
    assert_eq!(value["apiVersion"], "loams.flow/v1");
    assert_eq!(value["kind"], "Connector");

    // The manifest round-trips: what `to_yaml` writes is a manifest the loader takes
    // back to the same spec (D352, "the YAML is the source").
    let yaml = spec.to_yaml().expect("a manifest renders as YAML");
    let reparsed: Value = serde_norway::from_str(&yaml)
        .unwrap_or_else(|error| panic!("the rendered YAML does not parse: {error}"));
    let again = validate_manifest(&reparsed, schema.validator())
        .unwrap_or_else(|errors| panic!("the rendered manifest does not validate: {errors:?}"));
    assert_eq!(again, spec, "to_yaml must render what the loader accepts");
}

/// `connectors/schemas/kafka.config.json`, the schema §33 §4's `config.$ref` names.
fn kafka_config_schema() -> Value {
    let path = connectors_root().join("schemas/kafka.config.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("kafka's config schema is read"))
        .expect("kafka's config schema is JSON")
}

/// The manifest with one field changed, checked against CN1's rules with
/// `config_schema` as the instance-config schema the rule about secrets reads.
fn rules_for(mutate: impl FnOnce(&mut Value)) -> Vec<ManifestError> {
    rules_with_config(mutate, kafka_config_schema())
}

/// [`rules_for`] with an instance-config schema of the case's own, for the two rules
/// that read that document rather than the manifest.
fn rules_with_config(mutate: impl FnOnce(&mut Value), config: Value) -> Vec<ManifestError> {
    let schema = ManifestSchema::embedded().expect("the repository's manifest schema compiles");
    let mut value = kafka_example();
    mutate(&mut value);
    match validate_manifest_with_config_schema(&value, schema.validator(), Some(&config)) {
        Ok(_) => Vec::new(),
        Err(errors) => errors,
    }
}

/// An instance-config schema with a `key` property, the shape §33 §4's rule 1 asks a
/// sink that declares `upsert` to have, and with the two `writeOnly` properties the
/// example's `secrets` name.
fn config_schema_with_a_key() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string" },
            "sasl": {
                "type": "object",
                "properties": { "password": { "type": "string", "writeOnly": true } },
            },
            "tls": {
                "type": "object",
                "properties": { "key_pem": { "type": "string", "writeOnly": true } },
            },
        },
    })
}

/// One case per semantic rule, each named so a failure says which rule broke.
#[test]
fn semantic_rules_each_refuse() {
    // The rules below are the six CN1 plan Task 1 names. Each case breaks exactly one
    // of them, and each asserts the refusal names the field that broke — so a rule
    // that stopped firing, or fired on the wrong field, fails here.

    // Rule 1: a sink with `upsert: true` declares a key in its config schema. The
    // example's `sasl` object has no `key`, so a config schema without one is the
    // case; the resolved schema is replaced because the rule reads *that* document.
    // The example's `sasl` object has no `key`, so an instance-config schema without
    // one is the case.
    let errors = rules_with_config(
        |value| value["capabilities"]["sink"]["upsert"] = Value::Bool(true),
        serde_json::json!({
            "type": "object",
            "properties": { "brokers": { "type": "string" } },
        }),
    );
    assert!(
        errors
            .iter()
            .any(|error| error.path == "capabilities.sink.upsert"),
        "rule 1 (upsert declares a key) must refuse, got {errors:?}"
    );
    // The same manifest against a config schema that does declare `key` passes rule 1,
    // so the refusal above is the rule and not a side effect of the mutation.
    let errors = rules_with_config(
        |value| value["capabilities"]["sink"]["upsert"] = Value::Bool(true),
        config_schema_with_a_key(),
    );
    assert!(
        errors
            .iter()
            .all(|error| error.path != "capabilities.sink.upsert"),
        "an upsert sink whose config schema declares `key` must pass rule 1, got {errors:?}"
    );

    // Rule 2: `cdc: true` implies `source.streaming` and a non-empty position. Both
    // halves are broken at once, so both must be named.
    let errors = rules_for(|value| {
        value["capabilities"]["source"]["cdc"] = Value::Bool(true);
        value["capabilities"]["source"]["streaming"] = Value::Bool(false);
        value["capabilities"]["source"]["position"] = Value::String(String::new());
    });
    assert!(
        errors
            .iter()
            .any(|e| e.path == "capabilities.source.streaming")
            && errors
                .iter()
                .any(|e| e.path == "capabilities.source.position"),
        "rule 2 (cdc implies streaming and a position) must refuse both halves, got {errors:?}"
    );

    // Rule 3: `delivery.sink = exactly_once` requires `sink.transactional` or
    // `sink.idempotent`. The example's sink is idempotent, so it is turned off first.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("exactly_once".to_string());
        value["capabilities"]["sink"]["idempotent"] = Value::Bool(false);
        value["capabilities"]["sink"]["transactional"] = Value::Bool(false);
    });
    assert!(
        errors
            .iter()
            .any(|e| e.path == "capabilities.delivery.sink"),
        "rule 3 (exactly_once needs transactional or idempotent) must refuse, got {errors:?}"
    );

    // Rule 4: `starred` implies `priority = P1`.
    let errors = rules_for(|value| value["priority"] = Value::String("P2".to_string()));
    assert!(
        errors.iter().any(|e| e.path == "priority"),
        "rule 4 (starred implies P1) must refuse, got {errors:?}"
    );

    // Rule 5: every auth method is one of Appendix A's legend's slugs. The JSON Schema
    // enforces it, and the rule asserts the enforcement by probing the schema, so the
    // refusal here comes from the schema — reported as a violation under `auth`.
    let errors = rules_for(|value| {
        value["auth"] = Value::Array(vec![Value::String("not-a-method".to_string())]);
    });
    assert!(
        errors.iter().any(|e| e.path.starts_with("auth")),
        "rule 5 (every auth method is in the legend's enum) must refuse, got {errors:?}"
    );

    // Rule 6: `secrets` and the config schema's `writeOnly` properties are exactly
    // equal, in both directions.
    let errors = {
        let schema = ManifestSchema::embedded().expect("the repository's manifest schema compiles");
        let mut value = kafka_example();
        // A secret that names nothing.
        value["secrets"] = Value::Array(vec![
            Value::String("sasl.password".to_string()),
            Value::String("tls.ca_pem".to_string()),
        ]);
        match validate_manifest_with_config_schema(
            &value,
            schema.validator(),
            Some(&kafka_config_schema()),
        ) {
            Ok(_) => Vec::new(),
            Err(errors) => errors,
        }
    };
    assert!(
        errors
            .iter()
            .any(|e| e.path == "secrets.tls.ca_pem" && e.message.contains("writeOnly")),
        "rule 6 (a secret name resolves to a writeOnly property) must refuse, got {errors:?}"
    );
    // The other direction: a `writeOnly` property the manifest does not name is a
    // secret field no secret-store entry resolves.
    let errors = {
        let schema = ManifestSchema::embedded().expect("the repository's manifest schema compiles");
        let mut value = kafka_example();
        value["secrets"] = Value::Array(vec![Value::String("sasl.password".to_string())]);
        match validate_manifest_with_config_schema(
            &value,
            schema.validator(),
            Some(&kafka_config_schema()),
        ) {
            Ok(_) => Vec::new(),
            Err(errors) => errors,
        }
    };
    assert!(
        errors
            .iter()
            .any(|e| e.path == "secrets.tls.key_pem" && e.message.contains("writeOnly")),
        "rule 6 (every writeOnly property is named in secrets) must refuse, got {errors:?}"
    );
}

/// §33 §4 rule 1: a route that uses an undeclared capability is refused, and the
/// refusal names the connector, the capability and the manifest version.
#[test]
fn check_use_refuses_undeclared() {
    // `postgresql` declares a polling (non-streaming) source and no CDC: the routing
    // answer for a CDC route is the `debezium-postgres` manifest (CN1 Ruling 7).
    let postgresql = load(connectors_root().join("registry/postgresql.yaml"));
    assert_eq!(
        check_use(&postgresql, &uses_cdc()).unwrap_err().capability,
        "cdc"
    );

    // `clickhouse` is an append-only sink: a route that upserts needs a manifest that
    // declares `upsert`, which is `postgresql`, `mysql` or `elasticsearch`.
    let clickhouse = load(connectors_root().join("registry/clickhouse.yaml"));
    assert_eq!(
        check_use(&clickhouse, &uses_upsert())
            .unwrap_err()
            .capability,
        "upsert"
    );

    // `clickhouse` delivers at-least-once into a sink that neither dedupes nor
    // transactions, so `exactly_once` is refused rather than claimed (§33 §2.2).
    assert_eq!(
        check_use(&clickhouse, &uses_exactly_once())
            .unwrap_err()
            .capability,
        "delivery.sink"
    );

    // The refusals read like §33 §4 rule 1's sentence, naming all three things it says
    // the error must name.
    let error = check_use(&clickhouse, &uses_exactly_once()).unwrap_err();
    assert_eq!(error.connector, "clickhouse");
    assert_eq!(error.spec_version, "1.0.0");
    assert_eq!(
        error.to_string(),
        "clickhouse (manifest specVersion 1.0.0) does not declare the capability delivery.sink"
    );

    // And the same uses against a manifest that declares them all pass, so the three
    // refusals above are the rule and not a manifest that refuses everything.
    let postgresql = load(connectors_root().join("registry/postgresql.yaml"));
    assert!(
        check_use(
            &postgresql,
            &Uses {
                direction: Direction::Sink,
                sink_requirements: vec![SinkRequirement::Upsert],
                delivery: Some(Delivery::AtLeastOnce),
                arrow_bulk: true,
                ..Uses::default()
            }
        )
        .is_ok()
    );
    let debezium = load(connectors_root().join("registry/debezium-postgres.yaml"));
    assert!(
        check_use(&debezium, &uses_cdc()).is_ok(),
        "debezium-postgres declares cdc"
    );
    // Kafka's sink is idempotent but at-least-once, so `exactly_once` stays refused
    // even though the sink dedupes: the manifest is what the route is checked against.
    let kafka = load(connectors_root().join("registry/kafka.yaml"));
    assert!(check_use(&kafka, &uses_exactly_once()).is_err());
}

fn uses_cdc() -> Uses {
    Uses {
        direction: Direction::Source,
        mode: Some(Mode::Cdc),
        ..Uses::default()
    }
}

fn uses_upsert() -> Uses {
    Uses {
        direction: Direction::Sink,
        sink_requirements: vec![SinkRequirement::Upsert],
        ..Uses::default()
    }
}

fn uses_exactly_once() -> Uses {
    Uses {
        direction: Direction::Sink,
        delivery: Some(Delivery::ExactlyOnce),
        ..Uses::default()
    }
}

/// A manifest of the repository's own registry, loaded through the public entry point
/// so the tests exercise the same path `Registry::load` uses.
fn load(path: PathBuf) -> ConnectorSpec {
    load_manifest(&path, &connectors_root()).unwrap_or_else(|errors| {
        panic!(
            "{} must load; it did not:\n{}",
            path.display(),
            loams_flow::validate::render_errors(&errors)
        )
    })
}

/// The protobuf form of §33 §4's example, checked to carry the proto names the YAML
/// slugs translate to. `proto_and_yaml_agree` in `tests/registry.rs` does this over
/// all 21 ★ manifests; this is the single-manifest anchor for the enum mapping.
#[test]
fn the_proto_form_uses_proto_names_not_yaml_slugs() {
    let spec = load_manifest_value(
        "kafka.yaml",
        &kafka_example(),
        &connectors_root(),
        &ManifestSchema::embedded().expect("the manifest schema compiles"),
    )
    .expect("§33 §4's example loads");
    let proto: pb::ConnectorSpec = spec.to_proto().expect("the protobuf form is buildable");
    let json = serde_json::to_value(&proto).expect("the protobuf form renders as JSON");

    // §33 §4's `per_partition` is `ORDERING_PER_PARTITION` on the wire, and
    // `sasl-scram-256` is `AUTH_METHOD_SASL_SCRAM_256`: the proto3 JSON mapping uses
    // the proto name, which is why `proto_and_yaml_agree` translates in both
    // directions rather than comparing strings.
    assert_eq!(json["capabilities"]["ordering"], "ORDERING_PER_PARTITION");
    assert_eq!(json["priority"], "PRIORITY_P1");
    assert_eq!(json["runtime"]["kind"], "RUNTIME_KIND_NATIVE");
    assert_eq!(
        json["auth"],
        serde_json::json!([
            "AUTH_METHOD_NONE",
            "AUTH_METHOD_SASL_PLAIN",
            "AUTH_METHOD_SASL_SCRAM_256",
            "AUTH_METHOD_SASL_SCRAM_512",
            "AUTH_METHOD_MTLS",
            "AUTH_METHOD_AWS_MSK_IAM"
        ])
    );
    assert_eq!(json["configRef"], "schemas/kafka.config.json");
    assert_eq!(json["specVersion"], "1.0.0");
    assert_eq!(json["docs"], "docs/guides/connectors/kafka.md");
}
