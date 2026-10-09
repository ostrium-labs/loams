//! The gate server (plan SQ1 Task 4; §47 §12): a tokio listener with
//! rustls TLS 1.2+ and SNI certificates, the connection phase
//! ([`crate::codec::connection`]), Argon2id and fast-auth identity, an
//! upstream login over TLS with PROXY v2, and a relay that refuses
//! `COM_CHANGE_USER`, replication and admin commands (1235).
//!
//! Plaintext is accepted only from loopback peers ([`PlaintextPolicy`]).
//! Every connection gets a fresh OS-random nonce. The upstream profile is
//! static ([`TIDB_V8_5_8`]); TiDB's own greeting is never relayed.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello as TlsClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::auth::{Busy, FastAuthCache, ResolvedUser, UserResolver, Verifier};
use crate::codec::command::ErrPacket;
use crate::codec::connection::{ConnectionPhase, PhaseError, Step};
use crate::codec::handshake::{Capabilities, HandshakeV10, Limits, Nonce, TIDB_V8_5_8, advertise};
use crate::codec::packet::{HEADER_LEN, encode};
use crate::limits::{
    ActivitySink, LimitError, Limiter, LimitsConfig, PreAuth, PreAuthConfig, TokenBucket,
};
use crate::upstream::{
    ClientContext, CredentialStore, PoolResolver, Upstream, UpstreamError, connect,
};
use crate::wire::{ClientStream, Prefixed, SecretBuf};

/// The version the gate advertises (Q658, as TiDB's rendered config).
pub const SERVER_VERSION: &str = "8.0.11-TiDB-v8.5.8-Loams";

/// When plaintext (no TLS) is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaintextPolicy {
    /// From loopback peers only (127.0.0.0/8, ::1, v4-mapped loopback).
    LoopbackOnly,
    /// Never.
    Never,
}

impl std::str::FromStr for PlaintextPolicy {
    type Err = String;

    /// `never` or `loopback`.
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "never" => Ok(PlaintextPolicy::Never),
            "loopback" => Ok(PlaintextPolicy::LoopbackOnly),
            other => Err(format!("{other:?}: expected never or loopback")),
        }
    }
}

impl PlaintextPolicy {
    /// Whether a peer at `ip` may skip TLS.
    pub fn allows(self, ip: IpAddr) -> bool {
        match self {
            PlaintextPolicy::Never => false,
            PlaintextPolicy::LoopbackOnly => match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => {
                    v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
                }
            },
        }
    }
}

/// One certificate and the SNI names it serves.
pub struct SniCert {
    /// Host names (SNI) this certificate answers; the first certificate
    /// also answers clients that send no or an unknown name.
    pub names: Vec<String>,
    /// The chain, leaf first.
    pub chain: Vec<CertificateDer<'static>>,
    /// The private key.
    pub key: PrivateKeyDer<'static>,
}

impl fmt::Debug for SniCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SniCert")
            .field("names", &self.names)
            .field("key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug)]
struct SniResolver {
    by_name: HashMap<String, Arc<CertifiedKey>>,
    default: Arc<CertifiedKey>,
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: TlsClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let by_name = hello
            .server_name()
            .and_then(|n| self.by_name.get(&n.to_ascii_lowercase()));
        Some(by_name.unwrap_or(&self.default).clone())
    }
}

/// A TLS 1.2+ server config choosing the certificate by SNI.
pub fn sni_server_config(certs: Vec<SniCert>) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut by_name = HashMap::new();
    let mut default = None;
    for c in certs {
        let key = provider.key_provider.load_private_key(c.key)?;
        let ck = Arc::new(CertifiedKey::new(c.chain, key));
        for n in c.names {
            by_name.insert(n.to_ascii_lowercase(), ck.clone());
        }
        default.get_or_insert(ck);
    }
    let default = default.ok_or(rustls::Error::General("no certificate".into()))?;
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SniResolver { by_name, default }));
    Ok(Arc::new(config))
}

