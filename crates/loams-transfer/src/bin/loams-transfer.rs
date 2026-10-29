//! `loams-transfer`: the desktop's transfer sidecar and its dev server
//! (design §85, plan DQ1).
//!
//! - `serve`: the transfer API on a single-node metastore and a bucket,
//!   over HTTP/3 and HTTP/2+TLS on one port number.
//!
//! Later DQ1 tasks add `agent` (the desktop sidecar), `put`, `get`,
//! `runner` and `tail`.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use loams_store::Store;
use loams_transfer::TransfersConfig;
use loams_transfer::net::ServerIdentity;
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
    }
}
