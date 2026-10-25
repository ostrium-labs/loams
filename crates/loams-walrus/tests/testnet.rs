//! A round trip through Walrus Testnet's public publisher and aggregator:
//! store, read back, range read, quilt store, patch reads, patch listing.
//! Ignored by default (network); run with
//! `LOAMS_TEST_WALRUS_TESTNET=1 cargo test -p loams-walrus --test testnet -- --ignored --nocapture`.

#![allow(clippy::unwrap_used)]

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use loams_walrus::{ContentDigest, StoreOpts, Walrus, WalrusConfig};

#[tokio::test]
#[ignore = "network: Walrus Testnet; set LOAMS_TEST_WALRUS_TESTNET=1"]
async fn testnet_round_trip() {
    if std::env::var("LOAMS_TEST_WALRUS_TESTNET").as_deref() != Ok("1") {
        eprintln!("skipped: LOAMS_TEST_WALRUS_TESTNET is not 1");
        return;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let cfg = WalrusConfig {
        request_timeout_ms: 120_000,
        ..WalrusConfig::testnet()
    };
    let w = Walrus::new(&cfg).await.unwrap();
    let opts = StoreOpts {
        epochs: Some(1),
        permanent: false,
    };

    // A blob: store (which reads back verified), read again, range read.
    let data = Bytes::from(format!("loams-walrus testnet probe {nanos}"));
    let stored = w.store(data.clone(), opts).await.unwrap();
    eprintln!(
        "blob {} end_epoch {} new {}",
        stored.blob_id, stored.end_epoch, stored.newly_created
    );
    let got = w
        .read(&stored.blob_id, &ContentDigest::of(&data))
        .await
        .unwrap();
    assert_eq!(got, data);
    let head = w.read_range(&stored.blob_id, 0..6).await.unwrap();
    assert_eq!(&head.into_unverified_inner()[..], b"loams-");

    // A quilt: store (which reads every patch back), read by identifier,
    // list the patches.
    let a = Bytes::from(format!("first patch {nanos}"));
    let b = Bytes::from(format!("second patch {nanos}"));
    let quilt = w
        .store_quilt(
            vec![("a.txt".into(), a.clone()), ("b.txt".into(), b.clone())],
            opts,
        )
        .await
        .unwrap();
    eprintln!("quilt {} patches {:?}", quilt.blob.blob_id, quilt.patches);
    let got = w
        .read_quilt_patch(&quilt.blob.blob_id, "b.txt", &ContentDigest::of(&b))
        .await
        .unwrap();
    assert_eq!(got, b);
    let listed = w.list_patches(&quilt.blob.blob_id).await.unwrap();
    let mut names: Vec<_> = listed.iter().map(|p| p.identifier.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.txt", "b.txt"]);
}
