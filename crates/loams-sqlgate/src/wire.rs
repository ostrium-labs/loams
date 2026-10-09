//! Async packet I/O for the gate: framed reads through the codec's
//! `Assembler`, and the client stream (plain TCP or server TLS).

use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

use crate::codec::packet::{Assembler, encode};

/// Fills `bytes` with zeros in a way the optimiser keeps.
pub(crate) fn zero(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

/// A read buffer for bytes that may hold secrets (the client handshake:
/// scrambles and full-auth cleartext passwords, R3.12). Consumed bytes are
/// zeroed at once, storage it grows out of is zeroed before it is freed,
/// and everything is zeroed on drop. Its storage is always initialised.
pub struct SecretBuf {
    store: Box<[u8]>,
    start: usize,
    end: usize,
}

impl fmt::Debug for SecretBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBuf")
            .field("len", &(self.end - self.start))
            .finish_non_exhaustive()
    }
}

impl SecretBuf {
    /// An empty buffer of `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            store: vec![0; capacity].into_boxed_slice(),
            start: 0,
            end: 0,
        }
    }

    /// The unconsumed bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.store[self.start..self.end]
    }

    /// The whole storage, live bytes and zeros (for tests).
    #[doc(hidden)]
    pub fn storage(&self) -> &[u8] {
        &self.store
    }

    /// Drops the first `n` unconsumed bytes, zeroing them.
    pub fn consume(&mut self, n: usize) {
        let n = n.min(self.end - self.start);
        zero(&mut self.store[self.start..self.start + n]);
        self.start += n;
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
    }

    /// At least `n` writable bytes after the unconsumed ones: compacts, and
    /// grows (zeroing the old storage) when compacting is not enough.
    pub fn read_space(&mut self, n: usize) -> &mut [u8] {
        let live = self.end - self.start;
        if self.store.len() - self.end < n {
            if self.store.len() - live >= n {
                self.store.copy_within(self.start..self.end, 0);
                zero(&mut self.store[live..]);
            } else {
                let mut grown = vec![0; (2 * self.store.len()).max(live + n)].into_boxed_slice();
                grown[..live].copy_from_slice(&self.store[self.start..self.end]);
                zero(&mut self.store);
                self.store = grown;
            }
            self.start = 0;
            self.end = live;
        }
        &mut self.store[self.end..self.end + n]
    }

    /// Marks `n` bytes written into [`Self::read_space`] as unconsumed.
    pub fn advance(&mut self, n: usize) {
        self.end = (self.end + n).min(self.store.len());
    }

    /// Appends `bytes`.
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.read_space(bytes.len()).copy_from_slice(bytes);
        self.advance(bytes.len());
    }

    /// The unconsumed bytes, moved out (the buffer is left empty and zeroed).
    pub fn take(&mut self) -> Vec<u8> {
        let out = self.as_slice().to_vec();
        let n = self.end - self.start;
        self.consume(n);
        out
    }
}

impl Drop for SecretBuf {
    fn drop(&mut self) {
        zero(&mut self.store);
    }
}

/// A TCP stream that first yields bytes already read from it (a client
/// sends its TLS ClientHello right behind the SSLRequest).
#[derive(Debug)]
pub struct Prefixed {
    prefix: Vec<u8>,
    at: usize,
    inner: TcpStream,
}

impl Prefixed {
    /// `inner`, with `prefix` read first.
    pub fn new(prefix: Vec<u8>, inner: TcpStream) -> Self {
        Self {
            prefix,
            at: 0,
            inner,
        }
    }
}

impl AsyncRead for Prefixed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.at < me.prefix.len() {
            let n = (me.prefix.len() - me.at).min(buf.remaining());
            buf.put_slice(&me.prefix[me.at..me.at + n]);
            me.at += n;
            if me.at == me.prefix.len() {
                me.prefix = Vec::new();
                me.at = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A client connection: plaintext (loopback only) or TLS.
#[derive(Debug)]
pub enum ClientStream {
    /// Plain TCP.
    Plain(TcpStream),
    /// Server-side TLS.
    Tls(Box<tokio_rustls::server::TlsStream<Prefixed>>),
}

impl AsyncRead for ClientStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ClientStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ClientStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ClientStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_flush(cx),
            ClientStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ClientStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Framed packets over a stream, for the gate's upstream login.
pub(crate) struct PacketIo<S> {
    io: S,
    asm: Assembler,
    buf: Vec<u8>,
    next_seq: u8,
}

impl<S: AsyncRead + AsyncWrite + Unpin> PacketIo<S> {
    pub(crate) fn new(io: S, max_message: usize) -> Self {
        Self {
            io,
            asm: Assembler::new(max_message),
            buf: Vec::new(),
            next_seq: 0,
        }
    }

    /// Reads the next packet (its sequence id must follow the last one).
    pub(crate) async fn read(&mut self) -> io::Result<Vec<u8>> {
        self.asm.expect_seq(self.next_seq);
        loop {
            let (used, msg) = self
                .asm
                .push(&self.buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if let Some(m) = msg {
                self.buf.drain(..used);
                self.next_seq = self.asm.next_seq();
                return Ok(m.payload);
            }
            let mut chunk = [0u8; 8192];
            let n = self.io.read(&mut chunk).await?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Sets the next sequence id (after a TLS upgrade mid-handshake).
    pub(crate) fn set_next_seq(&mut self, seq: u8) {
        self.next_seq = seq;
    }

    /// Writes one packet with the next sequence id.
    pub(crate) async fn write(&mut self, payload: &[u8]) -> io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 4);
        encode(payload, &mut self.next_seq, &mut out);
        let written = self.io.write_all(&out).await;
        // The payload may be a login secret (R3.12).
        zero(&mut out);
        written?;
        self.io.flush().await
    }

    /// The stream back, if no unread bytes remain.
    pub(crate) fn into_inner(self) -> io::Result<S> {
        if !self.buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected bytes",
            ));
        }
        Ok(self.io)
    }
}
