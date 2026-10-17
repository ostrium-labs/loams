//! `loams migrate verify`'s store probes (UP1 Task 7).

mod support;

use bytes::Bytes;
use loams_common::meta::{PointerCas, collection_pointer_key};
use loams_common::schema::{CollectionSchema, DynamicMapping};
use loams_migrate::verify::{ProbeState, verify_stores};
use loams_store::Store as ObjectStore;
use support::Stack;
use tempfile::TempDir;

const RELEASE: &str = "0.1.0";

#[tokio::test]
async fn verify_reports_empty_for_unused_products() {
    let stack = Stack::new().await;
    stack.stamp(RELEASE).await;
    let report = verify_stores(&stack.cx(), RELEASE).await;
    assert_eq!(report.products["stack"].state, ProbeState::Ok);
    for product in ["collections", "streams", "durable", "house"] {
        assert_eq!(
            report.products[product].state,
            ProbeState::Empty,
            "{product}: {:?}",
            report.products[product]
        );
    }
    assert!(report.passed());
    // A stack record at another release fails the stack probe.
    let other = verify_stores(&stack.cx(), "0.1.0-preview.4").await;
    assert_eq!(other.products["stack"].state, ProbeState::Failed);
    assert!(!other.passed());
}

#[tokio::test]
async fn verify_fails_on_unreadable_collection() {
    let stack = Stack::new().await;
    stack.stamp(RELEASE).await;
    let bucket_dir = TempDir::new().expect("bucket");
    let bucket = ObjectStore::from_url(
        &format!("file://{}/", bucket_dir.path().display()),
        Vec::<(String, String)>::new(),
    )
    .expect("store");
    let meta = &*stack.meta;
    let ns = meta.create_namespace("app").await.expect("namespace");
    let schema = CollectionSchema::new(Vec::new(), Vec::new(), DynamicMapping::Ignore);
    let (cid, _, _) = meta
        .create_collection(ns, "docs", schema, 1)
        .await
        .expect("collection");
    bucket
        .put("m/1.pb", Bytes::from_static(b"OPCM not a manifest"))
        .await
        .expect("put");
    meta.cas_pointer(PointerCas {
        namespace: ns,
        key: collection_pointer_key(cid),
        expected: None,
        value: "m/1.pb".to_string(),
        fence: None,
        fresh: None,
    })
    .await
    .into_result()
    .expect("pointer");
    let mut cx = stack.cx();
    cx.bucket = Some(bucket);
    let report = verify_stores(&cx, RELEASE).await;
    let collections = &report.products["collections"];
    assert_eq!(collections.state, ProbeState::Failed, "{collections:?}");
    assert!(collections.detail.contains("m/1.pb"), "{collections:?}");
    assert_eq!(report.products["streams"].state, ProbeState::Ok);
    assert!(!report.passed());
}
