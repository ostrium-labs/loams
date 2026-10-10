//! [`SqlRuntime`] on one host: one `pingcap/tidb@<digest>` container per pool
//! member under Podman or Docker, host networking (the desktop and the
//! `LOAMS_IT_SQLDB` tests).
//!
//! State lives in `state_dir/<branch>/`: `pool.json` (class, replicas, the
//! ports given to each member index), the rendered `tidb.toml` and
//! `init.sql`, mounted read-only into every member. Containers carry
//! `io.loams.sqldb.*` labels; a member whose rendered config, image or class
//! changed (its fingerprint label) is replaced.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::{JobOutcome, JobSpec, Member, MemberState, PoolStatus, RuntimeError, SqlRuntime};
use crate::images::Images;
use crate::model::{BranchId, Class, Endpoints};
use crate::render;

const LABEL_INSTANCE: &str = "io.loams.sqldb.instance";
const LABEL_BRANCH: &str = "io.loams.sqldb.branch";
const LABEL_MEMBER: &str = "io.loams.sqldb.member";
const LABEL_CLASS: &str = "io.loams.sqldb.class";
const LABEL_FINGERPRINT: &str = "io.loams.sqldb.fingerprint";
const LABEL_JOB: &str = "io.loams.sqldb.job";

/// Podman or Docker, by program name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEngine {
    program: String,
}

impl ContainerEngine {
    /// Podman.
    pub fn podman() -> Self {
        Self {
            program: "podman".into(),
        }
    }

    /// Docker.
    pub fn docker() -> Self {
        Self {
            program: "docker".into(),
        }
    }

