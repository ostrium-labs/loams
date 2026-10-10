//! L3, the worker's OS sandbox (HS1 Task 6, §49 §13.2), against real workers.
//!
//! Every statement here goes straight to a worker through `WorkerLease::run`,
//! which is the deny list (L1) switched off: L1 is the front's HTTP path, and
//! nothing in the worker checks it again. What stops these statements is the
//! worker's own controls (L2) and the OS sandbox (L3); the tests that matter
//! most are the ones L2 does not stop at all (HS1 R1.10, R5.7): the scalar
//! `file()`, `INTO OUTFILE` and `executable()`, each also run once on an
//! unsealed worker to show that the sandbox is what refuses them. The
//! `SandboxProbe` hook does things natively in the worker (exec, read, write,
//! connect), so the OS layer is shown on its own as well.
//!
//! Linux only; elsewhere the file is empty, and says so.

#![cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, unused_imports, unused_mut, unused_variables)
)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Scratch, small, statement, tmp_root};
use loams_house::{
    CgroupLimits, HouseError, Outcome, ProcessLauncher, SandboxMode, WorkerCgroups, WorkerLease,
    WorkerPool,
};
use loams_house_ipc::{FORWARDER_PORT, SandboxProbe};

/// What the test S3 object holds.
const OBJECT: &str = "through-the-forwarder\n";

/// A loopback HTTP server on the host that answers every `HEAD` and `GET` with
/// [`OBJECT`] (S3 enough for chDB's `s3()` on one key) and remembers every
/// request line. It stands in for `house-cache` (HS1 Task 9) as the forwarder's
/// upstream, and is also the "test server on the host" a worker must not reach.
struct Upstream {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Upstream {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("upstream listener");
        let addr = listener.local_addr().expect("address");
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (seen, log) = (Arc::clone(&connections), Arc::clone(&requests));
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                seen.fetch_add(1, Ordering::SeqCst);
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve_http(conn, &log));
            }
        });
        Self {
            addr,
            connections,
            requests,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("log").clone()
    }
}

fn serve_http(conn: TcpStream, log: &Mutex<Vec<String>>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(mut writer) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(conn);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let request = line.trim_end().to_string();
        let mut range = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("range")
            {
                range = value.trim().strip_prefix("bytes=").and_then(|r| {
                    let (from, to) = r.split_once('-')?;
                    let from: usize = from.parse().ok()?;
                    let to: usize = to.parse().unwrap_or(OBJECT.len() - 1);
                    Some((from, to.min(OBJECT.len() - 1)))
                });
            }
        }
        log.lock().expect("log").push(request.clone());
        let head = request.starts_with("HEAD ");
        let (status, body, extra) = match range {
            Some((from, to)) if from <= to => (
                "206 Partial Content",
                &OBJECT.as_bytes()[from..=to],
                format!("Content-Range: bytes {from}-{to}/{}\r\n", OBJECT.len()),
            ),
            _ => ("200 OK", OBJECT.as_bytes(), String::new()),
        };
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\
             ETag: \"loams\"\r\nLast-Modified: Thu, 01 Oct 2026 00:00:00 GMT\r\n\
             Accept-Ranges: bytes\r\n{extra}\r\n",
            body.len()
        );
        if writer.write_all(header.as_bytes()).is_err() {
            return;
        }
        if !head && writer.write_all(body).is_err() {
            return;
        }
    }
}

/// A pool of one sealed worker whose forwarder leads to `upstream`.
async fn sealed(test: &str, upstream: &Upstream) -> (WorkerPool, PathBuf) {
    let root = tmp_root(test);
    let launcher = ProcessLauncher::new(common::WORKER, &root).with_forwarder(upstream.addr);
    assert_eq!(launcher.sandbox(), SandboxMode::Netns, "sealed by default");
    let pool = WorkerPool::start(small(1), Arc::new(launcher))
        .await
        .expect("the pool starts");
    (pool, root)
}

/// The same, with the sandbox forced off: the control that shows a refusal is
/// the sandbox's.
async fn unsealed(test: &str) -> WorkerPool {
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test)).unsandboxed();
    assert_eq!(launcher.sandbox(), SandboxMode::None);
    WorkerPool::start(small(1), Arc::new(launcher))
        .await
        .expect("the pool starts")
}

async fn run(lease: &mut WorkerLease, sql: &str) -> Result<Vec<u8>, HouseError> {
    let mut execute = statement(sql, "TSV");
    execute.session = common::session("sandbox");
    // chDB's S3 client retries a refused connection for minutes by default.
    execute.settings = vec![("s3_retry_attempts".to_string(), "0".to_string())];
    execute.limits.max_execution_time_ms = Some(20_000);
    tokio::time::timeout(Duration::from_secs(60), lease.run(execute))
        .await
        .unwrap_or_else(|_| panic!("{sql}: bounded"))
        .map(|collected| collected.bytes)
}

