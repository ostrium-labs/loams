//! Loopback-only oracle server; deliberately excluded from release images.
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};
#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:28126")]
    listen: SocketAddr,
    /// Directory published by `loams-house-mine oracle --fixtures`.
    #[arg(long)]
    fixtures: Option<PathBuf>,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let listener = loams_house_engine::http::bind(args.listen).await?;
    eprintln!(
        "House engine oracle listening on {}",
        listener.local_addr()?
    );
    loams_house_engine::http::serve(listener, args.fixtures).await
}
