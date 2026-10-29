//! The `temporal` module (design 74 §5.7, D1504; plan DX1 Task 9): the
//! Temporal API on its own listener, run as its own Deployment
//! (`loams-gateway --module temporal --config temporal.toml`). It is the
//! same frontend the standalone `loams-temporal` binary serves; the
//! configuration is `deploy/release/temporal-gateway.toml.example`.
//!
//! Metrics carry no tenant labels (D1021, D1532).

pub use loams_temporal_frontend::standalone::{Config, check, dev_config};

/// Reads a `temporal` module configuration file.
pub fn load(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    check(&config)?;
    Ok(config)
}

/// Serves the Temporal API until `shutdown`.
pub async fn serve(
    config: Config,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    loams_temporal_frontend::standalone::run(config, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_config_rejects_dev_store_on_public_listener() {
        let text = r#"
listen = "0.0.0.0:7233"
insecure_dev = true
dev_loams_namespace = "local"
[store]
kind = "memory"
[durability]
kind = "in_process"
"#;
        assert!(load(text).unwrap_err().contains("loopback"));
    }
}
