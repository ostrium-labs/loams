//! `loams-transfer`: the desktop's transfer sidecar and its dev server
//! (design §85, plan DQ1).
//!
//! - `serve`: the transfer API on a single-node metastore and a bucket,
//!   over HTTP/3 and HTTP/2+TLS on one port number.
//!
//! - `agent`: the desktop sidecar (JSON lines on stdio).
//! - `put` and `get`: one upload or download, resuming what an earlier run
//!   left, with progress as JSON lines.
//!
//! - `runner`: claims runs of the jobs in `jobs.toml`, runs them, and
//!   streams their logs and artifacts back.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use loams_store::Store;
use loams_transfer::TransfersConfig;
use loams_transfer::agent::ServerSpec;
use loams_transfer::api::Client;
use loams_transfer::client::{StateDir, Tracker, UploadOpts};
use loams_transfer::net::{Mode, ServerIdentity};
use loams_transfer::server::Access;
use loams_transfer::standalone::{self, Serve};

#[derive(Parser, Debug)]
#[command(
    name = "loams-transfer",
    version,
    about = "Resumable transfers and job streams over QUIC (design 85)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Serve the transfer API alone (tests, benchmarks, dev).
    Serve(ServeArgs),
    /// The desktop sidecar: JSON commands on stdin, JSON events on stdout.
    Agent {
        /// The state directory (`<userData>/transfers`).
        #[arg(long)]
        state: PathBuf,
    },
    /// Upload one file, resuming an unfinished upload of it.
    Put(PutArgs),
    /// Download one transfer, resuming a part file left beside the destination.
    Get(GetArgs),
    /// Run configured jobs for remote runs (CI and other jobs).
    Runner(RunnerArgs),
}

#[derive(clap::Args, Debug)]
struct RunnerArgs {
    #[command(flatten)]
    remote: Remote,
    /// The jobs this runner may run (`[jobs.<name>]` tables).
    #[arg(long)]
    jobs: PathBuf,
    /// This runner's name (default: a new id per process).
    #[arg(long)]
    runner_id: Option<String>,
}

async fn runner(args: RunnerArgs) -> Result<()> {
    let state = StateDir::new(&args.remote.state)?;
    let _lock = state.lock()?;
    let client = args.remote.client(&state)?;
    let jobs = loams_transfer::runner::JobsFile::load(&args.jobs)?;
    let mut opts = loams_transfer::runner::RunnerOpts::default();
    if let Some(id) = args.runner_id {
        opts.runner_id = id;
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        s2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    eprintln!(
        "runner {} serving jobs: {:?}",
        opts.runner_id,
        jobs.jobs.keys().collect::<Vec<_>>()
    );
    loams_transfer::runner::serve(&client, &state, &jobs, &opts, stop).await?;
    Ok(())
}

#[derive(clap::Args, Debug)]
struct ServeArgs {
    /// The address for UDP (HTTP/3) and TCP (HTTP/2+TLS).
    #[arg(long, default_value = "127.0.0.1:7790")]
    listen: SocketAddr,
    /// The metastore's directory; the bucket defaults to `<data-dir>/bucket`.
    #[arg(long)]
    data_dir: PathBuf,
    /// A `loams-store` URL for the bucket (`file:///…`, `s3://…`).
    #[arg(long)]
    store: Option<String>,
    /// PEM certificate chain; without it a self-signed certificate is made
    /// in the data directory and its SHA-256 printed for pinning.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// A file holding the bearer token (read, write and runner access).
    #[arg(long)]
    token_file: PathBuf,
    /// Serve HTTP/2+TLS only (no UDP), as when UDP is blocked.
    #[arg(long)]
    no_h3: bool,
    /// The smallest chunk size accepted (default 8 MiB).
    #[arg(long)]
    min_chunk: Option<u64>,
    /// Plain HTTP on this loopback address as well (a local engine's path).
    #[arg(long)]
    plain_listen: Option<SocketAddr>,
}

fn identity(args: &ServeArgs) -> Result<ServerIdentity> {
    if let (Some(c), Some(k)) = (&args.tls_cert, &args.tls_key) {
        return Ok(ServerIdentity::from_pem_files(c, k)?);
    }
    let (c, k) = (args.data_dir.join("tls.crt"), args.data_dir.join("tls.key"));
    if c.exists() && k.exists() {
        return Ok(ServerIdentity::from_pem_files(&c, &k)?);
    }
    let names = vec!["localhost".to_string(), args.listen.ip().to_string()];
    let (id, cert, key) = ServerIdentity::self_signed(&names)?;
    std::fs::write(&c, cert)?;
    write_private(&k, key.as_bytes())?;
    Ok(id)
}

fn write_private(path: &PathBuf, data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, data)?;
    Ok(())
}

