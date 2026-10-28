//! A gateway and a client for tests (plan KV2 Task 2): [`start`] serves
//! the gateway on a loopback port over a given store; [`Client`] sends
//! requests and reads replies with this crate's own frames.

use std::net::SocketAddr;

use bytes::BytesMut;
use loams_kv::Store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use crate::config::RespConfig;
use crate::frame::Value;
use crate::server::Server;

/// A running test gateway; stops when dropped.
#[derive(Debug)]
pub struct TestServer {
    pub addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// The configuration of a test gateway: loopback, an ephemeral port, and
/// the extra top-level TOML lines in `extra`.
///
/// # Panics
///
/// When the result is not a valid configuration.
pub fn config(extra: &str) -> RespConfig {
    RespConfig::from_toml(&format!(
        "listen = \"127.0.0.1:0\"\n{extra}\n[store]\nkind = \"memory\"\nname = \"unused\"\n"
    ))
    .expect("a valid test configuration")
}

/// Serves a gateway with `config` over `store` (used as given: the Redis
/// product root is not added again).
///
/// # Panics
///
/// When the listener cannot be bound.
pub async fn start(config: RespConfig, store: Store) -> TestServer {
    let server = Server::bind_on(config, store)
        .await
        .expect("a bound gateway");
    let addr = server.local_addr();
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let _ = server
            .serve(async {
                let _ = stopped.await;
            })
            .await;
    });
    TestServer {
        addr,
        stop: Some(stop),
    }
}

/// A RESP client.
#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    buf: BytesMut,
}

impl Client {
    /// Connects to `addr`.
    ///
    /// # Panics
    ///
    /// When the connection fails.
    pub async fn connect(addr: SocketAddr) -> Self {
        Client {
            stream: TcpStream::connect(addr).await.expect("a connection"),
            buf: BytesMut::new(),
        }
    }

    /// Sends one request without reading its reply.
    ///
    /// # Panics
    ///
    /// When the write fails.
    pub async fn send(&mut self, args: &[&[u8]]) {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for a in args {
            out.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
            out.extend_from_slice(a);
            out.extend_from_slice(b"\r\n");
        }
        self.stream.write_all(&out).await.expect("a write");
    }

    /// Sends raw bytes.
    ///
    /// # Panics
    ///
    /// When the write fails.
    pub async fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("a write");
    }

    /// Reads one reply; `None` when the server closed the connection.
    ///
    /// # Panics
    ///
    /// When the server sends an invalid reply.
    pub async fn read(&mut self) -> Option<Value> {
        loop {
            if let Some((v, n)) = Value::decode(&self.buf).expect("a valid reply") {
                let _ = self.buf.split_to(n);
                return Some(v);
            }
            let n = self.stream.read_buf(&mut self.buf).await.ok()?;
            if n == 0 {
                return None;
            }
        }
    }

    /// Sends a request and reads its reply.
    ///
    /// # Panics
    ///
    /// When the connection fails or closes.
    pub async fn call(&mut self, args: &[&str]) -> Value {
        let args: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
        self.send(&args).await;
        self.read().await.expect("a reply")
    }
}