/// The gate's settings.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Client TLS ([`sni_server_config`]).
    pub tls: Arc<rustls::ServerConfig>,
    /// TLS to TiDB (trusting the pool's CA).
    pub upstream_tls: Arc<rustls::ClientConfig>,
    /// When plaintext is allowed.
    pub plaintext: PlaintextPolicy,
    /// The client handshake deadline (10 s).
    pub handshake_timeout: Duration,
    /// The upstream connect and login deadline.
    pub upstream_timeout: Duration,
    /// Per-database limits.
    pub limits: LimitsConfig,
    /// Per-IP limits on connections in their handshake.
    pub pre_auth: PreAuthConfig,
    /// Open client connections at once, over all databases (the gate's own
    /// cap; beyond it new connections get 1040 instead of a greeting).
    pub max_connections: usize,
    /// The static upstream capability profile ([`TIDB_V8_5_8`]).
    pub profile: Capabilities,
    /// The advertised server version.
    pub server_version: String,
    /// Fast-auth cache size, in users.
    pub fast_auth_cache: usize,
    /// How long a fast-auth entry lives after its full check.
    pub fast_auth_ttl: Duration,
    /// Argon2id parameters of the decoy hash (those of the control plane's
    /// user hashes, so unknown users cost what known ones do).
    pub argon2: argon2::Params,
    /// Argon2id checks at once (default: the core count).
    pub verify_concurrency: usize,
    /// How long a check waits for its turn before the client gets 1040.
    pub verify_wait: Duration,
    /// Failed verifications per second, gate-wide, sustained; beyond the
    /// bucket every full check gets 1040, known user or not.
    pub auth_failure_rate_per_sec: u32,
    /// Failed verifications allowed in a burst.
    pub auth_failure_burst: u32,
    /// A session with no bytes either way for this long is closed (MySQL's
    /// default `wait_timeout`, 8 h).
    pub idle_timeout: Duration,
}

impl GateConfig {
    /// Defaults: no plaintext (a gateway; the desktop sets
    /// [`PlaintextPolicy::LoopbackOnly`]), 10 s handshake, 30 s upstream,
    /// default limits, the v8.5.8 profile.
    pub fn new(tls: Arc<rustls::ServerConfig>, upstream_tls: Arc<rustls::ClientConfig>) -> Self {
        Self {
            tls,
            upstream_tls,
            plaintext: PlaintextPolicy::Never,
            handshake_timeout: Duration::from_secs(10),
            upstream_timeout: Duration::from_secs(30),
            limits: LimitsConfig::default(),
            pre_auth: PreAuthConfig::default(),
            max_connections: 10_000,
            profile: TIDB_V8_5_8,
            server_version: SERVER_VERSION.into(),
            fast_auth_cache: 100_000,
            fast_auth_ttl: Duration::from_secs(3600),
            argon2: argon2::Params::default(),
            verify_concurrency: std::thread::available_parallelism().map_or(4, |n| n.get()),
            verify_wait: Duration::from_secs(2),
            auth_failure_rate_per_sec: 20,
            auth_failure_burst: 200,
            idle_timeout: Duration::from_secs(8 * 3600),
        }
    }
}

/// The gate's collaborators (the control plane's RPCs in later tasks).
pub struct GateDeps {
    /// `ResolveUser`.
    pub users: Arc<dyn UserResolver>,
    /// Internal `ri_<role>` passwords.
    pub credentials: Arc<dyn CredentialStore>,
    /// Pool members (`EnsureRunning`).
    pub pools: Arc<dyn PoolResolver>,
    /// `ReportActivity`.
    pub activity: Arc<dyn ActivitySink>,
}

impl fmt::Debug for GateDeps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GateDeps")
    }
}

/// Counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateStats {
    /// Logins that passed the full (Argon2id) check.
    pub full_auths: u64,
    /// Logins that passed the fast-auth cache.
    pub fast_hits: u64,
    /// The most Argon2id checks that ran at once.
    pub verify_peak: usize,
    /// Upstream logins refused because TiDB's greeting did not match the
    /// static profile (a TiDB upgraded without a re-captured profile).
    pub profile_mismatches: u64,
}

