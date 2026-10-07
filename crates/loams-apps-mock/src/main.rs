//! `loams-apps-mock [--listen 127.0.0.1:8084] [--heartbeat-secs 15] [--signed-out]`
//!
//! Serves the app protos with seed data until Ctrl-C. Fake tokens:
//! `Authorization: Bearer mock-access-usr_omar` (a fresh session) or
//! `mock-stale-usr_omar` (a session past the 5-minute step-up window).
//!
//! The console's `/api/v1/*` REST contract is served on the same listener,
//! so the React console, the desktop, the phone apps and the SDKs all point
//! at this one address.

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use loams_apps_mock::{DEFAULT_LISTEN, MockConfig, Seed, serve};

#[derive(Debug, Parser)]
#[command(about = "A mock of the Loams app protos and the console REST contract (design §37, AP0)")]
struct Args {
    /// Loopback address to listen on.
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: SocketAddr,
    /// Heartbeat period of every watch stream, in seconds.
    #[arg(long, default_value_t = 15)]
    heartbeat_secs: u64,
    /// Answer the console's `GET /api/v1/session` with 401, to build the
    /// console's sign-in and setup screens.
    #[arg(long)]
    signed_out: bool,
    /// The issuer the mock names when a device cannot reach loopback, for
    /// example `--public-url http://10.0.2.2:8084` for the Android emulator.
    /// Pairing payloads and the fake Authentik's discovery document use it.
    #[arg(long)]
    public_url: Option<String>,
    /// The console's build to serve at `/ui/`. Defaults to
    /// `web/apps/console/dist` if it has been built.
    #[arg(long)]
    ui_dir: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let handle = serve(MockConfig {
        listen: args.listen,
        seed: Seed::demo(),
        heartbeat: Duration::from_secs(args.heartbeat_secs.max(1)),
        console_signed_in: !args.signed_out,
        public_url: args.public_url,
        ui_dir: args.ui_dir,
    })
    .await?;
    println!("loams-apps-mock: {}", handle.url());
    println!(
        "  app protos: Connect, gRPC and gRPC-Web (loams.{{instance,devices,approvals,operations,notifications}}.v1)"
    );
    println!(
        "  console:    /api/v1/* (signed {})",
        if args.signed_out { "out" } else { "in" }
    );
    println!(
        "  auth:       POST /api/v1/oauth/token (pairing, exchange, refresh), /mock/authentik/..."
    );
    println!("  engine:     /health, /ready, /v1/namespaces/{{ns}}/collections");
    println!("  console UI: /ui/ (when web/apps/console/dist is built, or --ui-dir)");
    println!(
        "  controls:   /mock/pairing, /mock/approvals, /mock/drop-streams, /mock/tick, /mock/push-log"
    );
    println!("  tokens: Bearer mock-access-usr_omar (fresh), Bearer mock-stale-usr_omar (stale)");
    tokio::signal::ctrl_c().await?;
    handle.stop().await;
    Ok(())
}
