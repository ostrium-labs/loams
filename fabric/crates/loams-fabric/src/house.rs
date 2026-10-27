//! The `house` role (HS1 Task 7): [`resolve`] the settings from the flags, the
//! `--config` file and `--single-node`'s defaults, then [`start`] the worker pool
//! and the ClickHouse HTTP interface.
//!
//! # Precedence
//!
//! A flag beats the same key in the file, the file beats `--single-node`'s
//! defaults (§49 §17), and those beat the built-in ones (§49 Shared contracts).
//!
//! # What is refused at start, and what is only named
//!
//! * A plaintext listener off loopback is refused with FL2's message (D111). That
//!   holds for the native protocol's (9000) and the admin listener too.
//! * The TLS listener (`--house-tls-listen`, HS1 Task 20) may face other hosts.
//!   It needs a certificate and key (`--house-tls-cert`, `--house-tls-key` or
//!   `[house.tls]`); with `--single-node` and neither, a self-signed pair is
//!   made under `<data-dir>/tls/` on first start. Off loopback, no user may have
//!   an empty password. The native protocol's TLS listener
//!   (`--native-tls-listen`, 9440; HE0, R9.40) uses the same certificate, so it
//!   needs the TLS listener's configuration.
//! * `--sandbox=none` needs `--unsafe-no-sandbox` (HS1 R1.10, R6.8), except on a
//!   system with no L3 (macOS) in `--single-node` mode, and is logged either way.
//! * `--workers=inproc` needs `--single-node` and a build with `inproc-worker`.
//! * The native protocol (HE0, R9.40, ahead of HS1 Task 31) is served on
//!   `--native-listen`. `--single-node`'s default, 127.0.0.1:9000, is skipped
//!   with a note on stderr when that port is taken (a named address must bind).
//!   The admin listener serves `GET /metrics` (Task 23), loopback only.
//!   The store is read through `house-cache` by process workers (Task 9) and
//!   written by the front (Task 12).
//! * `--catalog` (HS1 Task 10) needs `--store`: the catalog of lake tables writes
//!   their metadata there (local) or places them there (REST, whose URL names the
//!   warehouse: `rest:<url>?warehouse=<name>`).

use std::io::{self, Write as _};
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use loams_cache::RangeCacheConfig;
use loams_house::catalog::{
    HouseCatalog, Lake, LakeRoot, LocalCatalog, PrefixGrants, RestCatalog, RestOptions,
};
use loams_house::config::{DEFAULT_ADMIN_LISTEN, DEFAULT_LISTEN, DEFAULT_NATIVE_LISTEN};
use loams_house::{
    CatalogSpec, CgroupLimits, ChError, HouseConfig, HouseError, HouseFile, HouseHandle, Launcher,
    PoolConfig, ProcessLauncher, SandboxMode, UserMap, WorkerCgroups, WorkerPool, WorkersMode,
    config, sandbox,
};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};

use crate::cli::HouseArgs;

/// Where `--data-dir` points when nothing says.
pub const DEFAULT_DATA_DIR: &str = "loams-house-data";

/// The worker binary's file name, looked up beside `loams-fabric` by default.
pub const WORKER_BINARY: &str = "loams-house-worker";

/// The user `--single-node` creates when no user is configured (§49 §17's
/// "generated local key"; HS1 Task 20 replaces it with the local engine's tokens
/// where the desktop runs one).
pub const LOCAL_USER: &str = "default";

/// The engine's link user (HS1 Task 20): it acts for `--principals`.
pub const LINK_USER: &str = "engine";

/// The namespace a single node's local user queries in.
pub const LOCAL_NAMESPACE: u64 = 1;

/// The local key's file under `--data-dir`, mode 0600.
pub const LOCAL_KEY_FILE: &str = "house.key";

/// The memory a worker's cgroup gets above the per-query cap (§49 §12: "the cap +
/// 512 MiB").
pub const CGROUP_HEADROOM: u64 = 512 * 1024 * 1024;