/// `/proc/<pid>/status` field `name`.
fn status_field(pid: u32, name: &str) -> String {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("status");
    status
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}:")))
        .unwrap_or_else(|| panic!("{name} in /proc/{pid}/status"))
        .trim()
        .to_string()
}

/// §49 §13.2: with L1 off, every way out the threat table names still fails,
/// and the ones only L3 stops are shown to be stopped by L3 (they succeed on an
/// unsealed worker).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_blocks_with_deny_list_off() {
    let host = Upstream::start();
    let upstream = Upstream::start();
    let scratch = Scratch::new("sandbox-blocks");
    let (pool, _root) = sealed("sandbox-blocks", &upstream).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    run(
        &mut lease,
        "CREATE TEMPORARY TABLE t (a String) ENGINE = Memory",
    )
    .await
    .expect("a temporary table");

    let tmp_outfile = format!("/tmp/loams-sandbox-{}.tsv", std::process::id());
    let scratch_outfile = scratch.path("outfile.tsv");
    let port = host.port();
    // Each refusal, and the words that say which layer refused it.
    let refused = [
        // L2's grants (HS1 R1.8), and L3 behind them.
        (
            format!("SELECT * FROM url('http://127.0.0.1:{port}/x', LineAsString)"),
            "ACCESS_DENIED",
        ),
        (
            format!("SELECT * FROM s3('http://127.0.0.1:{port}/bucket/k', 'k', 's', LineAsString)"),
            "ACCESS_DENIED",
        ),
        (
            format!("SELECT * FROM remote('127.0.0.1:{port}', system.one)"),
            "ACCESS_DENIED",
        ),
        (
            "SELECT * FROM file('/etc/passwd', LineAsString)".to_string(),
            "ACCESS_DENIED",
        ),
        // Only L1 and L3 stop these (HS1 R1.10, R5.7): Landlock's EACCES.
        (
            "SELECT file('/etc/passwd')".to_string(),
            "Permission denied",
        ),
        (
            format!("SELECT 'x' INTO OUTFILE '{tmp_outfile}' FORMAT TSV"),
            "Permission denied",
        ),
        (
            format!("SELECT 'x' INTO OUTFILE '{scratch_outfile}' FORMAT TSV"),
            "Permission denied",
        ),
        // L2's missing scripts directory; seccomp's `execve` ban is behind it
        // (`worker_cannot_exec`).
        (
            "SELECT * FROM executable('cat', TSV, 'a String')".to_string(),
            "user scripts folder",
        ),
        // Expressions in VALUES are off (R5.6): 344, literals only.
        (
            "INSERT INTO t VALUES (file('/etc/passwd'))".to_string(),
            "SUPPORT_IS_DISABLED",
        ),
    ];
    for (sql, why) in &refused {
        match run(&mut lease, sql).await {
            Ok(bytes) => panic!(
                "{sql} must fail, answered {:?}",
                String::from_utf8_lossy(&bytes)
            ),
            Err(err) => assert!(err.to_string().contains(why), "{sql}: {err}"),
        }
    }
    assert!(
        !Path::new(&tmp_outfile).exists(),
        "{tmp_outfile} was written"
    );
    assert!(
        !Path::new(&scratch_outfile).exists(),
        "{scratch_outfile} was written"
    );
    assert_eq!(host.connections(), 0, "the host server was reached");
    pool.release(lease, Outcome::Completed);

    // The control: on an unsealed worker, L2 alone lets the scalar file() and
    // INTO OUTFILE through, so the refusals above are L3's.
    let open = unsealed("sandbox-blocks-control").await;
    let mut lease = open.acquire("ns").await.expect("worker");
    let passwd = run(&mut lease, "SELECT file('/etc/passwd')")
        .await
        .expect("an unsealed worker reads host files: the sandbox is what stops it");
    assert!(!passwd.is_empty());
    run(
        &mut lease,
        &format!("SELECT 'x' INTO OUTFILE '{scratch_outfile}' FORMAT TSV"),
    )
    .await
    .expect("an unsealed worker writes host files: the sandbox is what stops it");
    assert!(Path::new(&scratch_outfile).exists());
    open.release(lease, Outcome::Completed);
}

