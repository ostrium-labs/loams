//! The Redis key layout and the meta record (design §77 §8.4, D1653, D1654;
//! plan KV2 Task 3).
//!
//! Every key is relative to the store's root, which already ends in the
//! product tag `R` ([`RespConfig::store_config`](crate::RespConfig::store_config)).
//! Under it:
//!
//! | Item | Key |
//! |---|---|
//! | Meta | `db ‖ 'm' ‖ enc(ukey)` |
//! | Data of one version | `db ‖ 'd' ‖ enc(ukey) ‖ ver ‖ kind ‖ …` |
//! | Expiry index | `db ‖ 'x' ‖ bucket ‖ expire_at_ms ‖ enc(ukey) ‖ ver` |
//! | GC queue | `'g' ‖ enc(db ‖ ukey) ‖ ver` |
//! | Worker leases | `'L' ‖ name` |
//!
//! `enc` is [`key::put_bytes`] (escaped and terminated, so no user key's
//! encoding is a prefix of another's), `ver` is [`key::put_u64`]. `db` is
//! below [`MAX_DATABASES`](crate::config::MAX_DATABASES), so a database's
//! range never reaches the keyspace-wide rows (ruling K2-8).
//!
//! The meta record is `[format u8][type u8][ver u64][expire_at_ms u64][trailer]`,
//! big-endian; `expire_at_ms` is Unix milliseconds, 0 for none. A string's
//! trailer is its encoding byte and then either its bytes (inline) or its
//! length (chunked, in [`CHUNK`]-byte chunks under `d`; Q1227).
//!
//! `ver` is the start timestamp of the transaction that created the key
//! (D1654), so a new key's data never shares keys with a deleted version
//! still waiting for GC. A transaction that deletes a collection and
//! creates one under the same name gets the same start timestamp twice, so
//! [`next_version`] steps past the old version instead.

use loams_kv::{MAX_VALUE_BYTES, key};

/// The meta record's format byte.
pub const FORMAT: u8 = 1;
/// The meta record's fixed header: format, type, version, expiry.
pub const HEADER: usize = 1 + 1 + 8 + 8;
/// The largest string kept inline in its meta record.
pub const INLINE_MAX: usize = MAX_VALUE_BYTES - HEADER - 1;
/// The size of a large string's chunks (Q1227).
pub const CHUNK: usize = 1024 * 1024;

/// The tag of a meta key, after the database byte.
pub const META: u8 = b'm';
/// The tag of a data key, after the database byte.
pub const DATA: u8 = b'd';
/// The tag of an expiry index key, after the database byte.
pub const EXPIRY: u8 = b'x';
/// The tag of the keyspace-wide GC queue.
pub const GC: u8 = b'g';
/// The tag of the keyspace-wide worker leases.
pub const LEASE: u8 = b'L';

/// The data kind of a large string's chunk, after `ver`.
pub const CHUNK_KIND: u8 = b'c';

/// A key's type, as its meta record stores it and `TYPE` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    String = 0,
    Hash = 1,
    Set = 2,
    Zset = 3,
    List = 4,
}

impl Kind {
    /// The type byte's kind.
    pub fn from_byte(b: u8) -> Option<Kind> {
        Some(match b {
            0 => Kind::String,
            1 => Kind::Hash,
            2 => Kind::Set,
            3 => Kind::Zset,
            4 => Kind::List,
            _ => return None,
        })
    }

    /// The name `TYPE` replies with.
    pub fn name(self) -> &'static str {
        match self {
            Kind::String => "string",
            Kind::Hash => "hash",
            Kind::Set => "set",
            Kind::Zset => "zset",
            Kind::List => "list",
        }
    }

    /// Whether the kind keeps data keys under `d` (collections do; strings
    /// only when chunked).
    pub fn is_collection(self) -> bool {
        self != Kind::String
    }
}

/// How a string's bytes are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StringBody {
    /// In the meta record.
    Inline(Vec<u8>),
    /// In chunks under `d`; the total length.
    Chunked(u64),
}

impl StringBody {
    /// The string's length.
    pub fn len(&self) -> u64 {
        match self {
            StringBody::Inline(b) => u64::try_from(b.len()).unwrap_or(u64::MAX),
            StringBody::Chunked(n) => *n,
        }
    }

    /// Whether the string is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A decoded meta record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub ver: u64,
    /// Unix milliseconds; 0 for no expiry.
    pub expire_at_ms: u64,
    /// The type's trailer: for a string, its body
    /// ([`Meta::string_body`]); for a collection, what its type keeps
    /// (list head and tail, Task 8).
    pub trailer: Vec<u8>,
}

/// A meta record that does not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError(pub String);

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LayoutError {}