/// The gate.
#[derive(Debug)]
pub struct Gate {
    config: GateConfig,
    deps: GateDeps,
    cache: FastAuthCache,
    limiter: Arc<Limiter>,
    pre_auth: Arc<PreAuth>,
    connections: Arc<Semaphore>,
    verifier: Verifier,
    auth_failures: std::sync::Mutex<TokenBucket>,
    shutdown: tokio::sync::watch::Sender<bool>,
    next_id: AtomicU32,
    full_auths: AtomicU64,
    fast_hits: AtomicU64,
    profile_mismatches: AtomicU64,
}

/// A client that passed authentication.
struct Authenticated {
    client: ClientStream,
    peer: SocketAddr,
    gate: SocketAddr,
    user: ResolvedUser,
    username: String,
    agreed: Capabilities,
    database: Option<String>,
    charset: u8,
    done: Vec<u8>,
}

fn err_access(user: &str) -> ErrPacket {
    ErrPacket::new(1045, *b"28000", &format!("Access denied for user '{user}'"))
}

fn err_limit(e: LimitError) -> ErrPacket {
    match e {
        LimitError::Cap => ErrPacket::new(1040, *b"08004", "Too many connections"),
        LimitError::Rate => ErrPacket::new(1040, *b"08004", "Too many connections, retry later"),
    }
}

/// The first accept retry delay, doubled per failure up to the maximum.
const ACCEPT_BACKOFF_MIN: Duration = Duration::from_millis(5);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Where the gate's connections come from: a [`TcpListener`], or a test's
/// listener that fails on demand.
#[async_trait::async_trait]
pub trait Acceptor: Send + 'static {
    /// The next connection.
    async fn accept(&mut self) -> io::Result<(TcpStream, SocketAddr)>;
}

#[async_trait::async_trait]
impl Acceptor for TcpListener {
    async fn accept(&mut self) -> io::Result<(TcpStream, SocketAddr)> {
        TcpListener::accept(self).await
    }
}

/// An ERR in place of the greeting (sequence id 0), then close.
async fn refuse_before_greeting(tcp: &mut TcpStream, err: &ErrPacket) {
    let mut out = Vec::new();
    let mut seq = 0;
    encode(&err.encode(), &mut seq, &mut out);
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let _ = tcp.write_all(&out).await;
        let _ = tcp.shutdown().await;
    })
    .await;
}

/// The client's view of an upstream login failure: a missing or forbidden
/// database keeps its MySQL code (1049, 1044) in the gate's own words (no
/// internal user name); everything else is a generic, retryable 1040.
fn err_upstream(e: &UpstreamError, user: &str, database: Option<&str>) -> ErrPacket {
    let db = database.unwrap_or("");
    match e {
        UpstreamError::Login(1049) => {
            ErrPacket::new(1049, *b"42000", &format!("Unknown database '{db}'"))
        }
        UpstreamError::Login(1044) => ErrPacket::new(
            1044,
            *b"42000",
            &format!("Access denied for user '{user}' to database '{db}'"),
        ),
        _ => err_unavailable(),
    }
}

fn err_unavailable() -> ErrPacket {
    ErrPacket::new(1040, *b"08004", "database is unavailable, retry")
}

/// The OK bytes of a finished login with the last packet (the OK) replaced
/// by `err` at the same sequence id.
fn replace_ok(done: &[u8], err: &ErrPacket) -> Vec<u8> {
    let mut start = 0;
    let mut last = 0;
    while start + HEADER_LEN <= done.len() {
        last = start;
        let len = usize::from(done[start])
            | usize::from(done[start + 1]) << 8
            | usize::from(done[start + 2]) << 16;
        start += HEADER_LEN + len;
    }
    let mut out = done[..last].to_vec();
    let mut seq = done.get(last + 3).copied().unwrap_or(2);
    encode(&err.encode(), &mut seq, &mut out);
    out
}

