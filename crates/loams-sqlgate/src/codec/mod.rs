//! The sans-I/O MySQL codec of the gate (plan SQ1 Task 3).
//!
//! Every decoder takes untrusted bytes and either returns a value or a
//! [`DecodeError`]; none panics, allocates past its stated bound, or reads
//! past its input. Encoders are total.
//!
//! - [`packet`]: framing, 16 MiB splits and sequence ids.
//! - [`handshake`]: `HandshakeV10`, `SSLRequest`, `HandshakeResponse41` and
//!   capability negotiation.
//! - [`auth`]: `caching_sha2_password` (fast and full), `AuthSwitchRequest`,
//!   `AuthMoreData`.
//! - [`command`]: command classification by first byte, OK and ERR packets.

pub mod auth;
pub mod command;
pub mod handshake;
pub mod packet;

/// Why a payload was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The payload ended inside `what`.
    #[error("truncated {what}")]
    Truncated {
        /// The field being read.
        what: &'static str,
    },
    /// `what` is longer than its bound.
    #[error("{what} longer than {limit} bytes")]
    TooLong {
        /// The field.
        what: &'static str,
        /// The bound.
        limit: usize,
    },
    /// `what` has a value the protocol (or the gate) does not allow.
    #[error("invalid {what}: {why}")]
    Invalid {
        /// The field.
        what: &'static str,
        /// Why.
        why: &'static str,
    },
}

pub(crate) fn truncated(what: &'static str) -> DecodeError {
    DecodeError::Truncated { what }
}

pub(crate) fn invalid(what: &'static str, why: &'static str) -> DecodeError {
    DecodeError::Invalid { what, why }
}

/// A bounds-checked cursor over an untrusted payload.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub(crate) fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], DecodeError> {
        if n > self.remaining() {
            return Err(truncated(what));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8, DecodeError> {
        Ok(self.bytes(1, what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16, DecodeError> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32, DecodeError> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Bytes up to a NUL (consumed, not returned), at most `max` long.
    pub(crate) fn nul_terminated(
        &mut self,
        max: usize,
        what: &'static str,
    ) -> Result<&'a [u8], DecodeError> {
        let rest = &self.buf[self.pos..];
        let window = &rest[..rest.len().min(max.saturating_add(1))];
        match window.iter().position(|&b| b == 0) {
            Some(n) => {
                let out = &rest[..n];
                self.pos += n + 1;
                Ok(out)
            }
            None if rest.len() > max => Err(DecodeError::TooLong { what, limit: max }),
            None => Err(truncated(what)),
        }
    }

    /// A length-encoded integer. `0xfb` (NULL) and `0xff` are refused.
    pub(crate) fn lenenc(&mut self, what: &'static str) -> Result<u64, DecodeError> {
        match self.u8(what)? {
            n @ 0..=0xfa => Ok(u64::from(n)),
            0xfc => Ok(u64::from(self.u16(what)?)),
            0xfd => {
                let b = self.bytes(3, what)?;
                Ok(u64::from(b[0]) | u64::from(b[1]) << 8 | u64::from(b[2]) << 16)
            }
            0xfe => {
                let b = self.bytes(8, what)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                Ok(u64::from_le_bytes(a))
            }
            _ => Err(invalid(what, "not a length-encoded integer")),
        }
    }

    /// Length-encoded bytes, at most `max` long.
    pub(crate) fn lenenc_bytes(
        &mut self,
        max: usize,
        what: &'static str,
    ) -> Result<&'a [u8], DecodeError> {
        let n = self.lenenc(what)?;
        let n = usize::try_from(n).map_err(|_| DecodeError::TooLong { what, limit: max })?;
        if n > max {
            return Err(DecodeError::TooLong { what, limit: max });
        }
        self.bytes(n, what)
    }
}

pub(crate) fn utf8(bytes: &[u8], what: &'static str) -> Result<String, DecodeError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| invalid(what, "not UTF-8"))
}

pub(crate) fn put_lenenc(out: &mut Vec<u8>, n: u64) {
    match n {
        0..=0xfa => out.push(n as u8),
        0xfb..=0xffff => {
            out.push(0xfc);
            out.extend_from_slice(&(n as u16).to_le_bytes());
        }
        0x1_0000..=0xff_ffff => {
            out.push(0xfd);
            out.extend_from_slice(&(n as u32).to_le_bytes()[..3]);
        }
        _ => {
            out.push(0xfe);
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
}

pub(crate) fn put_lenenc_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_lenenc(out, b.len() as u64);
    out.extend_from_slice(b);
}
