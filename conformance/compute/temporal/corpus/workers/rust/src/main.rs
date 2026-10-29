//! corpus-rust: the Temporal Rust SDK half of the history corpus (lane
//! TSDK-rust, D2155-D2159).
//!
//!   corpus-rust generate <address> <namespace> <out.json> [scenario,...]
//!   corpus-rust replay [dir]        # default $CORPUS_HISTORIES, else ../../histories/rust
//!   corpus-rust scenarios           # the scenarios this worker runs, comma-separated
//!
//! `generate` runs a worker for every corpus workflow on the queue
//! `corpus-rust` and drives each scenario with the workflow ID
//! `corpus-rust-<scenario>`; the Go exporter writes the histories after it.
//! CORPUS_REGISTER=1 first registers the namespace and the keyword attribute
//! `CorpusKeyword` (upstream Temporal; the gateway's leg leaves it unset).
//! TEMPORAL_API_KEY, when set, is sent as a bearer token in plain text.
//!
//! `replay` checks every history with sdk-core's replayer (replay.rs).

mod activities;
mod drive;
mod replay;
mod scenarios;
mod workflows;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

const USAGE: &str = "usage: corpus-rust generate <address> <namespace> <out.json> [scenarios] | replay [dir] | scenarios";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("generate") if (4..=5).contains(&args.len()) => {
            let register = std::env::var("CORPUS_REGISTER").is_ok_and(|v| v == "1");
            drive::generate(
                &args[1],
                &args[2],
                Path::new(&args[3]),
                args.get(4).map(String::as_str),
                register,
            )
            .await
        }
        Some("replay") if args.len() <= 2 => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("CORPUS_HISTORIES").map(PathBuf::from))
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../histories/rust")
                });
            replay::replay(&dir).await
        }
        Some("scenarios") => {
            let names: Vec<&str> = scenarios::SCENARIOS.iter().map(|(n, _)| *n).collect();
            println!("{}", names.join(","));
            Ok(())
        }
        _ => bail!(USAGE),
    }
}
