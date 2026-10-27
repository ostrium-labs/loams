//! The `spark` module (design 68 §7.4, D1263; design 81 R8; SP-a D1992):
//! the Spark Connect proxy on its own listener
//! (`loams-gateway --module spark --config spark.toml`). It is the same
//! proxy the standalone `loams-spark-proxy` binary serves; the configuration
//! is `deploy/release/spark/proxy.toml.example`.
//!
//! The proxy authenticates the Loams API key, routes to the namespace's
//! Spark Connect server (Sail or Apache Spark), enforces session limits and
//! idle timeouts, and forwards Spark Connect bytes; it parses no plans.

pub use loams_spark::standalone::{Config, check, dev_config};

/// Reads a `spark` module configuration file.
pub fn load(text: &str) -> Result<Config, String> {
    loams_spark::standalone::load(text)
}

/// Serves the Spark Connect proxy until `shutdown`.
pub async fn serve(
    config: Config,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    loams_spark::standalone::run(config, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_config_rejects_dev_settings_on_public_listener() {
        let text = r#"
listen = "0.0.0.0:15002"
insecure_dev = true
dev_namespace = "local"
[[routes]]
namespace = "local"
instance = "spark"
uri = "http://sail:50051"
"#;
        assert!(load(text).unwrap_err().contains("loopback"));
    }
}
