//! `loams-spark-proxy`: the Spark Connect proxy as its own process (SP-a
//! D1992). The same proxy runs inside `loams-gateway --module spark`.
//!
//! ```text
//! loams-spark-proxy serve --config proxy.toml
//! loams-spark-proxy check --config proxy.toml
//! loams-spark-proxy mint-key --namespace local --subject dev
//! loams-spark-proxy render --out /conf      # compose: spark-defaults.conf, sail.env
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use loams_authz::SubjectKind;
use loams_spark::auth::{KeyEntry, KeyTable};
use loams_spark::standalone;

#[derive(Debug, Parser)]
#[command(
    name = "loams-spark-proxy",
    version,
    about = "The Loams Spark Connect proxy"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the proxy.
    Serve {
        /// The configuration file (TOML).
        #[arg(long, env = "LOAMS_SPARK_PROXY_CONFIG")]
        config: PathBuf,
    },
    /// Check a configuration file and exit.
    Check {
        #[arg(long, env = "LOAMS_SPARK_PROXY_CONFIG")]
        config: PathBuf,
    },
    /// Print a new API key once, and the keys-file entry that admits it.
    MintKey {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        subject: String,
        /// user, service_account or agent.
        #[arg(long, default_value = "service_account")]
        kind: String,
    },
    /// Write the compose instance's `spark-defaults.conf` (Apache Spark)
    /// and `sail.env` (Sail, shell exports) into a directory.
    Render {
        #[arg(long, default_value = "local")]
        namespace: String,
        #[arg(long, default_value = "eng_compose")]
        instance: String,
        /// Lakekeeper's Iceberg REST URI.
        #[arg(
            long,
            env = "LOAMS_LAKEKEEPER_URI",
            default_value = "http://lakekeeper:8181/catalog"
        )]
        catalog_uri: String,
        /// The S3 endpoint.
        #[arg(long, env = "LOAMS_S3_ENDPOINT", default_value = "http://rustfs:9000")]
        s3_endpoint: String,
        #[arg(long)]
        out: PathBuf,
    },
}

fn render(
    namespace: &str,
    instance: &str,
    catalog_uri: String,
    s3_endpoint: String,
    out: &std::path::Path,
) -> Result<(), String> {
    let mut c = loams_spark::conf::compose_conf(namespace, instance);
    c.catalog.uri = catalog_uri;
    c.store.endpoint = Some(s3_endpoint);
    let defaults = c.spark_defaults().map_err(|e| e.to_string())?.to_file();
    let sail = loams_spark::conf::shell_env(&c.sail_env().map_err(|e| e.to_string())?);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (name, body) in [("spark-defaults.conf", defaults), ("sail.env", sail)] {
        let path = out.join(name);
        std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn random_chars(alphabet: &[u8], n: usize) -> String {
    // uuid v4 draws from the OS generator; take one byte per character and
    // reject the bytes that would bias the alphabet.
    let limit = 256 - (256 % alphabet.len());
    let mut out = String::with_capacity(n);
    while out.len() < n {
        for b in uuid::Uuid::new_v4().into_bytes() {
            if (b as usize) < limit && out.len() < n {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

fn mint(namespace: String, subject: String, kind: &str) -> Result<(), String> {
    let kind = match kind {
        "user" => SubjectKind::User,
        "service_account" => SubjectKind::ServiceAccount,
        "agent" => SubjectKind::Agent,
        other => return Err(format!("unknown kind {other:?}")),
    };
    let id = random_chars(CROCKFORD, 26);
    let key = format!("lk_{id}_{}", random_chars(BASE62, 32));
    let entry = KeyEntry {
        id,
        sha256: KeyTable::hash(&key),
        namespace,
        subject,
        kind,
    };
    let json = serde_json::to_string_pretty(&serde_json::json!({ "keys": [entry] }))
        .map_err(|e| e.to_string())?;
    eprintln!("The key is shown once. Keep it in a secret store.");
    println!("{key}");
    eprintln!("Add this entry to the proxy's keys file:\n{json}");
    Ok(())
}

async fn terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn read(path: &PathBuf) -> Result<standalone::Config, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    standalone::load(&text)
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let result = match Cli::parse().command {
        Command::Serve { config } => match read(&config) {
            Ok(c) => standalone::run(c, terminate()).await,
            Err(e) => Err(e),
        },
        Command::Check { config } => read(&config).and_then(|c| {
            standalone::build(&c)?;
            println!("ok: {} route(s)", c.routes.len());
            Ok(())
        }),
        Command::MintKey {
            namespace,
            subject,
            kind,
        } => mint(namespace, subject, &kind),
        Command::Render {
            namespace,
            instance,
            catalog_uri,
            s3_endpoint,
            out,
        } => render(&namespace, &instance, catalog_uri, s3_endpoint, &out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams-spark-proxy: {e}");
            ExitCode::FAILURE
        }
    }
}
