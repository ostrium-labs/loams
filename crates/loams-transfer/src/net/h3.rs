//! HTTP/3 on quinn and h3 (D2305, D2435): an axum router served over QUIC
//! with streamed request bodies, and a client with streamed bodies both
//! ways. Each request is its own QUIC stream, so a lost packet stalls one
//! chunk, not the others (§85 §7.1).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use bytes::{Buf, Bytes};
use futures::StreamExt;
use http::{Request, Response};
use http_body_util::BodyExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;

use super::tls::{self, ServerIdentity, Trust};
use super::{ByteStream, Resp, Transport};

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// QUIC windows sized for bulk transfers (§85 §10): 16 MiB per stream, 128
/// MiB per connection. CT1 Task 11 retunes them.
fn transport_config() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.stream_receive_window(quinn::VarInt::from_u32(16 << 20));
    t.receive_window(quinn::VarInt::from_u32(128 << 20));
    t.send_window(128 << 20);
    t.max_concurrent_bidi_streams(quinn::VarInt::from_u32(256));
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    if let Ok(idle) = quinn::IdleTimeout::try_from(Duration::from_secs(60)) {
        t.max_idle_timeout(Some(idle));
    }
    t
}

/// The QUIC endpoint type, for embedders that hold one.
pub type Endpoint = quinn::Endpoint;

/// Binds a QUIC endpoint for the server on `addr` (UDP).
pub fn bind_server(addr: SocketAddr, id: &ServerIdentity) -> io::Result<quinn::Endpoint> {
    let tls = tls::server_config(id, &[tls::ALPN_H3])?;
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(io_err)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(quic));
    cfg.transport_config(Arc::new(transport_config()));
    quinn::Endpoint::server(cfg, addr)
}

/// Serves `app` on `endpoint` until the endpoint closes. Handlers see
/// [`Transport::H3`] in their request extensions.
pub async fn serve(endpoint: quinn::Endpoint, app: Router) {
    while let Some(incoming) = endpoint.accept().await {
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(conn) = incoming.await else { return };
            let peer = conn.remote_address();
            let Ok(mut h3c) =
                h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await
            else {
                return;
            };
            while let Ok(Some(resolver)) = h3c.accept().await {
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(resolver, app, peer).await {
                        tracing::debug!(error = %e, "h3 request ended with an error");
                    }
                });
            }
        });
    }
}

async fn handle(
    resolver: h3::server::RequestResolver<h3_quinn::Connection, Bytes>,
    app: Router,
    peer: SocketAddr,
) -> io::Result<()> {
    let (req, stream) = resolver.resolve_request().await.map_err(io_err)?;
    let (mut tx, mut rx) = stream.split();
    // The request body flows through a small channel: at most a few frames
    // wait in memory, and QUIC flow control holds back the rest.
    let (btx, brx) = mpsc::channel::<io::Result<Bytes>>(4);
    let reader = tokio::spawn(async move {
        loop {
            match rx.recv_data().await {
                Ok(Some(mut b)) => {
                    let n = b.remaining();
                    if btx.send(Ok(b.copy_to_bytes(n))).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = btx.send(Err(io_err(e))).await;
                    break;
                }
            }
        }
    });
    let (parts, ()) = req.into_parts();
    let mut req = Request::from_parts(parts, Body::from_stream(ReceiverStream::new(brx)));
    req.extensions_mut().insert(Transport::H3);
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = match app.oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let (parts, mut body) = resp.into_parts();
    let result = async {
        tx.send_response(Response::from_parts(parts, ()))
            .await
            .map_err(io_err)?;
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.map_err(io_err)?.into_data() {
                tx.send_data(data).await.map_err(io_err)?;
            }
        }
        tx.finish().await.map_err(io_err)
    }
    .await;
    // A handler that answered without reading its body: dropping the
    // receive half stops the peer's sending (STOP_SENDING).
    reader.abort();
    result
}

/// An HTTP/3 connection to one server.
#[derive(Clone)]
pub struct H3Client {
    send: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    conn: quinn::Connection,
    _endpoint: quinn::Endpoint,
}

impl std::fmt::Debug for H3Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H3Client")
            .field("remote", &self.conn.remote_address())
            .finish_non_exhaustive()
    }
}

impl H3Client {
    /// Connects to `addr`, verifying the server as `host` under `trust`.
    pub async fn connect(addr: SocketAddr, host: &str, trust: &Trust) -> io::Result<Self> {
        let bind: SocketAddr = if addr.is_ipv6() {
            "[::]:0".parse().map_err(io_err)?
        } else {
            "0.0.0.0:0".parse().map_err(io_err)?
        };
        let mut endpoint = quinn::Endpoint::client(bind)?;
        let tls = tls::client_config(trust, tls::ALPN_H3)?;
        let quic = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(io_err)?;
        let mut cfg = quinn::ClientConfig::new(Arc::new(quic));
        cfg.transport_config(Arc::new(transport_config()));
        endpoint.set_default_client_config(cfg);
        let name = host.trim_start_matches('[').trim_end_matches(']');
        let conn = endpoint
            .connect(addr, name)
            .map_err(io_err)?
            .await
            .map_err(io_err)?;
        let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn.clone()))
            .await
            .map_err(io_err)?;
        tokio::spawn(async move {
            let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });
        Ok(H3Client {
            send,
            conn,
            _endpoint: endpoint,
        })
    }

    pub fn is_open(&self) -> bool {
        self.conn.close_reason().is_none()
    }

    /// Sends a request, streaming `body`, and returns the response with its
    /// body still streaming.
    pub async fn send(&self, req: Request<()>, body: Option<ByteStream>) -> io::Result<Resp> {
        let stream = self.send.clone().send_request(req).await.map_err(io_err)?;
        let (mut tx, mut rx) = stream.split();
        let sender = async move {
            if let Some(mut b) = body {
                while let Some(c) = b.next().await {
                    let c = c?;
                    if !c.is_empty() {
                        tx.send_data(c).await.map_err(io_err)?;
                    }
                }
            }
            tx.finish().await.map_err(io_err)
        };
        // Send and wait for the head together: a server may answer (and stop
        // our sending) before the body is through.
        let (sent, head) = tokio::join!(sender, rx.recv_response());
        let head = match head {
            Ok(h) => h,
            Err(e) => return Err(sent.err().unwrap_or_else(|| io_err(e))),
        };
        let body = futures::stream::unfold(Some(rx), |rx| async move {
            let mut rx = rx?;
            match rx.recv_data().await {
                Ok(Some(mut b)) => {
                    let n = b.remaining();
                    Some((Ok(b.copy_to_bytes(n)), Some(rx)))
                }
                Ok(None) => None,
                Err(e) => Some((Err(io_err(e)), None)),
            }
        })
        .boxed();
        Ok(Resp {
            status: head.status(),
            headers: head.headers().clone(),
            body,
        })
    }
}
