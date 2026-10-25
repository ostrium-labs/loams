//! ClickHouse's compressed framing of data blocks (R9.48): when a `Query` asks
//! for compression, every `Data` packet's block, both ways, is a sequence of
//! frames after the packet type and the table name:
//!
//! ```text
//! checksum: 16 bytes, CityHash128 (v1.0.2) of the rest of the frame, low half first
//! method:   1 byte (0x02 none, 0x82 LZ4, 0x90 ZSTD)
//! size:     UInt32, the frame's compressed size from `method` on (9 header bytes included)
//! raw:      UInt32, the decompressed size
//! data:     size - 9 bytes
//! ```
//!
//! The House reads all three methods and writes LZ4, as clickhouse-server does by
//! default (`network_compression_method`). A checksum that does not match is
//! refused, never decompressed.

use crate::Error;
use crate::wire::Reader;

/// No compression inside the framing.
pub const METHOD_NONE: u8 = 0x02;
/// LZ4 (block format).
pub const METHOD_LZ4: u8 = 0x82;
/// ZSTD.
pub const METHOD_ZSTD: u8 = 0x90;

const HEADER: usize = 9;
/// ClickHouse's own bound on one frame (`DBMS_MAX_COMPRESSED_SIZE`, 1 GiB).
const MAX_FRAME: usize = 1 << 30;
/// Blocks are written in frames of at most this many bytes, as ClickHouse's
/// `CompressedWriteBuffer` does (1 MiB).
const WRITE_FRAME: usize = 1 << 20;

/// CityHash128 v1.0.2 of `bytes`, as the 16 bytes ClickHouse writes.
fn checksum(bytes: &[u8]) -> [u8; 16] {
    let hash = cityhash_rs::cityhash_102_128(bytes);
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&((hash >> 64) as u64).to_le_bytes());
    out[8..].copy_from_slice(&(hash as u64).to_le_bytes());
    out
}

/// Reads one frame and appends its decompressed bytes to `out`.
pub fn read_frame(reader: &mut Reader<'_>, out: &mut Vec<u8>) -> Result<(), Error> {
    let sum = reader.take(16)?;
    let start = reader.position();
    let method = reader.u8()?;
    let size = u32_le(reader.take(4)?) as usize;
    let raw = u32_le(reader.take(4)?) as usize;
    if !(HEADER..=MAX_FRAME).contains(&size) || raw > MAX_FRAME {
        return Err(Error::Malformed(format!(
            "a compressed frame of {size} bytes ({raw} decompressed)"
        )));
    }
    let data = reader.take(size - HEADER)?;
    let framed = &reader.since(start);
    if checksum(framed) != sum {
        return Err(Error::Malformed(
            "a compressed frame whose checksum does not match".into(),
        ));
    }
    let before = out.len();
    match method {
        METHOD_NONE => out.extend_from_slice(data),
        METHOD_LZ4 => {
            out.resize(before + raw, 0);
            let n = lz4_flex::block::decompress_into(data, &mut out[before..])
                .map_err(|err| Error::Malformed(format!("an LZ4 frame: {err}")))?;
            out.truncate(before + n);
        }
        METHOD_ZSTD => {
            let bytes = zstd::bulk::decompress(data, raw)
                .map_err(|err| Error::Malformed(format!("a ZSTD frame: {err}")))?;
            out.extend_from_slice(&bytes);
        }
        other => {
            return Err(Error::Unsupported(format!(
                "compression method {other:#04x} (the House reads LZ4, ZSTD and none)"
            )));
        }
    }
    if out.len() - before != raw {
        return Err(Error::Malformed(format!(
            "a compressed frame of {} bytes where {raw} were announced",
            out.len() - before
        )));
    }
    Ok(())
}

/// Appends `bytes` as LZ4 frames.
pub fn write_frames(out: &mut Vec<u8>, bytes: &[u8]) {
    for piece in bytes.chunks(WRITE_FRAME) {
        let compressed = lz4_flex::block::compress(piece);
        let mut frame = Vec::with_capacity(HEADER + compressed.len());
        frame.push(METHOD_LZ4);
        frame.extend_from_slice(&((HEADER + compressed.len()) as u32).to_le_bytes());
        frame.extend_from_slice(&(piece.len() as u32).to_le_bytes());
        frame.extend_from_slice(&compressed);
        out.extend_from_slice(&checksum(&frame));
        out.extend_from_slice(&frame);
    }
}

fn u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tests::hex;

    #[test]
    fn clickhouse_clients_frames_decode() {
        // clickhouse-client 26.7 with --compression 1: the empty block that ends
        // a query's external tables, LZ4, as the pinned server received it.
        let frame =
            hex("2f52a1ea36f369c4298e958c545f01d882160000000c000000c0010002ffffffff0300000000");
        let mut out = Vec::new();
        let mut reader = Reader::new(&frame);
        read_frame(&mut reader, &mut out).expect("decodes");
        assert_eq!(reader.position(), frame.len());
        // Revision 54488's BlockInfo has field 3 (an empty list).
        assert_eq!(out, hex("010002ffffffff0300000000"));
        // The server's header block for `SELECT number, toString(number)`, ZSTD.
        let frame = hex(
            "d694ab2dab1c54a3e78fc7150074863890460000003400000028b52ffd2034a10100010002ffffffff0300000200066e756d6265720655496e7436340010746f537472696e67286e756d6265722906537472696e6700",
        );
        let mut out = Vec::new();
        read_frame(&mut Reader::new(&frame), &mut out).expect("decodes");
        assert_eq!(out.len(), 0x34);
        // A flipped byte is refused.
        let mut bad = frame.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(read_frame(&mut Reader::new(&bad), &mut Vec::new()).is_err());
    }

    #[test]
    fn written_frames_read_back() {
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut framed = Vec::new();
        write_frames(&mut framed, &bytes);
        let mut reader = Reader::new(&framed);
        let mut out = Vec::new();
        while !reader.remaining().is_empty() {
            read_frame(&mut reader, &mut out).expect("decodes");
        }
        assert_eq!(out, bytes);
    }
}
