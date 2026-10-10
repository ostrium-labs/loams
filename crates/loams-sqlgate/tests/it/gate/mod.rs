//! The gate server (SQ1 Task 4) against fake TiDB pools.
mod client;
mod fake_tidb;
mod pki;
mod wire;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_sqlgate::auth::{ResolvedUser, Role, StaticUsers, hash_password};
use loams_sqlgate::codec::auth::Password;
use loams_sqlgate::limits::{ActivityCounter, LimitsConfig, PreAuthConfig};
use loams_sqlgate::server::{
    Gate, GateConfig, GateDeps, PlaintextPolicy, SniCert, sni_server_config,
};
use loams_sqlgate::upstream::{
    Admission, Close, CredentialStore, PoolResolver, SessionLease, UpstreamError, UpstreamMember,
};
use rustls::pki_types::ServerName;

use fake_tidb::{FakeOpts, FakeTidb};
use pki::Pki;

const INTERNAL_PW: &[u8] = b"internal-ri-writer-password";

pub struct Pools(HashMap<String, SocketAddr>, &'static str, Arc<WakeCtl>);

/// What the tests' `EnsureRunning` does: answer `Resuming`, and ask
/// admitted sessions to close.
#[derive(Default)]
pub struct WakeCtl {
    pub resuming: std::sync::atomic::AtomicBool,
    leases: std::sync::Mutex<Vec<tokio::sync::watch::Sender<Close>>>,
    pub live: Arc<std::sync::atomic::AtomicUsize>,
}

impl WakeCtl {
    /// Asks every admitted session to close as `c` says.
    pub fn close(&self, c: Close) {
        for tx in self.leases.lock().expect("leases").iter() {
            let _ = tx.send(c);
        }
    }

    /// Sessions whose lease is still held.
    pub fn live(&self) -> usize {
        self.live.load(std::sync::atomic::Ordering::SeqCst)
    }
}

struct TestLease(
    tokio::sync::watch::Receiver<Close>,
    Arc<std::sync::atomic::AtomicUsize>,
);

#[async_trait]
impl SessionLease for TestLease {
    async fn closing(&mut self) -> Close {
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        *self.0.borrow_and_update()
    }
}

impl Drop for TestLease {
    fn drop(&mut self) {
        self.1.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl PoolResolver for Pools {
    async fn ensure_running(&self, branch: &str) -> Result<Admission, UpstreamError> {
        if self.2.resuming.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(UpstreamError::Resuming);
        }
        let addr = self.0.get(branch).ok_or(UpstreamError::Unavailable)?;
        let (tx, rx) = tokio::sync::watch::channel(Close::Open);
        self.2.leases.lock().expect("leases").push(tx);
        self.2
            .live
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Admission {
            members: vec![UpstreamMember {
                addr: *addr,
                server_name: ServerName::try_from(self.1).expect("name"),
            }],
            lease: Box::new(TestLease(rx, self.2.live.clone())),
        })
    }
}

pub struct Creds;

#[async_trait]
impl CredentialStore for Creds {
    async fn internal_password(&self, _branch: &str, role: Role) -> Option<Password> {
        (role == Role::Writer).then(|| Password::new(INTERNAL_PW.to_vec()))
    }
}

/// A listener whose first `failures` accepts fail with EMFILE.
pub struct Flaky {
    failures: usize,
    listener: tokio::net::TcpListener,
}

#[async_trait]
impl loams_sqlgate::server::Acceptor for Flaky {
    async fn accept(&mut self) -> std::io::Result<(tokio::net::TcpStream, SocketAddr)> {
        if self.failures > 0 {
            self.failures -= 1;
            return Err(std::io::Error::from_raw_os_error(24)); // EMFILE
        }
        self.listener.accept().await
    }
}

pub struct Harness {
    pub serving: tokio::task::JoinHandle<()>,
    pub pki: Pki,
    pub gate: Arc<Gate>,
    pub addr: SocketAddr,
    pub a: FakeTidb,
    pub b: FakeTidb,
    pub activity: Arc<ActivityCounter>,
    pub wake: Arc<WakeCtl>,
}

pub struct Options {
    pub plaintext: PlaintextPolicy,
    pub handshake_timeout: Duration,
    pub limits: LimitsConfig,
    pub upstream_ssl: bool,
    /// Flags the fake TiDBs leave out of their greeting.
    pub upstream_drop: loams_sqlgate::codec::handshake::Capabilities,
    pub pre_auth: PreAuthConfig,
    pub max_connections: usize,
    /// Accept calls that fail with EMFILE before the listener works.
    pub accept_failures: usize,
    pub verify_concurrency: usize,
    pub source_failure_burst: u32,
    pub user_failure_burst: u32,
    pub idle_timeout: Duration,
    pub drain_timeout: Duration,
    /// The name the gate checks TiDB's certificate against.
    pub upstream_name: &'static str,
    /// The gate trusts another CA than the one that signed TiDB's
    /// certificate.
    pub upstream_foreign_ca: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            plaintext: PlaintextPolicy::LoopbackOnly,
            handshake_timeout: Duration::from_secs(10),
            limits: LimitsConfig::default(),
            upstream_ssl: true,
            upstream_drop: loams_sqlgate::codec::handshake::Capabilities(0),
            pre_auth: PreAuthConfig::default(),
            max_connections: 10_000,
            accept_failures: 0,
            verify_concurrency: 4,
            source_failure_burst: 10,
            user_failure_burst: 10,
            idle_timeout: Duration::from_secs(3600),
            drain_timeout: Duration::from_secs(10),
            upstream_name: "tidb.test",
            upstream_foreign_ca: false,
        }
    }
}

