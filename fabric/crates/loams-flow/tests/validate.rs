//! CN1 plan Task 1: the manifest schema, CN1's six semantic rules and §33 §4 rule 1's
//! refusal.
//!
//! `schema_validates_kafka_example` uses design §33 §4's Kafka YAML **verbatim**,
//! embedded with `include_str!` from `tests/fixtures/kafka-section4.yaml`, which is
//! a byte-for-byte copy of the fenced block in `docs/design/33-connectors.md` lines
//! 59–106. It is a fixture rather than an inline literal so the copy can be diffed
//! against the design document, and so the file keeps the inline comments that make
//! the example readable. [`kafka_example`] sets one field, `secrets`, to what the
//! real config schema's `writeOnly` fields are now (CC1 Task 2).
//!
//! The rest build their manifests on the same YAML and change exactly one field per
//! case, so a failure names the rule that broke rather than a large diff.

// The rule this crate is held to is "no `unwrap()` outside tests"; clippy's
// `unwrap_used` is a warning and CI runs `-D warnings`, so a test file says so once
// and may then panic freely: a failing assertion is the point of a test.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use loams_flow::manifest::{
    Choice, ConnectorSpec, ManifestError, RuntimeKind, RuntimeSpec, load_manifest,
    load_manifest_value, pb,
};
use loams_flow::validate::{
    Direction, ManifestSchema, Mode, NATIVE_FIRST_ENFORCED, SinkRequirement, Uses, check_use,
    semantic_warnings, validate_manifest, validate_manifest_with_config_schema,
};
use serde_json::Value;

use loams_flow::manifest::{Category, Delivery, Ordering, Priority};

/// Design §33 §4's canonical Kafka manifest, verbatim.
const KAFKA_SECTION_4: &str = include_str!("fixtures/kafka-section4.yaml");

