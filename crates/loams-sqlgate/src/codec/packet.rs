//! MySQL packet framing: a 3-byte little-endian length, a sequence id, and
//! the payload. A payload of 16 MiB - 1 bytes or more is split into frames
//! of exactly [`MAX_FRAME`] bytes, ended by a shorter (possibly empty) frame.
//! Sequence ids increase by one per frame, wrapping, and restart at 0 with
//! each command.

/// The largest frame payload, `0xFF_FFFF`.
pub const MAX_FRAME: usize = 0xFF_FFFF;
/// The frame header length.
pub const HEADER_LEN: usize = 4;

/// Why framing failed. Either is fatal for the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// A frame arrived out of sequence.
    #[error("packet sequence {got}, expected {expected}")]
    Sequence {
        /// The id the assembler expected.
        expected: u8,
        /// The id received.
        got: u8,
    },
    /// The message would exceed the assembler's maximum.
    #[error("packet larger than {limit} bytes")]
    TooLarge {
        /// The maximum.
        limit: usize,
    },
}

/// A reassembled message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The sequence id of its first frame.
    pub seq: u8,
    /// The payload, frames joined.
    pub payload: Vec<u8>,
}

/// Reassembles messages from frames, checking sequence ids and a maximum
/// message size. The size is checked from each frame header, before the
/// caller has to buffer the frame's payload.
#[derive(Debug)]
pub struct Assembler {
    max_message: usize,
    next_seq: u8,
    first_seq: Option<u8>,
    buf: Vec<u8>,
}

impl Assembler {
    /// An assembler expecting sequence id 0 and refusing messages longer
    /// than `max_message` bytes.
    pub fn new(max_message: usize) -> Self {
        Self {
            max_message,
            next_seq: 0,
            first_seq: None,
            buf: Vec::new(),
        }
    }

    /// Sets the sequence id the next frame must carry (0 at each command).
    pub fn expect_seq(&mut self, seq: u8) {
        self.next_seq = seq;
    }

    /// The sequence id the next frame must carry; after a message, the id
    /// a reply to it starts with.
    pub fn next_seq(&self) -> u8 {
        self.next_seq
    }

    /// Consumes whole frames from the front of `input` until one message is
    /// complete. Returns the bytes consumed and the message, if complete.
    /// A partial frame is left unconsumed; call again with more input.
    pub fn push(&mut self, input: &[u8]) -> Result<(usize, Option<Message>), FrameError> {
        let mut used = 0;
        loop {
            let rest = &input[used..];
            if rest.len() < HEADER_LEN {
                return Ok((used, None));
            }
            let len = usize::from(rest[0]) | usize::from(rest[1]) << 8 | usize::from(rest[2]) << 16;
            let seq = rest[3];
            if seq != self.next_seq {
                return Err(FrameError::Sequence {
                    expected: self.next_seq,
                    got: seq,
                });
            }
            if self.buf.len().saturating_add(len) > self.max_message {
                return Err(FrameError::TooLarge {
                    limit: self.max_message,
                });
            }
            if rest.len() < HEADER_LEN + len {
                return Ok((used, None));
            }
            self.buf
                .extend_from_slice(&rest[HEADER_LEN..HEADER_LEN + len]);
            self.first_seq.get_or_insert(seq);
            self.next_seq = seq.wrapping_add(1);
            used += HEADER_LEN + len;
            if len < MAX_FRAME {
                let seq = self.first_seq.take().unwrap_or(seq);
                return Ok((
                    used,
                    Some(Message {
                        seq,
                        payload: std::mem::take(&mut self.buf),
                    }),
                ));
            }
        }
    }
}

/// Appends `payload` to `out` as frames starting at `*seq`, and advances
/// `*seq` past them. A payload that is a multiple of [`MAX_FRAME`] long
/// (including empty) ends with an empty frame.
pub fn encode(payload: &[u8], seq: &mut u8, out: &mut Vec<u8>) {
    let mut chunks = payload.chunks(MAX_FRAME);
    let mut last_len = MAX_FRAME;
    for chunk in chunks.by_ref() {
        frame(chunk, seq, out);
        last_len = chunk.len();
    }
    if last_len == MAX_FRAME {
        frame(&[], seq, out);
    }
}

fn frame(chunk: &[u8], seq: &mut u8, out: &mut Vec<u8>) {
    let len = chunk.len();
    out.extend_from_slice(&[len as u8, (len >> 8) as u8, (len >> 16) as u8, *seq]);
    out.extend_from_slice(chunk);
    *seq = seq.wrapping_add(1);
}
