//! The worker binary driven through the front's supervisor (HS1 Task 2): every
//! test here starts real `loams-house-worker` processes with `posix_spawn`, talks
//! `hsw1` to them over fd 3 and kills them with `SIGKILL`.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{eventually, pid_gone, pool, small, statement, tmp_root};
use loams_house::{ExitReason, Launcher, Outcome, PoolConfig, ProcessLauncher};
use loams_house_ipc::{Bind, EXIT_PROTOCOL, Frame, FrameCodec, InputSpec, PROTOCOL_VERSION};

fn bind(namespace: &str) -> Frame {
    Frame::Bind(Bind {
        namespace: namespace.to_string(),
        isolation_class: "shared".to_string(),
        settings: Vec::new(),
        proxy_endpoint: None,
        temp_dir_quota_bytes: 1 << 30,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_answers_select_one_in_every_format() {
    let pool = pool("formats", small(1)).await;
    let mut lease = pool.acquire("ns-formats").await.expect("a worker");
    let ready = lease.ready().expect("ready").clone();
    assert_eq!(ready.clickhouse_version, "26.9.2.1", "versions.rs's pin");
    assert_eq!(ready.chdb_version, "26.9.0");

    for format in [
        "TSV",
        "CSV",
        "JSONEachRow",
        "RowBinary",
        "Native",
        "Parquet",
        "Arrow",
        "ArrowStream",
    ] {
        let out = lease
            .run(statement("SELECT 1 AS one", format))
            .await
            .unwrap_or_else(|err| panic!("{format}: {err}"));
        let bytes = out.bytes;
        assert_eq!(out.stats.result_rows, 1, "{format}: {:?}", out.stats);
        assert!(out.stats.rss_bytes > 0, "the worker reports its memory");
        match format {
            "TSV" | "CSV" => assert_eq!(bytes, b"1\n", "{format}"),
            "JSONEachRow" => assert_eq!(bytes, b"{\"one\":1}\n"),
            "RowBinary" => assert_eq!(bytes, vec![1u8]),
            "Native" => {
                // FL2 Task 1's measured shape: no BlockInfo; columns, rows, then
                // name and type as varstrings, then the data.
                assert_eq!(&bytes[..2], &[1, 1], "one column, one row");
                assert!(bytes.windows(3).any(|w| w == b"one"));
                assert!(bytes.windows(5).any(|w| w == b"UInt8"));
                assert_eq!(bytes.last(), Some(&1));
            }
            "Parquet" => {
                use parquet::file::reader::FileReader;
                let reader =
                    parquet::file::reader::SerializedFileReader::new(Bytes::from(bytes.clone()))
                        .expect("a Parquet file");
                assert_eq!(reader.metadata().file_metadata().num_rows(), 1);
            }
            "Arrow" => {
                let reader =
                    arrow::ipc::reader::FileReader::try_new(std::io::Cursor::new(bytes), None)
                        .expect("an Arrow IPC file");
                let rows: usize = reader.map(|b| b.expect("batch").num_rows()).sum();
                assert_eq!(rows, 1);
            }
            "ArrowStream" => {
                let reader =
                    arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
                        .expect("an Arrow IPC stream");
                let rows: usize = reader.map(|b| b.expect("batch").num_rows()).sum();
                assert_eq!(rows, 1);
            }
            other => panic!("{other}"),
        }
    }
    pool.release(lease, Outcome::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_is_the_cancel() {
    let pool = pool("cancel", small(2)).await;
    let mut lease = pool.acquire("ns-cancel").await.expect("a worker");
    let pid = lease.pid();

    lease
        .start(statement("SELECT count() FROM numbers(1e12)", "TSV"))
        .await
        .expect("started");
    // The handle is for this statement: taken after `start` (review I1).
    let handle = lease.kill_handle().expect("handle");
    let killer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let at = Instant::now();
        handle.kill(ExitReason::Cancel);
        at
    });
    let err = loop {
        match lease.next_event().await {
            Ok(_) => continue,
            Err(err) => break err,
        }
    };
    let answered = Instant::now();
    let killed_at = killer.await.expect("killer");
    assert_eq!(err.code(), 394, "{err}");
    assert_eq!(err.name(), "QUERY_WAS_CANCELLED");
    assert!(
        answered.duration_since(killed_at) < Duration::from_secs(1),
        "the client is answered within 1 s of the cancel, took {:?}",
        answered.duration_since(killed_at)
    );
    pool.release(lease, Outcome::Completed);

    assert!(
        eventually(Duration::from_secs(2), || pid_gone(pid)).await,
        "the worker {pid} is gone"
    );
    assert_eq!(
        pool.stats().kills_for(ExitReason::Cancel),
        1,
        "{:?}",
        pool.stats()
    );
    assert!(
        eventually(Duration::from_secs(20), || pool.stats().idle_unbound >= 1).await,
        "the pool is replenished: {:?}",
        pool.stats()
    );
    let mut next = pool.acquire("ns-cancel").await.expect("another worker");
    assert_ne!(next.pid(), pid);
    assert_eq!(
        next.run(statement("SELECT 1", "TSV"))
            .await
            .expect("serves")
            .bytes,
        b"1\n"
    );
    pool.release(next, Outcome::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_does_not_reach_the_front() {
    let pool = pool("crash", small(3)).await;
    let mut doomed = pool.acquire("ns-a").await.expect("a worker");
    let mut other = pool.acquire("ns-b").await.expect("another worker");
    let doomed_pid = doomed.pid();

    doomed
        .start(statement("SELECT count() FROM numbers(1e12)", "TSV"))
        .await
        .expect("started");
    doomed.abort_for_test().await.expect("Abort sent");
    let err = loop {
        match doomed.next_event().await {
            Ok(_) => continue,
            Err(err) => break err,
        }
    };
    assert_eq!(err.code(), 210, "{err}");
    assert_eq!(err.name(), "NETWORK_ERROR");
    pool.release(doomed, Outcome::Completed);
    assert!(eventually(Duration::from_secs(2), || pid_gone(doomed_pid)).await);
    assert_eq!(
        pool.stats().kills_for(ExitReason::Crash),
        1,
        "{:?}",
        pool.stats()
    );

    // The front is untouched and the other worker keeps serving.
    assert_eq!(
        other
            .run(statement("SELECT 2", "TSV"))
            .await
            .expect("serves")
            .bytes,
        b"2\n"
    );
    pool.release(other, Outcome::Completed);
    let mut again = pool.acquire("ns-a").await.expect("ns-a is served again");
    assert_eq!(
        again
            .run(statement("SELECT 3", "TSV"))
            .await
            .expect("serves")
            .bytes,
        b"3\n"
    );
    pool.release(again, Outcome::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binding_is_exclusive() {
    // One worker per node: namespace B can only be served by retiring A's.
    let pool = pool("binding", small(1)).await;
    let mut a = pool.acquire("ns-a").await.expect("a");
    let a_pid = a.pid();
    a.run(statement("SELECT 1", "TSV")).await.expect("a runs");
    pool.release(a, Outcome::Completed);

    let mut a2 = pool.acquire("ns-a").await.expect("a again");
    assert_eq!(a2.pid(), a_pid, "a namespace gets its own idle worker back");
    pool.release(a2, Outcome::Completed);

    let mut b = pool.acquire("ns-b").await.expect("b");
    assert_ne!(b.pid(), a_pid, "a worker bound to ns-a never serves ns-b");
    assert_eq!(b.namespace(), "ns-b");
    b.run(statement("SELECT 1", "TSV")).await.expect("b runs");
    assert!(
        eventually(Duration::from_secs(2), || pid_gone(a_pid)).await,
        "ns-a's worker was retired to make room, not rebound"
    );
    assert_eq!(pool.stats().kills_for(ExitReason::Idle), 1);
    pool.release(b, Outcome::Completed);
    a2 = pool.acquire("ns-b").await.expect("b again");
    assert_eq!(a2.namespace(), "ns-b");
    pool.release(a2, Outcome::Completed);
}

#[test]
fn second_bind_is_refused_by_the_worker() {
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root("rebind"));
    let launched = launcher.launch("rebind").expect("launch");
    let mut socket: UnixStream = launched.socket;
    match FrameCodec::read(&mut socket).expect("frame") {
        Some(Frame::Ready(ready)) => assert_eq!(ready.protocol, PROTOCOL_VERSION),
        other => panic!("expected Ready, got {other:?}"),
    }
    FrameCodec::write(&mut socket, &bind("ns-a")).expect("Bind");
    assert_eq!(
        FrameCodec::read(&mut socket).expect("frame"),
        Some(Frame::Done)
    );
    FrameCodec::write(&mut socket, &bind("ns-b")).expect("Bind again");
    match FrameCodec::read(&mut socket).expect("frame") {
        Some(Frame::Error { error, poisoned }) => {
            assert!(poisoned, "a rebind attempt poisons the worker");
            assert_eq!(error.name, "LOAMS_ALREADY_BOUND");
            assert!(error.message.contains("ns-a"), "{error}");
        }
        other => panic!("expected Error, got {other:?}"),
    }
    launched.control.terminate();
}

#[test]
fn unknown_protocol_version_exits_70() {
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root("version"));
    let launched = launcher.launch("version").expect("launch");
    let mut socket = launched.socket;
    assert!(matches!(
        FrameCodec::read(&mut socket).expect("frame"),
        Some(Frame::Ready(_))
    ));
    let mut wire = FrameCodec::encode(&bind("ns")).expect("encode");
    wire[4] = PROTOCOL_VERSION + 1;
    socket.write_all(&wire).expect("write");

    let mut exit = launched.control.exit();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let status = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = exit.borrow().clone() {
                    return status;
                }
                exit.changed().await.expect("exit watch");
            }
        })
        .await
    });
    assert_eq!(
        status.expect("the worker exits promptly"),
        format!("exited with {EXIT_PROTOCOL}")
    );
    let mut rest = Vec::new();
    let _ = socket.read_to_end(&mut rest);
    assert!(rest.is_empty(), "nothing answered the bad frame");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recycle_after_budget() {
    let config = PoolConfig {
        max_queries_per_worker: 3,
        ..small(2)
    };
    let pool = pool("budget", config).await;
    let mut first_pid = None;
    for n in 0..3 {
        let mut lease = pool.acquire("ns-budget").await.expect("worker");
        first_pid.get_or_insert(lease.pid());
        assert_eq!(
            Some(lease.pid()),
            first_pid,
            "statement {n} reuses the worker"
        );
        lease.run(statement("SELECT 1", "TSV")).await.expect("runs");
        pool.release(lease, Outcome::Completed);
    }
    let first_pid = first_pid.expect("pid");
    assert_eq!(
        pool.stats().kills_for(ExitReason::Budget),
        1,
        "{:?}",
        pool.stats()
    );
    assert!(eventually(Duration::from_secs(2), || pid_gone(first_pid)).await);
    let mut fourth = pool.acquire("ns-budget").await.expect("worker");
    assert_ne!(
        fourth.pid(),
        first_pid,
        "the fourth statement gets a new worker"
    );
    fourth
        .run(statement("SELECT 1", "TSV"))
        .await
        .expect("runs");
    pool.release(fourth, Outcome::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_bound_worker_is_retired() {
    let config = PoolConfig {
        idle_unbind_after: Duration::from_millis(300),
        ..small(2)
    };
    let pool = pool("idle", config).await;
    let mut lease = pool.acquire("ns-idle").await.expect("worker");
    let pid = lease.pid();
    lease.run(statement("SELECT 1", "TSV")).await.expect("runs");
    pool.release(lease, Outcome::Completed);
    assert!(
        eventually(Duration::from_secs(5), || pid_gone(pid)).await,
        "a bound worker idle past idle_unbind_after is killed"
    );
    assert_eq!(pool.stats().kills_for(ExitReason::Idle), 1);
    assert_eq!(pool.stats().bound.get("ns-idle"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_kill_answers_159() {
    let pool = pool("deadline", small(2)).await;
    let mut lease = pool.acquire("ns-deadline").await.expect("worker");
    lease
        .start(statement("SELECT count() FROM numbers(1e12)", "TSV"))
        .await
        .expect("started");
    // The handle is for this statement: taken after `start` (review I1).
    let handle = lease.kill_handle().expect("handle");
    let _deadline = handle.kill_at(
        tokio::time::Instant::now() + Duration::from_millis(300),
        ExitReason::Timeout,
    );
    let err = loop {
        match lease.next_event().await {
            Ok(_) => continue,
            Err(err) => break err,
        }
    };
    assert_eq!(err.code(), 159, "{err}");
    pool.release(lease, Outcome::Completed);
    assert_eq!(pool.stats().kills_for(ExitReason::Timeout), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_errors_pass_through_and_the_worker_stays() {
    let pool = pool("engine-error", small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();
    let err = lease
        .run(statement("SELECT * FROM nope_loams", "TSV"))
        .await
        .expect_err("unknown table");
    assert_eq!(err.code(), 60, "{err}");
    assert_eq!(err.name(), "UNKNOWN_TABLE");
    assert_eq!(
        lease
            .run(statement("SELECT 1", "TSV"))
            .await
            .expect("still serves")
            .bytes,
        b"1\n"
    );
    assert_eq!(lease.pid(), pid);
    pool.release(lease, Outcome::Completed);
    assert_eq!(pool.stats().kills.values().sum::<u64>(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_is_sealed_by_its_own_config() {
    let pool = pool("sealed", small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let tsv = |out: loams_house::Collected| String::from_utf8(out.bytes).expect("utf-8");

    // HS1 R1.9: `default` is a Memory database, created right after the drop.
    let engine = lease
        .run(statement(
            "SELECT engine FROM system.databases WHERE name = 'default'",
            "TSV",
        ))
        .await
        .expect("runs");
    assert_eq!(tsv(engine), "Memory\n");

    // HS1 R1.9: the user connection is readonly = 2, and it sticks.
    let readonly = lease
        .run(statement("SELECT getSetting('readonly')", "TSV"))
        .await
        .expect("runs");
    assert_eq!(tsv(readonly), "2\n");
    let mut set = statement("SELECT 1", "TSV");
    set.settings = vec![("readonly".to_string(), "0".to_string())];
    assert_eq!(lease.run(set).await.expect_err("sticky").code(), 164);

    // HS1 R1.8: no FILE, URL or REMOTE grant; R1.5's settings are pinned.
    for (sql, code) in [
        ("SELECT * FROM file('/etc/hostname', 'LineAsString')", 497),
        (
            "SELECT * FROM url('http://127.0.0.1:1/x', 'LineAsString')",
            497,
        ),
        ("SELECT * FROM remote('127.0.0.1:1', system.one)", 497),
    ] {
        let err = lease.run(statement(sql, "TSV")).await.expect_err(sql);
        assert_eq!(err.code(), code, "{sql}: {err}");
    }
    let mut pinned = statement("SELECT 1", "TSV");
    pinned.settings = vec![("allow_insert_into_iceberg".to_string(), "1".to_string())];
    assert_eq!(lease.run(pinned).await.expect_err("pinned").code(), 452);

    // HS1 R1.4: the metadata caches are sized from the worker's memory.
    let caches = lease
        .run(statement(
            "SELECT value FROM system.server_settings WHERE name = 'parquet_metadata_cache_size'",
            "TSV",
        ))
        .await
        .expect("runs");
    let expected = loams_house_worker::config::CacheSizes::for_memory(
        loams_house_worker::config::DEFAULT_MEMORY_LIMIT,
    );
    assert_eq!(tsv(caches), format!("{}\n", expected.parquet_metadata));

    // A bad setting name never reaches a SET statement.
    let mut bad = statement("SELECT 1", "TSV");
    bad.settings = vec![("x = 1; SELECT 2".to_string(), "1".to_string())];
    assert_eq!(lease.run(bad).await.expect_err("refused").code(), 115);
    pool.release(lease, Outcome::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_frames_stream_into_an_insert() {
    let pool = pool("input", small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let mut create = statement(
        "CREATE TEMPORARY TABLE staged (n UInt64) ENGINE = Memory",
        "TSV",
    );
    create.session = Some("s1".to_string());
    lease.run(create).await.expect("temporary table");

    let mut insert = statement("SELECT count(), sum(n) FROM staged", "TSV");
    insert.session = Some("s1".to_string());
    insert.input = Some(InputSpec {
        insert: "INSERT INTO staged".to_string(),
        format: "TSV".to_string(),
    });
    lease.start(insert).await.expect("started");
    let body = (1..=1000).map(|n| format!("{n}\n")).collect::<String>();
    for piece in body.as_bytes().chunks(777) {
        lease
            .send_input(Bytes::copy_from_slice(piece))
            .await
            .expect("Input");
    }
    lease.end_input().await.expect("InputEnd");
    let mut out = Vec::new();
    let stats = loop {
        match lease.next_event().await.expect("event") {
            loams_house::Event::Chunk(chunk) => out.extend_from_slice(&chunk.bytes),
            loams_house::Event::Progress(_) => {}
            loams_house::Event::Done(stats) => break stats,
        }
    };
    assert_eq!(out, b"1000\t500500\n");
    assert_eq!(stats.written_rows, 1000, "{stats:?}");

    // A bad body fails the statement, the rest of it is drained, and the worker
    // stays in step.
    let mut bad = statement("", "TSV");
    bad.session = Some("s1".to_string());
    bad.input = Some(InputSpec {
        insert: "INSERT INTO staged".to_string(),
        format: "Parquet".to_string(),
    });
    lease.start(bad).await.expect("started");
    lease
        .send_input(Bytes::from_static(b"not parquet"))
        .await
        .expect("Input");
    lease.end_input().await.expect("InputEnd");
    let err = loop {
        match lease.next_event().await {
            Ok(_) => continue,
            Err(err) => break err,
        }
    };
    assert_ne!(err.code(), 0, "{err}");
    assert_eq!(
        lease
            .run(statement("SELECT 1", "TSV"))
            .await
            .expect("in step")
            .bytes,
        b"1\n"
    );
    pool.release(lease, Outcome::Completed);
}

/// The worker's environment and arguments carry nothing of the front's secrets.
///
/// Edition 2024 makes `set_var` `unsafe`, which the workspace forbids, so the
/// front's secrets are planted by running the real check, [`env_inner`], in a
/// child test process started with them in its environment.
#[test]
fn worker_env_has_no_secret() {
    const CANARY: &str = "loams-canary-7f3a9c";
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "env_inner", "--ignored", "--nocapture"])
        .env("AWS_SECRET_ACCESS_KEY", CANARY)
        .env("AWS_ACCESS_KEY_ID", CANARY)
        .env("LOAMS_HOUSE_STORE_SECRET", CANARY)
        .env("LOAMS_API_TOKEN", CANARY)
        .env("LOAMS_TEST_CANARY", CANARY)
        .output()
        .expect("the inner test runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the inner test failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run by worker_env_has_no_secret with secrets in its environment"]
async fn env_inner() {
    let canary = std::env::var("LOAMS_TEST_CANARY").expect("started by worker_env_has_no_secret");
    assert_eq!(
        std::env::var("AWS_SECRET_ACCESS_KEY").as_deref(),
        Ok(canary.as_str())
    );
    let pool = pool("env", small(1)).await;
    let mut lease = pool.acquire("ns-env").await.expect("worker");
    lease.run(statement("SELECT 1", "TSV")).await.expect("runs");
    let pid = lease.pid();

    let environ = std::fs::read(format!("/proc/{pid}/environ")).expect("environ");
    assert!(
        environ.is_empty(),
        "the worker's environment is empty, got {:?}",
        String::from_utf8_lossy(&environ)
    );
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).expect("cmdline");
    let cmdline = String::from_utf8_lossy(&cmdline);
    assert!(!cmdline.contains(&canary), "argv: {cmdline}");
    assert!(
        cmdline.contains("--worker-id"),
        "argv is WorkerArgs: {cmdline}"
    );

    // Nor do the files the worker wrote for itself.
    let dir = tmp_root_of(&cmdline);
    for file in ["config.xml", "users.xml"] {
        let text = std::fs::read_to_string(dir.join(file)).expect(file);
        assert!(!text.contains(&canary), "{file}");
        assert!(!text.to_lowercase().contains("secret"), "{file}");
    }
    pool.release(lease, Outcome::Completed);
}

/// The `--tmp-dir` in a NUL-separated command line.
fn tmp_root_of(cmdline: &str) -> std::path::PathBuf {
    let parts: Vec<&str> = cmdline.split('\0').collect();
    let at = parts
        .iter()
        .position(|p| *p == "--tmp-dir")
        .expect("--tmp-dir");
    std::path::PathBuf::from(parts[at + 1])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acquire_waits_then_answers_202() {
    let config = PoolConfig {
        max_workers_per_namespace: 1,
        acquire_timeout: Duration::from_millis(300),
        ..small(2)
    };
    let pool = pool("queue", config).await;
    let held = pool.acquire("ns").await.expect("first");
    let err = pool
        .acquire("ns")
        .await
        .expect_err("namespace is at its limit");
    assert_eq!(err.code(), 202, "{err}");
    let pool2 = pool.clone();
    let waiter = tokio::spawn(async move { pool2.acquire("ns").await.map(|l| l.pid()) });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let held_pid = held.pid();
    pool.release(held, Outcome::Completed);
    assert_eq!(
        waiter.await.expect("task").expect("handed over"),
        held_pid,
        "a released worker goes to the waiting caller"
    );
}

/// HS1 Task 2 review I3: a worker busy in a statement still exits as soon as its
/// socket closes. The reader thread `_exit`s; the statement is not waited for.
#[test]
fn busy_worker_exits_when_its_socket_closes() {
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root("orphan"));
    let launched = launcher.launch("orphan").expect("launch");
    let mut socket = launched.socket;
    assert!(matches!(
        FrameCodec::read(&mut socket).expect("frame"),
        Some(Frame::Ready(_))
    ));
    FrameCodec::write(&mut socket, &bind("ns")).expect("Bind");
    assert_eq!(
        FrameCodec::read(&mut socket).expect("frame"),
        Some(Frame::Done)
    );
    FrameCodec::write(
        &mut socket,
        &Frame::Execute(statement("SELECT count() FROM numbers(1e12)", "TSV")),
    )
    .expect("Execute");
    std::thread::sleep(Duration::from_millis(300));
    assert!(launched.control.exit().borrow().is_none(), "still running");

    let closed = Instant::now();
    socket.shutdown(std::net::Shutdown::Both).expect("shutdown");
    drop(socket);
    let mut exit = launched.control.exit();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let status = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(status) = exit.borrow().clone() {
                    return status;
                }
                exit.changed().await.expect("exit watch");
            }
        })
        .await
    });
    let status = status.unwrap_or_else(|_| {
        launched.control.terminate();
        panic!("the worker was still alive 1 s after its socket closed")
    });
    assert_eq!(status, "exited with 0");
    assert!(closed.elapsed() < Duration::from_secs(1));
}

/// HS1 Task 2 review I1: a `KillHandle` is for one statement of one lease. A kill
/// that arrives after `Done` and `release` — a late `KILL QUERY`, a deadline that
/// fired late — must not kill the next lease's statement on the same worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_kill_handle_cannot_kill_a_later_query() {
    let pool = pool("stale-handle", small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();
    lease
        .run(statement("SELECT 1", "TSV"))
        .await
        .expect("first");
    let stale = lease.kill_handle().expect("handle");

    // A later statement on the same lease.
    lease
        .start(statement("SELECT 2", "TSV"))
        .await
        .expect("started");
    assert_eq!(
        stale.kill(ExitReason::Cancel),
        None,
        "stale within the lease"
    );
    let mut out = Vec::new();
    loop {
        match lease.next_event().await.expect("second statement survives") {
            loams_house::Event::Chunk(chunk) => out.extend_from_slice(&chunk.bytes),
            loams_house::Event::Progress(_) => {}
            loams_house::Event::Done(_) => break,
        }
    }
    assert_eq!(out, b"2\n");
    let after_done = lease.kill_handle().expect("handle");
    pool.release(lease, Outcome::Completed);

    // Kill right after Done and release, then the next lease's statement.
    assert_eq!(
        after_done.kill(ExitReason::Cancel),
        None,
        "stale after release"
    );
    let mut next = pool.acquire("ns").await.expect("same worker");
    assert_eq!(next.pid(), pid, "the worker was not killed");
    assert_eq!(stale.kill(ExitReason::Timeout), None);
    assert_eq!(
        next.run(statement("SELECT 3", "TSV"))
            .await
            .expect("completes")
            .bytes,
        b"3\n"
    );
    pool.release(next, Outcome::Completed);
    assert_eq!(
        pool.stats().kills.values().sum::<u64>(),
        0,
        "{:?}",
        pool.stats()
    );
}

/// A launcher whose workers, while `hang` is set, never send `Ready`: an
/// `acquire` that has to start one sits in boot for as long as it is awaited.
#[derive(Debug)]
struct HangingLauncher {
    real: ProcessLauncher,
    hang: std::sync::atomic::AtomicBool,
    held: std::sync::Mutex<Vec<UnixStream>>,
}

#[derive(Debug)]
struct NeverReady {
    exit: tokio::sync::watch::Sender<Option<String>>,
}

impl loams_house::watchdog::WorkerControl for NeverReady {
    fn pid(&self) -> u32 {
        0
    }
    fn terminate(&self) {
        self.exit.send_replace(Some("terminated".to_string()));
    }
    fn exit(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        self.exit.subscribe()
    }
}

impl Launcher for HangingLauncher {
    fn launch(&self, id: &str) -> Result<loams_house::watchdog::Launched, loams_house::HouseError> {
        if !self.hang.load(std::sync::atomic::Ordering::SeqCst) {
            return self.real.launch(id);
        }
        let (front, worker) = UnixStream::pair().expect("pair");
        self.held.lock().expect("lock").push(worker);
        let (exit, _) = tokio::sync::watch::channel(None);
        Ok(loams_house::watchdog::Launched {
            socket: front,
            control: std::sync::Arc::new(NeverReady { exit }),
        })
    }
}

/// HS1 Task 2 review I2: dropping an `acquire` future while it boots a worker
/// gives the slot back. Before the fix the node stayed one worker short forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_acquire_during_boot_keeps_capacity() {
    let launcher = std::sync::Arc::new(HangingLauncher {
        real: ProcessLauncher::new(common::WORKER, tmp_root("cancel-safe")),
        hang: std::sync::atomic::AtomicBool::new(false),
        held: std::sync::Mutex::new(Vec::new()),
    });
    let config = PoolConfig {
        min_idle_workers: 0,
        max_workers: 2,
        max_workers_per_namespace: 2,
        acquire_timeout: Duration::from_secs(5),
        ..small(2)
    };
    let pool = loams_house::WorkerPool::start(config, launcher.clone())
        .await
        .expect("pool");
    let held = pool.acquire("ns-a").await.expect("the booted worker");

    launcher
        .hang
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let pool2 = pool.clone();
    let stuck = tokio::spawn(async move { pool2.acquire("ns-b").await.map(|l| l.pid()) });
    assert!(
        eventually(Duration::from_secs(2), || pool.stats().booting == 1).await,
        "the second acquire is booting: {:?}",
        pool.stats()
    );
    stuck.abort();
    assert!(stuck.await.is_err(), "cancelled");
    assert!(
        eventually(Duration::from_secs(1), || pool.stats().booting == 0).await,
        "the dropped acquire gave its slot back: {:?}",
        pool.stats()
    );

    launcher
        .hang
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let mut b = pool.acquire("ns-b").await.expect("capacity is intact");
    assert_eq!(
        b.run(statement("SELECT 1", "TSV"))
            .await
            .expect("runs")
            .bytes,
        b"1\n"
    );
    pool.release(b, Outcome::Completed);
    pool.release(held, Outcome::Completed);
}

/// HS1 Task 2 review M2: errors a user can cause at will do not retire the worker,
/// and a user cannot fake chDB's fatal-signal error to make the front report a
/// crash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_caused_errors_do_not_poison_the_worker() {
    let pool = pool("no-poison", small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();

    let nul = lease
        .run(statement("SELECT 1\0", "TSV"))
        .await
        .expect_err("a NUL byte in the SQL is refused");
    assert_eq!(nul.code(), 0, "a Loams-side refusal: {nul}");

    let mut custom = statement("SELECT 1", "TSV");
    custom.settings = vec![(
        "allow_custom_error_code_in_throwif".to_string(),
        "1".to_string(),
    )];
    assert_eq!(lease.run(custom).await.expect_err("pinned").code(), 452);
    let faked = lease
        .run(statement(
            "SELECT throwIf(1, 'The server is shutting down due to a fatal error', 236)",
            "TSV",
        ))
        .await
        .expect_err("throwIf");
    assert_ne!(faked.code(), 236, "a custom code is refused: {faked}");
    assert_ne!(faked.code(), 210, "not reported as a crash: {faked}");

    assert_eq!(
        lease
            .run(statement("SELECT 1", "TSV"))
            .await
            .expect("still serves")
            .bytes,
        b"1\n"
    );
    pool.release(lease, Outcome::Completed);
    let mut again = pool.acquire("ns").await.expect("worker");
    assert_eq!(again.pid(), pid, "the same worker, not retired as poisoned");
    pool.release(again, Outcome::Completed);
    assert_eq!(
        pool.stats().kills.values().sum::<u64>(),
        0,
        "{:?}",
        pool.stats()
    );
}