/// The secret fields of the real `connectors/schemas/kafka.config.json` (its
/// `writeOnly` properties), as `connectors/registry/kafka.yaml` lists them.
const KAFKA_SECRETS: [&str; 5] = [
    "sasl.password",
    "sasl.username_secret",
    "tls.key_pem",
    "schema_registry.basic_auth.password",
    "schema_registry.basic_auth.username_secret",
];

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
///
/// One field differs from §4: `secrets`. §4 names two; CC1 Task 2 (D1822) added
/// three secret-name fields to the real kafka.config.json (the username and Schema
/// Registry credentials), and rule 6 wants every `writeOnly` field named, so the
/// example names them too ([`KAFKA_SECRETS`]).
fn kafka_example() -> Value {
    let mut value: Value = serde_norway::from_str(KAFKA_SECTION_4)
        .unwrap_or_else(|error| panic!("the fixture is not valid YAML: {error}"));
    value["secrets"] = serde_json::json!(KAFKA_SECRETS);
    value
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
    assert_eq!(spec.secrets, KAFKA_SECRETS);
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
    // `snowflake` declares a batch source and no CDC, so a CDC route is refused.
    // (`postgresql` was the example until CN2 Task 2 gave it native CDC, OR-9.)
    let snowflake = load(connectors_root().join("registry/snowflake.yaml"));
    assert_eq!(
        check_use(&snowflake, &uses_cdc()).unwrap_err().capability,
        "cdc"
    );
    let postgresql = load(connectors_root().join("registry/postgresql.yaml"));
    assert!(
        check_use(&postgresql, &uses_cdc()).is_ok(),
        "CN2 Task 2: postgresql declares native CDC (OR-9)"
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

    // `clickhouse`'s sink dedupes by insert token but is not transactional, so it is
    // effectively once (CN2 R17.3) and `exactly_once` is refused rather than claimed
    // (§33 §2.2).
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
    assert_eq!(error.spec_version, "1.2.0");
    assert_eq!(
        error.to_string(),
        "clickhouse (manifest specVersion 1.2.0) does not declare the capability delivery.sink"
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
    // Kafka's sink is exactly-once through its transactional mode (CN2 Task 9). The
    // same manifest with only the idempotent producer's `effectively_once` refuses
    // `exactly_once` even though the sink dedupes: the manifest is what the route is
    // checked against.
    let kafka = load(connectors_root().join("registry/kafka.yaml"));
    assert!(check_use(&kafka, &uses_exactly_once()).is_ok());
    let mut idempotent_only = kafka.clone();
    idempotent_only.capabilities.delivery.sink = Delivery::EffectivelyOnce;
    assert!(check_use(&idempotent_only, &uses_exactly_once()).is_err());
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

// -------------------------------------------------------------------------------------
// CN2 plan Task 1: per-direction runtimes, fallbacks, profiles, delivery and suites
// (rules R-CN2-1 to R-CN2-8).
// -------------------------------------------------------------------------------------

/// Whether any error sits at exactly `path`.
fn has_path(errors: &[ManifestError], path: &str) -> bool {
    errors.iter().any(|error| error.path == path)
}

/// R-CN2-1: a direction override names a direction the manifest declares. A sink
/// override on a source-only manifest is refused; a source override on the declared
/// source is not.
#[test]
fn override_direction_must_be_declared() {
    let errors = rules_for(|value| {
        value["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove("sink");
        value["runtime"]["sink"] = serde_json::json!({
            "kind": "native",
            "ref": "loams_connector_kafka::sink::KafkaSink",
        });
    });
    assert!(
        has_path(&errors, "runtime.sink"),
        "R-CN2-1: a sink override on a manifest with no sink must be refused, got {errors:?}"
    );

    let errors = rules_for(|value| {
        value["runtime"]["source"] = serde_json::json!({
            "kind": "native",
            "ref": "loams_connector_kafka::source::KafkaSource",
        });
    });
    assert!(
        errors.is_empty(),
        "R-CN2-1: a source override on the declared source is legal, got {errors:?}"
    );
}

/// R-CN2-2: a profile is a native-runtime notion, so a manifest whose declared
/// directions do not all run on `native` by default cannot name one, and a profile
/// that names no file under `connectors/profiles/` is refused at load.
#[test]
fn profile_requires_native_direction() {
    // The whole manifest on Camel.
    let errors = rules_for(|value| {
        value["runtime"]["kind"] = Value::String("camel".to_string());
        value["runtime"]["profile"] = Value::String("redpanda".to_string());
    });
    assert!(
        has_path(&errors, "runtime.profile"),
        "R-CN2-2: a profile on a camel runtime must be refused, got {errors:?}"
    );

    // Native by default, but the sink direction overridden to Camel.
    let errors = rules_for(|value| {
        value["runtime"]["profile"] = Value::String("redpanda".to_string());
        value["runtime"]["sink"] = serde_json::json!({ "kind": "camel", "ref": "kafka" });
    });
    assert!(
        has_path(&errors, "runtime.profile"),
        "R-CN2-2: a profile with a non-native direction must be refused, got {errors:?}"
    );

    // Native in both directions passes the rule itself...
    let errors = rules_for(|value| {
        value["runtime"]["profile"] = Value::String("redpanda".to_string());
    });
    assert!(
        !has_path(&errors, "runtime.profile"),
        "R-CN2-2: a native manifest may name a profile, got {errors:?}"
    );

    // ...and the loader, which knows the `connectors/` root, refuses a profile that
    // is not a file under `connectors/profiles/`.
    let mut value = kafka_example();
    value["runtime"]["profile"] = Value::String("no-such-profile".to_string());
    let errors = load_manifest_value(
        "kafka.yaml",
        &value,
        &connectors_root(),
        &ManifestSchema::embedded().expect("the manifest schema compiles"),
    )
    .err()
    .unwrap_or_default();
    assert!(
        has_path(&errors, "runtime.profile"),
        "R-CN2-2: a profile with no file under connectors/profiles/ must be refused, got {errors:?}"
    );

    // A profile id is a registry-style id, never a path.
    let errors = rules_for(|value| {
        value["runtime"]["profile"] = Value::String("../registry/kafka".to_string());
    });
    assert!(
        errors.iter().any(|error| error.path == "runtime.profile"),
        "a profile id that is a path must be refused by the schema, got {errors:?}"
    );
}

/// R-CN2-3: a sink that replies (OR-19) must emit a type and let the instance name
/// the reply topic.
#[test]
fn reply_requires_reply_topic() {
    let with_reply_topic = {
        let mut config = kafka_config_schema();
        config["properties"]["reply_topic"] = serde_json::json!({ "type": "string" });
        config
    };

    // The config schema has no `reply_topic`.
    let errors = rules_for(|value| value["capabilities"]["sink"]["reply"] = Value::Bool(true));
    assert!(
        has_path(&errors, "capabilities.sink.reply"),
        "R-CN2-3: a replying sink without reply_topic must be refused, got {errors:?}"
    );

    // `reply_topic` is there, but the manifest emits nothing.
    let errors = rules_with_config(
        |value| {
            value["capabilities"]["sink"]["reply"] = Value::Bool(true);
            value["envelope"]["emits"] = Value::String(String::new());
        },
        with_reply_topic.clone(),
    );
    assert!(
        has_path(&errors, "capabilities.sink.reply"),
        "R-CN2-3: a replying sink that emits no type must be refused, got {errors:?}"
    );

    // Both present: legal.
    let errors = rules_with_config(
        |value| value["capabilities"]["sink"]["reply"] = Value::Bool(true),
        with_reply_topic,
    );
    assert!(
        errors.is_empty(),
        "R-CN2-3: a replying sink with an emitted type and reply_topic is legal, got {errors:?}"
    );

    // `reply` defaults to false when the manifest omits it.
    let schema = ManifestSchema::embedded().expect("the manifest schema compiles");
    let spec = validate_manifest(&kafka_example(), schema.validator()).unwrap();
    assert!(!spec.capabilities.sink.unwrap().reply);
}

/// R-CN2-5: `preview` and `stable` need the contract suite (OR-23).
#[test]
fn preview_requires_contract_suite() {
    for status in ["preview", "stable"] {
        let errors = rules_for(|value| {
            value["status"] = Value::String(status.to_string());
            value["conformance"] = serde_json::json!(["roundtrip", "kill-restart"]);
        });
        assert!(
            has_path(&errors, "conformance"),
            "R-CN2-5: status {status} without contract must be refused, got {errors:?}"
        );
    }
    let errors = rules_for(|value| {
        value["status"] = Value::String("planned".to_string());
        value["conformance"] = serde_json::json!([]);
    });
    assert!(
        !has_path(&errors, "conformance"),
        "R-CN2-5: a planned manifest needs no suite, got {errors:?}"
    );
}

/// The manifest with one field changed, and what R-CN2-6 says about it: its refusals
/// since CN2 Task 2 made the rule an error, and nothing on the warning channel.
fn native_first_findings(mutate: impl FnOnce(&mut Value)) -> Vec<ManifestError> {
    let schema = ManifestSchema::embedded().expect("the manifest schema compiles");
    let mut value = kafka_example();
    mutate(&mut value);
    let errors = match validate_manifest(&value, schema.validator()) {
        Ok(spec) => {
            assert!(
                semantic_warnings(&spec).is_empty(),
                "R-CN2-6 no longer warns once it is an error"
            );
            Vec::new()
        }
        Err(errors) => errors,
    };
    errors
        .into_iter()
        .filter(|error| error.path.starts_with("runtime"))
        .collect()
}

/// R-CN2-6 (OR-1): a P1 or P2 direction runs native by default unless a
/// `fallback-only:` note gives one of OR-4's reasons. CN2 Task 2 assigned the
/// runtimes and made it an error, so a manifest that breaks it no longer loads.
#[test]
fn p1_p2_default_runtime_is_native_or_explained() {
    const {
        assert!(
            NATIVE_FIRST_ENFORCED,
            "R-CN2-6 is an error since CN2 Task 2 assigned the runtimes"
        );
    }
    // Native in both directions: nothing to say.
    assert!(native_first_findings(|_| {}).is_empty());

    // A P2 manifest on Camel with no note is refused.
    let errors = native_first_findings(|value| {
        value["starred"] = Value::Bool(false);
        value["priority"] = Value::String("P2".to_string());
        value["runtime"]["kind"] = Value::String("camel".to_string());
        value["runtime"]["ref"] = Value::String("jdbc".to_string());
    });
    assert!(
        has_path(&errors, "runtime"),
        "R-CN2-6: a P2 camel default with no note must be refused, got {errors:?}"
    );

    // A per-direction override is named by its own path.
    let errors = native_first_findings(|value| {
        value["runtime"]["sink"] = serde_json::json!({ "kind": "iggy", "ref": "kafka_sink" });
    });
    assert!(
        has_path(&errors, "runtime.sink") && !has_path(&errors, "runtime"),
        "R-CN2-6: only the overridden direction is non-native, got {errors:?}"
    );

    // A `fallback-only:` note that names an OR-4 reason explains it, the Camel runtime
    // row's reason (CN2 Task 2) included.
    for note in [
        "fallback-only: generic JDBC is a Java API",
        "fallback-only: the Camel runtime itself, which OR-4 keeps as the fallback runtime",
    ] {
        let errors = native_first_findings(|value| {
            value["runtime"]["kind"] = Value::String("camel".to_string());
            value["notes"] = serde_json::json!([note]);
        });
        assert!(
            errors.is_empty(),
            "R-CN2-6: {note:?} explains a non-native default, got {errors:?}"
        );
    }

    // A note that is not `fallback-only:`, or gives no OR-4 reason, does not.
    for note in [
        "narrows: no transactions",
        "fallback-only: we have not written it yet",
        "fallback-only: camel is convenient",
    ] {
        let errors = native_first_findings(|value| {
            value["runtime"]["kind"] = Value::String("camel".to_string());
            value["notes"] = serde_json::json!([note]);
        });
        assert!(
            has_path(&errors, "runtime"),
            "R-CN2-6: {note:?} does not explain a camel default, got {errors:?}"
        );
    }

    // P3 is CN3's and outside OR-1.
    let errors = native_first_findings(|value| {
        value["starred"] = Value::Bool(false);
        value["priority"] = Value::String("P3".to_string());
        value["runtime"]["kind"] = Value::String("camel".to_string());
    });
    assert!(errors.is_empty(), "R-CN2-6 covers P1 and P2 only");

    // The refusal comes through the rules every load runs, not a side channel.
    let errors = rules_for(|value| {
        value["runtime"]["kind"] = Value::String("camel".to_string());
    });
    assert!(
        has_path(&errors, "runtime"),
        "R-CN2-6 is an error since CN2 Task 2, got {errors:?}"
    );
}

/// R-CN2-7 (OR-7): `exactly_once` is a `TransactionalSink` claim, proven by the
/// `exactly-once` suite. An idempotent sink alone is effectively once.
#[test]
fn exactly_once_requires_transactional_sink() {
    // Idempotent only: no longer enough (CN1's rule 3 accepted it).
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("exactly_once".to_string());
        value["conformance"] = serde_json::json!(["contract", "exactly-once"]);
    });
    assert!(
        has_path(&errors, "capabilities.delivery.sink"),
        "R-CN2-7: an idempotent but not transactional sink is not exactly once, got {errors:?}"
    );

    // Transactional, but without the suite that proves it.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("exactly_once".to_string());
        value["capabilities"]["sink"]["transactional"] = Value::Bool(true);
    });
    assert!(
        has_path(&errors, "conformance"),
        "R-CN2-7: exactly_once needs the exactly-once suite, got {errors:?}"
    );

    // Both: legal.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("exactly_once".to_string());
        value["capabilities"]["sink"]["transactional"] = Value::Bool(true);
        value["conformance"] = serde_json::json!(["contract", "exactly-once"]);
    });
    assert!(
        errors.is_empty(),
        "R-CN2-7: a transactional sink with the exactly-once suite is legal, got {errors:?}"
    );
}

