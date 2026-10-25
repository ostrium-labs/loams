//! Verified, hedged reads against mock aggregators.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use bytes::Bytes;
use loams_walrus::{
    AttemptFailure, ContentDigest, QuiltPatchId, Unverified, Walrus, WalrusConfig, WalrusError,
};
use support::{Behaviour, MockAggregator, MockPublisher, blob_id, config};

const DATA: &[u8] = b"public agent artifact v1";

#[tokio::test]
async fn disabled_client_makes_no_requests() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let publisher = MockPublisher::start(StatusCode::OK, "{}").await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = WalrusConfig {
        enabled: false,
        publisher: Some(publisher.url.clone()),
        publisher_auth: Some(dir.path().join("no-such-token")),
        cache_dir: Some(dir.path().join("cache")),
        ..config(&[&agg])
    };
    let w = Walrus::new(&cfg).await.unwrap();
    assert!(!w.is_enabled());
    let d = ContentDigest::of(DATA);
    let disabled = |r: Result<_, WalrusError>| matches!(r, Err(WalrusError::Disabled));
    assert!(disabled(w.read(&blob_id(1), &d).await.map(|_| ())));
    assert!(disabled(
        w.read_quilt_patch(&blob_id(1), "a", &d).await.map(|_| ())
    ));
    let patch = QuiltPatchId::parse("abc").unwrap();
    assert!(disabled(w.read_patch(&patch, &d).await.map(|_| ())));
    assert!(disabled(w.read_range(&blob_id(1), 0..4).await.map(|_| ())));
    assert!(disabled(w.list_patches(&blob_id(1)).await.map(|_| ())));
    assert!(disabled(
        w.read_to_path(&blob_id(1), &d, &dir.path().join("x"))
            .await
            .map(|_| ())
    ));
    assert!(disabled(
        w.store(Bytes::from_static(DATA), Default::default())
            .await
            .map(|_| ())
    ));
    assert!(disabled(
        w.store_quilt(
            vec![("a".into(), Bytes::from_static(DATA))],
            Default::default()
        )
        .await
        .map(|_| ())
    ));
    assert_eq!(agg.hits(), 0);
    assert!(publisher.seen().is_empty());
    assert!(
        !dir.path().join("cache").exists(),
        "no cache directory either"
    );
}

#[tokio::test]
async fn read_verifies_sha256() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&[&agg])).await.unwrap();
    let id = blob_id(1);
    let got = w.read(&id, &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(&got[..], DATA);
    assert_eq!(agg.paths(), vec![format!("/v1/blobs/{id}")]);
}

#[tokio::test]
async fn read_rejects_wrong_sha256() {
    let agg = MockAggregator::start(Behaviour::serve(&b"tampered"[..])).await;
    let w = Walrus::new(&config(&[&agg])).await.unwrap();
    let err = w
        .read(&blob_id(1), &ContentDigest::of(DATA))
        .await
        .unwrap_err();
    match err {
        WalrusError::Integrity {
            aggregator,
            expected,
            actual,
            ..
        } => {
            assert_eq!(aggregator, agg.url.as_str());
            assert_eq!(expected, ContentDigest::of(DATA));
            assert_eq!(actual, ContentDigest::of(b"tampered"));
        }
        other => panic!("expected Integrity, got {other:?}"),
    }
}

#[tokio::test]
async fn hedge_starts_second_after_delay() {
    let slow = MockAggregator::start(Behaviour::serve(DATA).after(Duration::from_secs(5))).await;
    let fast = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&[&slow, &fast])).await.unwrap();
    let started = Instant::now();
    let got = w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(&got[..], DATA);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(50),
        "hedged before the delay: {took:?}"
    );
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert_eq!(slow.hits(), 1, "aggregator 0 is asked first");
    assert_eq!(fast.completed(), 1);
    assert_eq!(slow.completed(), 0, "the slow attempt was cancelled");
}

#[tokio::test]
async fn hedge_not_started_when_first_answers_in_time() {
    let first = MockAggregator::start(Behaviour::serve(DATA)).await;
    let second = MockAggregator::start(Behaviour::serve(DATA)).await;
    let mut cfg = config(&[&first, &second]);
    cfg.hedge_after_ms = 5_000;
    let w = Walrus::new(&cfg).await.unwrap();
    w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(first.hits(), 1);
    assert_eq!(second.hits(), 0);
}

#[tokio::test]
async fn hedge_first_verified_wins() {
    // The first answer is fast and wrong; the slower, verified one wins.
    let liar = MockAggregator::start(Behaviour::serve(&b"tampered"[..])).await;
    let honest =
        MockAggregator::start(Behaviour::serve(DATA).after(Duration::from_millis(200))).await;
    let mut cfg = config(&[&liar, &honest]);
    cfg.hedge_after_ms = 5_000;
    let w = Walrus::new(&cfg).await.unwrap();
    let got = w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(&got[..], DATA);
}

#[tokio::test]
async fn hedge_skips_lying_aggregator() {
    let liar = MockAggregator::start(Behaviour::serve(&b"tampered"[..])).await;
    let broken = MockAggregator::start(Behaviour::status(StatusCode::SERVICE_UNAVAILABLE)).await;
    let honest = MockAggregator::start(Behaviour::serve(DATA)).await;
    let mut cfg = config(&[&liar, &broken, &honest]);
    // A failure starts the next aggregator at once, not after the hedge.
    cfg.hedge_after_ms = 30_000;
    let w = Walrus::new(&cfg).await.unwrap();
    let started = Instant::now();
    let got = w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(&got[..], DATA);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!((liar.hits(), broken.hits(), honest.hits()), (1, 1, 1));
}