    /// Another program speaking the Podman/Docker CLI (tests use a fake).
    pub fn custom(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// The first of `podman` and `docker` that answers `--version`.
    pub fn detect() -> Option<Self> {
        [Self::podman(), Self::docker()].into_iter().find(|e| {
            std::process::Command::new(&e.program)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// The program run.
    pub fn program(&self) -> &str {
        &self.program
    }
}

/// [`LocalRuntime`]'s settings.
#[derive(Debug, Clone)]
pub struct LocalRuntimeConfig {
    /// Podman or Docker.
    pub engine: ContainerEngine,
    /// Pool records and rendered config, one directory per branch.
    pub state_dir: PathBuf,
    /// Host directory with `ca.crt`, `tls.crt`, `tls.key` (gate → TiDB TLS),
    /// mounted at [`render::TLS_DIR`].
    pub tls_dir: PathBuf,
    /// The same for TiDB → PD/TiKV, required when the endpoints use cluster
    /// TLS; mounted at [`render::CLUSTER_TLS_DIR`].
    pub cluster_tls_dir: Option<PathBuf>,
    /// PD and the gate's networks.
    pub endpoints: Endpoints,
    /// Image pins; `tidb` runs the pools, jobs name their own.
    pub images: Images,
    /// Separates runtimes sharing an engine (label `io.loams.sqldb.instance`).
    pub instance: String,
    /// The address members listen on and advertise.
    pub host: IpAddr,
    /// First MySQL port; member ports are taken from `base..base + span`.
    pub mysql_port_base: u16,
    /// First status port, same span.
    pub status_port_base: u16,
    /// Ports available above each base.
    pub port_span: u16,
    /// Pass the class CPU as `--cpus`. Memory is always limited.
    pub cpu_limits: bool,
    /// Grace before a stopped member is killed.
    pub stop_timeout: Duration,
    /// Statements appended to the rendered `init.sql`, run once at the
    /// keyspace's first bootstrap: Task 11's `ri_control` (root cannot log
    /// in, R2.10/R2.11) and tests' own users. Never a plaintext secret in
    /// production: use `IDENTIFIED WITH tidb_auth_token` or a stored hash.
    pub extra_init_sql: Vec<String>,
}

impl LocalRuntimeConfig {
    /// Defaults: the pinned images, instance `default`, loopback, MySQL ports
    /// from 24000 and status ports from 25000 (the spike stack's ranges, R1),
    /// CPU limits on, 10 s stop grace.
    pub fn new(
        engine: ContainerEngine,
        state_dir: PathBuf,
        tls_dir: PathBuf,
        endpoints: Endpoints,
    ) -> Self {
        Self {
            engine,
            state_dir,
            tls_dir,
            cluster_tls_dir: None,
            endpoints,
            images: Images::load()
                .unwrap_or_else(|e| panic!("the embedded image pins are invalid: {e}")),
            instance: "default".into(),
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            mysql_port_base: 24_000,
            status_port_base: 25_000,
            port_span: 1_000,
            cpu_limits: true,
            stop_timeout: Duration::from_secs(10),
            extra_init_sql: Vec::new(),
        }
    }
}

/// See the module docs.
#[derive(Debug)]
pub struct LocalRuntime {
    config: LocalRuntimeConfig,
    /// One lock per branch: mutations of a branch are serialised, branches
    /// proceed independently (a wake on B never waits for a stop on A).
    branch_locks: std::sync::Mutex<HashMap<BranchId, Arc<Mutex<()>>>>,
    /// Held only while ports are chosen and the pool record is saved.
    ports: std::sync::Mutex<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PoolRecord {
    class: Class,
    replicas: u32,
    /// Member index → (MySQL port, status port), kept until `delete_pool`.
    ports: BTreeMap<u32, (u16, u16)>,
}

/// A container of a pool, from `inspect`.
#[derive(Debug)]
struct Container {
    name: String,
    index: u32,
    fingerprint: String,
    status: String,
    exit_code: Option<i32>,
}

impl LocalRuntime {
    /// Checks the settings and creates `state_dir`.
    pub fn new(config: LocalRuntimeConfig) -> Result<Self, RuntimeError> {
        let bad = |m: String| Err(RuntimeError::State(m));
        if config.endpoints.cluster_tls() && config.cluster_tls_dir.is_none() {
            return bad("cluster TLS needs cluster_tls_dir".into());
        }
        if config.instance.is_empty()
            || !config
                .instance
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return bad(format!(
                "instance {:?} must be [A-Za-z0-9-]+",
                config.instance
            ));
        }
        for dir in [Some(&config.tls_dir), config.cluster_tls_dir.as_ref()]
            .into_iter()
            .flatten()
        {
            for f in ["ca.crt", "tls.crt", "tls.key"] {
                if !dir.join(f).is_file() {
                    return bad(format!("{} is missing", dir.join(f).display()));
                }
            }
        }
        std::fs::create_dir_all(&config.state_dir).map_err(|e| io_state(&config.state_dir, &e))?;
        Ok(Self {
            config,
            branch_locks: std::sync::Mutex::default(),
            ports: std::sync::Mutex::default(),
        })
    }

    /// The settings.
    pub fn config(&self) -> &LocalRuntimeConfig {
        &self.config
    }

    fn branch_lock(&self, branch: &BranchId) -> Arc<Mutex<()>> {
        let mut locks = self
            .branch_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.entry(branch.clone()).or_default().clone()
    }

    fn pool_dir(&self, branch: &BranchId) -> PathBuf {
        self.config.state_dir.join(branch.as_str())
    }

    fn load(&self, branch: &BranchId) -> Result<Option<PoolRecord>, RuntimeError> {
        let path = self.pool_dir(branch).join("pool.json");
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| RuntimeError::State(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_state(&path, &e)),
        }
    }

    fn save(&self, branch: &BranchId, record: &PoolRecord) -> Result<(), RuntimeError> {
        let json =
            serde_json::to_vec_pretty(record).map_err(|e| RuntimeError::State(e.to_string()))?;
        write_atomic(&self.pool_dir(branch).join("pool.json"), &json)
    }

    /// Writes `tidb.toml` and `init.sql`; returns the members' fingerprint.
    fn render(&self, branch: &BranchId, class: Class) -> Result<String, RuntimeError> {
        let dir = self.pool_dir(branch);
        std::fs::create_dir_all(&dir).map_err(|e| io_state(&dir, &e))?;
        let toml = render::tidb(branch, class, &self.config.endpoints);
        let mut sql = render::tidb_init_sql(class);
        for stmt in &self.config.extra_init_sql {
            sql.push_str(stmt.trim_end_matches(';'));
            sql.push_str(";\n");
        }
        write_atomic(&dir.join("tidb.toml"), toml.as_bytes())?;
        write_atomic(&dir.join("init.sql"), sql.as_bytes())?;
        let mut h = Fnv64::new();
        for part in [
            toml.as_str(),
            sql.as_str(),
            &self.config.images.tidb().reference(),
            class.name(),
            if self.config.cpu_limits { "cpu" } else { "" },
        ] {
            h.write(part.as_bytes());
            h.write(&[0]);
        }
        Ok(format!("{:016x}", h.0))
    }

    /// Gives every member index below `replicas` a free port pair.
    fn allocate(&self, record: &mut PoolRecord) -> Result<(), RuntimeError> {
        let mut used: Vec<u16> = Vec::new();
        if let Ok(dirs) = std::fs::read_dir(&self.config.state_dir) {
            for d in dirs.flatten() {
                let Ok(b) = BranchId::parse(&d.file_name().to_string_lossy()) else {
                    continue;
                };
                if let Ok(Some(r)) = self.load(&b) {
                    used.extend(r.ports.values().map(|&(p, _)| p));
                }
            }
        }
        used.extend(record.ports.values().map(|&(p, _)| p));
        for index in 0..record.replicas {
            if record.ports.contains_key(&index) {
                continue;
            }
            let (base, sbase) = (self.config.mysql_port_base, self.config.status_port_base);
            let pair = (0..self.config.port_span)
                .filter_map(|o| Some((base.checked_add(o)?, sbase.checked_add(o)?)))
                .find(|&(p, s)| {
                    !used.contains(&p)
                        && port_free(self.config.host, p)
                        && port_free(self.config.host, s)
                })
                .ok_or_else(|| RuntimeError::State("no free port pair".into()))?;
            used.push(pair.0);
            record.ports.insert(index, pair);
        }
        Ok(())
    }

    async fn engine(&self, args: &[String]) -> Result<String, RuntimeError> {
        let out = Command::new(self.config.engine.program())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| {
                RuntimeError::Unavailable(format!("{}: {e}", self.config.engine.program()))
            })?;
        if !out.status.success() {
            return Err(RuntimeError::Unavailable(format!(
                "{} {}: {}",
                self.config.engine.program(),
                args.first().map_or("", String::as_str),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn containers(&self, branch: &BranchId) -> Result<Vec<Container>, RuntimeError> {
        let ids = self
            .engine(&strings([
                "ps",
                "-aq",
                "--filter",
                &format!("label={LABEL_INSTANCE}={}", self.config.instance),
                "--filter",
                &format!("label={LABEL_BRANCH}={branch}"),
            ]))
            .await?;
        let ids: Vec<String> = ids.split_whitespace().map(str::to_owned).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let format = format!(
            "{{{{.Name}}}}\t{{{{.State.Status}}}}\t{{{{.State.ExitCode}}}}\t\
             {{{{index .Config.Labels \"{LABEL_MEMBER}\"}}}}\t{{{{index .Config.Labels \"{LABEL_FINGERPRINT}\"}}}}"
        );
        let mut args = strings(["inspect", "--format", &format]);
        args.extend(ids.iter().cloned());
        let out = match self.engine(&args).await {
            Ok(out) => out,
            // A container removed between `ps` and `inspect` fails the whole
            // inspect: look at each one, skipping those that are gone.
            Err(_) => {
                let mut out = String::new();
                for id in &ids {
                    if let Ok(one) = self
                        .engine(&strings(["inspect", "--format", &format, id]))
                        .await
                    {
                        out.push_str(&one);
                    }
                }
                out
            }
        };
        let mut list = Vec::new();
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            let [name, status, code, index, fingerprint] = f[..] else {
                return Err(RuntimeError::Unavailable(format!(
                    "unexpected inspect line {line:?}"
                )));
            };
            let Ok(index) = index.parse() else { continue };
            list.push(Container {
                name: name.trim_start_matches('/').to_owned(),
                index,
                fingerprint: fingerprint.to_owned(),
                status: status.to_owned(),
                exit_code: code.parse().ok(),
            });
        }
        list.sort_by_key(|c| c.index);
        Ok(list)
    }

    async fn remove(&self, name: &str) -> Result<(), RuntimeError> {
        let secs = self.config.stop_timeout.as_secs().to_string();
        // `stop` first so TiDB leaves PD cleanly; `rm -f` covers the rest.
        let _ = self.engine(&strings(["stop", "-t", &secs, name])).await;
        self.engine(&strings(["rm", "-f", name])).await.map(drop)
    }

    fn member_name(&self, branch: &BranchId, index: u32) -> String {
        format!("loams-sqldb-{}-{branch}-{index}", self.config.instance)
    }

    async fn start(
        &self,
        branch: &BranchId,
        class: Class,
        index: u32,
        (port, status_port): (u16, u16),
        fingerprint: &str,
    ) -> Result<(), RuntimeError> {
        let dir = self.pool_dir(branch);
        let host = self.config.host.to_string();
        let mut a = strings([
            "run",
            "-d",
            "--name",
            &self.member_name(branch, index),
            "--network",
            "host",
        ]);
        for (k, v) in [
            (LABEL_INSTANCE, self.config.instance.as_str()),
            (LABEL_BRANCH, branch.as_str()),
            (LABEL_MEMBER, &index.to_string()),
            (LABEL_CLASS, class.name()),
            (LABEL_FINGERPRINT, fingerprint),
        ] {
            a.extend(strings(["--label", &format!("{k}={v}")]));
        }
        // Swap equal to memory: the class limit is the whole budget.
        let mem = format!("{}b", class.memory_bytes());
        a.extend(strings(["--memory", &mem, "--memory-swap", &mem]));
        if self.config.cpu_limits {
            let m = class.vcpu_millis();
            a.extend(strings([
                "--cpus",
                &format!("{}.{:03}", m / 1000, m % 1000),
            ]));
        }
        let mount = |host: &Path, at: &str| format!("{}:{at}:ro,z", host.display());
        a.extend(strings([
            "-v",
            &mount(&dir.join("tidb.toml"), render::CONFIG_PATH),
        ]));
        a.extend(strings([
            "-v",
            &mount(&dir.join("init.sql"), render::INIT_SQL_PATH),
        ]));
        // The operator's TLS directory is mounted read-only and not
        // relabelled; on SELinux hosts it must already carry a container
        // label (`chcon -Rt container_file_t`).
        a.extend(strings([
            "-v",
            &format!("{}:{}:ro", self.config.tls_dir.display(), render::TLS_DIR),
        ]));
        if let (true, Some(d)) = (
            self.config.endpoints.cluster_tls(),
            &self.config.cluster_tls_dir,
        ) {
            a.extend(strings([
                "-v",
                &format!("{}:{}:ro", d.display(), render::CLUSTER_TLS_DIR),
            ]));
        }
        a.push(self.config.images.tidb().reference());
        a.extend(strings([
            &format!("--config={}", render::CONFIG_PATH),
            &format!("--host={host}"),
            &format!("--advertise-address={host}"),
            "-P",
            &port.to_string(),
            &format!("--status-host={host}"),
            &format!("--status={status_port}"),
        ]));
        if let Err(e) = self.engine(&a).await {
            // A failed `run` can leave the container `created`; remove it so
            // the next reconcile starts clean.
            let _ = self
                .engine(&strings(["rm", "-f", &self.member_name(branch, index)]))
                .await;
            return Err(e);
        }
        Ok(())
    }

    /// Brings the containers of `branch` to `record`. Under the branch lock
    /// no `run` of ours is in flight, so a member that is not `running`
    /// (`created`, `configured`, `initialized`, `exited`) is dead and replaced.
    async fn reconcile(
        &self,
        branch: &BranchId,
        record: &PoolRecord,
        fingerprint: &str,
    ) -> Result<(), RuntimeError> {
        let mut have = Vec::new();
        for c in self.containers(branch).await? {
            let keep =
                c.index < record.replicas && c.fingerprint == fingerprint && c.status == "running";
            if keep {
                have.push(c.index);
            } else {
                self.remove(&c.name).await?;
            }
        }
        for index in (0..record.replicas).filter(|i| !have.contains(i)) {
            let ports = *record
                .ports
                .get(&index)
                .ok_or_else(|| RuntimeError::State(format!("member {index} has no ports")))?;
            self.start(branch, record.class, index, ports, fingerprint)
                .await?;
        }
        Ok(())
    }

    async fn status(
        &self,
        branch: &BranchId,
        record: &PoolRecord,
    ) -> Result<PoolStatus, RuntimeError> {
        let mut members = Vec::new();
        for c in self.containers(branch).await? {
            let Some(&(port, status_port)) = record.ports.get(&c.index) else {
                continue;
            };
            let mysql_addr = SocketAddr::new(self.config.host, port);
            let state = match c.status.as_str() {
                "running" if port_open(mysql_addr).await => MemberState::Ready,
                s if is_live(s) => MemberState::Starting,
                _ => MemberState::Exited { code: c.exit_code },
            };
            members.push(Member {
                index: c.index,
                name: c.name,
                mysql_addr,
                status_addr: SocketAddr::new(self.config.host, status_port),
                state,
            });
        }
        Ok(PoolStatus {
            branch: branch.clone(),
            class: record.class,
            replicas: record.replicas,
            members,
        })
    }

    async fn resize(
        &self,
        branch: &BranchId,
        record: &mut PoolRecord,
    ) -> Result<PoolStatus, RuntimeError> {
        let fingerprint = self.render(branch, record.class)?;
        {
            // Other branches allocate too: choose and save under one lock.
            let _p = self
                .ports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.allocate(record)?;
            self.save(branch, record)?;
        }
        self.reconcile(branch, record, &fingerprint).await?;
        self.status(branch, record).await
    }
}

#[async_trait]
impl SqlRuntime for LocalRuntime {
    async fn ensure_pool(
        &self,
        branch: &BranchId,
        class: Class,
        replicas: u32,
    ) -> Result<PoolStatus, RuntimeError> {
        let lock = self.branch_lock(branch);
        let _g = lock.lock().await;
        let mut record = self.load(branch)?.unwrap_or(PoolRecord {
            class,
            replicas,
            ports: BTreeMap::new(),
        });
        record.class = class;
        record.replicas = replicas;
        self.resize(branch, &mut record).await
    }

    async fn scale(&self, branch: &BranchId, replicas: u32) -> Result<PoolStatus, RuntimeError> {
        let lock = self.branch_lock(branch);
        let _g = lock.lock().await;
        let mut record = self
            .load(branch)?
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))?;
        record.replicas = replicas;
        self.resize(branch, &mut record).await
    }

    async fn pool_status(&self, branch: &BranchId) -> Result<Option<PoolStatus>, RuntimeError> {
        match self.load(branch)? {
            Some(record) => self.status(branch, &record).await.map(Some),
            None => Ok(None),
        }
    }

    async fn delete_pool(&self, branch: &BranchId) -> Result<(), RuntimeError> {
        let lock = self.branch_lock(branch);
        let _g = lock.lock().await;
        for c in self.containers(branch).await? {
            self.remove(&c.name).await?;
        }
        let dir = self.pool_dir(branch);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_state(&dir, &e)),
        }
    }

    async fn run_job(&self, spec: &JobSpec) -> Result<JobOutcome, RuntimeError> {
        if spec.name.is_empty()
            || !spec
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(RuntimeError::State(format!(
                "job name {:?} must be [A-Za-z0-9_.-]+",
                spec.name
            )));
        }
        let name = format!("loams-sqldb-{}-job-{}", self.config.instance, spec.name);
        let mut a = strings(["run", "--rm", "--name", &name, "--network", "host"]);
        a.extend(strings([
            "--label",
            &format!("{LABEL_INSTANCE}={}", self.config.instance),
        ]));
        a.extend(strings(["--label", &format!("{LABEL_JOB}={}", spec.name)]));
        for (k, v) in &spec.env {
            a.extend(strings(["-e", &format!("{k}={v}")]));
        }
        a.push(spec.image.reference());
        a.extend(spec.args.iter().cloned());
        // A container of the same name left by a crash would refuse the
        // name; the replayed job replaces it.
        let _ = self.engine(&strings(["rm", "-f", &name])).await;
        let child = Command::new(self.config.engine.program())
            .args(&a)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                RuntimeError::Unavailable(format!("{}: {e}", self.config.engine.program()))
            })?;
        match tokio::time::timeout(spec.timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let mut log = out.stdout;
                log.extend_from_slice(&out.stderr);
                let tail = &log[log.len().saturating_sub(4096)..];
                Ok(JobOutcome {
                    exit_code: out.status.code().unwrap_or(-1),
                    log_tail: String::from_utf8_lossy(tail).into_owned(),
                })
            }
            Ok(Err(e)) => Err(RuntimeError::Unavailable(e.to_string())),
            Err(_) => {
                let _ = self.engine(&strings(["rm", "-f", &name])).await;
                Err(RuntimeError::JobTimeout(spec.name.clone()))
            }
        }
    }
}

fn is_live(status: &str) -> bool {
    matches!(
        status,
        "created" | "configured" | "initialized" | "running" | "restarting"
    )
}

fn strings<const N: usize>(a: [&str; N]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

fn io_state(path: &Path, e: &std::io::Error) -> RuntimeError {
    RuntimeError::State(format!("{}: {e}", path.display()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| io_state(&tmp, &e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_state(path, &e))
}

fn port_free(host: IpAddr, port: u16) -> bool {
    TcpListener::bind((host, port)).is_ok()
}

async fn port_open(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(addr)
        )
        .await,
        Ok(Ok(_))
    )
}

/// FNV-1a, 64-bit: a fingerprint stable across Rust releases.
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