impl Gate {
    /// A gate with `config` and `deps`.
    pub fn new(config: GateConfig, deps: GateDeps) -> Arc<Self> {
        Arc::new(Self {
            cache: FastAuthCache::new(config.fast_auth_cache, config.fast_auth_ttl),
            limiter: Limiter::new(config.limits.clone()),
            pre_auth: PreAuth::new(config.pre_auth.clone()),
            connections: Arc::new(Semaphore::new(config.max_connections)),
            verifier: Verifier::new(
                &config.argon2,
                config.verify_concurrency,
                config.verify_wait,
            ),
            shutdown: tokio::sync::watch::Sender::new(false),
            auth_failures: std::sync::Mutex::new(TokenBucket::new(
                config.auth_failure_rate_per_sec,
                config.auth_failure_burst,
            )),
            config,
            deps,
            next_id: AtomicU32::new(1),
            full_auths: AtomicU64::new(0),
            fast_hits: AtomicU64::new(0),
            profile_mismatches: AtomicU64::new(0),
        })
    }

    /// Open client connections of database `db` (after login).
    pub fn open_connections(&self, db: &str) -> u32 {
        self.limiter.open(db)
    }

    /// Counters.
    pub fn stats(&self) -> GateStats {
        GateStats {
            full_auths: self.full_auths.load(Ordering::Relaxed),
            fast_hits: self.fast_hits.load(Ordering::Relaxed),
            verify_peak: self.verifier.peak(),
            profile_mismatches: self.profile_mismatches.load(Ordering::Relaxed),
        }
    }

