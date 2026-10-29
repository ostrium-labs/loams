//! `loams pg-control` and `loams pg-api-key` (PG2 Task 9): the flags of the
//! `pg-control` role, turned into a [`PgControlConfig`]; and `loams
//! pg-runner`, the compute runner that holds the container runtime's
//! socket for it (R-REL.1), turned into a [`PgRunnerConfig`].

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use loams::MetaBackend;
use loams::pg_control::{
    ComputesConfig, PgAuth, PgControl, PgControlConfig, PgRunner, PgRunnerConfig, PgRuntime,
    PgStore, RunnerClient, read_token,
};
use loams_pg_control::authz::{Actor, Grant, GrantObject, KeysFile, PrincipalKind, Relation};
use loams_pg_control::runtime::compose::CtlReach;
use loams_pg_control::runtime::images;
use loams_pg_control::runtime::runner::{DEFAULT_RUNNER_LISTEN, read_or_create_token};
use url::Url;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum StoreArg {
    /// One redb file in --data-dir (single node).
    Local,
    /// TiKV, at --meta.
    Tikv,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum RuntimeArg {
    /// compose; secrets in an age-encrypted file in --data-dir.
    Compose,
    /// Kubernetes: computes as Pods in loams-pg-<namespace>, secrets as
    /// Secrets in --kube-namespace.
    Kubernetes,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum CtlReachArg {
    /// pg-control runs on the host: each compute publishes compute_ctl on a
    /// loopback port.
    Host,
    /// pg-control runs on the compose network: http://<service>:3080.
    Network,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum AuthzArg {
    /// One principal with every relation; --listen must be loopback.
    Dev,
    /// API keys and their grants, from --pg-api-keys.
    ApiKey,
    /// API keys from --pg-api-keys for who calls, OpenFGA's model v1 for what they may do
    /// (LOAMS_AUTHZ_OPENFGA_GRPC, _STORE, _MODEL and optional _KEY).
    Openfga,
}

/// How the compose runtime runs computes: `pg-control`'s own, or the
/// compute runner's.
#[derive(Debug, clap::Args)]
pub(crate) struct ComposeArgs {
    /// The storage stack's network computes join (loams-neon_default for
    /// deploy/loams-postgres-dev, loams_default for the release stack).
    #[arg(long)]
    compose_network: Option<String>,
    /// Where the compose files live (default <data-dir>/compose).
    #[arg(long)]
    compose_dir: Option<PathBuf>,
    /// The upcall listener as computes reach it (pg-control's default:
    /// http://host.docker.internal:<port>, or host.containers.internal under
    /// Podman); --pg-upcall-listen must be an address they reach.
    #[arg(long)]
    compute_control_plane_uri: Option<String>,
    /// `<host>:<ip>` added to every compute's /etc/hosts (repeated).
    #[arg(long = "compose-extra-host")]
    compose_extra_host: Vec<String>,
    /// Where endpoints' ports are published.
    #[arg(long, default_value = "127.0.0.1")]
    compose_publish_ip: std::net::IpAddr,
    /// How pg-control reaches each compute's compute_ctl.
    #[arg(long, value_enum, default_value = "host")]
    compute_ctl_reach: CtlReachArg,
}

impl ComposeArgs {
    /// Whether a flag only the compose runtime reads was given.
    fn any_set(&self) -> bool {
        self.compose_network.is_some()
            || self.compose_dir.is_some()
            || self.compute_control_plane_uri.is_some()
            || !self.compose_extra_host.is_empty()
    }

    fn computes(self, images: Vec<(u32, String)>) -> ComputesConfig {
        ComputesConfig {
            compose_bin: None,
            compose_dir: self.compose_dir,
            network: self.compose_network,
            control_plane_uri: self.compute_control_plane_uri,
            extra_hosts: self.compose_extra_host,
            publish_ip: self.compose_publish_ip,
            ctl_reach: match self.compute_ctl_reach {
                CtlReachArg::Host => CtlReach::Host,
                CtlReachArg::Network => CtlReach::Network,
            },
            image_overrides: images.into_iter().collect(),
            runner: None,
            runtime_class: None,
            cluster_domain: None,
            warm_pool_size: None,
        }
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct PgControlArgs {
    /// Where the records live.
    #[arg(long, value_enum, default_value = "local")]
    store: StoreArg,
    /// With --store tikv: tikv://<pd-host:port>[,<pd…>]/<keyspace>[?root=<hex>]
    /// (the metastore's keyspace).
    #[arg(long, value_parser = MetaBackend::parse)]
    meta: Option<MetaBackend>,
    /// Where computes run (and secrets live).
    #[arg(long, value_enum, default_value = "compose")]
    runtime: RuntimeArg,
    #[command(flatten)]
    compose: ComposeArgs,
    /// Drive computes through this compute runner (`loams pg-runner`)
    /// instead of a compose command here; the compose flags are then the
    /// runner's.
    #[arg(long)]
    compute_runner: Option<Url>,
    /// With --compute-runner: the file holding the runner's token.
    #[arg(long)]
    compute_runner_token_file: Option<PathBuf>,
    /// `<major>=<repository>@sha256:<digest>`: the compute image of a
    /// Postgres major, over deploy/pins/loams-postgres-images.toml's
    /// (repeated). A compute runner must be given the same.
    #[arg(long = "compute-image", value_parser = parse_image)]
    compute_image: Vec<(u32, String)>,
    /// With --runtime kubernetes: the namespace of the Secrets.
    #[arg(long, default_value = "loams-postgres")]
    kube_namespace: String,
    /// With --runtime kubernetes: the compute Pods' runtimeClassName (a
    /// sandbox such as gVisor or Kata).
    #[arg(long)]
    kube_runtime_class: Option<String>,
    /// With --runtime kubernetes: the cluster's DNS domain.
    #[arg(long)]
    kube_cluster_domain: Option<String>,
    /// Warm computes kept per namespace, Postgres major and image (default
    /// 2 with --runtime kubernetes; 0 turns the pool off). The compose
    /// runtime has no warm pool.
    #[arg(long)]
    warm_pool_size: Option<u32>,
    /// Holds the local store and the secret file.
    #[arg(long, default_value = ".loams/pg-control")]
    data_dir: PathBuf,
    /// The API listener (`loams.postgres.v1`).
    #[arg(long, default_value = "127.0.0.1:7680")]
    listen: SocketAddr,
    /// The upcall listener Neon's components call; never the API's.
    #[arg(long, default_value = "127.0.0.1:7690")]
    pg_upcall_listen: SocketAddr,
    /// Who may call.
    #[arg(long, value_enum, default_value = "dev")]
    pg_authz: AuthzArg,
    /// With --pg-authz api-key or openfga: the keys file (`loams pg-api-key` makes entries).
    #[arg(long)]
    pg_api_keys: Option<PathBuf>,
    /// Serve grpc.reflection.v1 on --listen.
    #[arg(long)]
    reflection: bool,
    /// A pageserver's management API.
    #[arg(long, default_value = "http://127.0.0.1:9898")]
    pageserver: Url,
    /// The storage controller; every storage call then goes through it.
    #[arg(long)]
    storage_controller: Option<Url>,
    /// loams-wal's HTTP API.
    #[arg(long, default_value = "http://127.0.0.1:7676")]
    wal: Url,
    /// `<node id>=<libpq URL>` of a pageserver computes read from (repeated).
    #[arg(long = "pageserver-pg", value_parser = parse_pageserver)]
    pageserver_pg: Vec<(u64, String)>,
    /// The pageserver node of a branch no notify-attach placed.
    #[arg(long)]
    default_pageserver: Option<u64>,
    /// `<pool>=<connstr>[,<connstr>…]`: a loams-wal pool computes write to
    /// (repeated).
    #[arg(long = "wal-pool", value_parser = parse_pool)]
    wal_pool: Vec<(String, Vec<String>)>,
    /// Endpoint storage (`host:port`, as computes reach it), where they keep
    /// their file cache's state so a promoted replica can prewarm.
    #[arg(long)]
    endpoint_storage: Option<String>,
    /// Where to write the public key endpoint storage verifies tokens with
    /// (its `pemfile`), at start.
    #[arg(long, requires = "endpoint_storage")]
    endpoint_storage_pem_out: Option<PathBuf>,
    /// A file holding the storage controller's token; without it
    /// /notify-attach is not served.
    #[arg(long)]
    storage_controller_token_file: Option<PathBuf>,
}

fn parse_pageserver(s: &str) -> Result<(u64, String), String> {
    let (id, url) = s
        .split_once('=')
        .ok_or_else(|| format!("--pageserver-pg {s:?}: expected <node id>=<libpq URL>"))?;
    let id = id
        .parse()
        .map_err(|_| format!("--pageserver-pg {s:?}: the node id is a number"))?;
    if url.is_empty() {
        return Err(format!("--pageserver-pg {s:?}: the URL is empty"));
    }
    Ok((id, url.to_owned()))
}

fn parse_image(s: &str) -> Result<(u32, String), String> {
    images::parse_override(s).map_err(|e| format!("--compute-image: {e}"))
}

fn parse_pool(s: &str) -> Result<(String, Vec<String>), String> {
    let (name, list) = s
        .split_once('=')
        .ok_or_else(|| format!("--wal-pool {s:?}: expected <pool>=<connstr>[,<connstr>…]"))?;
    let conns: Vec<String> = list
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    if name.is_empty() || conns.is_empty() {
        return Err(format!(
            "--wal-pool {s:?}: a name and at least one connection string"
        ));
    }
    Ok((name.to_owned(), conns))
}

fn config(args: PgControlArgs) -> Result<PgControlConfig, String> {
    let mut config = PgControlConfig::new(args.data_dir);
    config.store = match (args.store, args.meta) {
        (StoreArg::Local, None) => PgStore::Local,
        (StoreArg::Local, Some(_)) => return Err("--meta goes with --store tikv".into()),
        (StoreArg::Tikv, None) => return Err("--store tikv needs --meta tikv://…".into()),
        #[cfg(feature = "tikv")]
        (StoreArg::Tikv, Some(MetaBackend::Tikv(meta))) => PgStore::Tikv(meta.tikv),
        #[allow(unreachable_patterns)]
        (StoreArg::Tikv, Some(_)) => return Err(loams::NO_TIKV_FEATURE.into()),
    };
    config.runtime = match args.runtime {
        RuntimeArg::Compose => PgRuntime::Compose,
        RuntimeArg::Kubernetes => PgRuntime::Kubernetes {
            namespace: args.kube_namespace,
        },
    };
    let runner = match (args.compute_runner, args.compute_runner_token_file) {
        (None, None) => None,
        (Some(url), Some(token_file)) => {
            if !matches!(config.runtime, PgRuntime::Compose) {
                return Err("--compute-runner goes with --runtime compose".into());
            }
            if args.compose.any_set() {
                return Err(
                    "with --compute-runner, the compose flags (--compose-network, --compose-dir, \
                     --compute-control-plane-uri, --compose-extra-host) are the runner's"
                        .into(),
                );
            }
            Some(RunnerClient {
                url,
                token: read_or_create_token(&token_file, false)
                    .map_err(|e| format!("--compute-runner-token-file {e}"))?,
            })
        }
        (Some(_), None) => return Err("--compute-runner needs --compute-runner-token-file".into()),
        (None, Some(_)) => {
            return Err("--compute-runner-token-file goes with --compute-runner".into());
        }
    };
    if matches!(config.runtime, PgRuntime::Compose) && args.warm_pool_size.is_some_and(|n| n > 0) {
        return Err(
            "--warm-pool-size goes with --runtime kubernetes: the compose runtime has no warm \
             pool (an unbound compute has no port to publish)"
                .into(),
        );
    }
    let mut computes = args.compose.computes(args.compute_image);
    computes.runner = runner;
    computes.runtime_class = args.kube_runtime_class;
    computes.cluster_domain = args.kube_cluster_domain;
    computes.warm_pool_size = args.warm_pool_size;
    config.computes = Some(computes);
    config.listen = args.listen;
    config.upcall_listen = args.pg_upcall_listen;
    config.auth = match (args.pg_authz, args.pg_api_keys) {
        (AuthzArg::Dev, None) => PgAuth::Dev,
        (AuthzArg::ApiKey, Some(path)) => PgAuth::ApiKeys(path),
        (AuthzArg::ApiKey, None) => return Err("--pg-authz api-key needs --pg-api-keys".into()),
        (AuthzArg::Openfga, Some(keys)) => PgAuth::OpenFga {
            keys,
            fga: loams::pg_control::FgaSettings::from_vars(|k| std::env::var(k).ok())?,
        },
        (AuthzArg::Openfga, None) => {
            return Err("--pg-authz openfga needs --pg-api-keys (who calls)".into());
        }
        (AuthzArg::Dev, Some(_)) => {
            return Err("--pg-api-keys goes with --pg-authz api-key or openfga".into());
        }
    };
    config.reflection = args.reflection;
    config.pageserver = args.pageserver;
    config.storage_controller = args.storage_controller;
    config.wal = args.wal;
    config.pageservers = args.pageserver_pg.into_iter().collect::<BTreeMap<_, _>>();
    config.default_pageserver = args.default_pageserver;
    config.wal_pools = args.wal_pool.into_iter().collect::<BTreeMap<_, _>>();
    config.endpoint_storage = args.endpoint_storage;
    config.endpoint_storage_pem_out = args.endpoint_storage_pem_out;
    config.notify_token = args
        .storage_controller_token_file
        .as_deref()
        .map(read_token)
        .transpose()?;
    Ok(config)
}

/// `loams pg-control`: runs until a shutdown signal.
pub(crate) async fn run(args: PgControlArgs) -> ExitCode {
    // The startup gate (design §54 §4.3, UP1 Task 4): refuse records this
    // release does not read, before anything listens.
    if let Err(err) = loams_common::formats::check_dir_stamp(
        &args.data_dir,
        loams_common::formats::PG_CONTROL_RECORD,
    ) {
        tracing::error!(code = "format_unreadable", %err, "refusing to start");
        eprintln!("loams pg-control: refusing to start: format_unreadable: {err}");
        return ExitCode::from(loams_migrate::startup::EXIT_REFUSED);
    }
    let config = match config(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("loams pg-control: {e}");
            return ExitCode::FAILURE;
        }
    };
    let control = match PgControl::start(config).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("loams pg-control: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "loams pg-control upcalls listening on http://{}",
        control.upcall_addr()
    );
    println!(
        "loams pg-control listening on http://{}",
        control.api_addr()
    );
    super::shutdown_signal().await;
    tracing::info!("shutting down");
    control.shutdown().await;
    ExitCode::SUCCESS
}

#[derive(Debug, clap::Args)]
pub(crate) struct PgRunnerArgs {
    /// Where pg-control calls the runner. Keep it on a network only
    /// pg-control joins.
    #[arg(long, default_value = DEFAULT_RUNNER_LISTEN)]
    listen: SocketAddr,
    /// The file holding the token pg-control presents
    /// (--compute-runner-token-file).
    #[arg(long)]
    token_file: PathBuf,
    /// Create --token-file (32 random bytes, mode 0600) when it is missing.
    #[arg(long)]
    create_token: bool,
    /// Holds the compose files (<data-dir>/compose).
    #[arg(long, default_value = ".loams/pg-runner")]
    data_dir: PathBuf,
    #[command(flatten)]
    compose: ComposeArgs,
    /// `<major>=<repository>@sha256:<digest>`: the compute image of a
    /// Postgres major, over deploy/pins/loams-postgres-images.toml's
    /// (repeated). The runner starts no other image.
    #[arg(long = "compute-image", value_parser = parse_image)]
    compute_image: Vec<(u32, String)>,
}

fn runner_config(args: PgRunnerArgs) -> Result<PgRunnerConfig, String> {
    let token = read_or_create_token(&args.token_file, args.create_token)
        .map_err(|e| format!("--token-file {e}"))?;
    Ok(PgRunnerConfig {
        listen: args.listen,
        token,
        computes: args.compose.computes(args.compute_image),
        data_dir: args.data_dir,
    })
}

/// `loams pg-runner`: runs until a shutdown signal.
pub(crate) async fn run_runner(args: PgRunnerArgs) -> ExitCode {
    let runner = match runner_config(args) {
        Ok(config) => PgRunner::start(config).await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let runner = match runner {
        Ok(r) => r,
        Err(e) => {
            eprintln!("loams pg-runner: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "loams pg-runner listening on http://{} (images: {})",
        runner.addr(),
        runner.images().join(", ")
    );
    super::shutdown_signal().await;
    tracing::info!("shutting down");
    runner.shutdown().await;
    ExitCode::SUCCESS
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum KindArg {
    User,
    Agent,
    ServiceAccount,
}

#[derive(Debug, clap::Args)]
pub(crate) struct PgApiKeyArgs {
    /// Whom the key authenticates.
    #[arg(long)]
    subject: String,
    #[arg(long, value_enum, default_value = "user")]
    kind: KindArg,
    /// `<kind>:<subject>` of an actor acting for the subject, such as
    /// `agent:release-bot` (repeated, nearest first).
    #[arg(long = "act", value_parser = parse_actor)]
    act: Vec<Actor>,
    /// `<relation>:namespace:<ns>` or `<relation>:project:<prj-…>`, the
    /// relation one of viewer, editor, admin, connect (repeated).
    #[arg(long = "grant", value_parser = parse_grant)]
    grant: Vec<Grant>,
}

fn kind(s: &str) -> Option<PrincipalKind> {
    match s {
        "user" => Some(PrincipalKind::User),
        "agent" => Some(PrincipalKind::Agent),
        "service_account" | "service-account" => Some(PrincipalKind::ServiceAccount),
        _ => None,
    }
}

fn parse_actor(s: &str) -> Result<Actor, String> {
    let (k, subject) = s
        .split_once(':')
        .ok_or_else(|| format!("--act {s:?}: expected <kind>:<subject>"))?;
    let kind =
        kind(k).ok_or_else(|| format!("--act {s:?}: kind is user, agent or service-account"))?;
    if subject.is_empty() {
        return Err(format!("--act {s:?}: the subject is empty"));
    }
    Ok(Actor {
        subject: subject.to_owned(),
        kind,
    })
}

fn parse_grant(s: &str) -> Result<Grant, String> {
    let mut parts = s.splitn(3, ':');
    let (Some(relation), Some(on), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "--grant {s:?}: expected <relation>:namespace:<ns> or <relation>:project:<prj-…>"
        ));
    };
    let relation = match relation {
        "viewer" => Relation::Viewer,
        "editor" => Relation::Editor,
        "admin" => Relation::Admin,
        "connect" => Relation::Connect,
        _ => {
            return Err(format!(
                "--grant {s:?}: relation is viewer, editor, admin or connect"
            ));
        }
    };
    let on = match on {
        "namespace" => GrantObject::Namespace(name.to_owned()),
        "project" => GrantObject::Project(name.to_owned()),
        _ => return Err(format!("--grant {s:?}: on namespace or project")),
    };
    Ok(Grant { relation, on })
}

/// `loams pg-api-key`: one JSON object, `{"api_key": …, "entry": …}`.
pub(crate) fn mint(args: &PgApiKeyArgs) -> ExitCode {
    let kind = match args.kind {
        KindArg::User => PrincipalKind::User,
        KindArg::Agent => PrincipalKind::Agent,
        KindArg::ServiceAccount => PrincipalKind::ServiceAccount,
    };
    let minted = loams_pg_control::authz::mint_api_key(
        &args.subject,
        kind,
        args.act.clone(),
        args.grant.clone(),
    );
    let (key, entry) = match minted {
        Ok(m) => m,
        Err(e) => {
            eprintln!("loams pg-api-key: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The entry must load: no admin for an agent, well-formed grants.
    let file = KeysFile {
        keys: vec![entry.clone()],
    };
    let checked = serde_json::to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|text| KeysFile::parse(&text).map_err(|e| e.to_string()));
    if let Err(e) = checked {
        eprintln!("loams pg-api-key: {e}");
        return ExitCode::FAILURE;
    }
    let out = serde_json::json!({ "api_key": key.expose(), "entry": entry });
    println!("{out}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[derive(clap::Parser)]
    struct Control {
        #[command(flatten)]
        args: PgControlArgs,
    }

    #[derive(clap::Parser)]
    struct Runner {
        #[command(flatten)]
        args: PgRunnerArgs,
    }

    /// R-REL.1: with a compute runner, the compose flags are the runner's,
    /// and its token file must exist.
    #[test]
    fn a_compute_runner_takes_the_compose_flags() {
        let dir = tempfile::tempdir().expect("temp dir");
        let token = dir.path().join("token");
        std::fs::write(&token, "secret\n").expect("write");
        let token = token.display().to_string();
        let parse = |extra: &[&str]| {
            let mut argv = vec!["pg-control", "--compute-runner", "http://pg-runner:7691"];
            argv.extend_from_slice(extra);
            config(Control::try_parse_from(argv).expect("flags parse").args)
        };
        let ok = parse(&["--compute-runner-token-file", &token]).expect("a config");
        let computes = ok.computes.expect("computes");
        let runner = computes.runner.expect("a runner");
        assert_eq!(runner.url.as_str(), "http://pg-runner:7691/");
        assert_eq!(runner.token.expose(), "secret");
        let e = parse(&[
            "--compute-runner-token-file",
            &token,
            "--compose-network",
            "loams_default",
        ])
        .expect_err("refused");
        assert!(e.contains("are the runner's"), "{e}");
        assert!(parse(&[]).is_err());
        let missing = dir.path().join("missing").display().to_string();
        assert!(parse(&["--compute-runner-token-file", &missing]).is_err());
        let e = parse(&[
            "--compute-runner-token-file",
            &token,
            "--runtime",
            "kubernetes",
        ])
        .expect_err("refused");
        assert!(e.contains("--runtime compose"), "{e}");
    }

    /// R14.2: the warm pool's size is Kubernetes'; compose takes only 0.
    #[test]
    fn the_warm_pool_is_kubernetes_only() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["pg-control"];
            argv.extend_from_slice(extra);
            config(Control::try_parse_from(argv).expect("flags parse").args)
        };
        let kube = parse(&["--runtime", "kubernetes", "--warm-pool-size", "3"]).expect("a config");
        assert_eq!(kube.computes.expect("computes").warm_pool_size, Some(3));
        let e = parse(&["--warm-pool-size", "2"]).expect_err("refused");
        assert!(e.contains("no warm pool"), "{e}");
        let off = parse(&["--warm-pool-size", "0"]).expect("a config");
        assert_eq!(off.computes.expect("computes").warm_pool_size, Some(0));
    }

    /// `loams pg-runner --create-token` makes the token file pg-control
    /// then reads.
    #[test]
    fn the_runner_creates_its_token() {
        let dir = tempfile::tempdir().expect("temp dir");
        let token = dir.path().join("run").join("token");
        let token_arg = token.display().to_string();
        let args = |create: bool| {
            let mut argv = vec![
                "pg-runner",
                "--token-file",
                &token_arg,
                "--compute-control-plane-uri",
                "http://pg-control:7690",
            ];
            if create {
                argv.push("--create-token");
            }
            Runner::try_parse_from(argv).expect("flags parse").args
        };
        assert!(runner_config(args(false)).is_err());
        let made = runner_config(args(true)).expect("a config");
        let read = read_or_create_token(&token, false).expect("the file");
        assert_eq!(made.token.expose(), read.expose());
        assert_eq!(
            made.computes.control_plane_uri.as_deref(),
            Some("http://pg-control:7690")
        );
    }

    #[test]
    fn grants_and_pools_parse() {
        let g = parse_grant("editor:namespace:acme").expect("a grant");
        assert_eq!(g.relation, Relation::Editor);
        assert_eq!(g.on, GrantObject::Namespace("acme".into()));
        assert!(parse_grant("owner:namespace:acme").is_err());
        assert!(parse_grant("editor:acme").is_err());
        let a = parse_actor("agent:bot").expect("an actor");
        assert_eq!(a.kind, PrincipalKind::Agent);
        assert_eq!(
            parse_pool("default=postgresql://wal-0:5454,postgresql://wal-1:5454").expect("a pool"),
            (
                "default".into(),
                vec![
                    "postgresql://wal-0:5454".into(),
                    "postgresql://wal-1:5454".into()
                ]
            )
        );
        assert!(parse_pool("default=").is_err());
        assert_eq!(
            parse_pageserver("1=postgresql://ps:6400").expect("a pageserver"),
            (1, "postgresql://ps:6400".into())
        );
        let digest = "sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3";
        let image = format!("ghcr.io/ostrium-labs/loams-postgres-compute-v17@{digest}");
        assert_eq!(
            parse_image(&format!("17={image}")).expect("an image"),
            (17, image)
        );
        assert!(parse_image("17=ghcr.io/x/compute:latest").is_err());
        assert!(
            parse_image(&format!(
                "17=ghcr.io/example/neon-archive/compute-node-v17@{digest}"
            ))
            .is_err()
        );
    }
}
