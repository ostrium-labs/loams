//! `InprocWorker` (feature `inproc-worker`, `--workers=inproc`): the same pool
//! and the same `hsw1` frames, with the worker's serve loop on a thread of this
//! process. Not isolated, and says so (§49 §4.1, Q686).

#![cfg(feature = "inproc-worker")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use loams_house::{ExitReason, InprocWorker, Outcome, PoolConfig, WorkerPool};
use loams_house_ipc::{Execute, Limits};

fn statement(sql: &str) -> Execute {
    Execute {
        query_id: "inproc-q".to_string(),
        session: None,
        settings: Vec::new(),
        views: Vec::new(),
        sql: sql.to_string(),
        format: "TSV".to_string(),
        params: Vec::new(),
        limits: Limits::default(),
        input: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inproc_worker_serves_and_cancels() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("house-inproc");
    let config = PoolConfig {
        min_idle_workers: 1,
        max_workers: 2,
        ..PoolConfig::default()
    };
    let launcher = InprocWorker::new(root);
    assert!(
        !launcher.engine_config().install_signal_handlers,
        "chDB's signal handlers stay out of the front's process (review I5)"
    );
    let pool = WorkerPool::start(config, Arc::new(launcher))
        .await
        .expect("the pool starts");

    let mut lease = pool.acquire("ns").await.expect("worker");
    assert_eq!(lease.pid(), std::process::id(), "in this process");
    let out = lease
        .run(statement("SELECT version()"))
        .await
        .expect("runs");
    assert_eq!(out.bytes, b"26.9.2.1\n");
    let err = lease
        .run(statement(
            "SELECT * FROM file('/etc/hostname', 'LineAsString')",
        ))
        .await
        .expect_err("the worker's grants apply in-process too");
    assert_eq!(err.code(), 497, "{err}");

    let handle = lease.kill_handle().expect("handle");
    lease
        .start(statement(
            "SELECT sleepEachRow(0.5) FROM numbers(4) SETTINGS max_block_size = 1",
        ))
        .await
        .expect("started");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let at = Instant::now();
    handle.kill(ExitReason::Cancel);
    let err = loop {
        match lease.next_event().await {
            Ok(_) => continue,
            Err(err) => break err,
        }
    };
    assert_eq!(err.code(), 394, "{err}");
    assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
    pool.release(lease, Outcome::Completed);
    assert_eq!(pool.stats().kills_for(ExitReason::Cancel), 1);

    let mut next = pool.acquire("ns").await.expect("another worker");
    assert_eq!(
        next.run(statement("SELECT 1")).await.expect("runs").bytes,
        b"1\n"
    );
    pool.release(next, Outcome::Completed);

    // Not isolated: the killed statement keeps its thread until it ends, and this
    // process must not exit before then (libchdb's static destructors racing it
    // crash the process once chDB's handlers are off, review I5).
    assert!(
        handle.wait_exit(Duration::from_secs(10)).await.is_some(),
        "the in-process worker's thread ends when its statement does"
    );
}
