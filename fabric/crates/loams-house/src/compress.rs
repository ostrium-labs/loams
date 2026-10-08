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

/// Decodes a request body a piece at a time.
pub enum Decoder {
    /// No coding: bytes pass through.
    Identity,
    /// gzip.
    Gzip(flate2::write::GzDecoder<Vec<u8>>),
    /// deflate.
    Deflate(flate2::write::ZlibDecoder<Vec<u8>>),
    /// zstd.
    Zstd(Box<zstd::stream::write::Decoder<'static, Vec<u8>>>),
}

impl Decoder {
    /// A decoder for `encoding`.
    pub fn new(encoding: Option<Encoding>) -> io::Result<Self> {
        Ok(match encoding {
            None => Self::Identity,
            Some(Encoding::Gzip) => Self::Gzip(flate2::write::GzDecoder::new(Vec::new())),
            Some(Encoding::Deflate) => Self::Deflate(flate2::write::ZlibDecoder::new(Vec::new())),
            Some(Encoding::Zstd) => {
                Self::Zstd(Box::new(zstd::stream::write::Decoder::new(Vec::new())?))
            }
        })
    }

    /// Feeds compressed bytes; returns what they decoded to so far.
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Identity => Ok(bytes.to_vec()),
            Self::Gzip(d) => {
                d.write_all(bytes)?;
                Ok(std::mem::take(d.get_mut()))
            }
            Self::Deflate(d) => {
                d.write_all(bytes)?;
                Ok(std::mem::take(d.get_mut()))
            }
            Self::Zstd(d) => {
                d.write_all(bytes)?;
                d.flush()?;
                Ok(std::mem::take(d.get_mut()))
            }
        }
    }

    /// Ends the body; returns the rest. A truncated stream is an error.
    pub fn finish(self) -> io::Result<Vec<u8>> {
        match self {
            Self::Identity => Ok(Vec::new()),
            Self::Gzip(d) => d.finish(),
            Self::Deflate(d) => d.finish(),
            Self::Zstd(mut d) => {
                d.flush()?;
                Ok(std::mem::take(d.get_mut()))
            }
        }
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

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Identity => "Decoder::Identity",
            Self::Gzip(_) => "Decoder::Gzip",
            Self::Deflate(_) => "Decoder::Deflate",
            Self::Zstd(_) => "Decoder::Zstd",
        })
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

    #[test]
    fn every_coding_round_trips_in_pieces() {
        let plain: Vec<u8> = (0..50_000u32).flat_map(|n| n.to_le_bytes()).collect();
        for coding in [Encoding::Gzip, Encoding::Deflate, Encoding::Zstd] {
            let mut encoder = Encoder::new(coding).expect("encoder");
            let mut wire = Vec::new();
            for piece in plain.chunks(7_000) {
                wire.extend(encoder.feed(piece).expect("encode"));
            }
            wire.extend(encoder.finish().expect("finish"));

            let mut decoder = Decoder::new(Some(coding)).expect("decoder");
            let mut back = Vec::new();
            for piece in wire.chunks(333) {
                back.extend(decoder.feed(piece).expect("decode"));
            }
            back.extend(decoder.finish().expect("finish"));
            assert_eq!(back, plain, "{coding:?}");
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
