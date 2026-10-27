//! Untagged memcomparable key components (design §77 §7, D1644; KV1 plan
//! KV0 Task 1).
//!
//! [`tuple`](crate::tuple) tags every element with its type, which a key
//! whose layout the caller knows does not need. This module writes the same
//! transforms without tags: the byte order of two encodings is the order of
//! the values, component by component, so a composite key built by
//! appending components sorts lexicographically by its components.
//!
//! - [`put_u16`], [`put_u32`], [`put_u64`]: big-endian;
//! - [`put_i64`]: big-endian with the sign bit flipped;
//! - [`put_f64`]: the tuple codec's float transform (every NaN canonical and
//!   last, `-0.0` written as `0.0`);
//! - [`put_bytes`]: the tuple codec's escape (`0x00` → `0x00 0xFF`) and the
//!   terminator `0x00 0x00`, so no encoding is a proper prefix of another.
//!
//! Every component is prefix-free (the integers and floats are fixed width),
//! so [`desc`], the bitwise inversion of one encoded component, reverses
//! that component's order without disturbing the components after it. The
//! `get_*_desc` decoders read an inverted component back.
//!
//! [`prefix_end`] is the exclusive end of a prefix range
//! ([`tuple::successor`](crate::tuple::successor)'s rule).
//!
//! The `get_*` decoders return the value and the number of bytes it took;
//! offsets in a [`CodecError`] count from the start of the buffer passed in.

use crate::CodecError;
use crate::tuple::{float_bits, float_from_bits};

const SIGN: u64 = 1 << 63;

/// Appends `v` big-endian.
pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian.
pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian.
pub fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian with the sign bit flipped, so negative values sort
/// before positive ones.
pub fn put_i64(out: &mut Vec<u8>, v: i64) {
    put_u64(out, (v as u64) ^ SIGN);
}

/// Appends the order-preserving 8 bytes of `v`: every NaN as one canonical
/// NaN that sorts after `+inf`, and `-0.0` as `0.0`.
pub fn put_f64(out: &mut Vec<u8>, v: f64) {
    put_u64(out, float_bits(v));
}

/// Appends `v` escaped (`0x00` → `0x00 0xFF`) and terminated (`0x00 0x00`).
pub fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    out.reserve(v.len() + 2);
    for &b in v {
        out.push(b);
        if b == 0x00 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0x00, 0x00]);
}

/// Inverts every bit of `encoded`, one encoded component, so that it sorts
/// in descending order. Applying it twice restores the component.
pub fn desc(encoded: &mut [u8]) {
    for b in encoded {
        *b = !*b;
    }
}

/// The exclusive end of the range of keys that start with `prefix`: the
/// smallest key greater than every extension of `prefix`. Empty when there
/// is none (`prefix` is empty or all `0xFF`), which callers read as "no
/// upper bound".
pub fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    crate::tuple::successor(prefix)
}

fn fixed<const N: usize>(buf: &[u8], invert: bool) -> Result<[u8; N], CodecError> {
    let mut bytes: [u8; N] = buf
        .get(..N)
        .and_then(|s| s.try_into().ok())
        .ok_or(CodecError::Truncated(buf.len()))?;
    if invert {
        desc(&mut bytes);
    }
    Ok(bytes)
}

/// Reads a [`put_u16`] component.
pub fn get_u16(buf: &[u8]) -> Result<(u16, usize), CodecError> {
    Ok((u16::from_be_bytes(fixed(buf, false)?), 2))
}

/// Reads a [`put_u32`] component.
pub fn get_u32(buf: &[u8]) -> Result<(u32, usize), CodecError> {
    Ok((u32::from_be_bytes(fixed(buf, false)?), 4))
}

/// Reads a [`put_u64`] component.
pub fn get_u64(buf: &[u8]) -> Result<(u64, usize), CodecError> {
    Ok((u64::from_be_bytes(fixed(buf, false)?), 8))
}

/// Reads a [`put_i64`] component.
pub fn get_i64(buf: &[u8]) -> Result<(i64, usize), CodecError> {
    let (raw, n) = get_u64(buf)?;
    Ok(((raw ^ SIGN) as i64, n))
}

/// Reads a [`put_f64`] component (a NaN comes back as the canonical NaN,
/// `-0.0` as `0.0`).
pub fn get_f64(buf: &[u8]) -> Result<(f64, usize), CodecError> {
    let (raw, n) = get_u64(buf)?;
    Ok((float_from_bits(raw), n))
}

/// Reads a [`put_bytes`] component.
pub fn get_bytes(buf: &[u8]) -> Result<(Vec<u8>, usize), CodecError> {
    unescape(buf, false)
}

/// Reads a [`put_u16`] component inverted by [`desc`].
pub fn get_u16_desc(buf: &[u8]) -> Result<(u16, usize), CodecError> {
    Ok((u16::from_be_bytes(fixed(buf, true)?), 2))
}

