//! Stores through a mock publisher, read back from a mock aggregator.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use bytes::Bytes;
use loams_walrus::{BlobId, Network, StoreOpts, Walrus, WalrusConfig, WalrusError};
use support::{Behaviour, MockAggregator, MockPublisher};

const BLOB_ID: &str = "M4hsZGQ1oCktdzegB6HnI6Mi28S2nqOPHxK-W7_4BUk";
const DATA: &[u8] = b"some string 12345";

/// A `newlyCreated` answer, in the shape of docs.wal.app's HTTP API page.
fn newly_created() -> String {
    format!(
        r#"{{"newlyCreated":{{"blobObject":{{"id":"0xe91eee8c5b6f35b9a250cfc29e30f0d9e5463a21fd8d1ddb0fc22d44db4eac50","registeredEpoch":34,"blobId":"{BLOB_ID}","size":17,"encodingType":"RS2","certifiedEpoch":34,"storage":{{"id":"0x4748cd83217b5ce7aa77e7f1ad6fc5f7f694e26a157381b9391ac65c47815faf","startEpoch":34,"endEpoch":35,"storageSize":66034000}},"deletable":true}},"resourceOperation":{{"registerFromScratch":{{"encodedLength":66034000,"epochsAhead":1}}}},"cost":132300}}}}"#
    )
}

/// An `alreadyCertified` answer, in the same page's shape.
fn already_certified() -> String {
    format!(
        r#"{{"alreadyCertified":{{"blobId":"{BLOB_ID}","event":{{"txDigest":"4XQHFa9S324wTzYHF3vsBSwpUZuLpmwTHYMFv9nsDCSW","eventSeq":"0"}},"endEpoch":35}}}}"#
    )
}

fn config(publisher: &MockPublisher, agg: &MockAggregator) -> WalrusConfig {
    WalrusConfig {
        enabled: true,
        aggregators: vec![agg.url.clone()],
        publisher: Some(publisher.url.clone()),
        hedge_after_ms: 50,
        ..WalrusConfig::default()
    }
}

#[tokio::test]
async fn store_parses_newly_created() {
    let p = MockPublisher::start(StatusCode::OK, newly_created()).await;
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    let out = w
        .store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap();
    assert_eq!(out.blob_id, BlobId::parse(BLOB_ID).unwrap());
    assert!(out.newly_created);
    assert_eq!(out.end_epoch, 35);
    assert_eq!(out.size, 17);
    assert_eq!(
        out.sui_object_id.as_deref(),
        Some("0xe91eee8c5b6f35b9a250cfc29e30f0d9e5463a21fd8d1ddb0fc22d44db4eac50")
    );
    let seen = p.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/blobs");
    // The config's default epochs; deletable is the default, so no flag.
    assert_eq!(seen[0].query.get("epochs").map(String::as_str), Some("5"));
    assert!(!seen[0].query.contains_key("permanent"));
    assert!(!seen[0].query.contains_key("deletable"));
    assert_eq!(&seen[0].body[..], DATA);
    assert_eq!(seen[0].auth, None);
}

#[tokio::test]
async fn store_parses_already_certified() {
    let p = MockPublisher::start(StatusCode::OK, already_certified()).await;
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    let opts = StoreOpts {
        epochs: Some(2),
        permanent: true,
    };
    let out = w.store(Bytes::from_static(DATA), opts).await.unwrap();
    assert_eq!(out.blob_id, BlobId::parse(BLOB_ID).unwrap());
    assert!(!out.newly_created);
    assert_eq!(out.end_epoch, 35);
    assert_eq!(out.size, DATA.len() as u64);
    assert_eq!(out.sui_object_id, None);
    let q = &p.seen()[0].query;
    assert_eq!(q.get("epochs").map(String::as_str), Some("2"));
    assert_eq!(q.get("permanent").map(String::as_str), Some("true"));
}

