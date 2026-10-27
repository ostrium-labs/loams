//! The Confluent Schema Registry client (R5.4) against a fake registry that
//! answers as Confluent's REST API documents. No Kafka involved.

mod common;

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use common::{Seen, fake_registry_with};
use loams_connector_kafka::ConfluentRegistry;
use loams_connector_sdk::schema::registry::{SchemaFormat, SchemaRegistry, frame, unframe};
use loams_connector_sdk::{AllowAll, DenyLocal, ErrorClass, Secret};
use serde_json::json;

/// The routes this suite's fake registry answers.
async fn fake_registry() -> (String, Arc<Mutex<Vec<Seen>>>) {
    fake_registry_with(vec![
        (
            "GET",
            "/schemas/ids/7",
            200,
            json!({ "schema": "syntax = \"proto3\"; message M { int64 a = 1; }", "schemaType": "PROTOBUF" }),
        ),
        ("GET", "/schemas/ids/8", 200, json!({ "schema": "{\"type\":\"string\"}" })),
        ("POST", "/subjects/orders-value/versions", 200, json!({ "id": 7 })),
        (
            "POST",
            "/subjects/strict-value/versions",
            409,
            json!({ "error_code": 409, "message": "Schema being registered is incompatible" }),
        ),
    ])
    .await
}

#[tokio::test]
async fn registry_by_id_register_and_errors() {
    let (base, seen) = fake_registry().await;
    let reg = ConfluentRegistry::new(&base, Arc::new(AllowAll))
        .with_basic_auth("api-key", Secret::new(b"api-secret".to_vec()));

    let s = reg.by_id(7).await.expect("schema 7");
    assert_eq!(s.format, SchemaFormat::Protobuf);
    assert!(s.schema.starts_with(b"syntax"));
    // Cached: a schema id never changes.
    reg.by_id(7).await.expect("schema 7 again");
    assert_eq!(
        seen.lock()
            .expect("log")
            .iter()
            .filter(|s| s.path == "/schemas/ids/7")
            .count(),
        1
    );
    // AVRO is the registry's default type.
    assert_eq!(
        reg.by_id(8).await.expect("schema 8").format,
        SchemaFormat::Avro
    );
    let missing = reg.by_id(9).await.expect_err("unknown id");
    assert_eq!(missing.class, ErrorClass::Poison);

    let id = reg
        .register(
            "orders-value",
            SchemaFormat::Protobuf,
            Bytes::from_static(b"syntax = \"proto3\";"),
        )
        .await
        .expect("register");
    assert_eq!(id, 7);
    let post = seen
        .lock()
        .expect("log")
        .iter()
        .find(|s| s.method == "POST")
        .cloned()
        .expect("a POST");
    assert_eq!(post.body["schemaType"], "PROTOBUF");
    // Basic credentials, never echoed in an error.
    assert_eq!(post.auth.as_deref(), Some("Basic YXBpLWtleTphcGktc2VjcmV0"));
    let refused = reg
        .register(
            "strict-value",
            SchemaFormat::Avro,
            Bytes::from_static(b"{\"type\":\"int\"}"),
        )
        .await
        .expect_err("incompatible");
    assert_eq!(refused.class, ErrorClass::SchemaIncompatible);
    assert!(refused.message.contains("incompatible"));
    assert!(!refused.message.contains("api-secret"));
    assert!(
        reg.register("../admin", SchemaFormat::Avro, Bytes::from_static(b"{}"))
            .await
            .is_err()
    );

    // Framing round-trips with the registry's ids.
    let framed = frame(id, b"payload");
    assert_eq!(unframe(&framed).expect("frame").schema_id, 7);
}

#[tokio::test]
async fn registry_requests_are_egress_checked() {
    let (base, _) = fake_registry().await;
    let reg = ConfluentRegistry::new(&base, Arc::new(DenyLocal));
    let err = reg.by_id(7).await.expect_err("loopback refused");
    assert!(err.message.contains("egress refused"), "{err}");
}