/// The worker's network namespace holds only `lo`; the forwarder reaches the
/// upstream, and nothing else is reachable: not the host's ports, not another
/// address, not a Unix socket on the host.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_reaches_only_its_forwarder() {
    let host = Upstream::start();
    let upstream = Upstream::start();
    let scratch = Scratch::new("sandbox-net");
    let unix_path = scratch.path("host.sock");
    let unix = std::os::unix::net::UnixListener::bind(&unix_path).expect("a host unix socket");
    unix.set_nonblocking(true).expect("nonblocking");
    let (pool, _root) = sealed("sandbox-net", &upstream).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();

    // Its own network namespace, with only the loopback in it.
    let ours = std::fs::read_link("/proc/self/ns/net").expect("our netns");
    let theirs = std::fs::read_link(format!("/proc/{pid}/ns/net")).expect("its netns");
    assert_ne!(ours, theirs, "the worker shares the front's network");
    let devices = std::fs::read_to_string(format!("/proc/{pid}/net/dev")).expect("its devices");
    let names: Vec<&str> = devices
        .lines()
        .skip(2)
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .collect();
    assert_eq!(names, vec!["lo"], "{devices}");

    // chDB reads the bucket through the forwarder.
    let sql = format!(
        "SELECT * FROM s3('http://127.0.0.1:{FORWARDER_PORT}/bucket/key.txt', 'dummy', 'dummy', LineAsString)"
    );
    let bytes = run(&mut lease, &sql)
        .await
        .expect("a read through the forwarder");
    assert_eq!(String::from_utf8_lossy(&bytes), OBJECT);
    assert!(
        upstream
            .requests()
            .iter()
            .any(|line| line.contains("/bucket/key.txt")),
        "{:?}",
        upstream.requests()
    );

    // Natively: the forwarder's port answers; the host's own port, another
    // address and a host Unix socket do not.
    lease
        .probe_for_test(SandboxProbe::Connect(format!("127.0.0.1:{FORWARDER_PORT}")))
        .await
        .expect("the forwarder is reachable");
    for target in [
        format!("127.0.0.1:{}", host.port()),
        format!("127.0.0.1:{}", upstream.port()),
        "1.1.1.1:80".to_string(),
        "10.0.0.1:443".to_string(),
    ] {
        let err = lease
            .probe_for_test(SandboxProbe::Connect(target.clone()))
            .await
            .expect_err(&target);
        assert!(err.to_string().contains("PROBE_REFUSED"), "{target}: {err}");
    }
    let err = lease
        .probe_for_test(SandboxProbe::UnixConnect(unix_path.clone()))
        .await
        .expect_err("a host unix socket");
    assert!(
        err.to_string().contains("Operation not permitted")
            || err.to_string().contains("Permission denied"),
        "{err}"
    );
    assert!(unix.accept().is_err(), "the host unix socket was reached");
    assert_eq!(host.connections(), 0, "the host server was reached");
    pool.release(lease, Outcome::Completed);
}

/// No `execve`, no new process, no signal to another process: seccomp. `executable()` fails for that reason as
/// well as its missing scripts directory (L2). The worker runs with no new
/// privileges and no capabilities.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_cannot_exec() {
    let upstream = Upstream::start();
    let (pool, _root) = sealed("sandbox-exec", &upstream).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();
    assert_eq!(status_field(pid, "Seccomp"), "2", "a seccomp filter");
    assert_eq!(status_field(pid, "NoNewPrivs"), "1");
    assert_eq!(status_field(pid, "CapEff"), "0000000000000000");
    assert_eq!(status_field(pid, "CapPrm"), "0000000000000000");

    for program in ["/bin/true", "/usr/bin/env", "/bin/sh"] {
        let err = lease
            .probe_for_test(SandboxProbe::Exec(program.to_string()))
            .await
            .expect_err(program);
        assert!(
            err.to_string().contains("Operation not permitted"),
            "{program}: {err}"
        );
    }
    // No signal leaves the worker (PR #391 review): not to the front, and not
    // to any other process of its user; to itself it still may.
    let front = std::process::id();
    let err = lease
        .probe_for_test(SandboxProbe::Signal(front))
        .await
        .expect_err("a signal to the front");
    assert!(err.to_string().contains("Operation not permitted"), "{err}");
    lease
        .probe_for_test(SandboxProbe::Signal(pid))
        .await
        .expect("a signal to itself");
    let err = run(
        &mut lease,
        "SELECT * FROM executable('cat', TSV, 'a String')",
    )
    .await
    .expect_err("executable()");
    // L2 refuses it first (no scripts directory); the probes above show that an
    // `execve` behind it would be refused too.
    assert!(err.to_string().contains("user scripts folder"), "{err}");
    // Still serving: a refused exec is an error, not a crash.
    let one = run(&mut lease, "SELECT 1").await.expect("still serving");
    assert_eq!(one, b"1\n");
    pool.release(lease, Outcome::Completed);
}

