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
    about = "The Loams Fabric: Loams House (ClickHouse SQL over Iceberg tables in your bucket)"
)]
pub struct Cli {
    /// The role this process runs.
    #[command(subcommand)]
    pub role: Role,
}

/// The roles. FL1 adds `ingest` and `flow`.
#[derive(Debug, Subcommand)]
pub enum Role {
    /// Loams House: the ClickHouse HTTP interface and its sealed chDB workers
    /// (design §49).
    House(HouseArgs),
}

/// `loams-fabric house`. Each flag beats the same key in `--config`, which beats
/// `--single-node`'s defaults and the built-in ones.
#[derive(Debug, Default, clap::Args)]
pub struct HouseArgs {
    /// A TOML file with `[house]`, `[house.limits]`, `[house.pool]`,
    /// `[house.catalog]`, `[house.store]` and `[house.tls]`.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// The ClickHouse HTTP listener (loopback only until TLS, HS1 Task 20)
    /// [default: 127.0.0.1:8123].
    #[arg(long)]
    pub house_listen: Option<SocketAddr>,
    /// The TLS HTTP listener (8443; HS1 Task 20).
    #[arg(long)]
    pub house_tls_listen: Option<SocketAddr>,
    /// The native protocol listener (9000; served from HS1 Task 31)
    /// [default with --single-node: 127.0.0.1:9000].
    #[arg(long)]
    pub native_listen: Option<SocketAddr>,
    /// The TLS native listener (9440; HS1 Task 20).
    #[arg(long)]
    pub native_tls_listen: Option<SocketAddr>,
    /// Admin and metrics (served from HS1 Task 23) [default: 127.0.0.1:8125].
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