/// What `loams-fabric house` runs with, after precedence.
#[derive(Debug)]
pub struct HouseSettings {
    /// The HTTP interface.
    pub http: HouseConfig,
    /// The worker pool.
    pub pool: PoolConfig,
    /// `--single-node`.
    pub single_node: bool,
    /// Where chDB runs.
    pub workers: WorkersMode,
    /// How workers are sealed.
    pub sandbox: SandboxMode,
    /// The data directory, absolute.
    pub data_dir: PathBuf,
    /// The native protocol's plaintext listener (HE0, R9.40).
    pub native_listen: Option<SocketAddr>,
    /// Whether [`HouseSettings::native_listen`] is `--single-node`'s default
    /// rather than named: then a port already taken skips it.
    pub native_defaulted: bool,
    /// The native protocol's TLS listener (HE0, R9.40).
    pub native_tls_listen: Option<SocketAddr>,
    /// Admin and metrics: `GET /metrics` (HS1 Task 23).
    pub admin_listen: SocketAddr,
    /// The catalog (used from HS1 Task 10).
    pub catalog: Option<CatalogSpec>,
    /// The store URL (used from HS1 Tasks 9 and 12).
    pub store: Option<String>,
    /// The worker binary (`--workers=process`).
    pub worker_binary: PathBuf,
    /// The memory each worker is sized for, when configured.
    pub worker_memory_limit: Option<u64>,
    /// The delegated cgroup workers get children of.
    pub cgroup_root: Option<PathBuf>,
    /// With `--single-node` and no configured user: the file holding the local
    /// user's password, created on first start.
    pub local_key: Option<PathBuf>,
    /// The TLS listener (HS1 Task 20).
    pub tls: Option<TlsSettings>,
    /// The Loams principals the engine's link acts for (HS1 Task 20).
    pub principals: Vec<loams_house::identity::PrincipalMap>,
    /// With `--single-node`: the engine's link key's file, created on first
    /// start.
    pub link_key: Option<PathBuf>,
    /// With `--single-node`: unlisted principals act as the local user.
    pub dev_unmapped_principals: bool,
    /// Where the audit is appended.
    pub audit_log: Option<PathBuf>,
    /// The OpenTelemetry collector spans go to (HS1 Task 23).
    pub otlp_endpoint: Option<String>,
}

/// The TLS listener's settings (HS1 Task 20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsSettings {
    /// Where it listens.
    pub listen: SocketAddr,
    /// The certificate chain (PEM).
    pub cert: PathBuf,
    /// The private key (PEM).
    pub key: PathBuf,
    /// Make a self-signed pair there when neither file exists (`--single-node`).
    pub make: bool,
}

