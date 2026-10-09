//! The gate's connection to a branch's `tidb-server` pool member: PROXY v2
//! (R2.12), TLS before any credential (R3.16), and login as the role's
//! internal user `ri_<role>` (plan SQ1 Task 4).

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use async_trait::async_trait;
use rustls::pki_types::ServerName;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::auth::Role;
use crate::codec::auth::{
    AuthMoreData, AuthSwitchRequest, CACHING_SHA2, Password, client_auth_response,
    client_full_auth_reply,
};
use crate::codec::command::ErrPacket;
use crate::codec::handshake::{
    Capabilities, HandshakeResponse41, HandshakeV10, SslRequest, upstream_capabilities,
};
use crate::wire::PacketIo;

/// One `tidb-server` of a branch's pool.
#[derive(Debug, Clone)]
pub struct UpstreamMember {
    /// Its MySQL address.
    pub addr: SocketAddr,
    /// The name its TLS certificate is checked against.
    pub server_name: ServerName<'static>,
}

/// Why the gate could not reach TiDB. The client sees a generic 1040.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamError {
    /// No running member, or none answered (Task 5 wakes the pool).
    #[error("no pool member available")]
    Unavailable,
    /// TiDB did not offer TLS: the gate never logs in without it (R3.16).
    #[error("upstream does not offer TLS")]
    TlsUnavailable,
    /// No internal credential for the role.
    #[error("no internal credential")]
    NoCredential,
    /// TiDB refused the internal login.
    #[error("upstream login refused ({0})")]
    Login(u16),
    /// A malformed or unexpected upstream packet, or an I/O error.
    #[error("upstream protocol error: {0}")]
    Protocol(String),
}

/// Which members serve a branch (`EnsureRunning`, Task 5).
#[async_trait]
pub trait PoolResolver: Send + Sync {
    /// The branch's running members.
    async fn members(&self, branch: &str) -> Result<Vec<UpstreamMember>, UpstreamError>;
}

/// The internal passwords of the `ri_<role>` users (Task 11's store).
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// `ri_<role>`'s password in `branch`'s keyspace.
    async fn internal_password(&self, branch: &str, role: Role) -> Option<Password>;
}

/// What the gate knows of the client when it connects upstream.
#[derive(Clone)]
pub struct ClientContext {
    /// The client's address (sent in the PROXY v2 header).
    pub peer: SocketAddr,
    /// The gate's address the client reached.
    pub gate: SocketAddr,
    /// Capabilities agreed with the client.
    pub agreed: Capabilities,
    /// The client's database, if any.
    pub database: Option<String>,
    /// The client's collation.
    pub charset: u8,
}

impl fmt::Debug for ClientContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientContext")
            .field("peer", &self.peer)
            .field("agreed", &self.agreed)
            .finish()
    }
}

/// The PROXY protocol v2 header for a TCP connection from `src` to `dst`
/// (IPv4 when both are IPv4, IPv6 otherwise, with v4-mapped addresses).
pub fn proxy_v2_header(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let mut out = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    out.push(0x21); // version 2, PROXY
    match (src.ip(), dst.ip()) {
        (IpAddr::V4(s), IpAddr::V4(d)) => {
            out.push(0x11); // TCP over IPv4
            out.extend_from_slice(&12u16.to_be_bytes());
            out.extend_from_slice(&s.octets());
            out.extend_from_slice(&d.octets());
        }
        (s, d) => {
            let v6 = |ip: IpAddr| match ip {
                IpAddr::V4(v4) => v4.to_ipv6_mapped(),
                IpAddr::V6(v6) => v6,
            };
            out.push(0x21); // TCP over IPv6
            out.extend_from_slice(&36u16.to_be_bytes());
            out.extend_from_slice(&v6(s).octets());
            out.extend_from_slice(&v6(d).octets());
        }
    }
    out.extend_from_slice(&src.port().to_be_bytes());
    out.extend_from_slice(&dst.port().to_be_bytes());
    out
}

/// A logged-in upstream connection.
pub type Upstream = tokio_rustls::client::TlsStream<TcpStream>;

fn protocol(e: impl fmt::Display) -> UpstreamError {
    UpstreamError::Protocol(e.to_string())
}

