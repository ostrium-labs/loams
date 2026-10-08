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

async fn query_one(addr: std::net::SocketAddr, sql: &str) -> String {
    let opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(addr.ip().to_string())
        .tcp_port(addr.port())
        .user(Some("root"));
    let mut conn = mysql_async::Conn::new(opts).await.expect("connect");
    let v: Option<String> = conn.query_first(sql).await.expect("query");
    conn.disconnect().await.expect("disconnect");
    v.expect("one row")
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
    let m0 = st.members[0].mysql_addr;
    assert_eq!(m0.port(), MYSQL_PORT_BASE);
    assert_eq!(query_one(m0, "SELECT CAST(1 AS CHAR)").await, "1");
    assert_eq!(
        query_one(m0, "SELECT @@version").await,
        "8.0.11-TiDB-v8.5.8-Loams"
    );
    // Gate → TiDB TLS is loaded from the mounted [security] paths.
    let logs = Command::new(rt.config().engine.program())
        .args(["logs", &st.members[0].name])
        .output()
        .expect("logs");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&logs.stdout),
        String::from_utf8_lossy(&logs.stderr)
    );
    assert!(
        logs.contains("secure connection is enabled"),
        "TLS not enabled:\n{logs}"
    );
    // init.sql ran at bootstrap: R2.1's class memory and log redaction.
    assert_eq!(
        query_one(m0, "SELECT @@global.tidb_server_memory_limit").await,
        "614MB"
    );
    assert_eq!(
        query_one(m0, "SELECT @@global.tidb_redact_log").await,
        "OFF"
    );
    assert_eq!(
        query_one(m0, "SELECT CAST(@@global.tidb_mem_quota_query AS CHAR)").await,
        ((768u64 * 2 / 5) << 20).to_string()
    );

    // A second member: a warm start on the bootstrapped keyspace.
    rt.scale(&branch, 2).await.expect("scale 2");
    let st = wait_ready(&rt, &branch, 2, Duration::from_secs(120)).await;
    let m1 = st
        .members
        .iter()
        .find(|m| m.index == 1)
        .expect("member 1")
        .mysql_addr;
    assert_eq!(query_one(m1, "SELECT CAST(1 AS CHAR)").await, "1");

    // Scale to zero keeps the pool; delete removes it.
    let st = rt.scale(&branch, 0).await.expect("scale 0");
    assert!(st.members.is_empty(), "{st:?}");
    let st = rt
        .pool_status(&branch)
        .await
        .expect("status")
        .expect("suspended pool exists");
    assert_eq!((st.class, st.replicas, st.members.len()), (Class::Xs, 0, 0));
    rt.delete_pool(&branch).await.expect("delete");
    assert!(rt.pool_status(&branch).await.expect("status").is_none());
    assert!(!state_dir.join("pools").join(branch.as_str()).exists());

    let left = Command::new(rt.config().engine.program())
        .args([
            "ps",
            "-aq",
            "--filter",
            &format!("label=io.loams.sqldb.instance={instance}"),
        ])
        .output()
        .expect("ps");
    assert!(left.stdout.is_empty(), "containers left behind");
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
