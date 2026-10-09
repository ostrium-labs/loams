//! Async packet I/O over our codec, for the test client and the fake TiDB.
use loams_sqlgate::codec::packet::{Assembler, encode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub struct Wire<S> {
    pub io: S,
    asm: Assembler,
    buf: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Wire<S> {
    pub fn new(io: S) -> Self {
        Self {
            io,
            asm: Assembler::new(1 << 24),
            buf: Vec::new(),
        }
    }

    /// Reads one packet; `None` at EOF or on a read error.
    pub async fn read(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            if let Some(&seq) = self.buf.get(3) {
                self.asm.expect_seq(seq);
                if let Ok((used, Some(m))) = self.asm.push(&self.buf) {
                    self.buf.drain(..used);
                    return Some((m.seq, m.payload));
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.io.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    pub async fn write(&mut self, seq: u8, payload: &[u8]) {
        let mut s = seq;
        let mut out = Vec::new();
        encode(payload, &mut s, &mut out);
        let _ = self.io.write_all(&out).await;
        let _ = self.io.flush().await;
    }

    /// The stream back, for a TLS upgrade (no unread bytes may remain).
    pub fn into_inner(self) -> S {
        assert!(self.buf.is_empty(), "bytes left before TLS");
        self.io
    }

    /// The stream and any bytes read past the last packet (a TLS
    /// ClientHello sent right behind an SSLRequest).
    pub fn into_parts(self) -> (S, Vec<u8>) {
        (self.io, self.buf)
    }
}
