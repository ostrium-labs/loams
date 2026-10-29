//! The transport under the transfer API (§85 §10): HTTP/3 over QUIC by
//! default, HTTP/2 over TLS 1.3 as the fallback when UDP is blocked, and
//! plain HTTP/2 (prior knowledge) to a loopback engine only (D2437).
//!
//! Bodies stream both ways: a request body is fed from disk as it is sent,
//! and a server handler reads it as it arrives. Nothing here buffers a body.
//!
//! When CT1 Task 7 lands `loams-transport`'s `Endpoint` and `Client`, they
//! replace [`h3`] and [`h2`]; the rest of the crate only sees [`Conn`].

pub mod h2;
pub mod h3;
pub mod race;
pub mod tls;

use std::io;

use bytes::Bytes;
use futures::stream::BoxStream;
use http::{HeaderMap, Request, StatusCode};

pub use race::{ConnectOpts, Mode, connect};
pub use tls::{ServerIdentity, Trust};

/// A streamed body.
pub type ByteStream = BoxStream<'static, io::Result<Bytes>>;

/// Which transport a connection uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    H3,
    H2,
    /// HTTP/2 without TLS, to a loopback engine only.
    H2c,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::H3 => "h3",
            Transport::H2 => "h2",
            Transport::H2c => "h2c",
        }
    }
}

/// A response whose body is still streaming.
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: ByteStream,
}

impl std::fmt::Debug for Resp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resp")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

/// One connection to a server, over either transport. Cheap to clone: every
/// clone multiplexes on the same connection.
#[derive(Clone, Debug)]
pub enum Conn {
    H3(h3::H3Client),
    H2(h2::H2Client),
}

impl Conn {
    pub fn transport(&self) -> Transport {
        match self {
            Conn::H3(_) => Transport::H3,
            Conn::H2(c) => c.transport(),
        }
    }

    /// Sends one request. `req`'s URI must be absolute (`https://host/path`).
    pub async fn send(&self, req: Request<()>, body: Option<ByteStream>) -> io::Result<Resp> {
        match self {
            Conn::H3(c) => c.send(req, body).await,
            Conn::H2(c) => c.send(req, body).await,
        }
    }

    /// Whether the connection can still carry requests.
    pub fn is_open(&self) -> bool {
        match self {
            Conn::H3(c) => c.is_open(),
            Conn::H2(c) => c.is_open(),
        }
    }
}
