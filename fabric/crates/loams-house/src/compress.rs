//! HTTP body compression (FL2 Task 2): request bodies sent with
//! `Content-Encoding: gzip|deflate|zstd`, and responses compressed when the
//! request says `enable_http_compression=1` and the client accepts one of them.
//!
//! Both directions are push-based (`write::` codecs over a `Vec`), so a body is
//! decoded and encoded a piece at a time as it streams and is never held whole.
//! `deflate` is HTTP's: zlib-wrapped, as ClickHouse reads and writes it.
//!
//! ClickHouse's *own* compressed framing (`compress=1`, `decompress=1`, LZ4 blocks
//! with CityHash128 checksums) is a different thing and is not served yet.

use std::io::{self, Write};

use crate::errors::{ChError, HouseError};

/// A content coding the House speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// `gzip`.
    Gzip,
    /// `deflate` (zlib-wrapped).
    Deflate,
    /// `zstd`.
    Zstd,
}

impl Encoding {
    /// The token, as it appears in `Content-Encoding`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Zstd => "zstd",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => Some(Self::Gzip),
            "deflate" => Some(Self::Deflate),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }
}

/// A request's `Content-Encoding`: `None` for identity or absent; any other
/// coding is `36 BAD_ARGUMENTS`.
pub fn content_encoding(value: Option<&str>) -> Result<Option<Encoding>, HouseError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.eq_ignore_ascii_case("identity") {
        return Ok(None);
    }
    Encoding::parse(value).map(Some).ok_or_else(|| {
        HouseError::from(ChError::bad_arguments(format!(
            "Unknown Content-Encoding of HTTP request: {value}"
        )))
    })
}

/// The coding to answer with: the first one the client lists (and does not refuse
/// with `q=0`) that the House speaks.
pub fn accepted(value: Option<&str>) -> Option<Encoding> {
    value?.split(',').find_map(|item| {
        let mut parts = item.split(';');
        let coding = Encoding::parse(parts.next()?)?;
        let refused = parts.any(|p| {
            p.trim()
                .strip_prefix("q=")
                .and_then(|q| q.trim().parse::<f32>().ok())
                == Some(0.0)
        });
        (!refused).then_some(coding)
    })
}

/// The most a request body may expand to, and by how much (Task 3 review I3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLimits {
    /// Decompressed bytes in all: §49 §12's 16 GiB per `INSERT`.
    pub max_bytes: u64,
    /// Decompressed bytes per compressed byte: §49 §12's 100.
    pub max_ratio: u64,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024 * 1024,
            max_ratio: 100,
        }
    }
}

/// Slack under the ratio cap, so a tiny body that expands to a few lines is not
/// a "bomb".
const RATIO_SLACK: u64 = 1024 * 1024;

/// How much compressed input a flate step takes: deflate expands at most ~1032x,
/// so one step yields at most about 2 MiB, under [`CHUNK_BYTES`].
const FLATE_STEP: usize = 2048;

/// The most bytes one decoded piece holds.
pub const CHUNK_BYTES: usize = loams_house_ipc::CHUNK_BYTES;

/// Decodes a request body: compressed bytes go in with [`Decoder::push`], and
/// decoded pieces of at most [`CHUNK_BYTES`] come out of [`Decoder::next_piece`],
/// so a body is never expanded whole (Task 3 review I3). A body that ends before
/// its stream does — a truncated gzip, zlib or zstd stream — is an error at the end
/// (review I5), so the caller can refuse to commit it.
pub struct Decoder {
    codec: Codec,
    /// Compressed input not yet decoded: `pending[consumed..]`. Consumed bytes are
    /// skipped, not drained, and dropped when more input arrives (fix round 2, N3).
    pending: Vec<u8>,
    consumed: usize,
    ended: bool,
    total_in: u64,
    total_out: u64,
    limits: BodyLimits,
}