/// Landlock: the worker writes only its private directory, and reads only what
/// it runs and the few kernel files chDB sizes itself from.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_cannot_write_outside_tmp() {
    let upstream = Upstream::start();
    let scratch = Scratch::new("sandbox-fs");
    let (pool, root) = sealed("sandbox-fs", &upstream).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let own = root.join(lease.worker_id()).join("files");

    let tmp_file = format!("/tmp/loams-sandbox-write-{}", std::process::id());
    for path in [
        scratch.path("probe"),
        tmp_file.clone(),
        root.join("sibling").display().to_string(),
    ] {
        let err = lease
            .probe_for_test(SandboxProbe::Write(path.clone()))
            .await
            .expect_err(&path);
        assert!(
            err.to_string().contains("Permission denied"),
            "{path}: {err}"
        );
        assert!(!Path::new(&path).exists(), "{path} was written");
    }
    lease
        .probe_for_test(SandboxProbe::Write(own.join("probe").display().to_string()))
        .await
        .expect("its own directory is writable");

    let front_environ = format!("/proc/{}/environ", std::process::id());
    for path in [
        "/etc/passwd",
        "/etc/hostname",
        "/home",
        front_environ.as_str(),
    ] {
        let err = lease
            .probe_for_test(SandboxProbe::Read(path.to_string()))
            .await
            .expect_err(path);
        assert!(
            err.to_string().contains("Permission denied")
                || err.to_string().contains("Is a directory"),
            "{path}: {err}"
        );
    }
    lease
        .probe_for_test(SandboxProbe::Read("/proc/self/status".to_string()))
        .await
        .expect("its own /proc");

    // Through chDB: INTO OUTFILE lands only in its own directory.
    let outside = scratch.path("outfile.tsv");
    run(
        &mut lease,
        &format!("SELECT 'x' INTO OUTFILE '{outside}' FORMAT TSV"),
    )
    .await
    .expect_err("INTO OUTFILE outside");
    assert!(!Path::new(&outside).exists());
    run(
        &mut lease,
        "SELECT 'x' INTO OUTFILE 'inside.tsv' FORMAT TSV",
    )
    .await
    .expect("INTO OUTFILE in its own directory");
    assert!(own.join("inside.tsv").exists());
    let _ = std::fs::remove_file(&tmp_file);
    pool.release(lease, Outcome::Completed);
}

/// A cgroup v2 directory this test may delegate to workers: a fresh sibling of
/// the test's own cgroup, when the parent is writable and has the controllers.
#[cfg(target_os = "linux")]
fn delegated_cgroup() -> Result<PathBuf, String> {
    let ours = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let path = ours
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("not on cgroup v2")?;
    let mine = Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
    let parent = mine.parent().ok_or("no parent cgroup")?;
    let controllers = std::fs::read_to_string(parent.join("cgroup.subtree_control"))
        .map_err(|e| e.to_string())?;
    for needed in ["memory", "cpu", "pids"] {
        if !controllers.split_whitespace().any(|c| c == needed) {
            return Err(format!("{} does not delegate {needed}", parent.display()));
        }
    }
    let root = parent.join(format!("loams-house-test-{}", std::process::id()));
    std::fs::create_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    Ok(root)
}

