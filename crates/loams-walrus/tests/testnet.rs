//! A round trip through Walrus Testnet's public publisher and aggregator:
//! store, read back, range read, quilt store, patch reads, patch listing.
//! Ignored by default (network); run with
//! `LOAMS_TEST_WALRUS_TESTNET=1 cargo test -p loams-walrus --test testnet -- --ignored --nocapture`.

#![allow(clippy::unwrap_used)]

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use loams_walrus::{ContentDigest, StoreOpts, Walrus, WalrusConfig, WalrusError};

/// Unwraps a Testnet result; a blob still `404` after the read-after-write
/// window (180 s by default) is named as Testnet aggregator lag.
fn lagging<T>(r: Result<T, WalrusError>) -> T {
    match r {
        Ok(v) => v,
        Err(WalrusError::NotFound(blob)) => panic!(
            "Walrus Testnet aggregator lag: blob {blob} was stored but is still 404 \
             after the {} ms read-after-write window",
            WalrusConfig::default().read_after_write_ms
        ),
        Err(e) => panic!("{e}"),
    }
}

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
    // The default read-after-write window (180 s, ruling 1 of PR #1232).
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
    let stored = lagging(w.store(data.clone(), opts).await);
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
    let listed = lagging(w.list_patches(&quilt.blob.blob_id).await);
    let mut names: Vec<_> = listed.iter().map(|p| p.identifier.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.txt", "b.txt"]);
}