/// Turns the flags and the file into settings, refusing what must not start.
pub fn resolve(args: &HouseArgs, file: HouseFile) -> Result<HouseSettings, HouseError> {
    let house = file.house;
    let single_node = args.single_node || house.single_node.unwrap_or(false);

    let listen = args
        .house_listen
        .or(house.listen)
        .unwrap_or_else(|| addr(DEFAULT_LISTEN));
    config::check_listen(listen)?;
    // Also LOAMS_HOUSE_NATIVE_TLS_LISTEN, so a compose overlay can turn it on for a
    // private network (the Factory's SigNoz, HE1 R9.67); empty is off.
    let native_tls_env = match std::env::var("LOAMS_HOUSE_NATIVE_TLS_LISTEN") {
        Ok(text) if !text.trim().is_empty() => Some(text.trim().parse().map_err(|_| {
            bad(format!(
                "LOAMS_HOUSE_NATIVE_TLS_LISTEN {text:?}: not a socket address (host:port)"
            ))
        })?),
        _ => None,
    };
    let native_tls_listen = args
        .native_tls_listen
        .or(house.native_tls_listen)
        .or(native_tls_env);
    let named_native = args.native_listen.or(house.native_listen);
    let native_defaulted = named_native.is_none() && single_node;
    let native_listen = named_native.or_else(|| single_node.then(|| addr(DEFAULT_NATIVE_LISTEN)));
    if let Some(native) = native_listen {
        config::check_listen(native)?;
    }
    let admin_listen = args
        .admin_listen
        .or(house.admin_listen)
        .unwrap_or_else(|| addr(DEFAULT_ADMIN_LISTEN));
    config::check_listen(admin_listen)?;

    let workers = match (args.workers, house.workers.as_deref()) {
        (Some(mode), _) => mode,
        (None, Some(text)) => text.parse().map_err(bad)?,
        (None, None) => WorkersMode::Process,
    };
    if workers == WorkersMode::Inproc {
        if !single_node {
            return Err(bad(
                "--workers=inproc runs chDB inside the front with no isolation: it is for \
                 --single-node only (§49 §17)"
                    .to_string(),
            ));
        }
        if !cfg!(feature = "inproc-worker") {
            return Err(bad(
                "--workers=inproc: this loams-fabric was built without the inproc-worker \
                 feature"
                    .to_string(),
            ));
        }
    }

    let sandbox = match (args.sandbox, house.sandbox.as_deref()) {
        (Some(mode), _) => mode,
        (None, Some(text)) => text.parse().map_err(bad)?,
        (None, None) => sandbox::default_mode(),
    };
    let no_l3_here = !cfg!(target_os = "linux") && single_node;
    if workers == WorkersMode::Process
        && sandbox == SandboxMode::None
        && !args.unsafe_no_sandbox
        && !no_l3_here
    {
        return Err(bad(
            "--sandbox=none runs workers without the OS sandbox; it is development only and \
             needs --unsafe-no-sandbox (House workers run sealed: netns or pods)"
                .to_string(),
        ));
    }

    let data_dir = args
        .data_dir
        .clone()
        .or(house.data_dir)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
    let data_dir = std::path::absolute(&data_dir)
        .map_err(|err| bad(format!("--data-dir {}: {err}", data_dir.display())))?;

    let mut http = HouseConfig {
        listen,
        tmp_dir: data_dir.join("spool"),
        users: house.users.iter().map(UserMap::from).collect(),
        ..HouseConfig::default()
    };
    house.limits.apply(&mut http);
    // Metrics labels, operator lines, the query log's retention (HS1 Task 23).
    house.observability.apply(&mut http).map_err(bad)?;
    let otlp_endpoint = args
        .otlp_endpoint
        .clone()
        .or(house.observability.otlp_endpoint.clone());
    // Namespace quotas (HS1 Task 22): `[house.quotas]` until the control plane
    // serves them.
    http.quotas = std::sync::Arc::new(house.quotas.to_quotas().map_err(bad)?);
    http.admin_listen = Some(admin_listen);
    // The engine whose streams pipes read (HS1 Task 16).
    http.engine = house.engine.link().map_err(bad)?;
    if let Some(hours) = house.snapshot_retention_hours {
        http.snapshot_retention = loams_house::retention::check_retention(Duration::from_secs(
            hours.saturating_mul(3600),
        ))
        .map_err(|err| bad(format!("[house] snapshot_retention_hours: {err}")))?;
    }
    let mut pool = PoolConfig::default();
    house.pool.apply(&mut pool);
    // The watchdog's cap is the front's `max_execution_time` cap (HS1 Task 21).
    pool.max_execution_time = Duration::from_secs_f64(http.session_limits.max_execution_time_s);

    let catalog = match (&args.catalog, &house.catalog.url) {
        (Some(spec), _) => Some(spec.clone()),
        (None, Some(text)) => Some(text.parse().map_err(bad)?),
        (None, None) => single_node.then(|| CatalogSpec::Local(data_dir.join("catalog.sqlite"))),
    };
    let store =
        args.store.clone().or(house.store.url).or_else(|| {
            single_node.then(|| format!("file://{}", data_dir.join("bucket").display()))
        });
    if let Some(store) = &store
        && !is_url(store)
    {
        return Err(bad(format!(
            "--store {store:?}: expected a URL such as s3://bucket/prefix or file:///dir"
        )));
    }

    let worker_binary = match args.worker_binary.clone().or(house.pool.worker_binary) {
        Some(path) => path,
        None => beside_this_binary(WORKER_BINARY)?,
    };
    let local_key = (single_node && http.users.is_empty()).then(|| {
        args.local_key_file
            .clone()
            .unwrap_or_else(|| data_dir.join(LOCAL_KEY_FILE))
    });
    // The Loams principals the engine's link may act for (HS1 Task 20).
    let mut principals = Vec::new();
    for entry in &house.principals {
        principals.push(
            entry
                .to_map()
                .map_err(|why| bad(format!("principal {:?}: {why}", entry.principal)))?,
        );
    }
    let listed = args
        .principals
        .clone()
        .or_else(|| std::env::var("LOAMS_HOUSE_PRINCIPALS").ok())
        .filter(|list| !list.trim().is_empty());
    if let Some(list) = listed {
        if !single_node {
            return Err(bad(
                "--principals names principals in the single node's namespace: with \
                 --single-node only (elsewhere, [[house.principals]] in --config)"
                    .to_string(),
            ));
        }
        for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
            principals.push(
                loams_house::identity::PrincipalMap::parse(entry, LOCAL_NAMESPACE)
                    .map_err(|why| bad(format!("--principals: {why}")))?,
            );
        }
    }
    for (flag, set) in [
        ("--link-key-file", args.link_key_file.is_some()),
        ("--dev-unmapped-principals", args.dev_unmapped_principals),
    ] {
        if set && !single_node {
            return Err(bad(format!("{flag} is for --single-node only")));
        }
    }
    let audit_log = args
        .audit_log
        .clone()
        .or(house.audit_log)
        .or_else(|| single_node.then(|| data_dir.join("audit.log")));
    let tls = match args.house_tls_listen.or(house.tls_listen) {
        None => None,
        Some(listen) => {
            let cert = args.house_tls_cert.clone().or(house.tls.cert);
            let key = args.house_tls_key.clone().or(house.tls.key);
            match (cert, key) {
                (Some(cert), Some(key)) => Some(TlsSettings {
                    listen,
                    cert,
                    key,
                    make: single_node,
                }),
                (None, None) if single_node => Some(TlsSettings {
                    listen,
                    cert: data_dir.join("tls").join("house.crt"),
                    key: data_dir.join("tls").join("house.key"),
                    make: true,
                }),
                _ => {
                    return Err(bad(format!(
                        "TLS listener {listen}: needs --house-tls-cert and --house-tls-key \
                         (or [house.tls] cert and key)"
                    )));
                }
            }
        }
    };

    if let Some(native_tls) = native_tls_listen
        && tls.is_none()
    {
        return Err(bad(format!(
            "native TLS listener {native_tls}: needs the TLS listener's certificate \
             (--house-tls-listen with --house-tls-cert and --house-tls-key, or [house.tls])"
        )));
    }

    Ok(HouseSettings {
        http,
        pool,
        single_node,
        workers,
        sandbox,
        data_dir,
        native_listen,
        native_defaulted,
        native_tls_listen,
        admin_listen,
        catalog,
        store,
        worker_binary,
        worker_memory_limit: house.pool.worker_memory_limit,
        cgroup_root: args.cgroup_root.clone().or(house.pool.cgroup_root),
        local_key,
        tls,
        principals,
        link_key: args.link_key_file.clone(),
        dev_unmapped_principals: args.dev_unmapped_principals,
        audit_log,
        otlp_endpoint,
    })
}

