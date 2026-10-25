//! The verified disk cache, alone and in front of the client.

#![allow(clippy::unwrap_used)]

mod support;

use axum::http::StatusCode;
use loams_walrus::{ContentDigest, VerifiedDiskCache, Walrus};
use support::{Behaviour, MockAggregator, blob_id, config};

fn file_of(dir: &std::path::Path, d: &ContentDigest) -> std::path::PathBuf {
    let hex = d.to_hex();
    dir.join("sha256").join(&hex[..2]).join(hex)
}

#[tokio::test]
async fn put_then_get_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let cache = VerifiedDiskCache::open(dir.path(), 1 << 20).await.unwrap();
    let d = ContentDigest::of(b"hello");
    assert!(cache.get(&d).await.unwrap().is_none());
    cache.put(Some("blob-a"), &d, b"hello").await.unwrap();
    assert_eq!(&cache.get(&d).await.unwrap().unwrap()[..], b"hello");
    assert!(file_of(dir.path(), &d).exists());
    assert_eq!(cache.lookup("blob-a"), Some(d));
}

#[tokio::test]
async fn cache_hit_needs_sha256() {
    const DATA: &[u8] = b"cached while online";
    let dir = tempfile::tempdir().unwrap();
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let mut cfg = config(&[&agg]);
    cfg.cache_dir = Some(dir.path().to_path_buf());
    let w = Walrus::new(&cfg).await.unwrap();
    w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(agg.hits(), 1);
    // Same blob id, another expected sha256: no hit, and the fetched bytes
    // are refused.
    assert!(
        w.read(&blob_id(1), &ContentDigest::of(b"other"))
            .await
            .is_err()
    );
    assert_eq!(agg.hits(), 2);
    // The right sha256 hits without a request.
    w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(agg.hits(), 2);
}

#[tokio::test]
async fn cache_never_stores_unverified() {
    let dir = tempfile::tempdir().unwrap();
    let cache = VerifiedDiskCache::open(dir.path(), 1 << 20).await.unwrap();
    let err = cache
        .put(None, &ContentDigest::of(b"hello"), b"other")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(cache.total_bytes(), 0);

    // Through the client: wrong bytes from the aggregator are not cached.
    let agg = MockAggregator::start(Behaviour::serve(&b"tampered"[..])).await;
    let mut cfg = config(&[&agg]);
    cfg.cache_dir = Some(dir.path().join("client"));
    let w = Walrus::new(&cfg).await.unwrap();
    assert!(
        w.read(&blob_id(1), &ContentDigest::of(b"hello"))
            .await
            .is_err()
    );
    let cache = w.cache().unwrap();
    assert_eq!(cache.total_bytes(), 0);
    assert!(!cache.contains(&ContentDigest::of(b"tampered")));
}

#[tokio::test]
async fn cache_evicts_lru() {
    let dir = tempfile::tempdir().unwrap();
    let cache = VerifiedDiskCache::open(dir.path(), 25).await.unwrap();
    let blobs: Vec<Vec<u8>> = (0u8..3).map(|i| vec![i; 10]).collect();
    let ds: Vec<_> = blobs.iter().map(|b| ContentDigest::of(b)).collect();
    cache.put(Some("b0"), &ds[0], &blobs[0]).await.unwrap();
    cache.put(Some("b1"), &ds[1], &blobs[1]).await.unwrap();
    // Read 0, so 1 is the least recently read when 2 arrives.
    cache.get(&ds[0]).await.unwrap().unwrap();
    cache.put(Some("b2"), &ds[2], &blobs[2]).await.unwrap();
    assert!(cache.contains(&ds[0]));
    assert!(!cache.contains(&ds[1]));
    assert!(cache.contains(&ds[2]));
    assert!(!file_of(dir.path(), &ds[1]).exists());
    assert_eq!(cache.total_bytes(), 20);
    assert_eq!(cache.lookup("b1"), None);
    assert_eq!(cache.lookup("b2"), Some(ds[2]));
}

#[tokio::test]
async fn cache_rehashes_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let good = ContentDigest::of(b"persisted");
    let rotten = ContentDigest::of(b"will rot");
    {
        let cache = VerifiedDiskCache::open(dir.path(), 1 << 20).await.unwrap();
        cache.put(Some("good"), &good, b"persisted").await.unwrap();
        cache
            .put(Some("rotten"), &rotten, b"will rot")
            .await
            .unwrap();
    }
    std::fs::write(file_of(dir.path(), &rotten), b"bit rot!").unwrap();
    let shard = file_of(dir.path(), &good).parent().unwrap().to_path_buf();
    std::fs::write(shard.join(".tmp-1-1"), b"partial").unwrap();

    let cache = VerifiedDiskCache::open(dir.path(), 1 << 20).await.unwrap();
    assert!(!shard.join(".tmp-1-1").exists(), "temp files are dropped");
    assert_eq!(cache.lookup("good"), Some(good), "the index survives");
    assert_eq!(&cache.get(&good).await.unwrap().unwrap()[..], b"persisted");
    assert!(cache.get(&rotten).await.unwrap().is_none());
    assert!(!file_of(dir.path(), &rotten).exists());
}

#[tokio::test]
async fn corrupt_file_is_fetched_again() {
    const DATA: &[u8] = b"build cache entry";
    let dir = tempfile::tempdir().unwrap();
    let agg = MockAggregator::start(Behaviour::serve(DATA)).await;
    let mut cfg = config(&[&agg]);
    cfg.cache_dir = Some(dir.path().to_path_buf());
    let w = Walrus::new(&cfg).await.unwrap();
    let d = ContentDigest::of(DATA);
    assert_eq!(&w.read(&blob_id(1), &d).await.unwrap()[..], DATA);
    assert_eq!(&w.read(&blob_id(1), &d).await.unwrap()[..], DATA);
    assert_eq!(agg.hits(), 1);
    std::fs::write(file_of(dir.path(), &d), b"bit rot").unwrap();
    assert_eq!(&w.read(&blob_id(1), &d).await.unwrap()[..], DATA);
    assert_eq!(agg.hits(), 2);
    assert_eq!(std::fs::read(file_of(dir.path(), &d)).unwrap(), DATA);
}

#[tokio::test]
async fn a_hit_needs_no_aggregator() {
    const DATA: &[u8] = b"cached earlier";
    let dir = tempfile::tempdir().unwrap();
    let cache = VerifiedDiskCache::open(dir.path(), 1 << 20).await.unwrap();
    cache
        .put(None, &ContentDigest::of(DATA), DATA)
        .await
        .unwrap();
    drop(cache);
    let down = MockAggregator::start(Behaviour::status(StatusCode::SERVICE_UNAVAILABLE)).await;
    let mut cfg = config(&[&down]);
    cfg.cache_dir = Some(dir.path().to_path_buf());
    let w = Walrus::new(&cfg).await.unwrap();
    let got = w.read(&blob_id(1), &ContentDigest::of(DATA)).await.unwrap();
    assert_eq!(&got[..], DATA);
    assert_eq!(down.hits(), 0);
}