const STRING_INLINE: u8 = 0;
const STRING_CHUNKED: u8 = 1;

impl Meta {
    /// A string's meta record.
    pub fn string(ver: u64, expire_at_ms: u64, body: &StringBody) -> Meta {
        let trailer = match body {
            StringBody::Inline(bytes) => {
                let mut t = Vec::with_capacity(1 + bytes.len());
                t.push(STRING_INLINE);
                t.extend_from_slice(bytes);
                t
            }
            StringBody::Chunked(len) => {
                let mut t = vec![STRING_CHUNKED];
                t.extend_from_slice(&len.to_be_bytes());
                t
            }
        };
        Meta {
            kind: Kind::String,
            ver,
            expire_at_ms,
            trailer,
        }
    }

    /// The record's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.trailer.len());
        out.push(FORMAT);
        out.push(self.kind as u8);
        out.extend_from_slice(&self.ver.to_be_bytes());
        out.extend_from_slice(&self.expire_at_ms.to_be_bytes());
        out.extend_from_slice(&self.trailer);
        out
    }

    /// Reads a record.
    pub fn decode(bytes: &[u8]) -> Result<Meta, LayoutError> {
        if bytes.len() < HEADER {
            return Err(LayoutError(format!(
                "meta record of {} bytes, shorter than its header",
                bytes.len()
            )));
        }
        if bytes[0] != FORMAT {
            return Err(LayoutError(format!(
                "meta record format {} is not {FORMAT}",
                bytes[0]
            )));
        }
        let kind = Kind::from_byte(bytes[1])
            .ok_or_else(|| LayoutError(format!("meta record type {} is unknown", bytes[1])))?;
        let u64_at = |at: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[at..at + 8]);
            u64::from_be_bytes(b)
        };
        Ok(Meta {
            kind,
            ver: u64_at(2),
            expire_at_ms: u64_at(10),
            trailer: bytes[HEADER..].to_vec(),
        })
    }

    /// Whether the key has expired at `now_ms`.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.expire_at_ms != 0 && self.expire_at_ms <= now_ms
    }

    /// A string's body; `None` for another type or a damaged trailer.
    pub fn string_body(&self) -> Option<StringBody> {
        if self.kind != Kind::String {
            return None;
        }
        match self.trailer.split_first() {
            Some((&STRING_INLINE, bytes)) => Some(StringBody::Inline(bytes.to_vec())),
            Some((&STRING_CHUNKED, len)) => {
                let len: [u8; 8] = len.try_into().ok()?;
                Some(StringBody::Chunked(u64::from_be_bytes(len)))
            }
            _ => None,
        }
    }
}

/// The meta key of `ukey` in database `db`.
pub fn meta_key(db: u8, ukey: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(ukey.len() + 4);
    k.push(db);
    k.push(META);
    key::put_bytes(&mut k, ukey);
    k
}

/// The user key of a meta key of database `db`; `None` when `meta` is not
/// one.
pub fn user_key_of_meta(db: u8, meta: &[u8]) -> Option<Vec<u8>> {
    let rest = meta.strip_prefix(&[db, META])?;
    let (ukey, n) = key::get_bytes(rest).ok()?;
    (n == rest.len()).then_some(ukey)
}

/// The range of every meta key of database `db`: `[start, end)`.
pub fn meta_range(db: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![db, META], vec![db, META + 1])
}

/// The range of every key of database `db`: `[start, end)`.
pub fn db_range(db: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![db], vec![db + 1])
}

/// The prefix of every data key of version `ver` of `ukey`.
pub fn data_prefix(db: u8, ukey: &[u8], ver: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(ukey.len() + 12);
    k.push(db);
    k.push(DATA);
    key::put_bytes(&mut k, ukey);
    key::put_u64(&mut k, ver);
    k
}

/// The key of chunk `i` of a large string.
pub fn chunk_key(db: u8, ukey: &[u8], ver: u64, i: u32) -> Vec<u8> {
    let mut k = data_prefix(db, ukey, ver);
    k.push(CHUNK_KIND);
    key::put_u32(&mut k, i);
    k
}

/// The number of chunks of a string of `len` bytes.
pub fn chunk_count(len: u64) -> u32 {
    let chunk = u64::try_from(CHUNK).unwrap_or(u64::MAX);
    u32::try_from(len.div_ceil(chunk)).unwrap_or(u32::MAX)
}

/// The tag of a hash field, after the version.
pub const HASH_FIELD: u8 = b'h';
/// The tag of a set member, after the version.
pub const SET_MEMBER: u8 = b's';
/// The tag of a collection's length shard, after the version (Q1226).
pub const LEN_SHARD: u8 = b'#';
/// How many shards a collection's length is kept in (Q1226).
pub const LEN_SHARDS: u16 = 16;

