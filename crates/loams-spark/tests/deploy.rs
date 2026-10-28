//! The checked-in proxy configurations parse and pass the proxy's checks:
//! engine-gateway's `[spark]` section, the Apache Spark compose proxy, the
//! Kubernetes one and the documented example.

use std::path::PathBuf;

fn repo(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn engine_gateway_spark_section_is_valid() {
    // gateway.toml has other modules' sections; read only `[spark]`.
    let doc: toml::Table = toml::from_str(&repo("deploy/release/engines/gateway.toml")).unwrap();
    let spark: loams_spark::standalone::Config = doc["spark"].clone().try_into().unwrap();
    loams_spark::standalone::check(&spark).unwrap();
    assert_eq!(spark.listen.port(), 15002);
    assert_eq!(spark.routes.len(), 1);
    // The namespace engine-gateway writes its key line for (compose.lake.yaml).
    assert_eq!(spark.routes[0].namespace, "default");
    assert!(spark.namespace_keys_file.is_some());
}

#[test]
fn compose_jvm_proxy_config_is_valid() {
    let c = loams_spark::standalone::load(&repo("deploy/release/spark/proxy-spark.toml")).unwrap();
    assert_eq!(c.routes[0].namespace, "default");
    assert_eq!(
        c.routes[0].token_env.as_deref(),
        Some("SPARK_CONNECT_AUTHENTICATE_TOKEN")
    );
}

#[test]
fn example_and_kubernetes_configs_are_valid() {
    loams_spark::standalone::load(&repo("deploy/release/spark/proxy.toml.example")).unwrap();
    loams_spark::standalone::load(&repo("deploy/spark/k8s/proxy.toml")).unwrap();
}