async fn serve(args: ServeArgs) -> Result<()> {
    std::fs::create_dir_all(&args.data_dir)?;
    let token = std::fs::read_to_string(&args.token_file)
        .with_context(|| format!("read {}", args.token_file.display()))?
        .trim()
        .to_string();
    if token.len() < 16 {
        bail!("the token must be at least 16 characters");
    }
    let store = match &args.store {
        Some(url) => Store::from_url(url, std::iter::empty::<(String, String)>())?,
        None => {
            let dir = args.data_dir.join("bucket");
            std::fs::create_dir_all(&dir)?;
            let dir = dir.canonicalize()?;
            Store::from_url(
                &format!("file://{}", dir.display()),
                std::iter::empty::<(String, String)>(),
            )?
        }
    };
    let id = identity(&args)?;
    let (_node, meta) = standalone::start_meta(&args.data_dir.join("meta"), store.clone()).await?;
    let mut cfg = TransfersConfig::default();
    if let Some(m) = args.min_chunk {
        cfg.min_chunk = m;
    }
    let api = standalone::meta_api(
        meta,
        store,
        cfg,
        &[(token, vec![Access::Read, Access::Write, Access::Runner])],
    );
    let app = standalone::app(api);
    let which = Serve {
        h3: !args.no_h3,
        h2: true,
    };
    let l = standalone::bind(args.listen, &id, app.clone(), which).await?;
    let _plain = match args.plain_listen {
        Some(a) => Some(standalone::bind_plain(a, app).await?),
        None => None,
    };
    // One line a supervisor or test can parse.
    println!(
        "{}",
        serde_json::json!({
            "event": "listening",
            "addr": l.addr.to_string(),
            "h3": which.h3,
            "cert_sha256": id.fingerprint(),
        })
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[derive(clap::Args, Debug)]
struct Remote {
    /// The server, `https://host:port`.
    #[arg(long)]
    server: String,
    /// Pin the server certificate by SHA-256 (dev servers).
    #[arg(long)]
    pin: Option<String>,
    /// A PEM CA bundle for the server.
    #[arg(long)]
    ca_file: Option<PathBuf>,
    /// `auto` (HTTP/3, falling back to HTTP/2), `h3` or `h2`.
    #[arg(long, default_value = "auto")]
    mode: String,
    #[arg(long)]
    ns: String,
    /// A file holding the bearer token.
    #[arg(long)]
    token_file: PathBuf,
    /// The local state directory (resume state, install id, lock).
    #[arg(long)]
    state: PathBuf,
    /// Chunks in flight.
    #[arg(long, default_value_t = 8)]
    window: usize,
    /// A pause before each chunk request, in ms (tests).
    #[arg(long, hide = true)]
    pace_ms: Option<u64>,
}

impl Remote {
    fn spec(&self) -> Result<ServerSpec> {
        let mode = match self.mode.as_str() {
            "auto" => Mode::Auto,
            "h3" => Mode::H3,
            "h2" => Mode::H2,
            m => bail!("unknown mode {m}"),
        };
        Ok(ServerSpec {
            base: self.server.clone(),
            pin: self.pin.clone(),
            ca_file: self.ca_file.clone(),
            mode,
        })
    }

    fn client(&self, state: &StateDir) -> Result<Client> {
        let token = std::fs::read_to_string(&self.token_file)
            .with_context(|| format!("read {}", self.token_file.display()))?;
        Ok(Client::new(
            self.spec()?.connect_opts(state),
            &self.ns,
            token.trim(),
        )?)
    }

    fn opts(&self) -> UploadOpts {
        UploadOpts {
            window: self.window.clamp(1, 64),
            pace: self.pace_ms.map(Duration::from_millis),
            ..UploadOpts::default()
        }
    }
}

#[derive(clap::Args, Debug)]
struct PutArgs {
    #[command(flatten)]
    remote: Remote,
    /// A fixed chunk size in bytes (a power of two).
    #[arg(long)]
    chunk_size: Option<u64>,
    /// The file to upload.
    file: PathBuf,
}

#[derive(clap::Args, Debug)]
struct GetArgs {
    #[command(flatten)]
    remote: Remote,
    #[arg(long)]
    tid: String,
    #[arg(long)]
    dest: PathBuf,
}

/// The peak resident set, in KiB, where the OS reports it.
fn vm_hwm_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

/// Prints the tracker's progress as JSON lines, at most every 250 ms.
fn spawn_reporter(slot: Arc<Mutex<Option<Tracker>>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let snap = slot
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(Tracker::snapshot));
            if let Some(p) = snap {
                println!(
                    "{}",
                    serde_json::json!({"event": "progress", "progress": p})
                );
            }
        }
    })
}

async fn put(args: PutArgs) -> Result<()> {
    let state = StateDir::new(&args.remote.state)?;
    let _lock = state.lock()?;
    let client = args.remote.client(&state)?;
    let path = args.file.canonicalize()?;
    let resume = loams_transfer::agent::find_resumable(&state, &client, &path);
    let mut opts = args.remote.opts();
    opts.chunk_size = args.chunk_size;
    let slot = Arc::new(Mutex::new(None));
    let reporter = spawn_reporter(Arc::clone(&slot));
    let s2 = Arc::clone(&slot);
    let r = loams_transfer::client::upload(
        &client,
        &state,
        &path,
        resume.as_deref(),
        &opts,
        move |t| {
            if let Ok(mut g) = s2.lock() {
                *g = Some(t);
            }
        },
    )
    .await;
    reporter.abort();
    let v = r?;
    println!(
        "{}",
        serde_json::json!({
            "event": "done", "tid": v.tid, "file_sha256": v.file_sha256,
            "resumed": resume.is_some(), "vm_hwm_kib": vm_hwm_kib(),
        })
    );
    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let state = StateDir::new(&args.remote.state)?;
    let _lock = state.lock()?;
    let client = args.remote.client(&state)?;
    let slot = Arc::new(Mutex::new(None));
    let reporter = spawn_reporter(Arc::clone(&slot));
    let s2 = Arc::clone(&slot);
    let r = loams_transfer::client::download(
        &client,
        &args.tid,
        &args.dest,
        &args.remote.opts(),
        move |t| {
            if let Ok(mut g) = s2.lock() {
                *g = Some(t);
            }
        },
    )
    .await;
    reporter.abort();
    let p = r?;
    println!(
        "{}",
        serde_json::json!({"event": "done", "path": p, "vm_hwm_kib": vm_hwm_kib()})
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOAMS_TRANSFER_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::Serve(a) => serve(a).await,
        Cmd::Agent { state } => loams_transfer::agent::run(state).await,
        Cmd::Put(a) => put(a).await,
        Cmd::Get(a) => get(a).await,
        Cmd::Runner(a) => runner(a).await,
    }
}