#[tokio::test]
async fn mixed_failures_are_reported_per_aggregator() {
    let a = MockAggregator::start(Behaviour::status(StatusCode::BAD_GATEWAY)).await;
    let b = MockAggregator::start(Behaviour::status(StatusCode::NOT_FOUND)).await;
    let w = Walrus::new(&config(&[&a, &b])).await.unwrap();
    match w
        .read(&blob_id(1), &ContentDigest::of(DATA))
        .await
        .unwrap_err()
    {
        WalrusError::AllAggregatorsFailed { attempts, .. } => {
            assert_eq!(attempts.len(), 2);
            assert!(attempts.iter().any(|(_, w)| *w == AttemptFailure::NotFound));
        }
        other => panic!("expected AllAggregatorsFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn not_found_everywhere_is_not_found_without_retry() {
    let a = MockAggregator::start(Behaviour::status(StatusCode::NOT_FOUND)).await;
    let b = MockAggregator::start(Behaviour::status(StatusCode::NOT_FOUND)).await;
    let w = Walrus::new(&config(&[&a, &b])).await.unwrap();
    let err = w
        .read(&blob_id(1), &ContentDigest::of(DATA))
        .await
        .unwrap_err();
    assert!(matches!(err, WalrusError::NotFound(_)), "{err:?}");
    // Not written by this client: no CDN retry.
    assert_eq!((a.hits(), b.hits()), (1, 1));
}

#[tokio::test]
async fn body_over_limit_refused() {
    let big = vec![0u8; 4096];
    let agg = MockAggregator::start(Behaviour::serve(big.clone())).await;
    let mut cfg = config(&[&agg]);
    cfg.max_blob_bytes = 1024;
    let w = Walrus::new(&cfg).await.unwrap();
    let err = w
        .read(&blob_id(1), &ContentDigest::of(&big))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WalrusError::TooLarge { max: 1024, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn quilt_patch_read_by_identifier() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&[&agg])).await.unwrap();
    let got = w
        .read_quilt_patch(&blob_id(9), "walrus.jpg", &ContentDigest::of(DATA))
        .await
        .unwrap();
    assert_eq!(&got[..], DATA);
    let patch = QuiltPatchId::parse("6XUOE-Q5-nAXHRifN6n9nomVDtHZQbGuAkW3PjlBuKoBAQDQAA").unwrap();
    w.read_patch(&patch, &ContentDigest::of(DATA))
        .await
        .unwrap();
    assert_eq!(
        agg.paths(),
        vec![
            format!("/v1/blobs/by-quilt-id/{}/walrus.jpg", blob_id(9)),
            format!("/v1/blobs/by-quilt-patch-id/{patch}"),
        ]
    );
}

#[tokio::test]
async fn list_patches_parses() {
    // The example answer of the aggregator's OpenAPI, plus tags.
    let listing = r#"[
      {"identifier": "walrus-38.jpeg", "patch_id": "uIiEbhP2qgZYygEGxJX1GeB-rQATo2yufC2DCp7B4iABAQANAA", "tags": {}},
      {"identifier": "walrus-39.avif", "patch_id": "uIiEbhP2qgZYygEGxJX1GeB-rQATo2yufC2DCp7B4iABDQBnAA", "tags": {"type": "image"}}
    ]"#;
    let agg = MockAggregator::start(Behaviour::serve(listing)).await;
    let w = Walrus::new(&config(&[&agg])).await.unwrap();
    let patches = w.list_patches(&blob_id(3)).await.unwrap();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].identifier, "walrus-38.jpeg");
    assert_eq!(
        patches[1].patch_id.as_str(),
        "uIiEbhP2qgZYygEGxJX1GeB-rQATo2yufC2DCp7B4iABDQBnAA"
    );
    assert_eq!(
        patches[1].tags.get("type").map(String::as_str),
        Some("image")
    );
    assert_eq!(
        agg.paths(),
        vec![format!("/v1/quilts/{}/patches", blob_id(3))]
    );
}

#[tokio::test]
async fn read_to_path_lands_only_verified_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let good = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&[&good])).await.unwrap();
    let dest = dir.path().join("artifact.bin");
    let n = w
        .read_to_path(&blob_id(1), &ContentDigest::of(DATA), &dest)
        .await
        .unwrap();
    assert_eq!(n, DATA.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), DATA);

    let bad = MockAggregator::start(Behaviour::serve(&b"tampered"[..])).await;
    let w = Walrus::new(&config(&[&bad])).await.unwrap();
    let dest2 = dir.path().join("bad.bin");
    let err = w
        .read_to_path(&blob_id(1), &ContentDigest::of(DATA), &dest2)
        .await
        .unwrap_err();
    assert!(matches!(err, WalrusError::Integrity { .. }), "{err:?}");
    assert!(!dest2.exists());
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("artifact.bin")]);
}

#[tokio::test]
async fn range_read_is_unverified() {
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let w = Walrus::new(&config(&[&agg])).await.unwrap();
    // The type says so: the bytes come wrapped, and unwrapping them is named.
    let got: Unverified<Bytes> = w.read_range(&blob_id(1), 7..12).await.unwrap();
    assert_eq!(&got.as_unverified()[..], &DATA[7..12]);
    let checked = got
        .clone()
        .verify(&ContentDigest::of(&DATA[7..12]))
        .unwrap();
    assert_eq!(&checked[..], &DATA[7..12]);
    assert!(got.verify(&ContentDigest::of(DATA)).is_err());
    let empty = w.read_range(&blob_id(1), 3..3).await.unwrap();
    assert!(empty.into_unverified_inner().is_empty());
}