/// Each worker joins a cgroup child the front made, with §49 §12's limits, and
/// the child goes when the worker does.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cgroup_limits_applied() {
    let root = match delegated_cgroup() {
        Ok(root) => root,
        Err(why) => {
            eprintln!("cgroup_limits_applied: skipped: no delegated cgroup here ({why})");
            return;
        }
    };
    let limits = CgroupLimits {
        memory_max: Some(3 * 1024 * 1024 * 1024),
        cpus: Some(2),
        pids_max: Some(1500),
    };
    let cgroups = WorkerCgroups::prepare(&root, limits).expect("the delegated cgroup");
    let upstream = Upstream::start();
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root("sandbox-cgroup"))
        .with_forwarder(upstream.addr)
        .with_cgroups(cgroups);
    let pool = WorkerPool::start(small(1), Arc::new(launcher))
        .await
        .expect("the pool starts");
    let mut lease = pool.acquire("ns").await.expect("worker");
    let pid = lease.pid();
    let child = root.join(lease.worker_id());
    let member = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).expect("its cgroup");
    let relative = child
        .strip_prefix("/sys/fs/cgroup")
        .expect("under the hierarchy");
    assert_eq!(
        member.trim(),
        format!("0::/{}", relative.display()),
        "the worker is in its own child"
    );
    let read = |file: &str| {
        std::fs::read_to_string(child.join(file))
            .expect(file)
            .trim()
            .to_string()
    };
    assert_eq!(read("memory.max"), (3u64 * 1024 * 1024 * 1024).to_string());
    assert_eq!(read("cpu.max"), "200000 100000");
    assert_eq!(read("pids.max"), "1500");
    if child.join("memory.swap.max").exists() {
        assert_eq!(read("memory.swap.max"), "0");
    }
    let threads: u64 = read("pids.current").parse().expect("a count");
    assert!(threads > 1 && threads < 1500, "{threads} threads");
    assert_eq!(run(&mut lease, "SELECT 1").await.expect("serves"), b"1\n");

    // The child goes with the worker.
    pool.kill(lease, loams_house::ExitReason::Cancel);
    let gone = common::eventually(Duration::from_secs(10), || !child.exists()).await;
    pool.shutdown();
    let _ = common::eventually(Duration::from_secs(10), || {
        std::fs::read_dir(&root).map_or(true, |entries| {
            entries
                .filter_map(Result::ok)
                .all(|entry| !entry.path().is_dir())
        })
    })
    .await;
    let _ = std::fs::remove_dir(&root);
    assert!(gone, "{} outlived its worker", child.display());
}

/// `--sandbox=pods` (§49 §13.2's fallback): on a kind cluster, a worker pod's
/// egress to anything but the front's forwarder port times out. Needs a kind
/// cluster with a NetworkPolicy-enforcing CNI and `kubectl` pointed at it:
/// `LOAMS_HOUSE_KIND=1 cargo test -p loams-house-worker --test sandbox -- --ignored`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs a kind cluster: LOAMS_HOUSE_KIND=1 and kubectl's context"]
fn pods_fallback_network_policy() {
    if std::env::var_os("LOAMS_HOUSE_KIND").is_none() {
        eprintln!("pods_fallback_network_policy: skipped: LOAMS_HOUSE_KIND is not set");
        return;
    }
    let manifest =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../deploy/house/kind/worker-pods.yaml");
    let kubectl = |args: &[&str]| {
        let output = std::process::Command::new("kubectl")
            .args(args)
            .output()
            .expect("kubectl");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        )
    };
    let namespace = "loams-house-pods-test";
    // A namespace left by an earlier run must be gone first, and a new one's
    // `default` service account appears a moment after it: retry the apply.
    let _ = kubectl(&[
        "wait",
        "--for=delete",
        &format!("namespace/{namespace}"),
        "--timeout=120s",
    ]);
    let manifest = manifest.display().to_string();
    let mut applied = kubectl(&["apply", "-f", &manifest]);
    for _ in 0..10 {
        if applied.0 {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        applied = kubectl(&["apply", "-f", &manifest]);
    }
    assert!(applied.0, "{}", applied.1);
    for (label, kind) in [
        ("loams-house-front", "front"),
        ("loams-house-worker", "worker"),
    ] {
        let (ok, out) = kubectl(&[
            "-n",
            namespace,
            "wait",
            "--for=condition=Ready",
            &format!("pod/{label}"),
            "--timeout=180s",
        ]);
        assert!(ok, "the {kind} pod: {out}");
    }
    let (_, front_ip) = kubectl(&[
        "-n",
        namespace,
        "get",
        "pod",
        "loams-house-front",
        "-o",
        "jsonpath={.status.podIP}",
    ]);
    // A TCP connect from the worker pod, 3 s at most: true if it connected.
    let probe = |host: &str, port: u16| {
        kubectl(&[
            "-n",
            namespace,
            "exec",
            "loams-house-worker",
            "--",
            "python3",
            "-c",
            "import socket, sys; socket.create_connection((sys.argv[1], int(sys.argv[2])), 3)",
            host,
            &port.to_string(),
        ])
        .0
    };
    let front_ip = front_ip.trim();
    let allowed = probe(front_ip, FORWARDER_PORT);
    let elsewhere = probe(front_ip, 8123);
    let outside = probe("1.1.1.1", 80);
    let _ = kubectl(&["delete", "namespace", namespace, "--wait=false"]);
    assert!(allowed, "the worker pod reaches the front's forwarder port");
    assert!(!elsewhere, "the worker pod reached the front's other port");
    assert!(!outside, "the worker pod reached the internet");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn sandbox_tests_are_linux_only() {
    eprintln!("tests/sandbox.rs: skipped: L3 (netns, Landlock, seccomp, cgroup v2) is Linux-only");
}
