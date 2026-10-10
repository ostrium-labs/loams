//! Wake on connect against real keyspace-mode TiDB v8.5.8 (plan SQ1 Task
//! 5): the gate with `loams_sqldb::sagas::Lifecycles` as its
//! `EnsureRunning`, `LocalRuntime` pools on the spike stack
//! (`scripts/sqldb/spike/up.sh`). Ignored unless `LOAMS_IT_SQLDB=1`; run
//! them one at a time (`--test-threads=1`), they share the stack's PD.
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::runtime::local::{ContainerEngine, LocalRuntime, LocalRuntimeConfig};
use loams_sqldb::runtime::{Member, SqlRuntime};
use loams_sqldb::sagas::{self, EnsureError, HostConfig, Lifecycles, MemoryStore, Prober};
use loams_sqlgate::auth::{ResolvedUser, Role, StaticUsers, hash_password};
use loams_sqlgate::codec::auth::Password;
use loams_sqlgate::codec::command::ErrPacket;
use loams_sqlgate::codec::handshake::TIDB_V8_5_8;
use loams_sqlgate::limits::ActivitySink;
use loams_sqlgate::server::{Gate, GateConfig, GateDeps, SniCert, sni_server_config};
use loams_sqlgate::upstream::{
    Admission, Close, PoolResolver, SessionLease, UpstreamError, UpstreamMember, probe,
};
use loams_sqlrouter::machines::lifecycle::State;
use mysql_async::prelude::Queryable;
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use super::client;
use super::pki::Pki;
use super::real::{Cleanup, Creds, keyspace};

fn upstream_member(m: &Member) -> UpstreamMember {
    UpstreamMember {
        addr: m.mysql_addr,
        server_name: ServerName::IpAddress(m.mysql_addr.ip().into()),
    }
}

/// The gate's `EnsureRunning` over the lifecycle host (Task 12 wires the
/// same in `loams dev`).
struct HostPools(Lifecycles);

struct Lease(sagas::SessionLease);

#[async_trait]
impl SessionLease for Lease {
    async fn closing(&mut self) -> Close {
        match self.0.closing().await {
            sagas::Close::Open => Close::Open,
            sagas::Close::WhenIdle => Close::WhenIdle,
            sagas::Close::Now => Close::Now,
        }
    }
}

#[async_trait]
impl PoolResolver for HostPools {
    async fn ensure_running(&self, branch: &str) -> Result<Admission, UpstreamError> {
        let b = BranchId::parse(branch).map_err(|_| UpstreamError::Unavailable)?;
        match self.0.ensure_running(&b).await {
            Ok(a) => Ok(Admission {
                members: a.members.iter().map(upstream_member).collect(),
                lease: Box::new(Lease(a.lease)),
            }),
            Err(EnsureError::Resuming) => Err(UpstreamError::Resuming),
            Err(_) => Err(UpstreamError::Unavailable),
        }
    }
}

/// `ReportActivity` into the idle detector.
struct HostActivity(Lifecycles);

impl ActivitySink for HostActivity {
    fn command(&self, branch: &str) {
        if let Ok(b) = BranchId::parse(branch) {
            self.0.activity(&b);
        }
    }
}

/// The resume probe through the gate's own connector.
struct GateProber {
    tls: TlsConnector,
    password: Vec<u8>,
}

impl std::fmt::Debug for GateProber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GateProber")
    }
}

#[async_trait]
impl Prober for GateProber {
    async fn probe(&self, _branch: &BranchId, member: &Member) -> Result<(), String> {
        let pw = Password::new(self.password.clone());
        probe(
            &upstream_member(member),
            &self.tls,
            "ri_writer",
            &pw,
            TIDB_V8_5_8,
        )
        .await
        .map_err(|e| e.to_string())
    }
}

struct Stack {
    pki: Pki,
    dir: PathBuf,
    rt: Arc<LocalRuntime>,
    host: Lifecycles,
    gate: SocketAddr,
    _cleanup: Cleanup,
}

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64
}

