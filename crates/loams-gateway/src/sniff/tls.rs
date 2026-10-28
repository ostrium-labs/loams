//! The TLS ClientHello: its SNI and ALPN, read by rustls' `Acceptor` over
//! the kept bytes, so Loams writes no TLS parser (design §57 §5).

use rustls::server::Acceptor;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::SniffError;

/// The largest ClientHello read (D1020).
pub const MAX_CLIENT_HELLO: usize = 64 * 1024;

/// What routing needs from a ClientHello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    /// The SNI, lower-cased.
    pub server_name: Option<String>,
    /// The ALPN protocols offered, in order.
    pub alpn: Vec<Vec<u8>>,
}

impl ClientHello {
    /// Whether the client offered `protocol` in ALPN.
    pub fn offers_alpn(&self, protocol: &[u8]) -> bool {
        self.alpn.iter().any(|p| p == protocol)
    }
}

/// Reads one ClientHello into `buf` (which may already hold its first
/// bytes). Every byte read stays in `buf`, including any that follow the
/// ClientHello.
pub async fn read_client_hello<R: AsyncRead + Unpin>(
    io: &mut R,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<ClientHello, SniffError> {
    let mut acceptor = Acceptor::default();
    let mut fed = 0;
    loop {
        while fed < buf.len() {
            let mut rest = &buf[fed..];
            let n = acceptor
                .read_tls(&mut rest)
                .map_err(|e| SniffError::NotClientHello(e.to_string()))?;
            if n == 0 {
                break;
            }
            fed += n;
        }
        match acceptor.accept() {
            Ok(Some(accepted)) => {
                let hello = accepted.client_hello();
                return Ok(ClientHello {
                    server_name: hello.server_name().map(str::to_ascii_lowercase),
                    alpn: hello
                        .alpn()
                        .map(|protocols| protocols.map(<[u8]>::to_vec).collect())
                        .unwrap_or_default(),
                });
            }
            Ok(None) => {}
            Err((e, _alert)) => return Err(SniffError::NotClientHello(e.to_string())),
        }
        if buf.len() >= limit {
            return Err(SniffError::TooLarge(limit));
        }
        let mut chunk = [0u8; 4096];
        let room = (limit - buf.len()).min(chunk.len());
        let n = io.read(&mut chunk[..room]).await?;
        if n == 0 {
            return Err(SniffError::Eof);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// TLS alert descriptions the gateway sends in clear, before any key
/// exchange (D1015).
pub mod alert {
    /// A namespace's org may not use Postgres now (the meter's gate, ME1
    /// Task 21).
    pub const ACCESS_DENIED: u8 = 49;
    pub const INTERNAL_ERROR: u8 = 80;
    pub const UNRECOGNIZED_NAME: u8 = 112;
    pub const NO_APPLICATION_PROTOCOL: u8 = 120;

    /// A fatal alert record (TLS 1.2 record version, as servers send before
    /// a ServerHello).
    pub fn record(description: u8) -> [u8; 7] {
        [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, description]
    }
}
