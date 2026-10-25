//! `loams-temporal`: the Temporal gateway as a standalone binary (Q1151,
//! D1539): the same frontend `loams-gateway`'s `temporal` module mounts,
//! with nothing from Loams, so it can serve any Resonate server.
//!
//! ```text
//! loams-temporal --dev                  # memory store, in-process Resonate, 127.0.0.1:7233
//! loams-temporal --config gateway.toml  # see deploy/release/temporal-gateway.toml.example
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use loams_temporal_frontend::standalone::{Config, dev_config, run};

#[derive(Parser, Debug)]
#[command(
    name = "loams-temporal",
    version,
    about = "The Loams Temporal gateway (design 74)"
)]
struct Cli {
    /// A TOML configuration file.
    #[arg(long, conflicts_with = "dev")]
    config: Option<PathBuf>,
    /// Development mode: memory store, in-process Resonate, no auth,
    /// loopback only, namespace `default`. Nothing survives a restart.
    #[arg(long)]
    dev: bool,
    /// Override the listen address.
    #[arg(long)]
    listen: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let mut config: Config = if cli.dev {
        dev_config()
    } else {
        let path = cli.config.ok_or("give --config FILE or --dev")?;
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
    };
    if let Some(l) = cli.listen {
        config.listen = l;
    }
    run(config, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
