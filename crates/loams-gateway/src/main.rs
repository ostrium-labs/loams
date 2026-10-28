//! The `loams-gateway` binary (see the library's docs).

use std::path::PathBuf;

use clap::Parser;
use loams_gateway::{Gateway, GatewayConfig};

/// Loams' gateway: the Postgres front door in front of each namespace's
/// PgDog (design §57).
#[derive(Parser)]
#[command(name = "loams-gateway", version)]
struct Args {
    /// The static config (gateway.toml, or the `temporal` module's file).
    #[arg(long, default_value = "/etc/loams-gateway/gateway.toml")]
    config: PathBuf,
    /// Which module this process runs: `pg` (the Postgres front door) or,
    /// with the `temporal` feature, `temporal` (its own Deployment, §74 §12),
    /// or with the `spark` feature, `spark` (the Spark Connect proxy), or
    /// `browser` (agents' CDP sessions to the browser service, D2118).
    #[arg(long, default_value = "pg")]
    module: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let text = std::fs::read_to_string(&args.config)
        .map_err(|e| format!("reading {}: {e}", args.config.display()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match args.module.as_str() {
        "pg" => {}
        "browser" => {
            let config = loams_gateway::browser::load(&text)?;
            return runtime.block_on(async move {
                loams_gateway::browser::serve(config, terminate())
                    .await
                    .map_err(Into::into)
            });
        }
        #[cfg(feature = "temporal")]
        "temporal" => {
            let config = loams_gateway::temporal::load(&text)?;
            return runtime.block_on(async move {
                loams_gateway::temporal::serve(config, terminate())
                    .await
                    .map_err(Into::into)
            });
        }
        #[cfg(feature = "spark")]
        "spark" => {
            let config = loams_gateway::spark::load(&text)?;
            return runtime.block_on(async move {
                loams_gateway::spark::serve(config, terminate())
                    .await
                    .map_err(Into::into)
            });
        }
        other => {
            return Err(format!(
                "unknown module {other:?} (built: pg, browser{}{})",
                if cfg!(feature = "temporal") {
                    ", temporal"
                } else {
                    ""
                },
                if cfg!(feature = "spark") {
                    ", spark"
                } else {
                    ""
                }
            )
            .into());
        }
    }
    let config = GatewayConfig::from_toml(&text)?;
    runtime.block_on(async move {
        let gateway = Gateway::start(config).await?;
        terminate().await;
        tracing::info!("shutting down: draining live connections");
        gateway.shutdown().await;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

/// SIGTERM or Ctrl-C.
async fn terminate() {
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