/// A running `house` role.
#[derive(Debug)]
pub struct Running {
    handle: HouseHandle,
    pool: WorkerPool,
}

impl Running {
    /// Where the HTTP interface listens.
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// Where the TLS listener listens, when there is one (HS1 Task 20).
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.handle.tls_addr()
    }

    /// Where the admin listener (`GET /metrics`) listens (HS1 Task 23).
    pub fn admin_addr(&self) -> Option<SocketAddr> {
        self.handle.admin_addr()
    }

    /// Where the native protocol listens, plaintext and TLS (R9.40).
    pub fn native_addrs(&self) -> (Option<SocketAddr>, Option<SocketAddr>) {
        (self.handle.native_addr(), self.handle.native_tls_addr())
    }

    /// The worker pool.
    pub fn pool(&self) -> &WorkerPool {
        &self.pool
    }

    /// Stops accepting connections; the pool's workers die with it.
    pub fn shutdown(self) {
        self.handle.shutdown();
    }
}

/// The House data directory's format version (registry id `house.dir`, UP1
/// Task 6).
pub const DATA_DIR_VERSION: u16 = 1;

/// Writes the data directory's `FORMAT` stamp when it has none: absent means
/// 1 (design §54 §4.1), so this records what is already there.
fn stamp_data_dir(dir: &Path) -> Result<(), HouseError> {
    use loams_common::formats::{HOUSE_DIR, STAMP_FILE};
    let stamp = dir.join(STAMP_FILE);
    if stamp.exists() {
        return Ok(());
    }
    let tmp = dir.join(format!("{STAMP_FILE}.tmp"));
    std::fs::write(&tmp, format!("{} {DATA_DIR_VERSION}\n", HOUSE_DIR.as_str()))
        .and_then(|()| std::fs::rename(&tmp, &stamp))
        .map_err(|err| bad(format!("stamping {}: {err}", stamp.display())))
}

