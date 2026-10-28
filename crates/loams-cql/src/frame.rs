//! Bounded Cassandra envelope parsing, backed by `cassandra-protocol`.
//!
//! V4 uses envelopes directly. This codec also handles the initial V5
//! negotiation envelope; V5's post-negotiation segmented frames need a
//! separate transport codec before V5 can be advertised to clients.

use cassandra_protocol::{
    compression::Compression,
    frame::{Direction, Envelope},
};
use std::{error::Error, fmt};

const HEADER_LEN: usize = 9;

/// A rejected or malformed Cassandra envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The body length is negative.
    InvalidLength,
    /// The body exceeds the configured connection limit.
    TooLarge,
    /// A client sent a response envelope or the server tried to send a request.
    Direction,
    /// Compression, warnings, custom payloads or unknown flags are not enabled.
    UnsupportedFlags,
    /// This envelope uses an unsupported protocol version or malformed body.
    InvalidEnvelope,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CQL envelope: {self:?}")
    }
}
impl Error for FrameError {}

/// An envelope transport, independent from the query executor.
pub trait ProtocolCodec {
    /// Decode one request; partial input returns `Ok(None)` without consumption.
    fn decode(&self, input: &[u8]) -> Result<Option<(Envelope, usize)>, FrameError>;
    /// Encode one response envelope.
    fn encode(&self, response: &Envelope) -> Result<Vec<u8>, FrameError>;
}

/// Enforces a body limit before passing complete envelopes to the library.
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeCodec {
    max_body_bytes: usize,
}
impl EnvelopeCodec {
    /// Construct a codec with the maximum allowed request or response body.
    #[must_use]
    pub fn new(max_body_bytes: usize) -> Self {
        Self { max_body_bytes }
    }
}

impl ProtocolCodec for EnvelopeCodec {
    fn decode(&self, input: &[u8]) -> Result<Option<(Envelope, usize)>, FrameError> {
        if input.len() < HEADER_LEN {
            return Ok(None);
        }
        if input[0] & 0x80 != 0 {
            return Err(FrameError::Direction);
        }
        if !matches!(input[0], 4 | 5) {
            return Err(FrameError::InvalidEnvelope);
        }
        // The dependency parses certain flag payloads with unchecked reads.
        // Refuse all flags until those extensions are explicitly supported.
        if input[1] != 0 {
            return Err(FrameError::UnsupportedFlags);
        }
        let raw_len = i32::from_be_bytes(input[5..9].try_into().expect("header has nine bytes"));
        let len = usize::try_from(raw_len).map_err(|_| FrameError::InvalidLength)?;
        if len > self.max_body_bytes {
            return Err(FrameError::TooLarge);
        }
        let total = HEADER_LEN.checked_add(len).ok_or(FrameError::TooLarge)?;
        if input.len() < total {
            return Ok(None);
        }
        let parsed = Envelope::from_buffer(&input[..total], Compression::None)
            .map_err(|_| FrameError::InvalidEnvelope)?;
        Ok(Some((parsed.envelope, parsed.envelope_len)))
    }

    fn encode(&self, response: &Envelope) -> Result<Vec<u8>, FrameError> {
        if response.direction != Direction::Response {
            return Err(FrameError::Direction);
        }
        if response.body.len() > self.max_body_bytes {
            return Err(FrameError::TooLarge);
        }
        response
            .encode_with(Compression::None)
            .map_err(|_| FrameError::InvalidEnvelope)
    }
}