pub async fn harness(opts: Options) -> Harness {
    let pki = Pki::new();
    let fake = |name, ssl| FakeOpts {
        name,
        ssl,
        drop: opts.upstream_drop,
        user: "ri_writer",
        password: INTERNAL_PW,
    };
    let a = fake_tidb::spawn(fake("tidb-a", opts.upstream_ssl), &pki).await;
    let b = fake_tidb::spawn(fake("tidb-b", opts.upstream_ssl), &pki).await;
    let users = StaticUsers::new(vec![
        (
            "u_a".into(),
            ResolvedUser {
                branch: "br_a".into(),
                role: Role::Writer,
                password_hash: hash_password(b"pa"),
            },
        ),
        (
            "u_b".into(),
            ResolvedUser {
                branch: "br_b".into(),
                role: Role::Writer,
                password_hash: hash_password(b"pb"),
            },
        ),
        (
            "u_nopool".into(),
            ResolvedUser {
                branch: "br_x".into(),
                role: Role::Writer,
                password_hash: hash_password(b"px"),
            },
        ),
    ]);
    let wake = Arc::new(WakeCtl::default());
    let pools = Pools(
        HashMap::from([("br_a".into(), a.addr), ("br_b".into(), b.addr)]),
        opts.upstream_name,
        wake.clone(),
    );
    let activity = Arc::new(ActivityCounter::default());
    let tls = sni_server_config(vec![SniCert {
        names: vec!["localhost".into(), "db-a.sql.test".into()],
        chain: pki.gate_chain.clone(),
        key: pki.gate_key.clone_key(),
    }])
    .expect("tls");
    let upstream_tls = if opts.upstream_foreign_ca {
        Pki::new().client_config()
    } else {
        pki.client_config()
    };
    let mut config = GateConfig::new(tls, upstream_tls);
    config.plaintext = opts.plaintext;
    config.handshake_timeout = opts.handshake_timeout;
    config.limits = opts.limits;
    config.pre_auth = opts.pre_auth;
    config.max_connections = opts.max_connections;
    config.verify_concurrency = opts.verify_concurrency;
    config.verify_wait = Duration::from_secs(30);
    config.source_failures.burst = opts.source_failure_burst;
    config.user_failures.burst = opts.user_failure_burst;
    config.idle_timeout = opts.idle_timeout;
    config.drain_timeout = opts.drain_timeout;
    let deps = GateDeps {
        users: Arc::new(users),
        credentials: Arc::new(Creds),
        pools: Arc::new(pools),
        activity: activity.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let gate = Gate::new(config, deps);
    let flaky = Flaky {
        failures: opts.accept_failures,
        listener,
    };
    let serving = tokio::spawn(gate.clone().serve(flaky));
    Harness {
        serving,
        pki,
        gate,
        addr,
        a,
        b,
        activity,
        wake,
    }
}

impl Harness {
    pub async fn tls(&self, user: &str, pw: &[u8]) -> Result<client::Client, client::Refused> {
        client::connect(
            self.addr,
            user,
            pw,
            Some((self.pki.client_config(), "localhost")),
            None,
        )
        .await
    }

    pub async fn tls_from(
        &self,
        source: &str,
        user: &str,
        pw: &[u8],
    ) -> Result<client::Client, client::Refused> {
        client::connect_from(
            Some(source.parse().expect("ip")),
            self.addr,
            user,
            pw,
            Some((self.pki.client_config(), "localhost")),
            None,
        )
        .await
    }

    pub async fn plain(&self, user: &str, pw: &[u8]) -> Result<client::Client, client::Refused> {
        client::connect(self.addr, user, pw, None, None).await
    }
}

mod real;
mod tests;
mod wake;
mod wake_real;