/// Starts the pool (booting one worker first, HS1 R1.12) and the HTTP interface.
pub async fn start(mut settings: HouseSettings) -> Result<Running, HouseError> {
    std::fs::create_dir_all(&settings.data_dir).map_err(|err| {
        bad(format!(
            "the data directory {}: {err}",
            settings.data_dir.display()
        ))
    })?;
    stamp_data_dir(&settings.data_dir)?;
    if let Some(path) = &settings.local_key {
        let password = local_key(path)?;
        settings
            .http
            .users
            .push(UserMap::dev(LOCAL_USER, &password, LOCAL_NAMESPACE, false));
    }
    // The engine's link and the principals it acts for (HS1 Task 20).
    if let Some(path) = &settings.link_key {
        let key = local_key(path)?;
        settings.http.users.push(UserMap::link(LINK_USER, &key));
    }
    settings.http.principals = settings.principals.clone();
    if settings.dev_unmapped_principals {
        settings.http.unmapped_principals = settings
            .http
            .users
            .iter()
            .find(|u| u.user == LOCAL_USER)
            .cloned();
    }
    if let Some(path) = &settings.audit_log {
        let sink = loams_house::audit::JsonLines::open(path)
            .map_err(|err| bad(format!("the audit log {}: {err}", path.display())))?;
        settings.http.audit = Some(Arc::new(sink));
    }
    if let Some(tls) = &settings.tls {
        if tls.make {
            let names: Vec<String> = loams_house::tls::DEFAULT_NAMES
                .iter()
                .map(|n| n.to_string())
                .collect();
            loams_house::tls::ensure_self_signed(&tls.cert, &tls.key, &names)?;
        }
        settings.http.tls = Some(loams_house::TlsListener {
            listen: tls.listen,
            config: loams_house::tls::server_config(&tls.cert, &tls.key)?,
        });
    }
    // The native protocol (HE0, R9.40): `--single-node`'s default port is
    // skipped when something else holds it.
    settings.http.native_listen = settings.native_listen;
    settings.http.native_listen_optional = settings.native_defaulted;
    settings.http.native_tls_listen = settings.native_tls_listen;
    // Spans (HS1 Task 23): to the collector, when one is named.
    if let Some(endpoint) = &settings.otlp_endpoint {
        let sink = loams_house::trace::OtlpHttp::start(endpoint, "loams-house").map_err(bad)?;
        settings.http.tracer = loams_house::trace::Tracer::new(sink);
    }
    let cache = house_cache(&settings).await?;
    if let Some(cache) = &cache {
        cache.set_tracer(Arc::clone(&settings.http.tracer));
        settings.http.cache_metrics = Some(Arc::new(cache.clone()));
    }
    let lake = lake(&settings, cache.as_ref()).await?;
    let launcher = launcher(&settings, cache)?;
    let pool = WorkerPool::start(settings.pool.clone(), launcher).await?;
    let handle = match lake {
        Some(lake) => loams_house::serve_with_lake(settings.http, pool.clone(), lake).await?,
        None => loams_house::serve(settings.http, pool.clone()).await?,
    };
    Ok(Running { handle, pool })
}

/// What [`start`] resolved but does not serve yet, one line each, for stderr.
pub fn not_served_yet(settings: &HouseSettings) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(store) = &settings.store {
        out.push(match settings.workers {
            WorkersMode::Process => {
                format!("store {store}: read through house-cache, written by the front")
            }
            WorkersMode::Inproc => format!(
                "store {store}: not read by in-process workers (they have no forwarder); \
                 written by the front"
            ),
        });
    }
    out
}

