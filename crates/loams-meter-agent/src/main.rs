//! The `loams-meter-agent` binary: the node metering agent's loop.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use loams_meter_agent::cgroup::DEFAULT_TARGETS;
use loams_meter_agent::final_read::FinalReads;
use loams_meter_agent::host::Sessions;
use loams_meter_agent::host::server::{self, HostEvent, HostServerConfig};
use loams_meter_agent::pods::{PodIndex, PodResolver};
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
    /// A file holding an operator API key, for a single node's main port
    /// (the meter's service ingest admits the operator there). Not needed
    /// on a cluster listener.
    #[arg(long, env = "LOAMS_METER_AGENT_API_KEY_FILE")]
    api_key_file: Option<PathBuf>,
    /// The meter stream's partition count (§63 §7.1: 32).
    #[arg(long, default_value_t = 32)]
    partitions: u32,
    /// The cluster named in each event's `source`
    /// (`loams://<cluster>/meter-agent/<node>`).
    #[arg(long, env = "LOAMS_CLUSTER", default_value = "local")]
    cluster: String,
    /// The host-report socket for T1 wasmtime hosts (design §63 §21,
    /// platform doc 06). Off unless set; `/run/loams/meter.sock` in the
    /// DaemonSet chart.
    #[arg(long, env = "LOAMS_METER_HOST_SOCKET")]
    host_socket: Option<PathBuf>,
    /// Cgroup patterns a reporting host must run in (repeatable). Default:
    /// `wasm-host.scope`.
    #[arg(long = "host-cgroup")]
    host_cgroups: Vec<String>,
    /// Resolve pod cgroups from the node's pods' `loams.dev/*` labels,
    /// watched from the Kubernetes API (in-cluster config).
    #[arg(long, env = "LOAMS_METER_KUBE")]
    kube: bool,
    /// Scan as soon as a cgroup's `cgroup.events` says it is empty
    /// (inotify), before it is removed.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    final_reads: bool,
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
    let fallback = match &cli.tenants {
        Some(path) => StaticResolver::from_file(path)?,
        None => StaticResolver::default(),
    };
    let pods = Arc::new(RwLock::new(PodIndex::new()));
    if cli.kube {
        let client = kube::Client::try_default()
            .await
            .map_err(|e| MeterError::Config(format!("kubernetes client: {e}")))?;
        tokio::spawn(loams_meter_agent::pods::watch(
            client,
            cli.node.clone(),
            pods.clone(),
        ));
        info!("resolving pods from the Kubernetes API");
    }
    let resolver = PodResolver::new(pods, fallback);
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
            config.cluster = cli.cluster;
            if let Some(path) = &cli.api_key_file {
                let key = std::fs::read_to_string(path).map_err(|e| {
                    MeterError::Config(format!("--api-key-file {}: {e}", path.display()))
                })?;
                config.api_key = Some(key.trim().to_string());
            }
            Some(StreamSink::new(config)?)
        }
        None => None,
    };
    info!(
        node = agent.node(),
        next_seq = agent.next_seq(),
        "loams-meter-agent started"
    );

    // The T1 source: hosts report on the socket; the server hands what is
    // new to this loop, which owns the spool.
    let (host_tx, mut host_rx) = tokio::sync::mpsc::channel::<HostEvent>(1024);
    if let Some(path) = &cli.host_socket {
        let listener = server::bind(path)
            .map_err(|e| MeterError::Config(format!("host socket {}: {e}", path.display())))?;
        let mut config = HostServerConfig::default();
        if !cli.host_cgroups.is_empty() {
            config.allowed_cgroups = cli.host_cgroups.clone();
        }
        agent.set_host_reports(true);
        let sessions = Arc::new(Mutex::new(Sessions::new()));
        tokio::spawn(server::serve(listener, Arc::new(config), sessions, host_tx));
        info!(socket = %path.display(), "host-report socket open");
    } else {
        drop(host_tx);
    }

    let mut interval = tokio::time::interval(Duration::from_millis(cli.interval_ms.max(1)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut host_open = cli.host_socket.is_some();
    let (final_tx, mut final_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut final_reads = if cli.final_reads {
        match FinalReads::new(&cli.cgroup_root, final_tx) {
            Ok(reads) => Some(reads),
            Err(e) => {
                warn!(error = %e, "no final reads; the periodic scan is the floor");
                None
            }
        }
    } else {
        drop(final_tx);
        None
    };
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let report = agent.tick(now_ms())?;
                tracing::debug!(?report, "tick");
                if let Some(reads) = final_reads.as_mut() {
                    reads.sync(agent.scanned());
                }
                if let Some(sink) = sink.as_mut() {
                    drain(&mut agent, sink).await?;
                }
                if cli.once {
                    return Ok(());
                }
            }
            finished = final_rx.recv(), if final_reads.is_some() => {
                if let Some(cgroup) = finished {
                    tracing::debug!(cgroup = %cgroup, "final read");
                    let report = agent.tick(now_ms())?;
                    tracing::debug!(?report, "final-read tick");
                }
            }
            event = host_rx.recv(), if host_open => match event {
                Some(event) => on_host_event(&mut agent, event, now_ms()),
                None => host_open = false,
            },
        }
    }
}

/// Applies one host-socket request to the agent and answers it.
fn on_host_event<R: loams_meter_agent::TenantResolver>(
    agent: &mut Agent<R>,
    event: HostEvent,
    now_ms: i64,
) {
    match event {
        HostEvent::Reports {
            host_id,
            seq,
            invocations,
            done,
        } => {
            let result = agent
                .record_host_report(&host_id, seq, &invocations)
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = done.send(result);
        }
        HostEvent::Gap { done, .. } => {
            let _ = done.send(agent.record_gap().map_err(|e| e.to_string()));
        }
        HostEvent::FinalRead { cgroup, done } => {
            // A full scan reads the finished cgroup with every other one,
            // before the host removes it (doc 06 §3.0).
            if let Err(e) = agent.tick(now_ms) {
                warn!(cgroup = %cgroup, error = %e, "final read failed");
            }
            let _ = done.send(());
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
