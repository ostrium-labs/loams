//! What the pool tests share: the worker binary, a private directory per test,
//! and one statement shape.

#![allow(dead_code)]

pub mod http;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use loams_house::{PoolConfig, ProcessLauncher, WorkerPool};
use loams_house_ipc::{Execute, Limits};

/// The worker binary this package builds.
pub const WORKER: &str = env!("CARGO_BIN_EXE_loams-house-worker");

/// A private directory for one test's workers, under the target directory rather
/// than `/tmp` (a small RAM tmpfs on the build machine).
pub fn tmp_root(test: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("house-workers")
        .join(format!("{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("test directory");
    root
}

/// A small pool: one warm worker, at most `max` in all.
pub fn small(max: usize) -> PoolConfig {
    PoolConfig {
        min_idle_workers: 1,
        max_workers: max,
        max_workers_per_namespace: max,
        acquire_timeout: Duration::from_secs(60),
        boot_timeout: Duration::from_secs(60),
        reap_every: Duration::from_millis(100),
        ..PoolConfig::default()
    }
}

/// Starts a pool of real worker processes.
pub async fn pool(test: &str, config: PoolConfig) -> WorkerPool {
    let launcher = ProcessLauncher::new(WORKER, tmp_root(test));
    WorkerPool::start(config, Arc::new(launcher))
        .await
        .unwrap_or_else(|err| panic!("the pool starts: {err}"))
}

/// A statement with no settings, views, parameters, limits or input.
pub fn statement(sql: &str, format: &str) -> Execute {
    Execute {
        query_id: format!("q-{}", sql.len()),
        session: None,
        settings: Vec::new(),
        views: Vec::new(),
        sql: sql.to_string(),
        format: format.to_string(),
        params: Vec::new(),
        limits: Limits::default(),
        input: None,
    }
}

/// Whether a pid is gone: reaped, or never ours.
pub fn pid_gone(pid: u32) -> bool {
    !PathBuf::from(format!("/proc/{pid}")).exists()
}

/// Polls `check` every 20 ms for up to `limit`.
pub async fn eventually(limit: Duration, mut check: impl FnMut() -> bool) -> bool {
    let until = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < until {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}

/// A session reference with the default 60 s idle timeout.
pub fn session(key: &str) -> Option<loams_house_ipc::SessionRef> {
    Some(loams_house_ipc::SessionRef {
        key: key.to_string(),
        timeout_ms: 60_000,
        close: false,
    })
}

/// A directory under the worktree's `scratch/` for files a test watches; removed
/// when dropped.
pub struct Scratch(std::path::PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../scratch")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir.canonicalize().expect("absolute"))
    }

    pub fn path(&self, name: &str) -> String {
        self.0.join(name).display().to_string()
    }

    pub fn files(&self) -> Vec<String> {
        std::fs::read_dir(&self.0)
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