/// A gate in front of a lifecycle host over a `LocalRuntime` whose
/// members use ports from `port_base`; `users` are (name, branch, password).
async fn stack(name: &str, port_base: u16, users: &[(&str, &BranchId, &str)]) -> Stack {
    let pki = Pki::new();
    let s = stamp();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sqlgate-{name}-{s}"));
    let tls = dir.join("tls");
    std::fs::create_dir_all(&tls).expect("tls dir");
    std::fs::write(tls.join("ca.crt"), &pki.ca_pem).expect("ca");
    std::fs::write(tls.join("tls.crt"), &pki.upstream_pem.0).expect("cert");
    std::fs::write(tls.join("tls.key"), &pki.upstream_pem.1).expect("key");
    let instance = format!("{name}-{}", std::process::id());
    let cleanup = Cleanup(instance.clone());
    let endpoints = Endpoints::new(
        vec!["127.0.0.1:29379".into()],
        vec!["127.0.0.1/32".parse().expect("cidr")],
    )
    .expect("endpoints");
    let internal: Vec<u8> = format!("ri-{s:x}-secret").into_bytes();
    let mut config =
        LocalRuntimeConfig::new(ContainerEngine::podman(), dir.join("pools"), tls, endpoints);
    config.instance = instance;
    config.mysql_port_base = port_base;
    config.status_port_base = port_base + 1000;
    config.cpu_limits = false;
    // Test only: Task 11 creates the ri_* users with stored credentials.
    config.extra_init_sql = vec![
        format!(
            "CREATE USER 'ri_writer'@'%' IDENTIFIED WITH caching_sha2_password BY '{}'",
            String::from_utf8_lossy(&internal)
        ),
        "GRANT ALL PRIVILEGES ON *.* TO 'ri_writer'@'%'".into(),
    ];
    let rt = Arc::new(LocalRuntime::new(config).expect("runtime"));
    let host_config = HostConfig {
        suspend_after: Duration::ZERO,
        ..HostConfig::default()
    };
    let host = Lifecycles::new(
        rt.clone(),
        Arc::new(MemoryStore::default()),
        Arc::new(GateProber {
            tls: TlsConnector::from(pki.client_config()),
            password: internal.clone(),
        }),
        host_config,
    );
    let users = StaticUsers::new(
        users
            .iter()
            .map(|(u, b, pw)| {
                (
                    (*u).to_owned(),
                    ResolvedUser {
                        branch: b.to_string(),
                        role: Role::Writer,
                        password_hash: hash_password(pw.as_bytes()),
                    },
                )
            })
            .collect(),
    );
    let server_tls = sni_server_config(vec![SniCert {
        names: vec!["localhost".into()],
        chain: pki.gate_chain.clone(),
        key: pki.gate_key.clone_key(),
    }])
    .expect("tls");
    let gate = Gate::new(
        GateConfig::new(server_tls, pki.client_config()),
        GateDeps {
            users: Arc::new(users),
            credentials: Arc::new(Creds(internal)),
            pools: Arc::new(HostPools(host.clone())),
            activity: Arc::new(HostActivity(host.clone())),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(gate.serve(listener));
    Stack {
        pki,
        dir,
        rt,
        host,
        gate: addr,
        _cleanup: cleanup,
    }
}

impl Stack {
    fn opts(&self, user: &str, pw: &str) -> mysql_async::OptsBuilder {
        mysql_async::OptsBuilder::default()
            .ip_or_hostname("127.0.0.1")
            .tcp_port(self.gate.port())
            .user(Some(user))
            .pass(Some(pw))
            .ssl_opts(Some(
                mysql_async::SslOpts::default()
                    .with_root_certs(vec![self.dir.join("tls/ca.crt").into()]),
            ))
    }

    /// Creates the keyspace's pool and waits out its first bootstrap
    /// (11–65 s, R1.2; never on the connect path), then hands the branch
    /// to the lifecycle host as running.
    async fn bootstrap(&self, branch: &BranchId) {
        keyspace(branch.as_str());
        self.rt
            .ensure_pool(branch, Class::Xs, 1)
            .await
            .expect("pool");
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let st = self
                .rt
                .pool_status(branch)
                .await
                .expect("status")
                .expect("pool");
            if st.ready() == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "{branch} not ready: {st:?}");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.host
            .register(branch, State::Running)
            .await
            .expect("register");
    }

    async fn suspend(&self, branch: &BranchId) {
        self.host.suspend_now(branch);
        tokio::time::timeout(
            Duration::from_secs(120),
            self.host.wait_for(branch, |r| r.state == State::Suspended),
        )
        .await
        .expect("suspended");
        let st = self
            .rt
            .pool_status(branch)
            .await
            .expect("status")
            .expect("pool");
        assert!(st.members.is_empty(), "suspended with members: {st:?}");
    }

    async fn teardown(self, branches: &[&BranchId]) {
        for b in branches {
            let _ = self.rt.delete_pool(b).await;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn branch_id(n: u64) -> BranchId {
    BranchId::parse(&format!("br_{n:016x}")).expect("id")
}

#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[tokio::test(flavor = "multi_thread")]
async fn suspended_database_wakes_and_first_query_succeeds() {
    let s = stamp();
    let (a, b) = (branch_id(s), branch_id(s + 1));
    let st = stack("wake-it", 24300, &[("u_a", &a, "pa"), ("u_b", &b, "pb")]).await;

    st.bootstrap(&a).await;
    let mut c = mysql_async::Conn::new(st.opts("u_a", "pa"))
        .await
        .expect("u_a");
    c.query_drop("CREATE DATABASE w").await.expect("create");
    c.query_drop("CREATE TABLE w.t (x INT PRIMARY KEY)")
        .await
        .expect("table");
    c.query_drop("INSERT INTO w.t VALUES (7)")
        .await
        .expect("insert");
    drop(c);

    // An idle session open across a suspend gets 1053, then the close.
    let mut idle = client::connect(
        st.gate,
        "u_a",
        b"pa",
        Some((st.pki.client_config(), "localhost")),
        None,
    )
    .await
    .expect("raw login");
    st.suspend(&a).await;
    let (_, p) = idle.wire.read().await.expect("ERR before the close");
    assert_eq!(ErrPacket::decode(&p).expect("ERR").code, 1053);
    assert_eq!(idle.wire.read().await, None);

    // Another keyspace's TiDB runs while A is woken (the Task 4 finding).
    st.bootstrap(&b).await;

    let t0 = Instant::now();
    let mut c = mysql_async::Conn::new(st.opts("u_a", "pa"))
        .await
        .expect("u_a wakes A");
    let x: Option<i64> = c
        .query_first("SELECT x FROM w.t")
        .await
        .expect("first query");
    let woke = t0.elapsed();
    assert_eq!(x, Some(7));
    assert_eq!(st.host.record(&a).map(|r| r.state), Some(State::Running));
    println!("wake to first result: {woke:?}");
    assert!(woke < Duration::from_secs(30), "{woke:?}");

    drop(c);
    st.teardown(&[&a, &b]).await;
}

/// DDL on `c` (database `w` and `table` in it) finishes within 120 s.
async fn ddl_finishes(c: &mut mysql_async::Conn, table: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(120), async {
        c.query_drop("CREATE DATABASE IF NOT EXISTS w")
            .await
            .expect("DDL");
        c.query_drop(format!("CREATE TABLE {table} (y INT PRIMARY KEY)"))
            .await
            .expect("DDL");
        c.query_drop(format!("INSERT INTO {table} VALUES (1)"))
            .await
            .expect("insert");
    })
    .await
    .is_ok()
}

/// The Task 4 finding, explained (plan R5.3): TiDB v8.5.8's global DDL owner
/// manager campaigns on `/tidb/ddl/fg/owner` without the keyspace's etcd
/// prefix, so the TiDBs of all keyspaces on one PD elect one DDL owner
/// between them, and DDL in every other keyspace waits forever. Fixed
/// upstream by pingcap/tidb#60403 (issue #60401), not in release-8.5.
#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[cfg_attr(
    loams_it_sqldb,
    ignore = "fails on TiDB v8.5.8 (pingcap/tidb#60401): needs the keyspace-prefixed DDL owner patch (R5.3)"
)]
#[tokio::test(flavor = "multi_thread")]
async fn ddl_runs_while_another_keyspace_runs() {
    let s = stamp();
    let (a, b) = (branch_id(s), branch_id(s + 1));
    let st = stack("ddl-it", 24600, &[("u_a", &a, "pa"), ("u_b", &b, "pb")]).await;
    st.bootstrap(&a).await;
    st.suspend(&a).await;
    st.bootstrap(&b).await;
    let mut c = mysql_async::Conn::new(st.opts("u_a", "pa"))
        .await
        .expect("u_a wakes A");
    let ok = ddl_finishes(&mut c, "w.t2").await;
    drop(c);
    st.teardown(&[&a, &b]).await;
    assert!(ok, "DDL on A did not finish while B's TiDB runs");
}

#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[tokio::test(flavor = "multi_thread")]
async fn resume_deadline_returns_1040() {
    // A branch whose keyspace PD does not know: TiDB never serves it, so
    // the resume never passes its probe.
    let c = branch_id(stamp());
    let st = stack("deadline-it", 24400, &[("u_c", &c, "pc")]).await;
    st.rt.ensure_pool(&c, Class::Xs, 0).await.expect("pool");
    st.host
        .register(&c, State::Suspended)
        .await
        .expect("register");
    let t0 = Instant::now();
    let Err(e) = client::connect(
        st.gate,
        "u_c",
        b"pc",
        Some((st.pki.client_config(), "localhost")),
        None,
    )
    .await
    else {
        panic!("admitted to a branch that cannot start");
    };
    let waited = t0.elapsed();
    assert_eq!(
        (e.code, e.message.as_str()),
        (1040, "database is resuming, retry")
    );
    assert!(
        waited >= Duration::from_secs(29) && waited < Duration::from_secs(40),
        "{waited:?}"
    );
    st.teardown(&[&c]).await;
}

#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[tokio::test(flavor = "multi_thread")]
async fn resume_p95_under_5s_on_spike_stack() {
    let a = branch_id(stamp());
    let st = stack("p95-it", 24500, &[("u_a", &a, "pa")]).await;
    st.bootstrap(&a).await;
    let mut samples = Vec::new();
    for cycle in 0..20 {
        st.suspend(&a).await;
        let t0 = Instant::now();
        let mut c = mysql_async::Conn::new(st.opts("u_a", "pa"))
            .await
            .expect("wake");
        let one: Option<i64> = c.query_first("SELECT 1").await.expect("SELECT 1");
        let ms = t0.elapsed().as_millis();
        assert_eq!(one, Some(1));
        drop(c);
        println!("cycle {cycle}: connect to first result {ms} ms");
        samples.push(ms);
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let at =
        |q: f64| sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1];
    let (p50, p95) = (at(0.5), at(0.95));
    println!(
        "resume through the gate: p50 {p50} ms, p95 {p95} ms, max {}",
        sorted[sorted.len() - 1]
    );
    // Recorded beside Task 1's measurements (target-spike/, git-ignored).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target-spike");
    let _ = std::fs::create_dir_all(&out);
    let lines: String = samples
        .iter()
        .enumerate()
        .map(|(i, ms)| format!("{{\"cycle\":{i},\"connect_to_first_result_ms\":{ms}}}\n"))
        .collect();
    let _ = std::fs::write(out.join("resume-through-gate.jsonl"), lines);
    // A TiDB resumed alone runs DDL (stale lease-less schema-version keys
    // of its earlier instances do not block it).
    let mut c = mysql_async::Conn::new(st.opts("u_a", "pa"))
        .await
        .expect("u_a");
    let ddl = ddl_finishes(&mut c, "w.t").await;
    drop(c);
    st.teardown(&[&a]).await;
    assert!(ddl, "DDL after 20 resumes did not finish in 120 s");
    assert!(p95 <= 5_000, "resume p95 {p95} ms > 5 s");
}
