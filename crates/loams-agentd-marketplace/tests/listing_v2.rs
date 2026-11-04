#![allow(clippy::unwrap_used)]

//! OP2-B Task 2: the typed listing, the v1 upgrade and values validation.

use std::path::{Path, PathBuf};

use loams_agentd_marketplace::{
    ListingError, ListingKind, ListingV2, Package, SchemaVersion, Target,
};
use serde_json::{Value, json};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/marketplace/testdata")
}

fn fixture(name: &str) -> Value {
    let path = testdata().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn v2_app_oci_parses() {
    let l = ListingV2::from_json(&fixture("v2/valid/app-oci.json")).unwrap();
    assert_eq!(l.kind, ListingKind::App);
    assert_eq!(l.schema, SchemaVersion::V2);
    assert_eq!(l.id, "acme/notes-app");
    assert_eq!(l.namespace(), "acme");
    assert!(
        matches!(l.package, Some(Package::Oci { ref digest, .. }) if digest.starts_with("sha256:"))
    );
    assert!(l.cost_hint.is_some());
}

#[test]
fn binary_targets_are_triples() {
    let l = ListingV2::from_json(&fixture("v2/valid/laptop-binary.json")).unwrap();
    let Some(Package::Binary { targets }) = l.package else {
        panic!("not a binary package");
    };
    assert_eq!(targets[0].0, "x86_64-unknown-linux-gnu");
    assert_eq!(l.targets, vec![Target::NodeLaptop]);
}

#[test]
fn v1_listing_parses_as_agent() {
    let l = ListingV2::from_json(&fixture("valid/agent-mastra-acp.json")).unwrap();
    assert_eq!(l.schema, SchemaVersion::V1);
    assert_eq!(l.kind, ListingKind::Agent);
    assert_eq!(l.targets, vec![Target::Cluster, Target::NodeLaptop]);
    assert!(l.package.is_none());
    assert!(l.raw.get("artifact").is_some());
}

#[test]
fn v1_byoc_server_maps_to_cluster() {
    let l = ListingV2::from_json(&fixture("valid/agent-pydantic-ai-a2a.json")).unwrap();
    assert_eq!(l.schema, SchemaVersion::V1);
    // `agentd` and `byoc-server` together.
    assert_eq!(l.targets, vec![Target::Cluster, Target::NodeLaptop]);

    let mut doc = fixture("valid/agent-pydantic-ai-a2a.json");
    doc["targets"] = json!(["byoc-server"]);
    let l = ListingV2::from_json(&doc).unwrap();
    assert_eq!(l.targets, vec![Target::Cluster]);
}

#[test]
fn v1_without_targets_is_an_agentd_listing() {
    let l = ListingV2::from_json(&fixture("valid/mcp-server-remote.json")).unwrap();
    assert_eq!(l.kind, ListingKind::McpServer);
    assert_eq!(l.kind.as_str(), "mcp-server");
    assert!(!l.targets.is_empty());
}

#[test]
fn values_validated_against_values_schema() {
    let l = ListingV2::from_json(&fixture("v2/valid/app-helm.json")).unwrap();
    l.validate_values(&json!({"replicas": 2})).unwrap();
    let issues = l.validate_values(&json!({"replicas": "x"})).unwrap_err();
    assert_eq!(issues[0].instance_path, "/replicas");
    // The cached validator serves a clone too.
    l.clone().validate_values(&json!({"replicas": 2})).unwrap();
}

fn with_values_schema(schema: Value) -> Result<ListingV2, ListingError> {
    let mut doc = fixture("v2/valid/app-helm.json");
    doc["values_schema"] = schema;
    ListingV2::from_json(&doc)
}

#[test]
fn values_schema_remote_ref_refused() {
    for reference in ["https://evil/x.json", "other.json#/a"] {
        let err = with_values_schema(json!({
            "type": "object",
            "properties": { "a": { "allOf": [ { "$ref": reference } ] } }
        }))
        .unwrap_err();
        assert!(
            matches!(err, ListingError::ValuesSchema(_)),
            "{reference}: {err:?}"
        );
    }
    let err = with_values_schema(json!({"$ref": "https://evil/x.json"})).unwrap_err();
    assert!(matches!(err, ListingError::ValuesSchema(_)), "{err:?}");
    // A local reference is fine.
    with_values_schema(json!({
        "type": "object",
        "$defs": { "n": { "type": "integer" } },
        "properties": { "a": { "$ref": "#/$defs/n" } }
    }))
    .unwrap();
}

#[test]
fn values_schema_over_64k_refused() {
    let mut schema = json!({"type": "object", "description": ""});
    let base = loams_agentd_marketplace::index::canonical(&schema).len();
    schema["description"] = Value::String("x".repeat(65_537 - base));
    assert_eq!(
        loams_agentd_marketplace::index::canonical(&schema).len(),
        65_537
    );
    let err = with_values_schema(schema.clone()).unwrap_err();
    assert!(matches!(err, ListingError::ValuesSchema(_)), "{err:?}");

    schema["description"] = Value::String("x".repeat(65_536 - base));
    with_values_schema(schema).unwrap();
}

#[test]
fn no_values_schema_accepts_only_empty_object() {
    let l = ListingV2::from_json(&fixture("v2/valid/app-oci.json")).unwrap();
    assert!(l.values_schema.is_none());
    l.validate_values(&json!({})).unwrap();
    assert!(l.validate_values(&json!({"a": 1})).is_err());
    assert!(l.validate_values(&json!(null)).is_err());
}

#[test]
fn package_round_trips_json() {
    let doc = fixture("v2/valid/laptop-binary.json");
    let l = ListingV2::from_json(&doc).unwrap();
    assert_eq!(
        serde_json::to_value(l.package.unwrap()).unwrap(),
        doc["package"]
    );
    let helm = fixture("v2/valid/app-helm.json");
    let l = ListingV2::from_json(&helm).unwrap();
    assert_eq!(
        serde_json::to_value(l.package.unwrap()).unwrap(),
        helm["package"]
    );
}

#[test]
fn schema_violations_are_schema_errors() {
    let mut doc = fixture("v2/valid/app-oci.json");
    doc.as_object_mut().unwrap().remove("cost_hint");
    assert!(matches!(
        ListingV2::from_json(&doc),
        Err(ListingError::Schema(_))
    ));
}