    /// Serves clients from `listener`. Accept errors (out of file
    /// descriptors, a connection reset before accept) are transient: they
    /// are logged and retried with a backoff of up to 1 s.
    pub async fn serve(self: Arc<Self>, mut listener: impl Acceptor) {
        let mut backoff = ACCEPT_BACKOFF_MIN;
        loop {
            match listener.accept().await {
                Ok((tcp, peer)) => {
                    backoff = ACCEPT_BACKOFF_MIN;
                    let gate = self.clone();
                    tokio::spawn(async move { gate.handle(tcp, peer).await });
                }
                Err(err) => {
                    tracing::warn!(%err, ?backoff, "accept failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(ACCEPT_BACKOFF_MAX);
                }
            }
        }
    }

    async fn handle(self: Arc<Self>, mut tcp: TcpStream, peer: SocketAddr) {
        let _ = tcp.set_nodelay(true);
        let Ok(local) = tcp.local_addr() else { return };
        // The gate's own cap, then the IP's handshake limits: refused
        // connections get an ERR instead of a greeting (as MySQL does).
        let Ok(_connection) = self.connections.clone().try_acquire_owned() else {
            refuse_before_greeting(&mut tcp, &err_limit(LimitError::Cap)).await;
            return;
        };
        let pre_auth = match self.pre_auth.admit(peer.ip()) {
            Ok(slot) => slot,
            Err(e) => {
                refuse_before_greeting(&mut tcp, &err_limit(e)).await;
                return;
            }
        };
        let authed = match tokio::time::timeout(
            self.config.handshake_timeout,
            self.handshake(tcp, peer, local),
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(_) => {
                tracing::debug!(%peer, "handshake deadline");
                return;
            }
        };
        drop(pre_auth);
        let Authenticated {
            mut client,
            user,
            username,
            done,
            ..
        } = authed;
        let ctx = ClientContext {
            peer: authed.peer,
            gate: authed.gate,
            agreed: authed.agreed,
            database: authed.database.clone(),
            charset: authed.charset,
        };
        // The database's rate and cap are charged only after a successful
        // login, so they never reveal whether a user exists (I1).
        if let Err(e) = self.limiter.admit(&user.branch) {
            let _ = client.write_all(&replace_ok(&done, &err_limit(e))).await;
            return;
        }
        let slot = match self.limiter.acquire(&user.branch) {
            Ok(slot) => slot,
            Err(e) => {
                let _ = client.write_all(&replace_ok(&done, &err_limit(e))).await;
                return;
            }
        };
        let upstream =
            tokio::time::timeout(self.config.upstream_timeout, self.upstream(&user, &ctx)).await;
        let upstream = match upstream {
            Ok(Ok(u)) => u,
            Ok(Err(e)) => {
                if e == UpstreamError::ProfileMismatch {
                    self.profile_mismatches.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(%peer, branch = %user.branch, error = %e, "upstream login failed");
                } else {
                    tracing::warn!(%peer, error = %e, "upstream login failed");
                }
                let err = err_upstream(&e, &username, ctx.database.as_deref());
                let _ = client.write_all(&replace_ok(&done, &err)).await;
                return;
            }
            Err(_) => {
                tracing::warn!(%peer, "upstream deadline");
                let _ = client
                    .write_all(&replace_ok(&done, &err_unavailable()))
                    .await;
                return;
            }
        };
        if client.write_all(&done).await.is_err() || client.flush().await.is_err() {
            return;
        }
        crate::relay::relay(
            client,
            upstream,
            crate::relay::Relay {
                branch: user.branch.clone(),
                activity: self.deps.activity.clone(),
                slot,
                idle_timeout: self.config.idle_timeout,
                shutdown: self.shutdown.subscribe(),
            },
        )
        .await;
    }

    async fn upstream(
        &self,
        user: &ResolvedUser,
        ctx: &ClientContext,
    ) -> Result<Upstream, crate::upstream::UpstreamError> {
        let password = self
            .deps
            .credentials
            .internal_password(&user.branch, user.role)
            .await
            .ok_or(crate::upstream::UpstreamError::NoCredential)?;
        let members = self.deps.pools.members(&user.branch).await?;
        if members.is_empty() {
            return Err(crate::upstream::UpstreamError::Unavailable);
        }
        let pick = self.next_id.fetch_add(1, Ordering::Relaxed) as usize % members.len();
        let connector = TlsConnector::from(self.config.upstream_tls.clone());
        connect(
            &members[pick],
            &connector,
            ctx,
            user.role.internal_user(),
            &password,
            self.config.profile,
        )
        .await
    }

    async fn resolve(
        &self,
        cached: &mut Option<Option<ResolvedUser>>,
        user: &str,
    ) -> Option<ResolvedUser> {
        if cached.is_none() {
            *cached = Some(self.deps.users.resolve(user).await);
        }
        cached.clone().flatten()
    }

    async fn handshake(
        &self,
        tcp: TcpStream,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Option<Authenticated> {
        let mut random = [0u8; 20];
        if getrandom::fill(&mut random).is_err() {
            tracing::error!("OS randomness unavailable");
            return None;
        }
        let nonce = Nonce::from_random(random);
        let greeting = HandshakeV10 {
            server_version: self.config.server_version.clone(),
            connection_id: self.next_id.fetch_add(1, Ordering::Relaxed),
            nonce,
            capabilities: advertise(self.config.profile),
            charset: 46,
            status: 0x0002,
            auth_plugin: crate::codec::auth::CACHING_SHA2.into(),
        };
        let plaintext = self.config.plaintext.allows(peer.ip());
        let (mut phase, hello) = ConnectionPhase::new(greeting, plaintext, Limits::default());
        let mut io = ClientStream::Plain(tcp);
        io.write_all(&hello).await.ok()?;
        // Holds scrambles and full-auth passwords: zeroed as consumed
        // (R3.12). Handshake messages are capped at 96 KiB.
        // Allocated on the first read.
        let mut buf = SecretBuf::with_capacity(0);
        let mut resolved: Option<Option<ResolvedUser>> = None;
        let mut pending: Option<Result<Step, PhaseError>> = None;
        loop {
            let step = match pending.take() {
                Some(s) => s,
                None => match phase.on_bytes(buf.as_slice()) {
                    Ok((used, step)) => {
                        buf.consume(used);
                        Ok(step)
                    }
                    Err(e) => Err(e),
                },
            };
            let step = match step {
                Ok(s) => s,
                Err(e) => {
                    let _ = io.write_all(&phase.error_packet(&e)).await;
                    return None;
                }
            };
            match step {
                Step::NeedMore => {
                    let n = io.read(buf.read_space(8192)).await.ok()?;
                    if n == 0 {
                        return None;
                    }
                    buf.advance(n);
                }
                Step::StartTls => {
                    // Bytes after the SSLRequest are the client's TLS
                    // ClientHello: TLS reads them first.
                    let ClientStream::Plain(tcp) = io else {
                        return None;
                    };
                    let prefixed = Prefixed::new(buf.take(), tcp);
                    let tls = TlsAcceptor::from(self.config.tls.clone())
                        .accept(prefixed)
                        .await
                        .ok()?;
                    io = ClientStream::Tls(Box::new(tls));
                    if phase.tls_established().is_err() {
                        return None;
                    }
                }
                Step::Write(bytes) => io.write_all(&bytes).await.ok()?,
                Step::CheckFast { user, scramble } => {
                    let r = self.resolve(&mut resolved, &user).await;
                    let hit = r.as_ref().is_some_and(|r| {
                        self.cache
                            .check(&user, &r.password_hash, nonce.as_bytes(), &scramble)
                    });
                    if hit {
                        self.fast_hits.fetch_add(1, Ordering::Relaxed);
                    }
                    pending = Some(phase.fast_result(hit));
                }
                Step::CheckFull { user, password } => {
                    // The failure bucket is the same for every user, known
                    // or not, so 1040 never tells them apart (I1).
                    if !self.failure_token() {
                        let _ = io
                            .write_all(&phase.refuse(&err_limit(LimitError::Rate)))
                            .await;
                        return None;
                    }
                    let r = self.resolve(&mut resolved, &user).await;
                    // Unknown users are checked against a decoy hash so
                    // they take as long as known ones.
                    let hash = r.as_ref().map_or_else(
                        || self.verifier.decoy().to_owned(),
                        |r| r.password_hash.clone(),
                    );
                    let ok = match self.verifier.verify(hash.clone(), password.clone()).await {
                        Ok(ok) => ok && r.is_some(),
                        Err(Busy) => {
                            let _ = io
                                .write_all(&phase.refuse(&err_limit(LimitError::Rate)))
                                .await;
                            return None;
                        }
                    };
                    if ok {
                        self.cache.remember(&user, &hash, &password);
                        self.full_auths.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.charge_failure();
                    }
                    pending = Some(phase.full_result(ok).map_err(|e| match e {
                        PhaseError::Auth(_) => {
                            PhaseError::Auth(crate::codec::auth::AuthError::AccessDenied)
                        }
                        other => other,
                    }));
                    if !ok {
                        let _ = io.write_all(&phase.refuse(&err_access(&user))).await;
                        return None;
                    }
                }
                Step::Done(done) => {
                    let response = phase.response()?;
                    let user = resolved.clone().flatten()?;
                    return Some(Authenticated {
                        client: io,
                        peer,
                        gate: local,
                        user,
                        username: response.username.clone(),
                        agreed: phase.agreed()?,
                        database: response.database.clone().filter(|d| !d.is_empty()),
                        charset: response.charset,
                        done,
                    });
                }
            }
        }
    }

    /// Whether the gate-wide failed-verification bucket allows another
    /// Argon2id check.
    fn failure_token(&self) -> bool {
        self.auth_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_token()
    }

    /// Charges one failed verification (an unknown user or a wrong
    /// password alike).
    fn charge_failure(&self) {
        self.auth_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .charge();
    }
}

/// A certificate for `names` from PEM files (chain, then key).
pub fn sni_cert_from_pem(
    names: Vec<String>,
    chain: &std::path::Path,
    key: &std::path::Path,
) -> io::Result<SniCert> {
    use rustls::pki_types::pem::PemObject;
    let bad = |e: rustls::pki_types::pem::Error| {
        io::Error::new(io::ErrorKind::InvalidData, e.to_string())
    };
    let chain = CertificateDer::pem_file_iter(chain)
        .map_err(bad)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(bad)?;
    let key = PrivateKeyDer::from_pem_file(key).map_err(bad)?;
    Ok(SniCert { names, chain, key })
}

/// A TLS client config for the gate's upstream side trusting the CA
/// certificates in a PEM file.
pub fn upstream_tls_from_ca_pem(ca: &std::path::Path) -> io::Result<Arc<rustls::ClientConfig>> {
    use rustls::pki_types::pem::PemObject;
    let bad = |e: String| io::Error::new(io::ErrorKind::InvalidData, e);
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca).map_err(|e| bad(e.to_string()))? {
        roots
            .add(cert.map_err(|e| bad(e.to_string()))?)
            .map_err(|e| bad(e.to_string()))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    .map_err(|e| bad(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}
