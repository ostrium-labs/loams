//! The command line: `loams-fabric <role> [flags]` (HS1 Task 7, §49 Shared
//! contracts' flags).

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use loams_house::{CatalogSpec, SandboxMode, WorkersMode};

/// `loams-fabric`.
#[derive(Debug, Parser)]
#[command(
    name = "loams-fabric",
    version,
    about = "The Loams Fabric: Loams House (ClickHouse SQL over Iceberg tables in your bucket) and connect (native connectors on a BYOC VM)"
)]
pub struct Cli {
    /// The role this process runs.
    #[command(subcommand)]
    pub role: Role,
}

/// The roles. FL1 adds `ingest` and `flow`.
// Parsed once per process: the variants' sizes do not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Role {
    /// Loams House: the ClickHouse HTTP interface and its sealed chDB workers
    /// (design §49).
    House(HouseArgs),
    /// Native connector instances (Kafka, Postgres, MySQL) reading into and
    /// writing from Loams streams, on a BYOC VM (design §80 §4.4, D1821).
    Connect(ConnectArgs),
}

/// `loams-fabric connect`.
#[derive(Debug, Clone, clap::Args)]
pub struct ConnectArgs {
    /// The directory of instance files (`*.yaml`), one instance each.
    #[arg(long, default_value = "/etc/loams/connect")]
    pub instances: PathBuf,
    /// The engine: its connect socket (`unix:///run/loams-connect/engine.sock`,
    /// the engine's --connect-socket) or, for development, its main port
    /// (`http://127.0.0.1:8080`).
    #[arg(long, default_value = "unix:///run/loams-connect/engine.sock")]
    pub engine: String,
    /// The Loams API key for the engine [default: <secrets-root>/engine-key].
    #[arg(long)]
    pub engine_key_file: Option<PathBuf>,
    /// The file secret store: `<root>/<namespace_id>/<secret>/<key>`,
    /// owner-only files.
    #[arg(long, default_value = "/var/lib/loams-connect/secrets")]
    pub secrets_root: PathBuf,
    /// The bucket the role keeps its lock, checkpoints and controls in, as a
    /// URL (`s3://bucket/prefix`, `file:///dir`).
    #[arg(long, default_value = "file:///var/lib/loams-connect/state")]
    pub store: String,
    /// The role's prefix in the bucket.
    #[arg(long, default_value = "_meta/connect/")]
    pub state_bucket_prefix: String,
    /// The admin API (status, pause, resume, rate). Loopback unless an
    /// address such as the tailnet's is named, which needs a token.
    #[arg(long, default_value = "127.0.0.1:7790")]
    pub admin_listen: SocketAddr,
    /// The admin API's bearer token [default: <secrets-root>/connect-admin-token].
    #[arg(long)]
    pub admin_token_file: Option<PathBuf>,
    /// The `connectors/` directory: the registry, the profiles and their
    /// schemas (the image ships the repository's).
    #[arg(long, default_value = "/usr/share/loams/connectors")]
    pub connectors_dir: PathBuf,
}

