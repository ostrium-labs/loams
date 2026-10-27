//! The command line (plan DD1 "Shared contracts").

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use loams_agentd_proto::daemon::DaemonMode;

/// Development override of `--config`: when set and non-empty it wins over
/// the flag, so a daemon that Loams Desktop starts can be pointed elsewhere.
pub const CONFIG_ENV: &str = "LOAMS_AGENTD_CONFIG";

#[derive(Debug, Parser)]
#[command(
    name = "loams-agentd",
    about = "Loams Desktop's per-user agent daemon",
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon in the foreground: the sessions engine, its harnesses
    /// and the loopback RPC server.
    Run(RunArgs),
    /// Run the daemon as `run` does, plus the remote Connect listener for
    /// the browser console: `serve --remote --listen <addr:port> --tls-cert
    /// <pem> --tls-key <pem> --issuer <url> --audience loams-agentd --subject
    /// <sub> --allow-origin <origin> [--allow-origin …] --config <path>`.
    /// Every flag but `--allow-origin` is required (design §53 §6.3).
    Serve(ServeArgs),
    /// Serve the MCP (Model Context Protocol) server on stdin/stdout,
    /// proxying to the running daemon's RPC. Harness runs get it injected,
    /// with LOAMS_AGENTD_RUN_TOKEN, LOAMS_AGENTD_PORT and
    /// LOAMS_AGENTD_SESSION_ID set; without LOAMS_AGENTD_PORT the port is read
    /// from the discovery file beside --config. Logs go to stderr; stdout is
    /// the protocol.
    Mcp(ConfigArg),
    /// Show the daemon directory and whether a daemon is running there.
    Status(ConfigArg),
    /// Manage the per-user service: a systemd user unit (Linux), a
    /// LaunchAgent (macOS) or a Run entry (Windows).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Copy the bundled binaries into the versioned runtime directory, check
    /// them against SHA256SUMS and switch `current` to them.
    InstallRuntime(InstallRuntimeArgs),
    /// Print the daemon's version.
    Version,
    /// Agent packages from the Loams Marketplace (design §73).
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Loams integration used by the Loams Bot harness (`loams bot-acp`).
    #[command(hide = true)]
    Loams {
        #[command(subcommand)]
        command: LoamsCommand,
    },
}

/// The link crate's CLI also has `login`, `logout`, `status`, `bot` and
/// `mock`; the daemon exposes only the entry point the Loams Bot harness
/// launches, so anything else is a usage error (plan DD1 ruling T1-2).
#[derive(Debug, Subcommand)]
pub enum LoamsCommand {
    /// Speak ACP on stdin/stdout as the Loams Bot agent.
    BotAcp,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Run the agent package conformance suite, LAPC Core (design §73 §9),
    /// against a package directory holding `listing.json`, or a listing
    /// whose package is `--package` or comes from the npm registry. Writes
    /// the JSON report to `--out` (else stdout) and a summary to stderr.
    /// Exits 0 when the package reaches Core, 1 when it does not, 2 when
    /// the run could not happen.
    Conformance(ConformanceArgs),
}

#[derive(Debug, Args)]
pub struct ConformanceArgs {
    /// A package directory with `listing.json`, or a `listing.json`.
    #[arg(value_name = "PACKAGE_DIR|LISTING_JSON")]
    pub target: PathBuf,
    /// The package for a listing file: a directory or an npm tarball
    /// (checked against the listing's integrity).
    #[arg(long, value_name = "DIR|TGZ")]
    pub package: Option<PathBuf>,
    /// Write the report here instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
    /// Sign the report with Sigstore keyless signing (`cosign sign-blob
    /// --bundle`), as the index CI's identity; writes `<out>.sigstore.json`.
    /// Needs `--out` and `cosign` on PATH.
    #[arg(long, requires = "out")]
    pub sign: bool,
    /// The `node` to run the agent with (default: from PATH).
    #[arg(long, value_name = "PATH")]
    pub node: Option<PathBuf>,
    /// The `npm` to install with (default: from PATH).
    #[arg(long, value_name = "PATH")]
    pub npm: Option<PathBuf>,
    /// Install and run here and keep the files (default: a temporary
    /// directory, removed afterwards).
    #[arg(long, value_name = "DIR")]
    pub work_dir: Option<PathBuf>,
    /// LOAMS_PG_URL for a listing that declares `postgres`, from a local Loams.
    #[arg(long, value_name = "URL")]
    pub pg_url: Option<String>,
    /// LOAMS_DURABLE_URL for a listing that declares `durable`.
    #[arg(long, value_name = "URL")]
    pub durable_url: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Install the service for this config and start it. The runtime must be
    /// installed first (`install-runtime`).
    Install(ConfigArg),
    /// Stop the service and remove it.
    Uninstall(ConfigArg),
    /// Start the installed service.
    Start(ConfigArg),
    /// Stop the service until the next login or `start`.
    Stop(ConfigArg),
    /// Whether the service is installed, and its state.
    Status(ConfigArg),
    /// Let the service start at boot and survive logout (Linux:
    /// `loginctl enable-linger`).
    EnableLinger,
    /// Undo `enable-linger`.
    DisableLinger,
}

