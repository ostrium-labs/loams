//! The `loams-meter-agent` binary: the node metering agent's loop.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use loams_meter_agent::cgroup::DEFAULT_TARGETS;
use loams_meter_agent::{
    Agent, CgroupReader, MeterError, Sink, Spool, SpoolConfig, StaticResolver, StreamSink,
    StreamSinkConfig, forward,
};
use tracing::{error, info, warn};

#[derive(Debug, Parser)]
#[command(
    name = "loams-meter-agent",
    version,
    about = "Loams Cloud's node metering agent"
)]
struct Cli {
    /// This node's name (the `<node>` of every idempotency key).
    #[arg(long, env = "NODE_NAME")]
    node: String,
    /// The cgroup v2 mount.
    #[arg(long, default_value = "/sys/fs/cgroup")]
    cgroup_root: PathBuf,
    /// Subtrees to walk, relative to the root (repeatable; default: all).
    #[arg(long = "scope")]
    scopes: Vec<String>,
    /// Directory-name patterns of the cgroups to meter (repeatable; `*` is
    /// a wildcard). Default: Kubernetes pod cgroups, `tenant-*.slice` and
    /// `wasm-host.scope`.
    #[arg(long = "target")]
    targets: Vec<String>,
    /// Static tenant rules (JSON: `{"rules": [{"prefix", "org",
    /// "namespace", "function"?, "tier"?}]}`).
    #[arg(long)]
    tenants: Option<PathBuf>,
    /// The spool directory (a hostPath in clusters).
    #[arg(long, default_value = "/var/lib/loams-meter/spool")]
    spool_dir: PathBuf,
    /// The spool's size bound in bytes.
    #[arg(long, default_value_t = 256 << 20)]
    spool_max_bytes: u64,
    /// The state file. Default: `state.json` next to the spool.
    #[arg(long)]
    state_file: Option<PathBuf>,
    /// Milliseconds between scans.
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
    /// The Loams engine's HTTP base URL. Without it, events stay spooled.
    #[arg(long)]
    stream_url: Option<String>,
    /// The meter stream's namespace.
    #[arg(long, default_value = "_system")]
    stream_namespace: String,
    /// The meter stream.
    #[arg(long, default_value = "_meter")]
    stream: String,
    /// The meter stream's partition count.
    #[arg(long, default_value_t = 16)]
    partitions: u32,
    /// Scan once, forward once, and exit.
    #[arg(long)]
    once: bool,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

async fn run(cli: Cli) -> Result<(), MeterError> {
    let targets = if cli.targets.is_empty() {
        DEFAULT_TARGETS.iter().map(|t| t.to_string()).collect()
    } else {
        cli.targets
    };
    let reader = CgroupReader::new(&cli.cgroup_root, cli.scopes, targets);
    let resolver = match &cli.tenants {
        Some(path) => StaticResolver::from_file(path)?,
        None => StaticResolver::default(),
    };
    let config = SpoolConfig {
        max_bytes: cli.spool_max_bytes,
        ..SpoolConfig::default()
    };
    let spool = Spool::open(&cli.spool_dir, config)?;
    let state_file = cli
        .state_file
        .unwrap_or_else(|| cli.spool_dir.join("state.json"));
    let mut agent = Agent::open(cli.node, state_file, reader, resolver, spool, now_ms())?;
    let mut sink = match cli.stream_url {
        Some(url) => {
            let mut config = StreamSinkConfig::new(url, cli.partitions);
            config.namespace = cli.stream_namespace;
            config.stream = cli.stream;
            Some(StreamSink::new(config)?)
        }
        None => None,
    };
    info!(
        node = agent.node(),
        next_seq = agent.next_seq(),
        "loams-meter-agent started"
    );

    let mut interval = tokio::time::interval(Duration::from_millis(cli.interval_ms.max(1)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let report = agent.tick(now_ms())?;
        tracing::debug!(?report, "tick");
        if let Some(sink) = sink.as_mut() {
            drain(&mut agent, sink).await?;
        }
        if cli.once {
            return Ok(());
        }
    }
}

/// Forwards spooled events until the spool is drained or the sink fails.
///
/// A rejected batch (`SinkRejected`: the stream answered a 4xx other than
/// 408 or 429) would be retried forever from the spool head, so it stops
/// the agent for an operator: the events stay spooled and the baselines
/// saved, the process exits non-zero, and the DaemonSet's restarts
/// (`CrashLoopBackOff`) are the alert. Transient failures leave the events
/// spooled for the next tick.
async fn drain<R: loams_meter_agent::TenantResolver>(
    agent: &mut Agent<R>,
    sink: &mut dyn Sink,
) -> Result<(), MeterError> {
    loop {
        match forward(agent.spool_mut(), sink, 500).await {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(MeterError::SinkUnavailable(e)) => {
                warn!(error = %e, "stream unavailable; events stay spooled");
                return Ok(());
            }
            Err(e @ MeterError::SinkRejected(_)) => {
                error!(
                    error = %e,
                    "the stream rejected the batch at the spool head; events stay spooled, \
                     stopping for operator action"
                );
                return Err(e);
            }
            Err(e) => {
                error!(error = %e, "forwarding failed; events stay spooled");
                return Ok(());
            }
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "loams-meter-agent stopped");
            ExitCode::FAILURE
        }
    }
}
