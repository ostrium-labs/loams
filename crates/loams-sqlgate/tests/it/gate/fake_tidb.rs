//! A fake keyspace-mode TiDB: PROXY v2 header, greeting (with or without
//! CLIENT_SSL), TLS, a switch to caching_sha2_password and full auth, then
//! OK for every command (its info names the fake).
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use loams_sqlgate::codec::auth::{
    AuthMoreData, AuthSwitchRequest, CACHING_SHA2, NATIVE, double_sha256, verify_caching_sha2,
};
use loams_sqlgate::codec::command::{ErrPacket, OkPacket};
use loams_sqlgate::codec::handshake::{
    Capabilities as C, ClientHello, HandshakeV10, Limits, Nonce, TIDB_V8_5_8, decode_client_hello,
};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use super::pki::Pki;
use super::wire::Wire;

#[derive(Debug, Default, Clone)]
pub struct Seen {
    pub proxy_sources: Vec<SocketAddr>,
    pub logins: Vec<String>,
    pub databases: Vec<Option<String>>,
    pub plaintext_credentials: usize,
    pub commands: Vec<u8>,
}

pub struct FakeTidb {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
}

pub struct FakeOpts {
    pub name: &'static str,
    pub ssl: bool,
    pub user: &'static str,
    pub password: &'static [u8],
}

pub async fn spawn(opts: FakeOpts, pki: &Pki) -> FakeTidb {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let tls = tokio_rustls::TlsAcceptor::from(pki.upstream_server_config());
    let opts = Arc::new(opts);
    let s = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (tls, opts, seen) = (tls.clone(), opts.clone(), s.clone());
            tokio::spawn(async move {
                let _ = serve(tcp, tls, &opts, &seen).await;
            });
        }
    });
    FakeTidb { addr, seen }
}

async fn read_proxy_v2(tcp: &mut tokio::net::TcpStream) -> Option<SocketAddr> {
    let mut head = [0u8; 16];
    tcp.read_exact(&mut head).await.ok()?;
    assert_eq!(&head[..12], b"\r\n\r\n\0\r\nQUIT\n", "PROXY v2 signature");
    assert_eq!(head[12], 0x21, "v2 PROXY command");
    let len = usize::from(u16::from_be_bytes([head[14], head[15]]));
    let mut body = vec![0u8; len];
    tcp.read_exact(&mut body).await.ok()?;
    match head[13] {
        0x11 => {
            let ip = std::net::Ipv4Addr::new(body[0], body[1], body[2], body[3]);
            Some(SocketAddr::new(
                ip.into(),
                u16::from_be_bytes([body[8], body[9]]),
            ))
        }
        0x21 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(&body[..16]);
            Some(SocketAddr::new(
                std::net::Ipv6Addr::from(a).into(),
                u16::from_be_bytes([body[32], body[33]]),
            ))
        }
        f => panic!("unexpected PROXY family {f:#x}"),
    }
}

async fn serve(
    mut tcp: tokio::net::TcpStream,
    tls: tokio_rustls::TlsAcceptor,
    opts: &FakeOpts,
    seen: &Mutex<Seen>,
) -> Option<()> {
    let src = read_proxy_v2(&mut tcp).await?;
    seen.lock().expect("seen").proxy_sources.push(src);
    let nonce = Nonce::from_random(*b"fake-tidb-nonce-0001");
    let caps = if opts.ssl {
        TIDB_V8_5_8
    } else {
        TIDB_V8_5_8.without(C::SSL)
    };
    let greeting = HandshakeV10 {
        server_version: "8.0.11-TiDB-v8.5.8-Loams".into(),
        connection_id: 1,
        nonce,
        capabilities: caps,
        charset: 46,
        status: 2,
        auth_plugin: NATIVE.into(),
    };
    let mut plain = Wire::new(tcp);
    plain.write(0, &greeting.encode()).await;
    let (_, first) = plain.read().await?;
    if let ClientHello::Response(_) = decode_client_hello(&first, &Limits::default()).ok()? {
        seen.lock().expect("seen").plaintext_credentials += 1;
        return None;
    }
    let (tcp, early) = plain.into_parts();
    let stream = tls
        .accept(loams_sqlgate::wire::Prefixed::new(early, tcp))
        .await
        .ok()?;
    let mut w = Wire::new(stream);
    let (_, r) = w.read().await?;
    let ClientHello::Response(response) = decode_client_hello(&r, &Limits::default()).ok()? else {
        return None;
    };
    {
        let mut s = seen.lock().expect("seen");
        s.logins.push(response.username.clone());
        s.databases.push(response.database.clone());
    }
    // Like TiDB: switch to the user's plugin with the same nonce.
    let mut data = nonce.as_bytes().to_vec();
    data.push(0);
    w.write(
        3,
        &AuthSwitchRequest {
            plugin: CACHING_SHA2.into(),
            data,
        }
        .encode(),
    )
    .await;
    let (_, scramble) = w.read().await?;
    let user_ok = response.username == opts.user
        && verify_caching_sha2(&double_sha256(opts.password), nonce.as_bytes(), &scramble);
    w.write(5, &AuthMoreData::PerformFullAuthentication.encode())
        .await;
    let (_, clear) = w.read().await?;
    let pw_ok = clear.strip_suffix(&[0]) == Some(opts.password);
    if !(user_ok && pw_ok) {
        w.write(
            7,
            &ErrPacket::new(1045, *b"28000", "Access denied").encode(),
        )
        .await;
        return None;
    }
    let ok = |info: &str| {
        OkPacket {
            affected_rows: 0,
            last_insert_id: 0,
            status: 2,
            warnings: 0,
            info: info.as_bytes().to_vec(),
        }
        .encode(C::PROTOCOL_41)
    };
    w.write(7, &ok("")).await;
    loop {
        let (_, cmd) = w.read().await?;
        let first = *cmd.first()?;
        seen.lock().expect("seen").commands.push(first);
        if first == 0x01 {
            return Some(());
        }
        w.write(1, &ok(opts.name)).await;
    }
}