/// Reads a [`put_u32`] component inverted by [`desc`].
pub fn get_u32_desc(buf: &[u8]) -> Result<(u32, usize), CodecError> {
    Ok((u32::from_be_bytes(fixed(buf, true)?), 4))
}

/// Reads a [`put_u64`] component inverted by [`desc`].
pub fn get_u64_desc(buf: &[u8]) -> Result<(u64, usize), CodecError> {
    Ok((u64::from_be_bytes(fixed(buf, true)?), 8))
}

/// Reads a [`put_i64`] component inverted by [`desc`].
pub fn get_i64_desc(buf: &[u8]) -> Result<(i64, usize), CodecError> {
    let (raw, n) = get_u64_desc(buf)?;
    Ok(((raw ^ SIGN) as i64, n))
}

/// Reads a [`put_f64`] component inverted by [`desc`].
pub fn get_f64_desc(buf: &[u8]) -> Result<(f64, usize), CodecError> {
    let (raw, n) = get_u64_desc(buf)?;
    Ok((float_from_bits(raw), n))
}

/// Reads a [`put_bytes`] component inverted by [`desc`].
pub fn get_bytes_desc(buf: &[u8]) -> Result<(Vec<u8>, usize), CodecError> {
    unescape(buf, true)
}

/// Undoes [`put_bytes`], reading every byte inverted when `invert`.
fn unescape(buf: &[u8], invert: bool) -> Result<(Vec<u8>, usize), CodecError> {
    let at = |pos: usize| buf.get(pos).map(|&b| if invert { !b } else { b });
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        let b = at(pos).ok_or(CodecError::Truncated(pos))?;
        if b != 0x00 {
            out.push(b);
            pos += 1;
            continue;
        }
        match at(pos + 1) {
            Some(0xFF) => {
                out.push(0x00);
                pos += 2;
            }
            Some(0x00) => return Ok((out, pos + 2)),
            Some(_) => return Err(CodecError::BadEscape(pos)),
            None => return Err(CodecError::Truncated(pos + 1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut out = Vec::new();
        f(&mut out);
        out
    }

    #[test]
    fn layouts_are_the_documented_bytes() {
        assert_eq!(enc(|o| put_u16(o, 0x0102)), [1, 2]);
        assert_eq!(enc(|o| put_u32(o, 1)), [0, 0, 0, 1]);
        assert_eq!(enc(|o| put_i64(o, 0)), [0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            enc(|o| put_i64(o, -1)),
            [0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            enc(|o| put_bytes(o, b"a\0b")),
            [b'a', 0x00, 0xFF, b'b', 0x00, 0x00]
        );
        assert_eq!(enc(|o| put_bytes(o, b"")), [0x00, 0x00]);
        assert_eq!(enc(|o| put_f64(o, -0.0)), enc(|o| put_f64(o, 0.0)));
        // The tuple codec's float bytes, without the tag.
        let mut tagged = Vec::new();
        crate::tuple::encode(&mut tagged, &crate::tuple::Elem::F64(1.5));
        assert_eq!(enc(|o| put_f64(o, 1.5)), tagged[1..]);
    }

    #[test]
    fn decoders_refuse_malformed_input() {
        assert_eq!(get_u16(&[1]).unwrap_err(), CodecError::Truncated(1));
        assert_eq!(get_u64(&[]).unwrap_err(), CodecError::Truncated(0));
        assert_eq!(get_bytes(b"ab").unwrap_err(), CodecError::Truncated(2));
        assert_eq!(
            get_bytes(&[b'a', 0x00]).unwrap_err(),
            CodecError::Truncated(2)
        );
        assert_eq!(
            get_bytes(&[b'a', 0x00, 0x01]).unwrap_err(),
            CodecError::BadEscape(1)
        );
        assert_eq!(
            get_bytes_desc(&[0xFF, 0xFE]).unwrap_err(),
            CodecError::BadEscape(0)
        );
    }

    #[test]
    fn decoders_report_what_they_consumed() {
        let mut buf = enc(|o| put_bytes(o, b"x\0"));
        put_u16(&mut buf, 7);
        let (v, n) = get_bytes(&buf).unwrap();
        assert_eq!((v.as_slice(), n), (b"x\0".as_slice(), 5));
        assert_eq!(get_u16(&buf[n..]).unwrap(), (7, 2));
    }

    #[test]
    fn prefix_end_edge_cases() {
        assert_eq!(prefix_end(b""), Vec::<u8>::new());
        assert_eq!(prefix_end(&[0xFF]), Vec::<u8>::new());
        assert_eq!(prefix_end(&[0x01, 0xFF]), vec![0x02]);
        assert_eq!(prefix_end(b"ab"), b"ac".to_vec());
    }
}
