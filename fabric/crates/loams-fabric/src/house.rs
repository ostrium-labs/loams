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
//! * A plaintext listener off loopback is refused with FL2's message (D111): the
//!   House serves loopback only until TLS and the verifier (HS1 Task 20). That
//!   holds for the native and admin listeners too, though they are not served yet.
//! * A TLS listener is refused: there is no TLS until HS1 Task 20, and serving
//!   plaintext where TLS was asked for would be worse than not serving.
//! * `--sandbox=none` needs `--unsafe-no-sandbox` (HS1 R1.10, R6.8), except on a
//!   system with no L3 (macOS) in `--single-node` mode, and is logged either way.
//! * `--workers=inproc` needs `--single-node` and a build with `inproc-worker`.
//! * The native listener (Task 31), admin (Task 23), the catalog (Task 10) and
//!   the store (Tasks 9 and 12) are resolved and validated now, and [`start`]
//!   says on stderr that they are not served yet.

use std::io::{self, Write as _};
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use loams_house::config::{DEFAULT_ADMIN_LISTEN, DEFAULT_LISTEN, DEFAULT_NATIVE_LISTEN};
use loams_house::{
    CatalogSpec, CgroupLimits, ChError, HouseConfig, HouseError, HouseFile, HouseHandle, Launcher,
    PoolConfig, ProcessLauncher, SandboxMode, UserMap, WorkerCgroups, WorkerPool, WorkersMode,
    config, sandbox,
};

use crate::cli::HouseArgs;

/// Where `--data-dir` points when nothing says.
pub const DEFAULT_DATA_DIR: &str = "loams-house-data";

/// The worker binary's file name, looked up beside `loams-fabric` by default.
pub const WORKER_BINARY: &str = "loams-house-worker";

/// The user `--single-node` creates when no user is configured (§49 §17's
/// "generated local key"; HS1 Task 20 replaces it with the local engine's tokens
/// where the desktop runs one).
pub const LOCAL_USER: &str = "default";

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
    /// The native listener (not served until HS1 Task 31).
    pub native_listen: Option<SocketAddr>,
    /// Admin and metrics (not served until HS1 Task 23).
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
    if let Some(tls) = args
        .house_tls_listen
        .or(house.tls_listen)
        .or(args.native_tls_listen)
        .or(house.native_tls_listen)
    {
        return Err(bad(format!(
            "TLS listener {tls}: TLS arrives with HS1 Task 20; until then the House serves \
             loopback plaintext only (D111)"
        )));
    }
    let native_listen = args
        .native_listen
        .or(house.native_listen)
        .or_else(|| single_node.then(|| addr(DEFAULT_NATIVE_LISTEN)));
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
    let mut pool = PoolConfig::default();
    house.pool.apply(&mut pool);

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
    let local_key = (single_node && http.users.is_empty()).then(|| data_dir.join(LOCAL_KEY_FILE));

    Ok(HouseSettings {
        http,
        pool,
        single_node,
        workers,
        sandbox,
        data_dir,
        native_listen,
        admin_listen,
        catalog,
        store,
        worker_binary,
        worker_memory_limit: house.pool.worker_memory_limit,
        cgroup_root: args.cgroup_root.clone().or(house.pool.cgroup_root),
        local_key,
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

    /// The worker pool.
    pub fn pool(&self) -> &WorkerPool {
        &self.pool
    }

    /// Stops accepting connections; the pool's workers die with it.
    pub fn shutdown(self) {
        self.handle.shutdown();
    }
}

/// Starts the pool (booting one worker first, HS1 R1.12) and the HTTP interface.
pub async fn start(mut settings: HouseSettings) -> Result<Running, HouseError> {
    std::fs::create_dir_all(&settings.data_dir).map_err(|err| {
        bad(format!(
            "the data directory {}: {err}",
            settings.data_dir.display()
        ))
    })?;
    if let Some(path) = &settings.local_key {
        let password = local_key(path)?;
        settings
            .http
            .users
            .push(UserMap::dev(LOCAL_USER, &password, LOCAL_NAMESPACE, false));
    }
    let launcher = launcher(&settings)?;
    let pool = WorkerPool::start(settings.pool.clone(), launcher).await?;
    let handle = loams_house::serve(settings.http, pool.clone()).await?;
    Ok(Running { handle, pool })
}

/// What [`start`] resolved but does not serve yet, one line each, for stderr.
pub fn not_served_yet(settings: &HouseSettings) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(native) = settings.native_listen {
        out.push(format!(
            "native protocol {native}: not served until HS1 Task 31"
        ));
    }
    out.push(format!(
        "admin and metrics {}: not served until HS1 Task 23",
        settings.admin_listen
    ));
    if let Some(catalog) = &settings.catalog {
        out.push(format!("catalog {catalog}: not used until HS1 Task 10"));
    }
    if let Some(store) = &settings.store {
        out.push(format!("store {store}: not used until HS1 Tasks 9 and 12"));
    }
    out
}

fn launcher(settings: &HouseSettings) -> Result<Arc<dyn Launcher>, HouseError> {
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
    let mut launcher = ProcessLauncher::new(&settings.worker_binary, workers_dir);
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
