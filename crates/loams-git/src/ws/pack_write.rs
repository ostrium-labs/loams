//! Packs the server writes itself (plan GT1 Task 25): the trees and the
//! snapshot commit a server-side change to a workspace makes
//! (`RestoreSalvage`; `ApplyEdits` in Task 26), as one workspace pack, and
//! the repository pack `PushWorkspace` assembles from workspace packs
//! (Task 27), whose stored entries are copied rather than recompressed.
//!
//! Written from `gitformat-pack(5)`: a version 2 pack of whole objects (no
//! deltas, so the pack is self-contained as T24.2 requires) and its
//! version 2 idx, with the CRC32 column filled in, then bundled as D390's
//! `.lpk` by [`LpkWriter::from_parts`].

use std::collections::BTreeMap;
use std::io::Write as _;

use bytes::Bytes;
use gix_object::Kind;

use crate::blob::{BlobError, BlobId, LpkWriter};
use crate::format::pb;
use crate::ids::ObjectId;

/// Offsets at or past this go to the idx's 8-byte table.
const LARGE_OFFSET: u64 = 0x8000_0000;

/// One pack of `objects` (each once, by id), bundled: its blob id, the
/// `PackRef` a snapshot names it by, and the `.lpk` bytes.
pub fn write_ws_pack(
    objects: impl IntoIterator<Item = (Kind, Bytes)>,
) -> Result<(BlobId, pb::PackRef, Bytes), BlobError> {
    let mut unique: BTreeMap<ObjectId, PackEntry> = BTreeMap::new();
    for (kind, data) in objects {
        let oid = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)
            .map_err(|err| BlobError::InvalidPack(format!("hashing an object: {err}")))?;
        unique.entry(oid).or_insert(PackEntry::Whole(kind, data));
    }
    write_pack(&unique)
}

/// One entry of a pack [`write_pack`] writes.
#[derive(Clone, Debug)]
pub enum PackEntry {
    /// An object to store whole, compressed here.
    Whole(Kind, Bytes),
    /// A stored entry of another pack that is not a delta (its header and
    /// compressed data, as `RangeOdb::raw_entry` reads them), copied as it
    /// is (Task 27's copy mode).
    Raw(Bytes),
}

/// A version 2 pack of `entries` (by id, each self-contained: no deltas)
/// and its idx, bundled.
pub(crate) fn write_pack(
    entries: &BTreeMap<ObjectId, PackEntry>,
) -> Result<(BlobId, pb::PackRef, Bytes), BlobError> {
    let count = u32::try_from(entries.len())
        .map_err(|_| BlobError::InvalidPack("more objects than a pack holds".into()))?;
    let mut pack = Vec::new();
    pack.extend_from_slice(b"PACK");
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&count.to_be_bytes());
    // (oid, offset, crc32) in object id order, as the idx lists them.
    let mut idx_entries = Vec::with_capacity(entries.len());
    for (oid, entry) in entries {
        let offset = pack.len() as u64;
        let start = pack.len();
        match entry {
            PackEntry::Whole(kind, data) => {
                entry_header(kind_number(*kind), data.len() as u64, &mut pack);
                let mut z = gix_zlib::stream::deflate::Write::new(
                    Vec::with_capacity(data.len() / 2 + 64),
                    gix_zlib::Compression::default(),
                );
                z.write_all(data)
                    .and_then(|()| z.flush())
                    .map_err(|err| BlobError::InvalidPack(format!("compressing {oid}: {err}")))?;
                pack.extend_from_slice(&z.into_inner());
            }
            PackEntry::Raw(raw) => pack.extend_from_slice(raw),
        }
        idx_entries.push((*oid, offset, crc32(&pack[start..])));
    }
    let pack_sum = sha1(&pack)?;
    pack.extend_from_slice(pack_sum.as_bytes());
    let idx = write_idx(&idx_entries, &pack_sum)?;
    LpkWriter::from_parts(Bytes::from(pack), Bytes::from(idx))
}

/// git's pack type number of a kind.
fn kind_number(kind: Kind) -> u8 {
    match kind {
        Kind::Commit => 1,
        Kind::Tree => 2,
        Kind::Blob => 3,
        Kind::Tag => 4,
    }
}

/// An entry's type and size: the type in bits 4–6 of the first byte with
/// the size's low 4 bits, then 7 bits a byte, the top bit saying more.
fn entry_header(kind: u8, mut size: u64, out: &mut Vec<u8>) {
    let mut byte = (kind << 4) | (size & 0x0f) as u8;
    size >>= 4;
    while size > 0 {
        out.push(byte | 0x80);
        byte = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(byte);
}

/// A version 2 idx: magic, version, fan-out, names, CRC32s, offsets (and
/// the 8-byte table), the pack's checksum, then its own.
fn write_idx(entries: &[(ObjectId, u64, u32)], pack_sum: &ObjectId) -> Result<Vec<u8>, BlobError> {
    let mut idx = Vec::with_capacity(8 + 1024 + entries.len() * 28 + 40);
    idx.extend_from_slice(&[0xff, b't', b'O', b'c']);
    idx.extend_from_slice(&2u32.to_be_bytes());
    let mut fanout = [0u32; 256];
    for (oid, _, _) in entries {
        fanout[usize::from(oid.as_bytes()[0])] += 1;
    }
    let mut total = 0u32;
    for count in fanout {
        total += count;
        idx.extend_from_slice(&total.to_be_bytes());
    }
    for (oid, _, _) in entries {
        idx.extend_from_slice(oid.as_bytes());
    }
    for (_, _, crc) in entries {
        idx.extend_from_slice(&crc.to_be_bytes());
    }
    let mut large = Vec::new();
    for (_, offset, _) in entries {
        if *offset < LARGE_OFFSET {
            idx.extend_from_slice(&(*offset as u32).to_be_bytes());
        } else {
            let at = u32::try_from(large.len()).expect("fewer than 2^31 objects");
            idx.extend_from_slice(&(at | 0x8000_0000).to_be_bytes());
            large.push(*offset);
        }
    }
    for offset in large {
        idx.extend_from_slice(&offset.to_be_bytes());
    }
    idx.extend_from_slice(pack_sum.as_bytes());
    let idx_sum = sha1(&idx)?;
    idx.extend_from_slice(idx_sum.as_bytes());
    Ok(idx)
}

fn sha1(bytes: &[u8]) -> Result<ObjectId, BlobError> {
    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha1);
    hasher.update(bytes);
    hasher
        .try_finalize()
        .map_err(|err| BlobError::InvalidPack(format!("hashing: {err}")))
}

/// CRC-32 (IEEE 802.3, reflected, as zlib's `crc32`), which the idx keeps
/// per entry.
fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    });
    let mut crc = !0u32;
    for byte in bytes {
        crc = TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_check_value() {
        // The CRC-32/ISO-HDLC check value.
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn entry_header_spans_bytes() {
        let mut out = Vec::new();
        entry_header(3, 0x1234, &mut out);
        assert_eq!(out, vec![0x80 | (3 << 4) | 0x4, 0x80 | 0x23, 0x2]);
        let mut out = Vec::new();
        entry_header(2, 15, &mut out);
        assert_eq!(out, vec![(2 << 4) | 0xf]);
    }
}
