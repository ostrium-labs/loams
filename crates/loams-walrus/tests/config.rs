//! The `[walrus]` section.

#![allow(clippy::unwrap_used)]

use loams_walrus::{Network, Walrus, WalrusConfig, WalrusError};

#[derive(serde::Deserialize)]
struct File {
    walrus: WalrusConfig,
}

#[test]
fn plan_example_parses() {
    // The section as MS1's plan writes it.
    let file: File = toml::from_str(
        r#"
        [walrus]
        enabled = false
        network = "testnet"
        aggregators = ["https://aggregator.walrus-testnet.walrus.space"]
        publisher = "https://publisher.walrus-testnet.walrus.space"
        publisher_auth = ""
        epochs = 5
        hedge_after_ms = 300
        request_timeout_ms = 30000
        max_blob_bytes = 10485760
        cache_dir = ""
        cache_max_bytes = 4294967296
        "#,
    )
    .unwrap();
    let w = file.walrus;
    assert!(!w.enabled);
    assert_eq!(w.network, Network::Testnet);
    assert_eq!(w.publisher_auth, None);
    assert_eq!(w.cache_dir, None);
    assert_eq!(w.max_blob_bytes, 10 << 20);
    assert_eq!(w.cache_max_bytes, 4 << 30);
    assert_eq!(w.hedge_after().as_millis(), 300);
    // Not in the plan's example: ruling 1 of PR #1232 added it.
    assert_eq!(w.read_after_write_ms, 180_000);
}

#[test]
fn empty_section_is_the_default() {
    let file: File = toml::from_str("[walrus]\n").unwrap();
    assert_eq!(file.walrus, WalrusConfig::default());
}

#[test]
fn mainnet_section_has_no_publisher() {
    let file: File = toml::from_str("[walrus]\nenabled = true\nnetwork = \"mainnet\"\n").unwrap();
    assert_eq!(file.walrus.effective_publisher(), None);
    assert_eq!(
        file.walrus.effective_aggregators()[0].as_str(),
        "https://aggregator.walrus-mainnet.walrus.space/"
    );
}

#[test]
fn unknown_keys_are_refused() {
    assert!(toml::from_str::<File>("[walrus]\nenable = true\n").is_err());
}

#[test]
fn data_dir_fills_the_cache_dir() {
    let cfg = WalrusConfig::default().with_data_dir(std::path::Path::new("/data"));
    assert_eq!(
        cfg.cache_dir.as_deref(),
        Some(std::path::Path::new("/data/walrus-cache"))
    );
}

#[tokio::test]
async fn zero_epochs_or_empty_token_file_is_a_config_error() {
    let cfg = WalrusConfig {
        epochs: 0,
        ..WalrusConfig::testnet()
    };
    assert!(matches!(
        Walrus::new(&cfg).await.unwrap_err(),
        WalrusError::Config(_)
    ));
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "\n").unwrap();
    let cfg = WalrusConfig {
        publisher_auth: Some(token),
        ..WalrusConfig::testnet()
    };
    assert!(matches!(
        Walrus::new(&cfg).await.unwrap_err(),
        WalrusError::Config(_)
    ));
}
