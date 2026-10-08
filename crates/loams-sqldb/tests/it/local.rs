//! LocalRuntime against the spike stack (`scripts/sqldb/spike/up.sh`: PD on
//! 127.0.0.1:29379, TiKV on 30160). Ignored unless `LOAMS_IT_SQLDB=1`.
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use loams_sqldb::images::Images;
use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::runtime::local::{ContainerEngine, LocalRuntime, LocalRuntimeConfig};
use loams_sqldb::runtime::{MemberState, PoolStatus, SqlRuntime};
use mysql_async::prelude::Queryable;

const PD_HTTP: &str = "http://127.0.0.1:29379";
/// Inside the spike stack's TiDB range (24000+N, 25000+N), above Task 1's N.
const MYSQL_PORT_BASE: u16 = 24100;
const STATUS_PORT_BASE: u16 = 25100;

/// Removes every container this runtime labelled, even when the test panics.
struct Cleanup {
    engine: ContainerEngine,
    instance: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let label = format!("io.loams.sqldb.instance={}", self.instance);
        let ids = Command::new(self.engine.program())
            .args(["ps", "-aq", "--filter", &format!("label={label}")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        for id in ids.split_whitespace() {
            let _ = Command::new(self.engine.program())
                .args(["rm", "-f", "-t", "0", id])
                .output();
        }
    }
}

fn new_branch() -> BranchId {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    BranchId::parse(&format!("br_{:016x}", n as u64)).expect("id")
}

/// Creates the branch's keyspace. PD answers 500 while it is still settling
/// after start (seen right after `up.sh`), so a few retries.
fn create_keyspace(name: &str) {
    let mut last = String::new();
    for _ in 0..10 {
        let out = Command::new("curl")
            .args([
                "-fsS",
                "-X",
                "POST",
                &format!("{PD_HTTP}/pd/api/v2/keyspaces"),
            ])
            .args(["-H", "content-type: application/json"])
            .args(["-d", &format!("{{\"name\":\"{name}\"}}")])
            .output()
            .expect("curl");
        if out.status.success() {
            return;
        }
        last = String::from_utf8_lossy(&out.stderr).into_owned();
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("create keyspace {name}: {last}");
}

/// A self-signed CA, server certificate and key for TiDB's [security] paths.
fn write_tls(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("tls dir");
    let run = |args: &[&str]| {
        let out = Command::new("openssl")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("openssl");
        assert!(
            out.status.success(),
            "openssl {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "2",
        "-subj",
        "/CN=loams-it-ca",
        "-keyout",
        "ca.key",
        "-out",
        "ca.crt",
    ]);
    run(&[
        "req",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        "/CN=127.0.0.1",
        "-keyout",
        "tls.key",
        "-out",
        "tls.csr",
    ]);
    run(&[
        "x509",
        "-req",
        "-in",
        "tls.csr",
        "-CA",
        "ca.crt",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-days",
        "2",
        "-out",
        "tls.crt",
    ]);
}

async fn wait_ready(rt: &LocalRuntime, b: &BranchId, want: usize, within: Duration) -> PoolStatus {
    let deadline = Instant::now() + within;
    loop {
        let st = rt
            .pool_status(b)
            .await
            .expect("status")
            .expect("pool exists");
        if st.ready() >= want {
            return st;
        }
        assert!(
            !st.members
                .iter()
                .any(|m| matches!(m.state, MemberState::Exited { .. })),
            "a member exited: {st:?}"
        );
        assert!(Instant::now() < deadline, "not ready in {within:?}: {st:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// root@localhost over the member's Unix socket (auth_socket, R2.10). Under
/// rootless Podman the host user is the container's root.
async fn root(rt: &LocalRuntime, b: &BranchId, index: u32) -> mysql_async::Conn {
    let socket = rt
        .socket_path(b, index)
        .expect("state")
        .expect("member has a port");
    let opts = mysql_async::OptsBuilder::default()
        .socket(Some(socket.to_string_lossy().into_owned()))
        .user(Some("root"));
    mysql_async::Conn::new(opts)
        .await
        .expect("root over the socket")
}

async fn q(conn: &mut mysql_async::Conn, sql: &str) -> String {
    let v: Option<String> = conn
        .query_first(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    v.expect("one row")
}

fn engine_out(rt: &LocalRuntime, args: &[&str]) -> String {
    let out = Command::new(rt.config().engine.program())
        .args(args)
        .output()
        .expect("engine");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[tokio::test(flavor = "multi_thread")]
async fn local_runtime_starts_and_stops_a_pool() {
    let engine = ContainerEngine::detect().expect("podman or docker");
    let branch = new_branch();
    create_keyspace(branch.as_str());

    let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sqldb-it-{branch}"));
    let tls_dir = state_dir.join("tls");
    write_tls(&tls_dir);
    let instance = format!("it-{}", std::process::id());
    let _cleanup = Cleanup {
        engine: engine.clone(),
        instance: instance.clone(),
    };

    let endpoints = Endpoints::new(
        vec!["127.0.0.1:29379".into()],
        vec!["10.89.0.0/16".parse().expect("cidr")],
    )
    .expect("endpoints");
    let mut config = LocalRuntimeConfig::new(engine, state_dir.join("pools"), tls_dir, endpoints);
    config.images = Images::load().expect("pins");
    config.instance = instance.clone();
    config.mysql_port_base = MYSQL_PORT_BASE;
    config.status_port_base = STATUS_PORT_BASE;
    // A first bootstrap at 0.25 vCPU takes minutes; memory stays enforced.
    config.cpu_limits = false;
    let rt = LocalRuntime::new(config).expect("runtime");

    // One member: the keyspace's first bootstrap.
    let st = rt
        .ensure_pool(&branch, Class::Xs, 1)
        .await
        .expect("ensure_pool");
    assert_eq!((st.class, st.replicas, st.members.len()), (Class::Xs, 1, 1));
    let st = wait_ready(&rt, &branch, 1, Duration::from_secs(240)).await;
    let m0 = st.members[0].clone();
    assert_eq!(m0.mysql_addr.port(), MYSQL_PORT_BASE);
    let logs = engine_out(&rt, &["logs", &m0.name]);
    assert!(
        logs.contains("bootstrap successful"),
        "first start bootstraps:\n{logs}"
    );
    // Gate → TiDB TLS is loaded from the mounted [security] paths.
    assert!(
        logs.contains("secure connection is enabled"),
        "TLS not enabled:\n{logs}"
    );
    // init.sql ran, and no statement in it failed (failures are warnings only).
    assert!(logs.contains("executing -initialize-sql-file"), "{logs}");
    assert!(
        !logs.contains("InitializeSQLFile error"),
        "init.sql failed:\n{logs}"
    );

    let mut c = root(&rt, &branch, 0).await;
    assert_eq!(q(&mut c, "SELECT CAST(1 AS CHAR)").await, "1");
    assert_eq!(
        q(&mut c, "SELECT @@version").await,
        "8.0.11-TiDB-v8.5.8-Loams"
    );
    assert_eq!(
        q(&mut c, "SELECT @@global.tidb_server_memory_limit").await,
        "80%"
    );
    assert_eq!(
        q(&mut c, "SELECT CAST(@@global.tidb_mem_quota_query AS CHAR)").await,
        ((768u64 * 2 / 5) << 20).to_string()
    );
    // SEM (R2.9): even root cannot see restricted variables.
    let hidden: Result<Option<String>, _> = c.query_first("SELECT @@global.tidb_redact_log").await;
    assert!(
        hidden.is_err(),
        "SEM off: tidb_redact_log visible to root: {hidden:?}"
    );
    // A marker that a second bootstrap or a lost keyspace would not keep.
    c.query_drop("SET GLOBAL tidb_mem_quota_query = 123456789")
        .await
        .expect("set marker");
    c.disconnect().await.expect("disconnect");

    // Root lockdown (R2.10): no root over TCP.
    let tcp = mysql_async::OptsBuilder::default()
        .ip_or_hostname(m0.mysql_addr.ip().to_string())
        .tcp_port(m0.mysql_addr.port())
        .user(Some("root"));
    assert!(
        mysql_async::Conn::new(tcp).await.is_err(),
        "root logged in over TCP"
    );

    // A second member: a warm start on the bootstrapped keyspace.
    rt.scale(&branch, 2).await.expect("scale 2");
    wait_ready(&rt, &branch, 2, Duration::from_secs(120)).await;
    let mut c1 = root(&rt, &branch, 1).await;
    assert_eq!(q(&mut c1, "SELECT CAST(1 AS CHAR)").await, "1");
    c1.disconnect().await.expect("disconnect");

    // Suspend: scale to zero keeps the pool.
    let st = rt.scale(&branch, 0).await.expect("scale 0");
    assert!(st.members.is_empty(), "{st:?}");
    let st = rt
        .pool_status(&branch)
        .await
        .expect("status")
        .expect("suspended pool exists");
    assert_eq!((st.class, st.replicas, st.members.len()), (Class::Xs, 0, 0));

    // Resume 0 → 1: the same ports, no second bootstrap, the keyspace's state kept.
    let t = Instant::now();
    rt.scale(&branch, 1).await.expect("resume");
    let st = wait_ready(&rt, &branch, 1, Duration::from_secs(60)).await;
    let resume = t.elapsed();
    let r0 = st.members[0].clone();
    assert_eq!(
        (r0.mysql_addr, r0.status_addr),
        (m0.mysql_addr, m0.status_addr)
    );
    let logs = engine_out(&rt, &["logs", &r0.name]);
    assert!(
        !logs.contains("bootstrap successful"),
        "resume bootstrapped again:\n{logs}"
    );
    assert!(!logs.contains("executing -initialize-sql-file"), "{logs}");
    let mut c = root(&rt, &branch, 0).await;
    assert_eq!(
        q(&mut c, "SELECT CAST(@@global.tidb_mem_quota_query AS CHAR)").await,
        "123456789"
    );
    c.disconnect().await.expect("disconnect");
    println!("resume 0 -> 1 to port open: {resume:?}");

    // Class change: the member is replaced at the new memory limit, same port.
    let old_id = engine_out(&rt, &["inspect", "--format", "{{.Id}}", &r0.name]);
    rt.ensure_pool(&branch, Class::S, 1)
        .await
        .expect("class change");
    let st = wait_ready(&rt, &branch, 1, Duration::from_secs(60)).await;
    assert_eq!(st.class, Class::S);
    let s0 = st.members[0].clone();
    assert_eq!(s0.mysql_addr, m0.mysql_addr);
    assert_ne!(
        engine_out(&rt, &["inspect", "--format", "{{.Id}}", &s0.name]),
        old_id,
        "not replaced"
    );
    let mem = engine_out(
        &rt,
        &[
            "inspect",
            "--format",
            "{{.HostConfig.Memory}} {{.HostConfig.MemorySwap}}",
            &s0.name,
        ],
    );
    assert_eq!(
        mem.trim(),
        format!("{0} {0}", Class::S.memory_bytes()),
        "memory and swap limits"
    );
    let mut c = root(&rt, &branch, 0).await;
    assert_eq!(q(&mut c, "SELECT CAST(1 AS CHAR)").await, "1");
    c.disconnect().await.expect("disconnect");

    // Delete removes everything.
    rt.delete_pool(&branch).await.expect("delete");
    assert!(rt.pool_status(&branch).await.expect("status").is_none());
    assert!(!state_dir.join("pools").join(branch.as_str()).exists());
    let left = engine_out(
        &rt,
        &[
            "ps",
            "-aq",
            "--filter",
            &format!("label=io.loams.sqldb.instance={instance}"),
        ],
    );
    assert!(left.trim().is_empty(), "containers left behind: {left}");
    let _ = std::fs::remove_dir_all(&state_dir);
}

/// TiDB v8.5.8 itself accepts every rendered key (`--config-check
/// --config-strict` fails on unknown or removed items).
#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and a container engine"
)]
#[test]
fn rendered_config_passes_tidb_config_check() {
    let engine = ContainerEngine::detect().expect("podman or docker");
    let image = Images::load().expect("pins").tidb().reference();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqldb-config-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let branch = BranchId::parse("br_0123456789abcdef").expect("id");
    let endpoints = Endpoints::new(
        vec!["127.0.0.1:29379".into()],
        vec!["10.89.0.0/16".parse().expect("cidr")],
    )
    .expect("endpoints");
    for (class, endpoints) in [
        (Class::Xs, endpoints.clone()),
        (Class::Xl, endpoints.with_cluster_tls(true)),
    ] {
        let path = dir.join(format!("{class}.toml"));
        std::fs::write(&path, loams_sqldb::render::tidb(&branch, class, &endpoints))
            .expect("write");
        let out = Command::new(engine.program())
            .args(["run", "--rm", "--network", "none", "-v"])
            .arg(format!("{}:/etc/tidb/tidb.toml:ro,z", path.display()))
            .args([
                image.as_str(),
                "--config=/etc/tidb/tidb.toml",
                "--config-check",
                "--config-strict",
            ])
            .output()
            .expect("run tidb");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{class}: {text}");
        assert!(text.contains("config check successful"), "{class}: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