#[derive(Debug, Args)]
pub struct InstallRuntimeArgs {
    /// The bundled binaries: `loams-agentd`, `loams`, `SHA256SUMS` and `VERSION`.
    #[arg(long, value_name = "DIR")]
    pub from: PathBuf,
    /// The runtime directory, `<userData>/agentd/runtime`.
    #[arg(long, value_name = "DIR")]
    pub to: PathBuf,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Run as the per-user service. On Windows this is the parent that runs
    /// and restarts the `--worker`.
    #[arg(long, conflicts_with_all = ["child", "worker"])]
    pub service: bool,
    /// Run as Loams Desktop's child process (background agents off).
    #[arg(long, conflicts_with = "worker")]
    pub child: bool,
    /// Run as the service's worker, under the Windows parent. Elsewhere it
    /// is the same as `--service`.
    #[arg(long)]
    pub worker: bool,
    #[command(flatten)]
    pub config: ConfigArg,
}

impl RunArgs {
    /// The mode the flags ask for; `None` leaves it to `config.toml`.
    pub fn mode(&self) -> Option<DaemonMode> {
        match (self.service || self.worker, self.child) {
            (true, _) => Some(DaemonMode::Service),
            (_, true) => Some(DaemonMode::Child),
            _ => None,
        }
    }
}

/// `serve` (plan SD1 Task 7). The flags are optional to clap so that a
/// missing one exits 4 naming it, as the Shared contracts say, rather than
/// with clap's usage error.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Serve `loams.agentd.v1` to the browser console over TLS.
    #[arg(long)]
    pub remote: bool,
    /// The address and port to listen on, for example 0.0.0.0:8443.
    #[arg(long, value_name = "ADDR:PORT")]
    pub listen: Option<String>,
    /// The PEM certificate chain.
    #[arg(long, value_name = "PEM")]
    pub tls_cert: Option<PathBuf>,
    /// The PEM private key.
    #[arg(long, value_name = "PEM")]
    pub tls_key: Option<PathBuf>,
    /// The OIDC issuer, for example https://auth.example.com/application/o/loams/.
    #[arg(long, value_name = "URL")]
    pub issuer: Option<String>,
    /// The audience access tokens must carry: loams-agentd.
    #[arg(long, value_name = "AUDIENCE")]
    pub audience: Option<String>,
    /// The one subject (`sub`) admitted: the person this agentd serves.
    #[arg(long, value_name = "SUB")]
    pub subject: Option<String>,
    /// A console origin allowed to call it (repeatable).
    #[arg(long = "allow-origin", value_name = "ORIGIN")]
    pub allow_origin: Vec<String>,
    #[command(flatten)]
    pub run: RunArgs,
}

impl ServeArgs {
    /// The flags as the library reads them.
    pub fn flags(&self) -> loams_agentd::remote::RemoteFlags {
        loams_agentd::remote::RemoteFlags {
            remote: self.remote,
            listen: self.listen.clone(),
            tls_cert: self.tls_cert.clone(),
            tls_key: self.tls_key.clone(),
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
            subject: self.subject.clone(),
            allow_origin: self.allow_origin.clone(),
        }
    }
}

#[derive(Debug, Args)]
pub struct ConfigArg {
    /// The daemon's config.toml. The daemon keeps its other files (token,
    /// discovery, lock, store, logs) in the same directory.
    /// LOAMS_AGENTD_CONFIG overrides it.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

impl ConfigArg {
    /// `LOAMS_AGENTD_CONFIG` when set, else `--config`.
    pub fn resolve(&self) -> Option<PathBuf> {
        std::env::var_os(CONFIG_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.config.clone())
    }
}