/// The key of `item` (a field or a member) under tag `tag` of version
/// `ver` of `ukey`: `data_prefix ‖ tag ‖ enc(item)`.
pub fn item_key(db: u8, ukey: &[u8], ver: u64, tag: u8, item: &[u8]) -> Vec<u8> {
    let mut k = data_prefix(db, ukey, ver);
    k.push(tag);
    key::put_bytes(&mut k, item);
    k
}

/// The range of every key under tag `tag` of version `ver` of `ukey`.
pub fn tag_range(db: u8, ukey: &[u8], ver: u64, tag: u8) -> (Vec<u8>, Vec<u8>) {
    let mut start = data_prefix(db, ukey, ver);
    let mut end = start.clone();
    start.push(tag);
    end.push(tag + 1);
    (start, end)
}

/// The item an [`item_key`] holds, given the length of its
/// `data_prefix ‖ tag`; `None` when the rest does not decode.
pub fn item_of(full: &[u8], prefix_len: usize) -> Option<Vec<u8>> {
    let rest = full.get(prefix_len..)?;
    let (item, n) = key::get_bytes(rest).ok()?;
    (n == rest.len()).then_some(item)
}

/// The key of length shard `shard` of version `ver` of `ukey`.
pub fn len_shard_key(db: u8, ukey: &[u8], ver: u64, shard: u16) -> Vec<u8> {
    let mut k = data_prefix(db, ukey, ver);
    k.push(LEN_SHARD);
    key::put_u16(&mut k, shard);
    k
}

/// The length shard an item counts in: a hash of the item, so concurrent
/// writes of different items usually touch different shards, and an item
/// is always counted in, and removed from, the same one.
pub fn len_shard(item: &[u8]) -> u16 {
    u16::try_from(fnv1a(item) % u32::from(LEN_SHARDS)).unwrap_or(0)
}

/// FNV-1a, 32 bits: stable across processes and versions.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The bucket count of the expiry index (D1655).
pub const EXPIRY_BUCKETS: u8 = 16;

/// The expiry bucket of `ukey`: FNV-1a over its bytes, so it is stable
/// across processes and versions.
pub fn expiry_bucket(ukey: &[u8]) -> u8 {
    u8::try_from(fnv1a(ukey) % u32::from(EXPIRY_BUCKETS)).unwrap_or(0)
}

/// The expiry index key of version `ver` of `ukey`, expiring at
/// `expire_at_ms` (D1655; its worker is plan Task 10).
pub fn expiry_key(db: u8, ukey: &[u8], ver: u64, expire_at_ms: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(ukey.len() + 24);
    k.push(db);
    k.push(EXPIRY);
    k.push(expiry_bucket(ukey));
    key::put_u64(&mut k, expire_at_ms);
    key::put_bytes(&mut k, ukey);
    key::put_u64(&mut k, ver);
    k
}

/// The GC queue key of version `ver` of `ukey` in database `db`; its value
/// is the type byte (D1656).
pub fn gc_key(db: u8, ukey: &[u8], ver: u64) -> Vec<u8> {
    let mut scoped = Vec::with_capacity(ukey.len() + 1);
    scoped.push(db);
    scoped.extend_from_slice(ukey);
    let mut k = Vec::with_capacity(scoped.len() + 12);
    k.push(GC);
    key::put_bytes(&mut k, &scoped);
    key::put_u64(&mut k, ver);
    k
}

/// The range of the GC queue.
pub fn gc_range() -> (Vec<u8>, Vec<u8>) {
    (vec![GC], vec![GC + 1])
}

/// A worker lease's key.
pub fn lease_key(name: &str) -> Vec<u8> {
    let mut k = vec![LEASE];
    k.extend_from_slice(name.as_bytes());
    k
}