/// Connects to `member` for a client: PROXY v2 header, greeting, TLS
/// (refused if TiDB does not offer it: no credential is ever sent in
/// clear, R3.16), then login as `user` with `password`.
pub async fn connect(
    member: &UpstreamMember,
    tls: &TlsConnector,
    client: &ClientContext,
    user: &str,
    password: &Password,
    profile: Capabilities,
) -> Result<Upstream, UpstreamError> {
    let mut tcp = TcpStream::connect(member.addr)
        .await
        .map_err(|_| UpstreamError::Unavailable)?;
    tcp.set_nodelay(true).map_err(protocol)?;
    tcp.write_all(&proxy_v2_header(client.peer, client.gate))
        .await
        .map_err(|_| UpstreamError::Unavailable)?;
    let mut plain = PacketIo::new(tcp, 64 * 1024);
    let greeting =
        HandshakeV10::decode(&plain.read().await.map_err(protocol)?).map_err(protocol)?;
    if !greeting.capabilities.contains(Capabilities::SSL) {
        return Err(UpstreamError::TlsUnavailable);
    }
    let mut caps = upstream_capabilities(client.agreed, profile);
    if client.database.is_none() {
        caps = caps.without(Capabilities::CONNECT_WITH_DB);
    }
    // Only what TiDB offered (SSL is required above).
    caps = caps.intersect(greeting.capabilities);
    let ssl = SslRequest {
        capabilities: caps,
        max_packet: 1 << 24,
        charset: client.charset,
    };
    plain.write(&ssl.encode()).await.map_err(protocol)?;
    let seq = 2;
    let tcp = plain.into_inner().map_err(protocol)?;
    let stream = tls
        .connect(member.server_name.clone(), tcp)
        .await
        .map_err(protocol)?;
    let mut io = PacketIo::new(stream, 64 * 1024);
    io.set_next_seq(seq);

    let response = HandshakeResponse41 {
        capabilities: caps,
        max_packet: 1 << 24,
        charset: client.charset,
        username: user.to_owned(),
        auth_response: client_auth_response(
            CACHING_SHA2,
            password,
            greeting.nonce.as_bytes(),
            true,
        )
        .map_err(protocol)?,
        database: client.database.clone(),
        auth_plugin: Some(CACHING_SHA2.into()),
        attributes: vec![(b"_client_name".to_vec(), b"loams-sqlgate".to_vec())],
        zstd_level: None,
    };
    io.write(&response.encode().map_err(protocol)?)
        .await
        .map_err(protocol)?;
    loop {
        let p = io.read().await.map_err(protocol)?;
        match p.first() {
            Some(0x00) => break,
            Some(0xff) => {
                let e = ErrPacket::decode(&p).map_err(protocol)?;
                return Err(UpstreamError::Login(e.code));
            }
            Some(0xfe) => {
                let switch = AuthSwitchRequest::decode(&p).map_err(protocol)?;
                let nonce = switch.nonce().map_or(&switch.data[..], |n| &n[..]);
                let reply = client_auth_response(&switch.plugin, password, nonce, true)
                    .map_err(protocol)?;
                io.write(reply.expose()).await.map_err(protocol)?;
            }
            Some(0x01) => match AuthMoreData::decode(&p).map_err(protocol)? {
                AuthMoreData::FastAuthSuccess => {}
                AuthMoreData::PerformFullAuthentication => {
                    let reply = client_full_auth_reply(password, true).map_err(protocol)?;
                    io.write(reply.expose()).await.map_err(protocol)?;
                }
            },
            _ => return Err(protocol("unexpected login packet")),
        }
    }
    io.into_inner().map_err(protocol)
}

/// No pools yet (the desktop gate before `loams.sqldb.v1` is wired, Task
/// 12): every branch is unavailable.
#[derive(Debug, Default)]
pub struct NoPools;

#[async_trait]
impl PoolResolver for NoPools {
    async fn members(&self, _branch: &str) -> Result<Vec<UpstreamMember>, UpstreamError> {
        Err(UpstreamError::Unavailable)
    }
}

/// No internal credentials yet (Task 11).
#[derive(Debug, Default)]
pub struct NoCredentials;

#[async_trait]
impl CredentialStore for NoCredentials {
    async fn internal_password(&self, _branch: &str, _role: Role) -> Option<Password> {
        None
    }
}