enum Codec {
    Identity,
    Gzip(Box<flate2::write::GzDecoder<Vec<u8>>>),
    Deflate {
        state: Box<flate2::Decompress>,
        done: bool,
    },
    Zstd {
        state: Box<zstd::stream::raw::Decoder<'static>>,
        /// One output buffer for the whole body (N3).
        out: Vec<u8>,
        /// zstd's hint after the last call: 0 once a frame is complete.
        remaining: usize,
    },
}

fn bad(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

impl Decoder {
    /// A decoder for `encoding`, within `limits`.
    pub fn new(encoding: Option<Encoding>, limits: BodyLimits) -> io::Result<Self> {
        let codec = match encoding {
            None => Codec::Identity,
            Some(Encoding::Gzip) => {
                Codec::Gzip(Box::new(flate2::write::GzDecoder::new(Vec::new())))
            }
            Some(Encoding::Deflate) => Codec::Deflate {
                state: Box::new(flate2::Decompress::new(true)),
                done: false,
            },
            Some(Encoding::Zstd) => Codec::Zstd {
                state: Box::new(zstd::stream::raw::Decoder::new()?),
                out: Vec::new(),
                remaining: 0,
            },
        };
        Ok(Self {
            codec,
            pending: Vec::new(),
            consumed: 0,
            ended: false,
            total_in: 0,
            total_out: 0,
            limits,
        })
    }

    /// Adds compressed bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.total_in += bytes.len() as u64;
        if self.consumed > 0 {
            self.pending.drain(..self.consumed);
            self.consumed = 0;
        }
        self.pending.extend_from_slice(bytes);
    }

    /// Says no more bytes will come; [`Decoder::next_piece`] then checks that the
    /// stream really ended.
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// The next decoded piece, at most [`CHUNK_BYTES`]; `None` when the input so far
    /// is used up (and, after [`Decoder::end`], when the body is complete).
    pub fn next_piece(&mut self) -> io::Result<Option<Vec<u8>>> {
        let piece = self.step()?;
        if let Some(piece) = &piece {
            self.total_out += piece.len() as u64;
            if self.total_out > self.limits.max_bytes {
                return Err(bad(format!(
                    "the decompressed request body is larger than {} bytes",
                    self.limits.max_bytes
                )));
            }
            if self.total_out
                > self
                    .limits
                    .max_ratio
                    .saturating_mul(self.total_in)
                    .saturating_add(RATIO_SLACK)
            {
                return Err(bad(format!(
                    "the request body decompresses more than {}x",
                    self.limits.max_ratio
                )));
            }
        }
        Ok(piece)
    }

    fn step(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let input = &self.pending[self.consumed..];
            match &mut self.codec {
                Codec::Identity => {
                    if input.is_empty() {
                        return Ok(None);
                    }
                    let take = CHUNK_BYTES.min(input.len());
                    let piece = input[..take].to_vec();
                    self.consumed += take;
                    return Ok(Some(piece));
                }
                Codec::Gzip(decoder) => {
                    if input.is_empty() {
                        if !self.ended {
                            return Ok(None);
                        }
                        // The trailer's CRC and length must have arrived: a
                        // truncated gzip stream fails here.
                        decoder
                            .try_finish()
                            .map_err(|_| bad("the gzip request body is truncated or corrupt"))?;
                        let rest = std::mem::take(decoder.get_mut());
                        return Ok((!rest.is_empty()).then_some(rest));
                    }
                    let take = FLATE_STEP.min(input.len());
                    decoder.write_all(&input[..take])?;
                    self.consumed += take;
                    let out = std::mem::take(decoder.get_mut());
                    if !out.is_empty() {
                        return Ok(Some(out));
                    }
                }
                Codec::Deflate { state, done } => {
                    if *done || input.is_empty() {
                        if self.ended && !*done {
                            return Err(bad("the deflate request body is truncated"));
                        }
                        if *done && !input.is_empty() {
                            return Err(bad("bytes after the end of the deflate request body"));
                        }
                        return Ok(None);
                    }
                    let mut out = Vec::with_capacity(CHUNK_BYTES.min(1 << 20));
                    let before_in = state.total_in();
                    let status = state
                        .decompress_vec(input, &mut out, flate2::FlushDecompress::None)
                        .map_err(|err| {
                            bad(format!("the deflate request body is corrupt: {err}"))
                        })?;
                    let used = (state.total_in() - before_in) as usize;
                    self.consumed += used;
                    if status == flate2::Status::StreamEnd {
                        *done = true;
                    }
                    if !out.is_empty() {
                        return Ok(Some(out));
                    }
                    if used == 0 && !*done {
                        // Needs more input than there is.
                        if self.ended {
                            return Err(bad("the deflate request body is truncated"));
                        }
                        return Ok(None);
                    }
                }
                Codec::Zstd {
                    state,
                    out,
                    remaining,
                } => {
                    if input.is_empty() {
                        if self.ended && *remaining != 0 {
                            return Err(bad("the zstd request body is truncated"));
                        }
                        return Ok(None);
                    }
                    if out.len() != CHUNK_BYTES {
                        out.resize(CHUNK_BYTES, 0);
                    }
                    let status =
                        zstd::stream::raw::Operation::run_on_buffers(state.as_mut(), input, out)
                            .map_err(|err| {
                                bad(format!("the zstd request body is corrupt: {err}"))
                            })?;
                    self.consumed += status.bytes_read;
                    *remaining = status.remaining;
                    if status.bytes_written > 0 {
                        return Ok(Some(out[..status.bytes_written].to_vec()));
                    }
                    if status.bytes_read == 0 {
                        return Ok(None);
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let codec = match self.codec {
            Codec::Identity => "identity",
            Codec::Gzip(_) => "gzip",
            Codec::Deflate { .. } => "deflate",
            Codec::Zstd { .. } => "zstd",
        };
        f.debug_struct("Decoder")
            .field("codec", &codec)
            .field("total_in", &self.total_in)
            .field("total_out", &self.total_out)
            .finish_non_exhaustive()
    }
}

/// Encodes a response body a piece at a time.
pub enum Encoder {
    /// gzip.
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
    /// deflate.
    Deflate(flate2::write::ZlibEncoder<Vec<u8>>),
    /// zstd.
    Zstd(Box<zstd::stream::write::Encoder<'static, Vec<u8>>>),
}

impl Encoder {
    /// An encoder for `encoding`.
    pub fn new(encoding: Encoding) -> io::Result<Self> {
        let level = flate2::Compression::default();
        Ok(match encoding {
            Encoding::Gzip => Self::Gzip(flate2::write::GzEncoder::new(Vec::new(), level)),
            Encoding::Deflate => Self::Deflate(flate2::write::ZlibEncoder::new(Vec::new(), level)),
            Encoding::Zstd => {
                Self::Zstd(Box::new(zstd::stream::write::Encoder::new(Vec::new(), 3)?))
            }
        })
    }

    /// Feeds plain bytes; returns the compressed bytes produced so far (often
    /// none: the codecs buffer).
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Gzip(e) => {
                e.write_all(bytes)?;
                Ok(std::mem::take(e.get_mut()))
            }
            Self::Deflate(e) => {
                e.write_all(bytes)?;
                Ok(std::mem::take(e.get_mut()))
            }
            Self::Zstd(e) => {
                e.write_all(bytes)?;
                Ok(std::mem::take(e.get_mut()))
            }
        }
    }

    /// Ends the stream; returns the rest, trailer included.
    pub fn finish(self) -> io::Result<Vec<u8>> {
        match self {
            Self::Gzip(e) => e.finish(),
            Self::Deflate(e) => e.finish(),
            Self::Zstd(e) => e.finish(),
        }
    }
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Gzip(_) => "Encoder::Gzip",
            Self::Deflate(_) => "Encoder::Deflate",
            Self::Zstd(_) => "Encoder::Zstd",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compress(coding: Encoding, plain: &[u8]) -> Vec<u8> {
        let mut encoder = Encoder::new(coding).expect("encoder");
        let mut wire = Vec::new();
        for piece in plain.chunks(7_000) {
            wire.extend(encoder.feed(piece).expect("encode"));
        }
        wire.extend(encoder.finish().expect("finish"));
        wire
    }

    /// Decodes `wire` pushed in `step`-byte pieces.
    fn decode(
        coding: Option<Encoding>,
        wire: &[u8],
        step: usize,
        limits: BodyLimits,
    ) -> io::Result<(Vec<u8>, usize)> {
        let mut decoder = Decoder::new(coding, limits).expect("decoder");
        let mut back = Vec::new();
        let mut largest = 0;
        for piece in wire.chunks(step.max(1)) {
            decoder.push(piece);
            while let Some(out) = decoder.next_piece()? {
                largest = largest.max(out.len());
                back.extend(out);
            }
        }
        decoder.end();
        while let Some(out) = decoder.next_piece()? {
            largest = largest.max(out.len());
            back.extend(out);
        }
        Ok((back, largest))
    }

    #[test]
    fn every_coding_round_trips_in_pieces() {
        let plain: Vec<u8> = (0..50_000u32).flat_map(|n| n.to_le_bytes()).collect();
        for coding in [Encoding::Gzip, Encoding::Deflate, Encoding::Zstd] {
            let wire = compress(coding, &plain);
            for step in [1, 333, 1 << 20] {
                let (back, _) =
                    decode(Some(coding), &wire, step, BodyLimits::default()).expect("decodes");
                assert_eq!(back, plain, "{coding:?} in {step}-byte pieces");
            }
        }
        let (back, _) = decode(None, &plain, 4096, BodyLimits::default()).expect("identity");
        assert_eq!(back, plain);
    }

    #[test]
    fn truncated_bodies_are_errors() {
        let plain: Vec<u8> = (0..20_000u32).flat_map(|n| n.to_le_bytes()).collect();
        for coding in [Encoding::Gzip, Encoding::Deflate, Encoding::Zstd] {
            let wire = compress(coding, &plain);
            for cut in [wire.len() - 1, wire.len() - 5, wire.len() / 2] {
                let result = decode(Some(coding), &wire[..cut], 1000, BodyLimits::default());
                assert!(
                    result.is_err(),
                    "{coding:?} cut at {cut} of {} must fail",
                    wire.len()
                );
            }
        }
    }

    #[test]
    fn bombs_are_refused_in_bounded_pieces() {
        let zeros = vec![0u8; 16 * 1024 * 1024];
        for coding in [Encoding::Gzip, Encoding::Deflate, Encoding::Zstd] {
            let wire = compress(coding, &zeros);
            let err = decode(Some(coding), &wire, 1 << 20, BodyLimits::default())
                .expect_err("over the ratio");
            assert!(err.to_string().contains("100x"), "{coding:?}: {err}");
            // Within a generous ratio, it decodes, and no piece is over CHUNK_BYTES.
            let generous = BodyLimits {
                max_ratio: 1 << 20,
                ..BodyLimits::default()
            };
            let (back, largest) = decode(Some(coding), &wire, 1 << 20, generous).expect("decodes");
            assert_eq!(back.len(), zeros.len());
            assert!(largest <= CHUNK_BYTES, "{coding:?}: a {largest}-byte piece");
            // And the total cap holds.
            let small = BodyLimits {
                max_bytes: 1024 * 1024,
                max_ratio: 1 << 20,
            };
            let err = decode(Some(coding), &wire, 1 << 20, small).expect_err("over the total");
            assert!(err.to_string().contains("larger than"), "{coding:?}: {err}");
        }
    }

    #[test]
    fn negotiation() {
        assert_eq!(accepted(Some("br, zstd;q=0.9, gzip")), Some(Encoding::Zstd));
        assert_eq!(accepted(Some("gzip;q=0, deflate")), Some(Encoding::Deflate));
        assert_eq!(accepted(Some("br")), None);
        assert_eq!(accepted(None), None);
        assert_eq!(content_encoding(Some("identity")).expect("ok"), None);
        assert_eq!(
            content_encoding(Some("GZIP")).expect("ok"),
            Some(Encoding::Gzip)
        );
        assert_eq!(content_encoding(Some("br")).expect_err("br").code(), 36);
    }
}
