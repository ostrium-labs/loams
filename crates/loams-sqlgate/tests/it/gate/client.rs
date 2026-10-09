//! A raw MySQL client over our codec: SSLRequest and TLS (optional),
//! caching_sha2_password fast or full auth, then single-packet commands.
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use loams_sqlgate::codec::auth::Password;
use loams_sqlgate::codec::auth::{AuthSwitchRequest, CACHING_SHA2, scramble_caching_sha2};
use loams_sqlgate::codec::command::ErrPacket;
use loams_sqlgate::codec::handshake::{
    Capabilities as C, HandshakeResponse41, HandshakeV10, SslRequest,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use super::wire::Wire;

/// Plain TCP or client TLS.
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

pub struct Client {
    pub wire: Wire<Stream>,
    pub local: SocketAddr,
}

/// A login failure: the ERR packet.
pub type Refused = ErrPacket;

pub const CAPS: C = C(C::PROTOCOL_41.0
    | C::SECURE_CONNECTION.0
    | C::PLUGIN_AUTH.0
    | C::LONG_PASSWORD.0
    | C::TRANSACTIONS.0
    | C::DEPRECATE_EOF.0
    | C::CONNECT_WITH_DB.0);

/// Connects as `user`; `tls` with the server name to verify, or plaintext.
pub async fn connect(
    addr: SocketAddr,
    user: &str,
    password: &[u8],
    tls: Option<(Arc<rustls::ClientConfig>, &str)>,
    database: Option<&str>,
) -> Result<Client, Refused> {
    connect_from(None, addr, user, password, tls, database).await
}

/// A TCP connection from `source` (any 127/8 address works on loopback).
pub async fn tcp_from(source: Option<std::net::IpAddr>, addr: SocketAddr) -> TcpStream {
    match source {
        None => TcpStream::connect(addr).await.expect("connect"),
        Some(ip) => {
            let socket = tokio::net::TcpSocket::new_v4().expect("socket");
            socket.bind(SocketAddr::new(ip, 0)).expect("bind source");
            socket.connect(addr).await.expect("connect")
        }
    }
}

/// [`connect`] from a chosen source address.
pub async fn connect_from(
    source: Option<std::net::IpAddr>,
    addr: SocketAddr,
    user: &str,
    password: &[u8],
    tls: Option<(Arc<rustls::ClientConfig>, &str)>,
    database: Option<&str>,
) -> Result<Client, Refused> {
    let tcp = tcp_from(source, addr).await;
    let local = tcp.local_addr().expect("local");
    let mut plain = Wire::new(tcp);
    let (_, g) = plain
        .read()
        .await
        .ok_or_else(|| ErrPacket::new(0, *b"00000", "closed before greeting"))?;
    if g.first() == Some(&0xff) {
        // Refused in place of the greeting (a connection limit).
        return Err(ErrPacket::decode(&g).expect("err"));
    }
    let greeting = HandshakeV10::decode(&g).expect("greeting");
    let mut caps = CAPS;
    let mut seq = 1;
    let mut wire = match tls {
        Some((config, name)) => {
            caps = caps | C::SSL;
            plain
                .write(
                    1,
                    &SslRequest {
                        capabilities: caps,
                        max_packet: 1 << 24,
                        charset: 0xff,
                    }
                    .encode(),
                )
                .await;
            let name = rustls::pki_types::ServerName::try_from(name.to_owned()).expect("name");
            let tls = tokio_rustls::TlsConnector::from(config)
                .connect(name, plain.into_inner())
                .await
                .expect("tls");
            seq = 2;
            Wire::new(Stream::Tls(Box::new(tls)))
        }
        None => Wire::new(Stream::Plain(plain.into_inner())),
    };
    let is_tls = tls_flag(&wire);
    let response = HandshakeResponse41 {
        capabilities: caps,
        max_packet: 1 << 24,
        charset: 0xff,
        username: user.into(),
        auth_response: Password::new(scramble_caching_sha2(password, greeting.nonce.as_bytes())),
        database: database.map(str::to_owned),
        auth_plugin: Some(CACHING_SHA2.into()),
        attributes: vec![(b"_client_name".to_vec(), b"loams-test".to_vec())],
        zstd_level: None,
    };
    wire.write(seq, &response.encode().expect("encode")).await;
    loop {
        let Some((s, p)) = wire.read().await else {
            return Err(ErrPacket::new(0, *b"00000", "closed during login"));
        };
        match p.first() {
            Some(0x00) => return Ok(Client { wire, local }),
            Some(0xff) => return Err(ErrPacket::decode(&p).expect("err")),
            Some(0xfe) => {
                let sw = AuthSwitchRequest::decode(&p).expect("switch");
                let nonce = sw.nonce().expect("nonce");
                wire.write(s.wrapping_add(1), &scramble_caching_sha2(password, nonce))
                    .await;
            }
            Some(0x01) if p == [0x01, 0x03] => {}
            Some(0x01) if p == [0x01, 0x04] => {
                if is_tls {
                    wire.write(s.wrapping_add(1), &[password, &[0]].concat())
                        .await;
                } else {
                    wire.write(s.wrapping_add(1), &[0x02]).await;
                }
            }
            _ => panic!("unexpected login packet {p:?}"),
        }
    }
}

fn tls_flag(w: &Wire<Stream>) -> bool {
    matches!(w.io, Stream::Tls(_))
}

impl Client {
    /// Sends a one-packet command and returns the first response packet.
    pub async fn command(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        self.wire.write(0, payload).await;
        self.wire.read().await.map(|(_, p)| p)
    }

    /// `COM_QUERY` → the OK's info (the fake TiDB names itself there).
    pub async fn query_info(&mut self, sql: &str) -> String {
        let p = self
            .command(&[&[0x03], sql.as_bytes()].concat())
            .await
            .expect("reply");
        assert_eq!(p[0], 0x00, "OK expected, got {p:?}");
        let ok = loams_sqlgate::codec::command::OkPacket::decode(&p, C::PROTOCOL_41).expect("ok");
        String::from_utf8(ok.info).expect("utf8")
    }
}

impl Client {
    /// `COM_QUERY` returning a text result set: the rows, as strings.
    pub async fn query_rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let first = self
            .command(&[&[0x03], sql.as_bytes()].concat())
            .await
            .expect("reply");
        assert!(
            first[0] != 0xff,
            "query failed: {:?}",
            ErrPacket::decode(&first)
        );
        let columns = usize::from(first[0]);
        for _ in 0..columns {
            self.wire.read().await.expect("column definition");
        }
        let mut rows = Vec::new();
        loop {
            let (_, p) = self.wire.read().await.expect("row");
            if p[0] == 0xfe && p.len() < 0xff_ffff {
                return rows;
            }
            let mut at = 0;
            let mut row = Vec::new();
            for _ in 0..columns {
                if p[at] == 0xfb {
                    row.push(None);
                    at += 1;
                    continue;
                }
                let (len, skip) = match p[at] {
                    n @ 0..=0xfa => (usize::from(n), 1),
                    0xfc => (usize::from(u16::from_le_bytes([p[at + 1], p[at + 2]])), 3),
                    _ => panic!("long value"),
                };
                row.push(Some(
                    String::from_utf8_lossy(&p[at + skip..at + skip + len]).into_owned(),
                ));
                at += skip + len;
            }
            rows.push(row);
        }
    }
}