/// R-CN2-7 (OR-7): `effectively_once` needs a sink that dedupes (idempotent or
/// upsert) or a source whose ids are stable across re-reads.
#[test]
fn effectively_once_requires_dedup_or_stable_ids() {
    // A sink that neither dedupes nor upserts.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("effectively_once".to_string());
        value["capabilities"]["sink"]["idempotent"] = Value::Bool(false);
    });
    assert!(
        has_path(&errors, "capabilities.delivery.sink"),
        "R-CN2-7: an effectively-once sink must dedupe, got {errors:?}"
    );
    // Idempotent (the §33 §4 example) or upsert: legal.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["sink"] = Value::String("effectively_once".to_string());
    });
    assert!(errors.is_empty(), "idempotent suffices, got {errors:?}");
    let errors = rules_with_config(
        |value| {
            value["capabilities"]["delivery"]["sink"] =
                Value::String("effectively_once".to_string());
            value["capabilities"]["sink"]["idempotent"] = Value::Bool(false);
            value["capabilities"]["sink"]["upsert"] = Value::Bool(true);
        },
        config_schema_with_a_key(),
    );
    assert!(errors.is_empty(), "upsert suffices, got {errors:?}");

    // A source without stable ids.
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["source"] = Value::String("effectively_once".to_string());
    });
    assert!(
        has_path(&errors, "capabilities.delivery.source"),
        "R-CN2-7: an effectively-once source needs envelope.stable_ids, got {errors:?}"
    );
    let errors = rules_for(|value| {
        value["capabilities"]["delivery"]["source"] = Value::String("effectively_once".to_string());
        value["envelope"]["stable_ids"] = Value::Bool(true);
    });
    assert!(errors.is_empty(), "stable ids suffice, got {errors:?}");

    // Effectively once sits between at-least-once and exactly-once for §33 §4
    // rule 1: a route asking for it is refused by an at-least-once sink and accepted
    // by an effectively-once one, which still refuses exactly-once.
    let schema = ManifestSchema::embedded().expect("the manifest schema compiles");
    let at_least_once = validate_manifest(&kafka_example(), schema.validator()).unwrap();
    let uses = Uses {
        direction: Direction::Sink,
        delivery: Some(Delivery::EffectivelyOnce),
        ..Uses::default()
    };
    assert_eq!(
        check_use(&at_least_once, &uses).unwrap_err().capability,
        "delivery.sink"
    );
    let mut value = kafka_example();
    value["capabilities"]["delivery"]["sink"] = Value::String("effectively_once".to_string());
    let effectively_once = validate_manifest(&value, schema.validator()).unwrap();
    assert!(check_use(&effectively_once, &uses).is_ok());
    assert!(check_use(&effectively_once, &uses_exactly_once()).is_err());
}