/// The catalog of lake tables (HS1 Task 10): `--catalog` over `--store`, with
/// external tables' locations vouched to `house-cache`. A REST catalog names its
/// warehouse in the URL's query, `rest:http://lakekeeper:8181/catalog?warehouse=house`.
async fn lake(
    settings: &HouseSettings,
    cache: Option<&HouseCache>,
) -> Result<Option<Arc<Lake>>, HouseError> {
    let Some(spec) = &settings.catalog else {
        return Ok(None);
    };
    let Some(store_url) = &settings.store else {
        return Err(bad(format!(
            "--catalog {spec}: needs --store, where the House keeps its tables"
        )));
    };
    let root = LakeRoot::from_store_url(store_url).map_err(bad)?;
    let catalog: Arc<dyn HouseCatalog> = match spec {
        CatalogSpec::Local(path) => {
            if let Some(dir) = store_url.strip_prefix("file://") {
                std::fs::create_dir_all(dir)
                    .map_err(|err| bad(format!("--store {store_url:?}: {err}")))?;
            }
            let store = loams_store::Store::from_url(store_url, Vec::<(String, String)>::new())
                .map_err(|err| bad(format!("--store {store_url:?}: {err}")))?;
            Arc::new(
                LocalCatalog::open(path, store, root.clone())
                    .map_err(|err| bad(format!("--catalog {spec}: {err}")))?,
            )
        }
        CatalogSpec::Rest(url) => {
            let (uri, warehouse) = rest_warehouse(url)?;
            Arc::new(
                RestCatalog::connect(RestOptions {
                    uri,
                    warehouse,
                    token: None,
                })
                .await
                .map_err(|err| bad(format!("--catalog {spec}: {err}")))?,
            )
        }
    };
    // The front reads manifests (`system.parts`, `FINAL` views; HS1 Task 11)
    // from the same store.
    let files = loams_store::Store::from_url(store_url, Vec::<(String, String)>::new())
        .map_err(|err| bad(format!("--store {store_url:?}: {err}")))?;
    let mut lake = Lake::new(catalog, root)
        .with_store(files)
        .with_retention(settings.http.snapshot_retention)
        .with_object_caps(settings.http.object_caps);
    if let Some(cache) = cache {
        lake = lake.with_grants(Arc::new(cache.clone()) as Arc<dyn PrefixGrants>);
    }
    Ok(Some(Arc::new(lake)))
}

/// A REST catalog URL and its `?warehouse=` name.
pub fn rest_warehouse(url: &str) -> Result<(String, String), HouseError> {
    let missing = || {
        bad(format!(
            "--catalog rest:{url}: name the warehouse, as rest:<url>?warehouse=<name>"
        ))
    };
    let (base, query) = url.split_once('?').ok_or_else(missing)?;
    let warehouse = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("warehouse="))
        .filter(|w| !w.is_empty())
        .ok_or_else(missing)?;
    Ok((base.to_string(), warehouse.to_string()))
}

/// `house-cache` over `--store` (HS1 Task 9): the only way process workers
/// reach the bucket. RAM only for now; its size and NVMe tier are Task 21's and
/// Task 26's.
async fn house_cache(settings: &HouseSettings) -> Result<Option<HouseCache>, HouseError> {
    let Some(url) = &settings.store else {
        return Ok(None);
    };
    if settings.workers != WorkersMode::Process {
        return Ok(None);
    }
    let upstream = Upstream::open(url, StoreOptions::default(), RangeCacheConfig::default())
        .await
        .map_err(|err| bad(format!("--store {url:?}: {err}")))?;
    Ok(Some(HouseCache::new(Arc::new(OneStore(Arc::new(
        upstream,
    ))))))
}

