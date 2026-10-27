//! The `resp` module (design §77 §8, §10; plan KV2 Task 2, ruling K2-2):
//! the Redis gateway on its own listener, run as its own Deployment
//! (`loams-gateway --module resp --config resp.toml`). The configuration is
//! `deploy/release/resp-gateway.toml.example`.

pub use loams_resp::config::RespConfig as Config;

/// Reads a `resp` module configuration file.
pub fn load(text: &str) -> Result<Config, String> {
    Config::from_toml(text)
}

/// Serves the Redis protocol until `shutdown`.
pub async fn serve(
    config: Config,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    let server = loams_resp::Server::bind(config).await?;
    tracing::info!(listen = %server.local_addr(), "resp: serving");
    server.serve(shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_config_rejects_public_listener_without_auth() {
        let text = "listen = \"0.0.0.0:6379\"\n[store]\nkind = \"memory\"\nname = \"x\"\n";
        assert!(load(text).unwrap_err().contains("requirepass"));
    }
}