/// R-CN2-8 (OR-4): a fallback is Camel, Debezium or Iggy, in declared directions,
/// and names the task that replaces or replaced it.
#[test]
fn fallback_needs_replaced_by() {
    let fallback = |replaced_by: &str| {
        serde_json::json!({
            "kind": "iggy",
            "ref": "kafka_sink",
            "directions": ["sink"],
            "replaced_by": replaced_by,
        })
    };

    for blank in ["", "   "] {
        let errors = rules_for(|value| value["runtime"]["fallback"] = fallback(blank));
        assert!(
            has_path(&errors, "runtime.fallback.replaced_by"),
            "R-CN2-8: replaced_by {blank:?} must be refused, got {errors:?}"
        );
    }

    // A native or openapi "fallback" is not one of OR-4's fallback runtimes.
    for kind in ["native", "openapi"] {
        let errors = rules_for(|value| {
            let mut block = fallback("CN2 Task 9");
            block["kind"] = Value::String(kind.to_string());
            value["runtime"]["fallback"] = block;
        });
        assert!(
            has_path(&errors, "runtime.fallback.kind"),
            "R-CN2-8: a {kind} fallback must be refused, got {errors:?}"
        );
    }

    // A fallback direction the manifest does not declare.
    let errors = rules_for(|value| {
        value["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove("sink");
        value["runtime"]["fallback"] = fallback("CN2 Task 9");
    });
    assert!(
        has_path(&errors, "runtime.fallback.directions"),
        "R-CN2-8: a fallback in an undeclared direction must be refused, got {errors:?}"
    );

    // The whole block present and right: legal.
    let errors = rules_for(|value| value["runtime"]["fallback"] = fallback("CN2 Task 9"));
    assert!(
        errors.is_empty(),
        "a complete fallback is legal, got {errors:?}"
    );

    // The schema requires `replaced_by` to be there at all.
    let errors = rules_for(|value| {
        let mut block = fallback("CN2 Task 9");
        block.as_object_mut().unwrap().remove("replaced_by");
        value["runtime"]["fallback"] = block;
    });
    assert!(
        errors
            .iter()
            .any(|error| error.path.starts_with("runtime.fallback")),
        "the schema requires replaced_by, got {errors:?}"
    );
}

/// `RuntimeSpec::for_direction`: the default choice never returns the fallback, and
/// the fallback is returned only for the directions it declares.
#[test]
fn for_direction_falls_back_only_when_asked() {
    let runtime: RuntimeSpec = serde_json::from_value(serde_json::json!({
        "kind": "native",
        "ref": "loams_connector_postgres::Postgres",
        "version": "loams-connector-postgres 0.1",
        "source": { "kind": "native", "ref": "loams_connector_postgres::cdc::PgCdc" },
        "fallback": {
            "kind": "debezium",
            "ref": "io.debezium.connector.postgresql.PostgresConnector",
            "directions": ["source"],
            "replaced_by": "CN2 Task 10",
        },
    }))
    .unwrap();

    // The default: the override where there is one, the top level otherwise.
    assert_eq!(
        runtime.for_direction(Direction::Source, Choice::Default),
        Some((RuntimeKind::Native, "loams_connector_postgres::cdc::PgCdc"))
    );
    assert_eq!(
        runtime.for_direction(Direction::Sink, Choice::Default),
        Some((RuntimeKind::Native, "loams_connector_postgres::Postgres"))
    );

    // The fallback, only when asked and only for its declared direction.
    assert_eq!(
        runtime.for_direction(Direction::Source, Choice::Fallback),
        Some((
            RuntimeKind::Debezium,
            "io.debezium.connector.postgresql.PostgresConnector"
        ))
    );
    assert_eq!(
        runtime.for_direction(Direction::Sink, Choice::Fallback),
        None,
        "the fallback declares the source only"
    );

    // A runtime with no fallback has none to give.
    let plain: RuntimeSpec = serde_json::from_value(serde_json::json!({
        "kind": "camel",
        "ref": "jdbc",
    }))
    .unwrap();
    assert_eq!(
        plain.for_direction(Direction::Sink, Choice::Default),
        Some((RuntimeKind::Camel, "jdbc"))
    );
    assert_eq!(
        plain.for_direction(Direction::Source, Choice::Fallback),
        None
    );
    assert_eq!(plain.for_direction(Direction::Sink, Choice::Fallback), None);
}