fn launcher(
    settings: &HouseSettings,
    cache: Option<HouseCache>,
) -> Result<Arc<dyn Launcher>, HouseError> {
    let workers_dir = settings.data_dir.join("workers");
    if settings.workers == WorkersMode::Inproc {
        return inproc(&workers_dir);
    }
    if !settings.worker_binary.is_file() {
        return Err(bad(format!(
            "the worker binary {} does not exist (--worker-binary, or install {WORKER_BINARY} \
             beside loams-fabric)",
            settings.worker_binary.display()
        )));
    }
    // D1571: the workers' profile holds the front's `max_execution_time` cap.
    let mut launcher = ProcessLauncher::new(&settings.worker_binary, workers_dir)
        .with_max_execution_time(settings.http.session_limits.max_execution_time_s.ceil() as u64);
    if settings.sandbox != launcher.sandbox() {
        launcher = match settings.sandbox {
            SandboxMode::None => launcher.unsandboxed(),
            mode => launcher.with_sandbox(mode)?,
        };
    }
    let memory = settings.worker_memory_limit.or_else(|| {
        settings
            .cgroup_root
            .is_some()
            .then(|| settings.http.session_limits.max_memory_usage + CGROUP_HEADROOM)
    });
    if let Some(bytes) = memory {
        launcher = launcher.with_memory_limit(bytes);
    }
    if let (Some(root), Some(bytes)) = (&settings.cgroup_root, memory) {
        launcher = launcher.with_cgroups(WorkerCgroups::prepare(
            root,
            CgroupLimits::for_worker(bytes),
        )?);
    }
    if let Some(cache) = cache {
        launcher = launcher.with_forward_target(Arc::new(cache));
    }
    Ok(Arc::new(launcher))
}

#[cfg(feature = "inproc-worker")]
fn inproc(dir: &Path) -> Result<Arc<dyn Launcher>, HouseError> {
    eprintln!(
        "{}",
        sandbox::unsandboxed_warning("--workers=inproc: chDB runs inside this process")
    );
    Ok(Arc::new(loams_house::InprocWorker::new(dir)))
}

#[cfg(not(feature = "inproc-worker"))]
fn inproc(_dir: &Path) -> Result<Arc<dyn Launcher>, HouseError> {
    Err(bad(
        "--workers=inproc: this loams-fabric was built without the inproc-worker feature"
            .to_string(),
    ))
}

/// The single node's local password: read from `path`, or generated (128 random
/// bits, hex) and written there with mode 0600 on first start. An existing file
/// is put back to 0600 before it is used, so a key another local user could read
/// is not served. Never printed.
pub fn local_key(path: &Path) -> Result<String, HouseError> {
    use std::os::unix::fs::PermissionsExt as _;
    let failed = |err: io::Error| bad(format!("the local key {}: {err}", path.display()));
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            let mode = std::fs::metadata(path)
                .map_err(failed)?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .map_err(failed)?;
            }
            return Ok(text.trim().to_string());
        }
        Ok(_) => return Err(failed(io::Error::other("the file is empty"))),
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(failed(err)),
        Err(_) => {}
    }
    let key = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(failed)?;
    file.write_all(format!("{key}\n").as_bytes())
        .map_err(failed)?;
    Ok(key)
}

fn beside_this_binary(name: &str) -> Result<PathBuf, HouseError> {
    let exe =
        std::env::current_exe().map_err(|err| bad(format!("where loams-fabric is: {err}")))?;
    Ok(exe.with_file_name(name))
}

fn is_url(text: &str) -> bool {
    text.split_once("://").is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && !rest.is_empty()
    })
}

fn addr(text: &str) -> SocketAddr {
    text.parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)))
}

fn bad(message: String) -> HouseError {
    HouseError::from(ChError::bad_arguments(message))
}

/// UP1 Task 6: the data directory's stamp.
#[cfg(test)]
mod format_tests {
    use loams_common::formats::{self, HOUSE_DIR, STAMP_FILE};

    use super::*;

    #[test]
    fn format_constants_match_registry() {
        assert_eq!(DATA_DIR_VERSION, formats::spec_of(HOUSE_DIR).current);
    }

    #[test]
    fn house_dir_format_stamp_absent_reads_as_one() {
        let dir = tempfile::tempdir().expect("dir");
        // A preview.2 directory has no stamp: it reads as 1.
        assert_eq!(
            formats::check_dir_stamp(dir.path(), HOUSE_DIR).expect("absent"),
            1
        );
        stamp_data_dir(dir.path()).expect("stamp");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(STAMP_FILE)).expect("read"),
            "house.dir 1\n"
        );
        assert_eq!(
            formats::check_dir_stamp(dir.path(), HOUSE_DIR).expect("stamped"),
            1
        );
        // A stamp already there is kept.
        std::fs::write(dir.path().join(STAMP_FILE), "house.dir 1\n").expect("write");
        stamp_data_dir(dir.path()).expect("again");
    }
}
