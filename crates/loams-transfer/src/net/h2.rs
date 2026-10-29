//! HTTP/2 over TLS 1.3 (the fallback, D2435) and plain HTTP/2 with prior
//! knowledge to a loopback engine (D2437): an axum router served on TCP, and
//! a client with streamed bodies both ways.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use http::Request;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

use super::tls::{self, ServerIdentity, Trust};
use super::{ByteStream, Resp, Transport};

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

const STREAM_WINDOW: u32 = 16 << 20;
const CONN_WINDOW: u32 = 128 << 20;

fn builder() -> auto::Builder<TokioExecutor> {
    let mut b = auto::Builder::new(TokioExecutor::new());
    b.http2()
        .max_concurrent_streams(256)
        .initial_stream_window_size(STREAM_WINDOW)
        .initial_connection_window_size(CONN_WINDOW);
    b
}

async fn serve_io<I>(io: I, app: Router, transport: Transport, peer: SocketAddr)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
        let mut req = req.map(Body::new);
        req.extensions_mut().insert(transport);
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        app.clone().oneshot(req)
    });
    let _ = builder().serve_connection(TokioIo::new(io), svc).await;
}

/// Serves `app` over TLS (ALPN `h2`, and `http/1.1` for plain tools) on
/// `listener` until it fails.
pub async fn serve_tls(listener: TcpListener, id: &ServerIdentity, app: Router) -> io::Result<()> {
    let cfg = tls::server_config(id, &[tls::ALPN_H2, tls::ALPN_H1])?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    loop {
        let (tcp, peer) = listener.accept().await?;
        let _ = tcp.set_nodelay(true);
        let (acceptor, app) = (acceptor.clone(), app.clone());
        tokio::spawn(async move {
            // A handshake gets ten seconds, so a slow client never holds a task.
            let tls =
                match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await {
                    Ok(Ok(t)) => t,
                    _ => return,
                };
            serve_io(tls, app, Transport::H2, peer).await;
        });
    }
}

/// Serves `app` without TLS (HTTP/1.1 or HTTP/2 prior knowledge). Callers
/// bind loopback only (D2437).
pub async fn serve_plain(listener: TcpListener, app: Router) -> io::Result<()> {
    loop {
        let (tcp, peer) = listener.accept().await?;
        let _ = tcp.set_nodelay(true);
        let app = app.clone();
        tokio::spawn(serve_io(tcp, app, Transport::H2c, peer));
    }
}

type ReqBody = UnsyncBoxBody<Bytes, io::Error>;

/// An HTTP/2 connection to one server, over TLS or (loopback) plain.
#[derive(Clone)]
pub struct H2Client {
    send: hyper::client::conn::http2::SendRequest<ReqBody>,
    transport: Transport,
}

impl std::fmt::Debug for H2Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2Client")
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl H2Client {
    /// Connects over TLS to `addr`, verifying the server as `host`.
    pub async fn connect_tls(addr: SocketAddr, host: &str, trust: &Trust) -> io::Result<Self> {
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        let cfg = tls::client_config(trust, tls::ALPN_H2)?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
            .connect(tls::server_name(host)?, tcp)
            .await?;
        Self::handshake(tls, Transport::H2).await
    }

    /// Connects without TLS: loopback engines only (the caller checks).
    pub async fn connect_plain(addr: SocketAddr) -> io::Result<Self> {
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        Self::handshake(tcp, Transport::H2c).await
    }

    async fn handshake<I>(io: I, transport: Transport) -> io::Result<Self>
    where
        I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (send, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .initial_stream_window_size(STREAM_WINDOW)
            .initial_connection_window_size(CONN_WINDOW)
            .handshake(TokioIo::new(io))
            .await
            .map_err(io_err)?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Ok(H2Client { send, transport })
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn is_open(&self) -> bool {
        !self.send.is_closed()
    }

    /// Sends a request, streaming `body`, and returns the response with its
    /// body still streaming.
    pub async fn send(&self, req: Request<()>, body: Option<ByteStream>) -> io::Result<Resp> {
        let (parts, ()) = req.into_parts();
        let body: ReqBody = match body {
            Some(s) => BodyExt::boxed_unsync(StreamBody::new(s.map_ok(Frame::data))),
            None => BodyExt::boxed_unsync(Empty::<Bytes>::new().map_err(|never| match never {})),
        };
        let mut send = self.send.clone();
        send.ready().await.map_err(io_err)?;
        let resp = send
            .send_request(Request::from_parts(parts, body))
            .await
            .map_err(io_err)?;
        let (parts, body) = resp.into_parts();
        let body = body.into_data_stream().map(|r| r.map_err(io_err)).boxed();
        Ok(Resp {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}