/// The version a key created in a transaction started at `start_ts` gets:
/// the start timestamp, or one past the version it replaces when that is
/// not lower (a delete and a create of one name in one transaction).
pub fn next_version(start_ts: u64, replaced: Option<u64>) -> u64 {
    match replaced {
        Some(old) if old >= start_ts => old.saturating_add(1),
        _ => start_ts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn meta_record_roundtrip() {
        for meta in [
            Meta::string(7, 0, &StringBody::Inline(b"hello".to_vec())),
            Meta::string(u64::MAX, 1_700_000_000_000, &StringBody::Chunked(3 << 20)),
            Meta::string(1, 0, &StringBody::Inline(Vec::new())),
            Meta {
                kind: Kind::Hash,
                ver: 42,
                expire_at_ms: 5,
                trailer: vec![],
            },
            Meta {
                kind: Kind::List,
                ver: 9,
                expire_at_ms: 0,
                trailer: vec![1, 2, 3],
            },
        ] {
            let bytes = meta.encode();
            assert_eq!(Meta::decode(&bytes), Ok(meta.clone()));
        }
        let body = StringBody::Chunked(5);
        assert_eq!(
            Meta::string(1, 0, &body).string_body(),
            Some(body),
            "a chunked body reads back"
        );
        assert!(Meta::decode(&[FORMAT, 0]).is_err());
        assert!(Meta::decode(&[9; HEADER]).is_err());
        let mut bad_type = Meta::string(1, 0, &StringBody::Inline(vec![])).encode();
        bad_type[1] = 77;
        assert!(Meta::decode(&bad_type).is_err());
    }

    #[test]
    fn expiry_is_inclusive_and_zero_is_never() {
        let mut m = Meta::string(1, 0, &StringBody::Inline(vec![]));
        assert!(!m.expired(u64::MAX));
        m.expire_at_ms = 100;
        assert!(!m.expired(99));
        assert!(m.expired(100));
    }

    #[test]
    fn next_version_steps_past_the_replaced() {
        assert_eq!(next_version(10, None), 10);
        assert_eq!(next_version(10, Some(3)), 10);
        assert_eq!(next_version(10, Some(10)), 11);
        assert_eq!(next_version(10, Some(12)), 13);
    }

    #[test]
    fn items_and_shards() {
        let k = item_key(2, b"user", 9, HASH_FIELD, b"f\x00ield");
        let (start, end) = tag_range(2, b"user", 9, HASH_FIELD);
        assert!(k >= start && k < end);
        assert_eq!(item_of(&k, start.len()), Some(b"f\x00ield".to_vec()));
        let shard = len_shard_key(2, b"user", 9, 3);
        assert!(
            !(shard >= start && shard < end),
            "shards sit outside the fields"
        );
        let (s_start, s_end) = tag_range(2, b"user", 9, LEN_SHARD);
        assert!(shard >= s_start && shard < s_end);
        for item in [&b""[..], b"a", b"field-17"] {
            assert!(len_shard(item) < LEN_SHARDS);
            assert_eq!(len_shard(item), len_shard(item));
        }
    }

    #[test]
    fn chunk_counts() {
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(CHUNK as u64), 1);
        assert_eq!(chunk_count(CHUNK as u64 + 1), 2);
    }

    #[test]
    fn keyspace_rows_sort_after_every_database() {
        let max = u8::try_from(crate::config::MAX_DATABASES).unwrap_or(u8::MAX);
        let (_, last_db_end) = db_range(max - 1);
        for row in [lease_key("expire"), gc_range().0] {
            assert!(row >= last_db_end, "{row:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Meta, data, expiry and GC keys never share a prefix across kinds,
        /// users keys or databases: no key of one is a prefix of another's
        /// range, and each meta key decodes back to its user key.
        #[test]
        fn layout_keys_are_disjoint_by_kind(
            a in proptest::collection::vec(any::<u8>(), 0..24),
            b in proptest::collection::vec(any::<u8>(), 0..24),
            db_a in 0u8..64,
            db_b in 0u8..64,
            ver in any::<u64>(),
        ) {
            let meta_a = meta_key(db_a, &a);
            prop_assert_eq!(user_key_of_meta(db_a, &meta_a), Some(a.clone()));
            let keys_a = [
                meta_a.clone(),
                data_prefix(db_a, &a, ver),
                expiry_key(db_a, &a, ver, 1),
                gc_key(db_a, &a, ver),
            ];
            let keys_b = [
                meta_key(db_b, &b),
                data_prefix(db_b, &b, ver),
                expiry_key(db_b, &b, ver, 1),
                gc_key(db_b, &b, ver),
            ];
            // Within one user key, the four kinds are disjoint.
            for (i, x) in keys_a.iter().enumerate() {
                for (j, y) in keys_a.iter().enumerate() {
                    if i != j {
                        prop_assert!(!y.starts_with(x), "{:?} prefixes {:?}", x, y);
                    }
                }
            }
            // Across user keys or databases, the same kind is disjoint.
            if (db_a, &a) != (db_b, &b) {
                for (x, y) in keys_a.iter().zip(keys_b.iter()) {
                    prop_assert!(!y.starts_with(x) && !x.starts_with(y), "{:?} ~ {:?}", x, y);
                }
            }
            // Meta keys of a database stay inside its meta range.
            let (start, end) = meta_range(db_a);
            prop_assert!(meta_a >= start && meta_a < end);
            let (start, end) = db_range(db_a);
            prop_assert!(keys_a[2] >= start && keys_a[2] < end);
        }
    }
}