#[tokio::test]
async fn store_reads_back_and_verifies() {
    let p = MockPublisher::start(StatusCode::OK, newly_created()).await;
    let liar = MockAggregator::start(Behaviour::serve(&b"not what was stored"[..])).await;
    let w = Walrus::new(&config(&p, &liar)).await.unwrap();
    let err = w
        .store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap_err();
    assert!(matches!(err, WalrusError::Integrity { .. }), "{err:?}");
    assert_eq!(liar.paths(), vec![format!("/v1/blobs/{BLOB_ID}")]);
}

#[tokio::test]
async fn not_found_after_write_retries() {
    let p = MockPublisher::start(StatusCode::OK, newly_created()).await;
    // A CDN still answers 404 twice just after certification.
    let agg = MockAggregator::start(Behaviour::serve(DATA).not_found_first(2)).await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    let started = Instant::now();
    w.store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap();
    assert_eq!(agg.hits(), 3);
    assert!(started.elapsed() >= Duration::from_millis(250 + 500));
}

#[tokio::test]
async fn read_after_write_window_is_configurable() {
    let p = MockPublisher::start(StatusCode::OK, newly_created()).await;
    let agg = MockAggregator::start(Behaviour::serve(DATA).not_found_first(100)).await;
    let mut cfg = config(&p, &agg);
    cfg.read_after_write_ms = 0;
    let w = Walrus::new(&cfg).await.unwrap();
    let err = w
        .store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap_err();
    match err {
        WalrusError::NotFound(blob) => assert_eq!(blob, BLOB_ID),
        other => panic!("expected NotFound, got {other:?}"),
    }
    assert_eq!(agg.hits(), 1, "no window, no retry");
}

#[tokio::test]
async fn body_over_limit_refused_before_upload() {
    let p = MockPublisher::start(StatusCode::OK, newly_created()).await;
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let mut cfg = config(&p, &agg);
    cfg.max_blob_bytes = 8;
    let w = Walrus::new(&cfg).await.unwrap();
    let err = w
        .store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            WalrusError::TooLarge {
                size: 17,
                max: 8,
                ..
            }
        ),
        "{err:?}"
    );
    let err = w
        .store_quilt(
            vec![
                ("a".into(), Bytes::from_static(b"12345")),
                ("b".into(), Bytes::from_static(b"6789")),
            ],
            StoreOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, WalrusError::TooLarge { size: 9, .. }),
        "{err:?}"
    );
    assert!(p.seen().is_empty());
}

#[tokio::test]
async fn store_quilt_parses_patches() {
    let quilt = "6XUOE-Q5-nAXHRifN6n9nomVDtHZQbGuAkW3PjlBuKo";
    let p1 = format!("{quilt}BAQDQAA");
    let p2 = format!("{quilt}B0AB7Ag");
    let answer = format!(
        r#"{{"blobStoreResult":{{"newlyCreated":{{"blobObject":{{"id":"0xabc","blobId":"{quilt}","size":1000,"storage":{{"endEpoch":40}}}},"cost":1}}}},"storedQuiltBlobs":[{{"identifier":"walrus.jpg","quiltPatchId":"{p2}"}},{{"identifier":"another_walrus.jpg","quiltPatchId":"{p1}"}}]}}"#
    );
    let p = MockPublisher::start(StatusCode::OK, answer).await;
    let agg = MockAggregator::start(
        Behaviour::serve(&b"?"[..])
            .path(
                format!("/v1/blobs/by-quilt-patch-id/{p2}"),
                &b"first file"[..],
            )
            .path(
                format!("/v1/blobs/by-quilt-patch-id/{p1}"),
                &b"second file"[..],
            ),
    )
    .await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    let out = w
        .store_quilt(
            vec![
                ("walrus.jpg".into(), Bytes::from_static(b"first file")),
                (
                    "another_walrus.jpg".into(),
                    Bytes::from_static(b"second file"),
                ),
            ],
            StoreOpts::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.blob.blob_id.as_str(), quilt);
    assert!(out.blob.newly_created);
    assert_eq!(out.blob.end_epoch, 40);
    assert_eq!(out.patches.len(), 2);
    assert_eq!(out.patches[0].0, "walrus.jpg");
    assert_eq!(out.patches[0].1.as_str(), p2);

    let seen = &p.seen()[0];
    assert_eq!(seen.path, "/v1/quilts");
    let ct = seen.content_type.clone().unwrap();
    let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
    let body = String::from_utf8(seen.body.to_vec()).unwrap();
    assert!(body.contains(
        "Content-Disposition: form-data; name=\"walrus.jpg\"; filename=\"walrus.jpg\"\r\n"
    ));
    assert!(body.contains("\r\n\r\nfirst file\r\n"));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
    // Every patch was read back verified.
    assert_eq!(agg.hits(), 2);
}

