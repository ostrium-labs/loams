//! The protocol's primitives: LEB128 `VarUInt`s, length-prefixed strings and
//! little-endian fixed-width integers.
//!
//! Decoding is incremental: a [`Reader`] over the bytes received so far answers
//! [`Error::Incomplete`] when a value runs past them, and the caller reads more
//! and starts the packet again. Nothing is consumed until a whole packet decodes.

use crate::Error;

/// The longest string the codec accepts: a query, a setting, a column name, a
/// type name or one `String` value in a block. ClickHouse's own limit for a
/// packet field is 1 GiB; a statement over `max_query_size` is refused later.
pub const MAX_STRING: u64 = 1 << 30;

/// A cursor over received bytes.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// How many bytes have been read.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// The bytes not read yet.
    pub fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    /// The bytes from `start` to the current position.
    pub fn since(&self, start: usize) -> &'a [u8] {
        &self.buf[start..self.pos]
    }

    /// `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Incomplete)?;
        if end > self.buf.len() {
            return Err(Error::Incomplete);
        }
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    /// `n` bytes skipped, where `n` comes from the wire (so may be absurd).
    pub fn skip(&mut self, n: u64) -> Result<(), Error> {
        let n = usize::try_from(n).map_err(|_| Error::Incomplete)?;
        self.take(n).map(|_| ())
    }

    /// One byte.
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    /// A `Bool`: one byte, any non-zero value true.
    pub fn bool(&mut self) -> Result<bool, Error> {
        Ok(self.u8()? != 0)
    }

    /// A little-endian `Int32`.
    pub fn i32(&mut self) -> Result<i32, Error> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A little-endian `UInt64`.
    pub fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.take(8)?;
        let mut le = [0u8; 8];
        le.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(le))
    }

    /// A `VarUInt` (LEB128, at most ten bytes).
    pub fn varuint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Malformed("a VarUInt longer than ten bytes".into()))
    }

    /// A length-prefixed byte string.
    pub fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = self.varuint()?;
        if len > MAX_STRING {
            return Err(Error::Malformed(format!(
                "a string of {len} bytes, over the {MAX_STRING} the House reads"
            )));
        }
        self.take(len as usize)
    }

    /// A length-prefixed string, which must be UTF-8.
    pub fn string(&mut self) -> Result<String, Error> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::Malformed("a string that is not UTF-8".into()))
    }
}

/// Appends a `VarUInt`.
pub fn put_varuint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Appends a length-prefixed string.
pub fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varuint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// Appends a length-prefixed string.
pub fn put_str(out: &mut Vec<u8>, text: &str) {
    put_bytes(out, text.as_bytes());
}

/// Appends a little-endian `Int32`.
pub fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `UInt64`.
pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn varuint_matches_clickhouse() {
        // The oracle's Hello carries revision 54492 as dc a9 03.
        let mut out = Vec::new();
        put_varuint(&mut out, 54492);
        assert_eq!(out, [0xdc, 0xa9, 0x03]);
        assert_eq!(Reader::new(&out).varuint().expect("decodes"), 54492);
    }

    #[test]
    fn short_input_is_incomplete_not_malformed() {
        assert!(matches!(
            Reader::new(&[0x80]).varuint(),
            Err(Error::Incomplete)
        ));
        assert!(matches!(
            Reader::new(&[5, b'a']).bytes(),
            Err(Error::Incomplete)
        ));
        assert!(matches!(
            Reader::new(&[0xff; 11]).varuint(),
            Err(Error::Malformed(_))
        ));
    }

    proptest! {
        #[test]
        fn varuint_roundtrip(value: u64) {
            let mut out = Vec::new();
            put_varuint(&mut out, value);
            let mut reader = Reader::new(&out);
            prop_assert_eq!(reader.varuint().expect("decodes"), value);
            prop_assert_eq!(reader.position(), out.len());
        }

        #[test]
        fn string_roundtrip(text in ".{0,300}") {
            let mut out = Vec::new();
            put_str(&mut out, &text);
            prop_assert_eq!(Reader::new(&out).string().expect("decodes"), text);
        }
    }
}
