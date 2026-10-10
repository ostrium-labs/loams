//! The gate in front of real keyspace-mode TiDB v8.5.8 pools (LocalRuntime
//! on the spike stack: `scripts/sqldb/spike/up.sh`). Ignored unless
//! `LOAMS_IT_SQLDB=1`.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use loams_sqldb::model::{BranchId, Class, Endpoints};
use loams_sqldb::runtime::SqlRuntime;
use loams_sqldb::runtime::local::{ContainerEngine, LocalRuntime, LocalRuntimeConfig};
use loams_sqlgate::auth::{ResolvedUser, Role, StaticUsers, hash_password};
use loams_sqlgate::codec::auth::Password;
use loams_sqlgate::limits::ActivityCounter;
use loams_sqlgate::server::{Gate, GateConfig, GateDeps, SniCert, sni_server_config};
use loams_sqlgate::upstream::{
    Admission, CredentialStore, NoLease, PoolResolver, UpstreamError, UpstreamMember,
};
use mysql_async::prelude::Queryable;
use rustls::pki_types::ServerName;

use super::client;
use super::pki::Pki;

const PD_HTTP: &str = "http://127.0.0.1:29379";

pub(super) struct Cleanup(pub(super) String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let ids = Command::new("podman")
            .args([
                "ps",
                "-aq",
                "--filter",
                &format!("label=io.loams.sqldb.instance={}", self.0),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        for id in ids.split_whitespace() {
            let _ = Command::new("podman")
                .args(["rm", "-f", "-t", "0", id])
                .output();
        }
    }
}

pub(super) fn keyspace(name: &str) {
    for _ in 0..10 {
        let ok = Command::new("curl")
            .args([
                "-fsS",
                "-X",
                "POST",
                &format!("{PD_HTTP}/pd/api/v2/keyspaces"),
            ])
            .args([
                "-H",
                "content-type: application/json",
                "-d",
                &format!(r#"{{"name":"{name}"}}"#),
            ])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("keyspace {name}");
}

#[derive(Default)]
struct Members(std::sync::Mutex<HashMap<String, std::net::SocketAddr>>);

#[async_trait]
impl PoolResolver for Members {
    async fn ensure_running(&self, branch: &str) -> Result<Admission, UpstreamError> {
        let addr = *self
            .0
            .lock()
            .expect("members")
            .get(branch)
            .ok_or(UpstreamError::Unavailable)?;
        Ok(Admission {
            members: vec![UpstreamMember {
                addr,
                server_name: ServerName::IpAddress(addr.ip().into()),
            }],
            lease: Box::new(NoLease),
        })
    }
}

pub(super) struct Creds(pub(super) Vec<u8>);

#[async_trait]
impl CredentialStore for Creds {
    async fn internal_password(&self, _branch: &str, role: Role) -> Option<Password> {
        (role == Role::Writer).then(|| Password::new(self.0.clone()))
    }
}

#[cfg_attr(
    not(loams_it_sqldb),
    ignore = "needs LOAMS_IT_SQLDB=1 and the spike stack"
)]
#[tokio::test(flavor = "multi_thread")]
async fn user_of_db_a_cannot_reach_db_b() {
    let pki = Pki::new();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64;
    let br_a = BranchId::parse(&format!("br_{:016x}", stamp)).expect("id");
    let br_b = BranchId::parse(&format!("br_{:016x}", stamp + 1)).expect("id");
    keyspace(br_a.as_str());
    keyspace(br_b.as_str());

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sqlgate-it-{stamp}"));
    let tls = dir.join("tls");
    std::fs::create_dir_all(&tls).expect("tls dir");
    std::fs::write(tls.join("ca.crt"), &pki.ca_pem).expect("ca");
    std::fs::write(tls.join("tls.crt"), &pki.upstream_pem.0).expect("cert");
    std::fs::write(tls.join("tls.key"), &pki.upstream_pem.1).expect("key");
    let instance = format!("gate-it-{}", std::process::id());
    let _cleanup = Cleanup(instance.clone());

    // R2.12: the gate always sends PROXY v2, loopback included.
    let endpoints = Endpoints::new(
        vec!["127.0.0.1:29379".into()],
        vec!["127.0.0.1/32".parse().expect("cidr")],
    )
    .expect("endpoints");
    let internal: Vec<u8> = format!("ri-{stamp:x}-secret").into_bytes();
    let mut config = LocalRuntimeConfig::new(
        ContainerEngine::podman(),
        dir.join("pools"),
        tls.clone(),
        endpoints,
    );
    config.instance = instance.clone();
    config.mysql_port_base = 24200;
    config.status_port_base = 25200;
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
    // One keyspace's TiDB at a time for bootstrap and DDL. On this stack
    // (one PD, v8.5.8), with another keyspace's TiDB up, a first bootstrap
    // stalled, and DDL on a resumed TiDB never ran: its DDL owner was never
    // elected and the stopped instance's schema-version key stayed in etcd
    // (a Task 5 finding, task-4-report.md). So A takes its DDL alone, is
    // suspended for B's bootstrap, and is read again after its warm resume.
    let ready = |branch: BranchId| {
        let rt = rt.clone();
        async move {
            let deadline = Instant::now() + Duration::from_secs(300);
            loop {
                let st = rt
                    .pool_status(&branch)
                    .await
                    .expect("status")
                    .expect("pool");
                if st.ready() == 1 {
                    return st.members[0].mysql_addr;
                }
                if Instant::now() >= deadline {
                    let logs = Command::new("podman")
                        .args(["logs", "--tail", "20", &st.members[0].name])
                        .output();
                    let logs = logs
                        .map(|o| {
                            String::from_utf8_lossy(&o.stderr).into_owned()
                                + &String::from_utf8_lossy(&o.stdout)
                        })
                        .unwrap_or_default();
                    panic!("{branch} not ready: {st:?}\n{logs}");
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };

    let users = StaticUsers::new(vec![
        (
            "u_a".into(),
            ResolvedUser {
                branch: br_a.to_string(),
                role: Role::Writer,
                password_hash: hash_password(b"pa"),
            },
        ),
        (
            "u_b".into(),
            ResolvedUser {
                branch: br_b.to_string(),
                role: Role::Writer,
                password_hash: hash_password(b"pb"),
            },
        ),
    ]);
    let server_tls = sni_server_config(vec![SniCert {
        names: vec!["localhost".into()],
        chain: pki.gate_chain.clone(),
        key: pki.gate_key.clone_key(),
    }])
    .expect("tls");
    let members = Arc::new(Members::default());
    let gate = Gate::new(
        GateConfig::new(server_tls, pki.client_config()),
        GateDeps {
            users: Arc::new(users),
            credentials: Arc::new(Creds(internal)),
            pools: members.clone(),
            activity: Arc::new(ActivityCounter::default()),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(gate.serve(listener));
    let set = |branch: &BranchId, member: std::net::SocketAddr| {
        members
            .0
            .lock()
            .expect("members")
            .insert(branch.to_string(), member);
    };

    let ca_path = tls.join("ca.crt");
    let opts = |user: &str, pw: &str| {
        mysql_async::OptsBuilder::default()
            .ip_or_hostname("127.0.0.1")
            .tcp_port(addr.port())
            .user(Some(user))
            .pass(Some(pw))
            .ssl_opts(Some(
                mysql_async::SslOpts::default().with_root_certs(vec![ca_path.clone().into()]),
            ))
    };

    // A alone: DDL and a row through the gate.
    rt.ensure_pool(&br_a, Class::Xs, 1).await.expect("pool a");
    set(&br_a, ready(br_a.clone()).await);
    let mut a = mysql_async::Conn::new(opts("u_a", "pa"))
        .await
        .expect("u_a over TLS");
    a.query_drop("CREATE DATABASE only_a")
        .await
        .expect("create");
    a.query_drop("CREATE TABLE only_a.t (x INT PRIMARY KEY)")
        .await
        .expect("table");
    a.query_drop("INSERT INTO only_a.t VALUES (7)")
        .await
        .expect("insert");
    // TiDB sees the client's address through PROXY v2, not the gate's.
    let mut raw = client::connect(
        addr,
        "u_a",
        b"pa",
        Some((pki.client_config(), "localhost")),
        None,
    )
    .await
    .expect("raw login");
    let rows = raw
        .query_rows("SELECT host FROM information_schema.processlist WHERE id = CONNECTION_ID()")
        .await;
    assert_eq!(
        rows[0][0].as_deref(),
        Some(raw.local.to_string().as_str()),
        "processlist host"
    );
    drop(raw);
    let _ = a.disconnect().await;
    rt.scale(&br_a, 0).await.expect("suspend a");

    // B: its own keyspace sees nothing of A's.
    rt.ensure_pool(&br_b, Class::Xs, 1).await.expect("pool b");
    set(&br_b, ready(br_b.clone()).await);
    let mut b = mysql_async::Conn::new(opts("u_b", "pb"))
        .await
        .expect("u_b over TLS");
    let dbs: Vec<String> = b.query("SHOW DATABASES").await.expect("show");
    assert!(
        !dbs.iter().any(|d| d == "only_a"),
        "u_b sees A's database: {dbs:?}"
    );
    assert!(
        b.query_drop("SELECT x FROM only_a.t").await.is_err(),
        "u_b reads A's table"
    );
    let e = mysql_async::Conn::new(opts("u_b", "pa"))
        .await
        .expect_err("A's password does not open B");
    assert!(e.to_string().contains("1045"), "{e}");

    // A resumed next to B: u_a reads its row, u_b still cannot.
    rt.scale(&br_a, 1).await.expect("resume a");
    set(&br_a, ready(br_a.clone()).await);
    let mut a = mysql_async::Conn::new(opts("u_a", "pa"))
        .await
        .expect("u_a after resume");
    let x: Option<i64> = a
        .query_first("SELECT x FROM only_a.t")
        .await
        .expect("select");
    assert_eq!(x, Some(7));
    assert!(
        b.query_drop("SELECT x FROM only_a.t").await.is_err(),
        "u_b reads A's table while A runs"
    );

    drop((a, b));
    rt.delete_pool(&br_a).await.expect("delete a");
    rt.delete_pool(&br_b).await.expect("delete b");
    let _ = std::fs::remove_dir_all(&dir);
}