#[tokio::test]
async fn quilt_identifiers_are_checked() {
    let p = MockPublisher::start(StatusCode::OK, "{}").await;
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    for files in [
        vec![("a\"b".to_string(), Bytes::new())],
        vec![("_metadata".to_string(), Bytes::new())],
        vec![
            ("a".to_string(), Bytes::new()),
            ("a".to_string(), Bytes::new()),
        ],
        vec![],
    ] {
        let err = w
            .store_quilt(files, StoreOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, WalrusError::Config(_)), "{err:?}");
    }
    assert!(p.seen().is_empty());
}

#[tokio::test]
async fn mainnet_without_publisher_is_read_only() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let cfg = WalrusConfig {
        enabled: true,
        network: Network::Mainnet,
        aggregators: vec![agg.url.clone()],
        publisher: None,
        ..WalrusConfig::default()
    };
    let w = Walrus::new(&cfg).await.unwrap();
    assert!(!w.can_write());
    let err = w
        .store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap_err();
    assert!(matches!(err, WalrusError::NoPublisher), "{err:?}");
    let err = w
        .store_quilt(
            vec![("a".into(), Bytes::from_static(DATA))],
            StoreOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, WalrusError::NoPublisher), "{err:?}");
    // Reads still work.
    w.read(
        &loams_walrus::BlobId::from_bytes([1; 32]),
        &loams_walrus::ContentDigest::of(DATA),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn bearer_token_is_read_from_its_file() {
    let p = MockPublisher::start(StatusCode::OK, already_certified()).await;
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "jwt-token\n").unwrap();
    let mut cfg = config(&p, &agg);
    cfg.publisher_auth = Some(token);
    let w = Walrus::new(&cfg).await.unwrap();
    w.store(Bytes::from_static(DATA), StoreOpts::default())
        .await
        .unwrap();
    assert_eq!(p.seen()[0].auth.as_deref(), Some("Bearer jwt-token"));
}

#[tokio::test]
async fn publisher_errors_are_typed() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    for (status, body) in [
        (StatusCode::PAYLOAD_TOO_LARGE, "too big".to_string()),
        (StatusCode::OK, r#"{"somethingElse":{}}"#.to_string()),
        (
            StatusCode::OK,
            format!(r#"{{"markedInvalid":{{"blobId":"{BLOB_ID}","event":{{}}}}}}"#),
        ),
    ] {
        let p = MockPublisher::start(status, body).await;
        let w = Walrus::new(&config(&p, &agg)).await.unwrap();
        let err = w
            .store(Bytes::from_static(DATA), StoreOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, WalrusError::Publisher(_)), "{err:?}");
    }
    let p = MockPublisher::start(StatusCode::OK, already_certified()).await;
    let w = Walrus::new(&config(&p, &agg)).await.unwrap();
    let zero = StoreOpts {
        epochs: Some(0),
        permanent: false,
    };
    assert!(matches!(
        w.store(Bytes::from_static(DATA), zero).await.unwrap_err(),
        WalrusError::Config(_)
    ));
    assert!(p.seen().is_empty());
}