/// `loams-fabric house`. Each flag beats the same key in `--config`, which beats
/// `--single-node`'s defaults and the built-in ones.
#[derive(Debug, Default, clap::Args)]
pub struct HouseArgs {
    /// A TOML file with `[house]`, `[house.limits]`, `[house.pool]`,
    /// `[house.catalog]`, `[house.store]` and `[house.tls]`.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// The ClickHouse HTTP listener, plaintext and so loopback only
    /// [default: 127.0.0.1:8123].
    #[arg(long)]
    pub house_listen: Option<SocketAddr>,
    /// The TLS HTTP listener (8443; HS1 Task 20), which may face other hosts.
    #[arg(long)]
    pub house_tls_listen: Option<SocketAddr>,
    /// The TLS listener's certificate chain (PEM). With --single-node and no
    /// certificate, a self-signed one is made here on first start
    /// [default with --single-node: <data-dir>/tls/house.crt].
    #[arg(long)]
    pub house_tls_cert: Option<PathBuf>,
    /// The TLS listener's private key (PEM)
    /// [default with --single-node: <data-dir>/tls/house.key].
    #[arg(long)]
    pub house_tls_key: Option<PathBuf>,
    /// With --single-node: where the local user's key is kept (created on first
    /// start, mode 0600) [default: <data-dir>/house.key].
    #[arg(long)]
    pub local_key_file: Option<PathBuf>,
    /// With --single-node: where the engine's link key is kept (created on first
    /// start, mode 0600). The link acts for the Loams principals of
    /// --principals; without this flag the House has no link user.
    #[arg(long)]
    pub link_key_file: Option<PathBuf>,
    /// With --single-node: the Loams principals the engine's link may act for,
    /// `principal=scopes` separated by `;` (scopes: read, write, ddl, admin,
    /// joined by `+` or `,`), such as
    /// `tailscale:alice@example.com=read+write+ddl;apikey:ci=read`. Also
    /// LOAMS_HOUSE_PRINCIPALS. A principal not listed has no House access.
    #[arg(long)]
    pub principals: Option<String>,
    /// With --single-node, for development: a principal not listed acts as the
    /// local user instead of being refused.
    #[arg(long)]
    pub dev_unmapped_principals: bool,
    /// The audit log, one JSON line per statement
    /// [default with --single-node: <data-dir>/audit.log].
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    /// The OpenTelemetry collector the statements' spans go to, over OTLP/HTTP
    /// (`http://otel-collector:4318`); none by default (HS1 Task 23).
    #[arg(long)]
    pub otlp_endpoint: Option<String>,
    /// The ClickHouse native protocol's listener (9000, loopback only)
    /// [default with --single-node: 127.0.0.1:9000, skipped when taken].
    #[arg(long)]
    pub native_listen: Option<SocketAddr>,
    /// The native protocol's TLS listener (9440), with the TLS listener's
    /// certificate; a user with an empty password is refused off loopback.
    /// Also LOAMS_HOUSE_NATIVE_TLS_LISTEN.
    #[arg(long)]
    pub native_tls_listen: Option<SocketAddr>,
    /// Admin and metrics, `GET /metrics` (HS1 Task 23) [default: 127.0.0.1:8125].
    #[arg(long)]
    pub admin_listen: Option<SocketAddr>,
    /// One machine: the local catalog and store under --data-dir, loopback
    /// listeners, a generated local key, and --workers=inproc allowed (§49 §17).
    #[arg(long)]
    pub single_node: bool,
    /// Where chDB runs: sealed worker processes, or (single node, built with
    /// inproc-worker) threads of this process with no isolation
    /// [default: process].
    #[arg(long)]
    pub workers: Option<WorkersMode>,
    /// The catalog: rest:<url> (Lakekeeper) or local:<path> (SQLite)
    /// [default with --single-node: local:<data-dir>/catalog.sqlite].
    #[arg(long)]
    pub catalog: Option<CatalogSpec>,
    /// The store, as a URL (s3://…, file:///…)
    /// [default with --single-node: file://<data-dir>/bucket].
    #[arg(long)]
    pub store: Option<String>,
    /// How workers are sealed: netns, pods or none [default: netns on Linux].
    #[arg(long)]
    pub sandbox: Option<SandboxMode>,
    /// Allow --sandbox=none: workers run without the OS sandbox. Development
    /// only; logged at start.
    #[arg(long)]
    pub unsafe_no_sandbox: bool,
    /// The spool, the workers' private directories and, with --single-node, the
    /// local catalog, bucket and key [default: ./loams-house-data].
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// The loams-house-worker binary [default: beside this binary].
    #[arg(long)]
    pub worker_binary: Option<PathBuf>,
    /// A delegated cgroup v2 directory this process owns; each worker gets a
    /// child with its limits.
    #[arg(long)]
    pub cgroup_root: Option<PathBuf>,
}
