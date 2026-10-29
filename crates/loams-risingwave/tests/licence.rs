//! D1901 (ENG81 R4): no Loams-run RisingWave uses the licence key built
//! into RisingWave's release images. Every path that starts RisingWave
//! (the controller's renders, the release compose bundle, the Helm chart,
//! the stream bench) points the meta node at an empty licence key file.
//!
//! The render itself is checked by `render::tests::licence_key_is_always_the_empty_file`.
//! These tests check the shipped files, and `live_cluster_refuses_premium`
//! checks a running cluster:
//!
//! ```sh
//! touch /tmp/empty.key   # any empty file
//! podman run -d --rm --name rw-licence -p 127.0.0.1:14566:4566 --cpus 2 --memory 8g \
//!   -v /tmp/empty.key:/risingwave/config/license.key:ro \
//!   -e RW_LICENSE_KEY_PATH=/risingwave/config/license.key \
//!   docker.io/risingwavelabs/risingwave:v3.1.0@sha256:ee2e8e7a10b728d01b0e149d2a4f35864f0f52a0c889fa6cf9bc32c8ff46c602 \
//!   single_node
//! LOAMS_RW_LICENCE_URL='host=127.0.0.1 port=14566 user=root dbname=dev' \
//!   cargo test -p loams-risingwave --test licence -- --ignored
//! ```

use loams_risingwave::render::{CONFIG_PATH, LICENSE_KEY_FILE, LICENSE_KEY_PATH, config_toml};

const RELEASE_COMPOSE: &str = include_str!("../../../deploy/release/compose.risingwave.yaml");
const RELEASE_KEY: &str = include_str!("../../../deploy/release/risingwave/license.key");
const RELEASE_CONFIG: &str = include_str!("../../../deploy/release/risingwave/risingwave.toml");
const HELM_CONFIGMAP: &str =
    include_str!("../../../deploy/helm/loams-risingwave/templates/configmap.yaml");
const HELM_WORKLOADS: &str =
    include_str!("../../../deploy/helm/loams-risingwave/templates/workloads.yaml");
const HELM_VALUES: &str = include_str!("../../../deploy/helm/loams-risingwave/values.yaml");
const BENCH_COMPOSE: &str = include_str!("../../../bench/stream/compose.yaml");

/// Lines that set a licence key directly (not through the key file).
fn sets_a_key(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter(|l| {
            l.starts_with("RW_LICENSE_KEY:")
                || l.starts_with("RW_LICENSE_KEY=")
                || l.contains("\"RW_LICENSE_KEY\"")
                || l.contains("--license-key ")
                || l.contains("--license-key=")
                || l.starts_with("license_key")
                || l.to_ascii_lowercase().starts_with("licensekey")
        })
        .collect()
}

#[test]
fn the_key_file_is_empty_and_mounted_next_to_the_config() {
    assert_eq!(LICENSE_KEY_FILE, "");
    assert_eq!(
        RELEASE_KEY, "",
        "deploy/release/risingwave/license.key must stay empty"
    );
    let dir = |p: &'static str| p.rsplit_once('/').map(|(d, _)| d);
    assert_eq!(dir(LICENSE_KEY_PATH), dir(CONFIG_PATH));
}

#[test]
fn release_compose_locks_the_licence() {
    assert!(
        RELEASE_COMPOSE.contains(&format!("RW_LICENSE_KEY_PATH: {LICENSE_KEY_PATH}")),
        "the meta node must read the empty key file"
    );
    assert!(RELEASE_COMPOSE.contains(&format!("./risingwave/license.key:{LICENSE_KEY_PATH}:ro")));
    assert_eq!(sets_a_key(RELEASE_COMPOSE), Vec::<&str>::new());
}

#[test]
fn release_config_is_the_rendered_one_and_names_no_key() {
    assert_eq!(RELEASE_CONFIG, config_toml(false));
    assert!(!RELEASE_CONFIG.contains("license"));
    let v: toml::Value = toml::from_str(RELEASE_CONFIG).unwrap();
    assert!(v.get("system").is_none());
}

#[test]
fn helm_chart_mounts_an_empty_key_that_values_cannot_change() {
    assert!(HELM_CONFIGMAP.contains("  license.key: \"\"\n"));
    assert!(!HELM_CONFIGMAP.contains(".Values.license"));
    // The ConfigMap is mounted as the whole config directory.
    assert!(HELM_WORKLOADS.contains("mountPath: /risingwave/config"));
    assert_eq!(sets_a_key(HELM_VALUES), Vec::<&str>::new());
    assert_eq!(sets_a_key(HELM_WORKLOADS), Vec::<&str>::new());
}

#[test]
fn stream_bench_runs_the_free_tier_too() {
    assert!(BENCH_COMPOSE.contains(&format!("RW_LICENSE_KEY_PATH: {LICENSE_KEY_PATH}")));
    assert!(BENCH_COMPOSE.contains(&format!(
        "deploy/release/risingwave/license.key:{LICENSE_KEY_PATH}:ro"
    )));
    assert_eq!(sets_a_key(BENCH_COMPOSE), Vec::<&str>::new());
}

/// A running cluster set up like a managed one refuses Premium features,
/// and cannot be switched back to the built-in trial key.
#[tokio::test]
#[ignore = "needs a RisingWave at LOAMS_RW_LICENCE_URL (see the module docs)"]
async fn live_cluster_refuses_premium() {
    let url = std::env::var("LOAMS_RW_LICENCE_URL").expect("LOAMS_RW_LICENCE_URL");
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(conn);

    let row = client
        .query_one("SELECT rw_license()::varchar", &[])
        .await
        .unwrap();
    let licence: String = row.get(0);
    let licence: serde_json::Value = serde_json::from_str(&licence).unwrap();
    assert_eq!(licence["tier"], "free", "{licence}");

    // rw_test_paid_tier() is deprecated in favour of rw_test_feature(), and
    // both exist in v3.1.0.
    for sql in [
        "SELECT rw_test_paid_tier()",
        "SELECT rw_test_feature('SecretManagement')",
        "SELECT rw_test_feature('TimeTravel')",
        "SELECT rw_test_feature('ResourceGroup')",
        "SELECT rw_test_feature('DatabaseFailureIsolation')",
        "SELECT rw_test_feature('CdcAutoSchemaChange')",
    ] {
        assert!(client.simple_query(sql).await.is_err(), "{sql} succeeded");
    }
    let available = client
        .query("SELECT name FROM rw_features WHERE available", &[])
        .await
        .unwrap();
    let names: Vec<String> = available.iter().map(|r| r.get(0)).collect();
    assert!(names.is_empty(), "Premium features available: {names:?}");

    for sql in [
        "ALTER SYSTEM SET license_key TO DEFAULT",
        "ALTER SYSTEM SET license_key TO 'x'",
    ] {
        let err = client.simple_query(sql).await.expect_err(sql);
        assert!(
            format!("{err:?}").contains("license-key-path"),
            "{sql}: {err:?}"
        );
    }
}
