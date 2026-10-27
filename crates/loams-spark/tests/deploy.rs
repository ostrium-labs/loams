//! The checked-in proxy configurations parse and pass the proxy's checks
//! (compose.spark.yaml's two, the Kubernetes one and the documented example).

use std::path::PathBuf;

fn repo(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn compose_proxy_configs_are_valid() {
    for f in ["proxy-sail.toml", "proxy-spark.toml"] {
        let c = loams_spark::standalone::load(&repo(&format!("deploy/release/spark/{f}")))
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        assert_eq!(c.routes.len(), 1, "{f}");
        assert_eq!(c.routes[0].namespace, "local");
    }
    let spark =
        loams_spark::standalone::load(&repo("deploy/release/spark/proxy-spark.toml")).unwrap();
    assert_eq!(
        spark.routes[0].token_env.as_deref(),
        Some("SPARK_CONNECT_AUTHENTICATE_TOKEN")
    );
}

#[test]
fn example_and_kubernetes_configs_are_valid() {
    loams_spark::standalone::load(&repo("deploy/release/spark/proxy.toml.example")).unwrap();
    loams_spark::standalone::load(&repo("deploy/spark/k8s/proxy.toml")).unwrap();
}

#[test]
fn compose_keys_file_is_well_formed() {
    let v: serde_json::Value =
        serde_json::from_str(&repo("deploy/release/spark/keys.json")).unwrap();
    assert!(v["keys"].is_array());
}
