//! Strings (plan KV2 Task 3; design §77 §8.3 "Strings", Q1227).
//!
//! A string up to [`INLINE_MAX`](layout::INLINE_MAX) bytes lives in its meta
//! record; a larger one in [`CHUNK`](layout::CHUNK)-byte chunks under its
//! version's data prefix. Written from Valkey's documented semantics; Tidis
//! (`src/tikv/string.rs`) was read as a reference, not ported.

use loams_kv::TxnError;

use super::{Found, Kv, WrongType, drop_version, read_live};
use crate::layout::{self, Kind, Meta, StringBody};

/// A string's value, or `WrongType`.
pub(crate) type Read<T> = Result<T, WrongType>;

/// The live string at `ukey`: its meta record, or `None`; `WrongType` for
/// another type.
pub(crate) async fn meta<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<(Found, Read<Option<Meta>>), TxnError> {
    let found = read_live(txn, db, ukey, now_ms).await?;
    let read = super::expect_kind(&found, Kind::String).map(Option::<&Meta>::cloned);
    Ok((found, read))
}

/// The whole value of the string `meta` describes.
pub(crate) async fn body<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<Vec<u8>, TxnError> {
    match meta.string_body() {
        Some(StringBody::Inline(bytes)) => Ok(bytes),
        Some(StringBody::Chunked(len)) => {
            let len_usize = usize::try_from(len).unwrap_or(usize::MAX);
            read_range(txn, db, ukey, meta.ver, len, 0, len_usize).await
        }
        None => Err(TxnError::Fatal("resp: a string meta without a body".into())),
    }
}

/// Bytes `start..end` of a chunked string of `len` bytes (clamped to it).
async fn read_range<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    ver: u64,
    len: u64,
    start: usize,
    end: usize,
) -> Result<Vec<u8>, TxnError> {
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    let end = end.min(len);
    if start >= end {
        return Ok(Vec::new());
    }
    let first = start / layout::CHUNK;
    let last = (end - 1) / layout::CHUNK;
    let keys: Vec<Vec<u8>> = (first..=last)
        .map(|i| layout::chunk_key(db, ukey, ver, u32::try_from(i).unwrap_or(u32::MAX)))
        .collect();
    let pairs = txn.batch_get(keys.clone()).await?;
    let mut out = Vec::with_capacity(end - start);
    for (n, k) in keys.iter().enumerate() {
        let chunk = pairs
            .iter()
            .find(|(pk, _)| pk == k)
            .map(|(_, v)| v.as_slice())
            .ok_or_else(|| TxnError::Fatal("resp: a string chunk is missing".into()))?;
        let base = (first + n) * layout::CHUNK;
        let from = start.saturating_sub(base).min(chunk.len());
        let to = (end - base).min(chunk.len());
        out.extend_from_slice(&chunk[from..to]);
    }
    Ok(out)
}

/// `GET`: the live string's value.
pub(crate) async fn get<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Option<Vec<u8>>>, TxnError> {
    let (_, read) = meta(txn, db, ukey, now_ms).await?;
    match read {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok(None)),
        Ok(Some(m)) => Ok(Ok(Some(body(txn, db, ukey, &m).await?))),
    }
}

/// Writes `value` as the string at `ukey`, replacing whatever `stored` is
/// (any type, live or expired): the meta key first (ruling K2-4), then the
/// old version's data is dropped, then the new chunks are written.
pub(crate) async fn write<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    value: &[u8],
    expire_at_ms: u64,
    stored: Option<&Meta>,
) -> Result<(), TxnError> {
    let ver = layout::next_version(txn.start_ts(), stored.map(|m| m.ver));
    let len = u64::try_from(value.len()).unwrap_or(u64::MAX);
    let body = if value.len() <= layout::INLINE_MAX {
        StringBody::Inline(value.to_vec())
    } else {
        StringBody::Chunked(len)
    };
    let meta = Meta::string(ver, expire_at_ms, &body);
    txn.put(&layout::meta_key(db, ukey), meta.encode()).await?;
    if let Some(old) = stored {
        drop_version(txn, db, ukey, old).await?;
    }
    if let StringBody::Chunked(_) = body {
        for (i, chunk) in value.chunks(layout::CHUNK).enumerate() {
            let i = u32::try_from(i).unwrap_or(u32::MAX);
            txn.put(&layout::chunk_key(db, ukey, ver, i), chunk.to_vec())
                .await?;
        }
    }
    Ok(())
}

/// The string's length (from the meta record; no chunk is read).
pub(crate) fn len(meta: &Meta) -> u64 {
    meta.string_body().map_or(0, |b| b.len())
}

/// Valkey's `GETRANGE` bounds over a string of `len` bytes: the byte range
/// `start..end`, empty when the bounds select nothing.
pub fn range_bounds(start: i64, end: i64, len: u64) -> (u64, u64) {
    let len_i = i64::try_from(len).unwrap_or(i64::MAX);
    if len_i == 0 || (start < 0 && end < 0 && start > end) {
        return (0, 0);
    }
    let mut s = if start < 0 { len_i + start } else { start };
    let mut e = if end < 0 { len_i + end } else { end };
    s = s.max(0);
    e = e.max(0);
    if e >= len_i {
        e = len_i - 1;
    }
    if s > e {
        return (0, 0);
    }
    (
        u64::try_from(s).unwrap_or(0),
        u64::try_from(e + 1).unwrap_or(0),
    )
}

/// `GETRANGE`: bytes `start..=end` (Valkey's negative-index rules).
pub(crate) async fn getrange<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    start: i64,
    end: i64,
    now_ms: u64,
) -> Result<Read<Vec<u8>>, TxnError> {
    let (_, read) = meta(txn, db, ukey, now_ms).await?;
    let m = match read {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(Vec::new())),
        Ok(Some(m)) => m,
    };
    let (s, e) = range_bounds(start, end, len(&m));
    let (s, e) = (
        usize::try_from(s).unwrap_or(usize::MAX),
        usize::try_from(e).unwrap_or(usize::MAX),
    );
    match m.string_body() {
        Some(StringBody::Inline(bytes)) => Ok(Ok(bytes
            .get(s..e.min(bytes.len()))
            .unwrap_or_default()
            .to_vec())),
        Some(StringBody::Chunked(total)) => {
            Ok(Ok(read_range(txn, db, ukey, m.ver, total, s, e).await?))
        }
        None => Err(TxnError::Fatal("resp: a string meta without a body".into())),
    }
}

/// `SETRANGE`: overwrites from `offset`, zero-padding a short string, and
/// returns the new length. An empty `value` changes nothing (and creates
/// nothing). The caller has checked `offset + value.len()` against the
/// size limit.
pub(crate) async fn setrange<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    offset: usize,
    value: &[u8],
    now_ms: u64,
) -> Result<Read<u64>, TxnError> {
    let (found, read) = meta(txn, db, ukey, now_ms).await?;
    let live = match read {
        Err(w) => return Ok(Err(w)),
        Ok(live) => live,
    };
    let current = match &live {
        Some(m) => body(txn, db, ukey, m).await?,
        None => Vec::new(),
    };
    if value.is_empty() {
        return Ok(Ok(u64::try_from(current.len()).unwrap_or(u64::MAX)));
    }
    let mut next = current;
    let end = offset + value.len();
    if next.len() < end {
        next.resize(end, 0);
    }
    next[offset..end].copy_from_slice(value);
    let expire = live.as_ref().map_or(0, |m| m.expire_at_ms);
    write(txn, db, ukey, &next, expire, found.stored.as_ref()).await?;
    Ok(Ok(u64::try_from(next.len()).unwrap_or(u64::MAX)))
}

/// `APPEND`: appends `value` and returns the new length; the TTL is kept.
pub(crate) async fn append<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    value: &[u8],
    now_ms: u64,
) -> Result<Read<u64>, TxnError> {
    let (found, read) = meta(txn, db, ukey, now_ms).await?;
    let live = match read {
        Err(w) => return Ok(Err(w)),
        Ok(live) => live,
    };
    let mut next = match &live {
        Some(m) => body(txn, db, ukey, m).await?,
        None => Vec::new(),
    };
    next.extend_from_slice(value);
    let expire = live.as_ref().map_or(0, |m| m.expire_at_ms);
    write(txn, db, ukey, &next, expire, found.stored.as_ref()).await?;
    Ok(Ok(u64::try_from(next.len()).unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;

    #[test]
    fn getrange_bounds_follow_valkey() {
        // "Hello World", 11 bytes.
        for (s, e, want) in [
            (0, 3, (0, 4)),
            (-3, -1, (8, 11)),
            (0, -1, (0, 11)),
            (10, 100, (10, 11)),
            (5, 3, (0, 0)),
            (-1, -5, (0, 0)),
            (-100, 2, (0, 3)),
            (20, 30, (0, 0)),
        ] {
            assert_eq!(range_bounds(s, e, 11), want, "{s}..{e}");
        }
        assert_eq!(range_bounds(0, -1, 0), (0, 0));
    }

    #[tokio::test]
    async fn large_string_chunks_roundtrip() {
        let mut txn = Recording {
            ts: 5,
            ..Recording::default()
        };
        let value: Vec<u8> = (0..layout::INLINE_MAX + 3 * layout::CHUNK + 17)
            .map(|i| u8::try_from(i % 251).unwrap_or(0))
            .collect();
        write(&mut txn, 0, b"big", &value, 0, None)
            .await
            .expect("a write");
        let m = super::super::read_meta(&mut txn, 0, b"big")
            .await
            .expect("a read")
            .expect("the meta");
        assert_eq!(
            m.string_body(),
            Some(StringBody::Chunked(value.len() as u64))
        );
        assert_eq!(
            get(&mut txn, 0, b"big", 0).await.expect("a get"),
            Ok(Some(value.clone()))
        );
        // A range across a chunk boundary.
        let s = layout::CHUNK - 3;
        let e = layout::CHUNK + 4;
        let got = getrange(&mut txn, 0, b"big", s as i64, e as i64, 0)
            .await
            .expect("a range");
        assert_eq!(got, Ok(value[s..=e].to_vec()));
        // Overwritten small: the chunks are gone.
        txn.ts = 6;
        let m_old = m.clone();
        write(&mut txn, 0, b"big", b"small", 0, Some(&m_old))
            .await
            .expect("a write");
        let prefix = layout::data_prefix(0, b"big", m_old.ver);
        assert!(
            !txn.data.keys().any(|k| k.starts_with(&prefix)),
            "the old chunks are deleted"
        );
        assert_eq!(
            get(&mut txn, 0, b"big", 0).await.expect("a get"),
            Ok(Some(b"small".to_vec()))
        );
    }

    #[tokio::test]
    async fn meta_key_is_written_first() {
        let mut txn = Recording {
            ts: 10,
            ..Recording::default()
        };
        let meta_key = layout::meta_key(0, b"k");
        // A chunked write over a hash: meta, then the GC entry, then chunks.
        let hash = Meta {
            kind: Kind::Hash,
            ver: 3,
            expire_at_ms: 0,
            trailer: vec![],
        };
        txn.put(&meta_key, hash.encode()).await.expect("a put");
        txn.writes.clear();
        let big = vec![7u8; layout::INLINE_MAX + 1];
        write(&mut txn, 0, b"k", &big, 0, Some(&hash))
            .await
            .expect("a write");
        assert_eq!(txn.writes.first(), Some(&meta_key));
        assert!(txn.writes.contains(&layout::gc_key(0, b"k", 3)));
        // Append, setrange and delete write the meta key first too.
        for step in 0..3 {
            txn.writes.clear();
            match step {
                0 => {
                    append(&mut txn, 0, b"k", b"x", 0).await.expect("append");
                }
                1 => {
                    setrange(&mut txn, 0, b"k", 2, b"yy", 0)
                        .await
                        .expect("setrange");
                }
                _ => {
                    super::super::delete_key(&mut txn, 0, b"k", 0)
                        .await
                        .expect("delete");
                }
            }
            assert_eq!(txn.writes.first(), Some(&meta_key), "step {step}");
        }
        assert!(txn.data.keys().all(|k| k.first() == Some(&layout::GC)));
    }

    #[tokio::test]
    async fn wrongtype_on_every_write() {
        let mut txn = Recording::default();
        let set = Meta {
            kind: Kind::Set,
            ver: 1,
            expire_at_ms: 0,
            trailer: vec![],
        };
        txn.put(&layout::meta_key(0, b"s"), set.encode())
            .await
            .expect("a put");
        assert_eq!(
            get(&mut txn, 0, b"s", 0).await.expect("get"),
            Err(WrongType)
        );
        assert_eq!(
            append(&mut txn, 0, b"s", b"x", 0).await.expect("append"),
            Err(WrongType)
        );
        assert_eq!(
            setrange(&mut txn, 0, b"s", 0, b"x", 0)
                .await
                .expect("setrange"),
            Err(WrongType)
        );
        assert_eq!(
            getrange(&mut txn, 0, b"s", 0, -1, 0)
                .await
                .expect("getrange"),
            Err(WrongType)
        );
        // Nothing was written over the set.
        let m = super::super::read_meta(&mut txn, 0, b"s")
            .await
            .expect("a read");
        assert_eq!(m, Some(set));
    }

    #[tokio::test]
    async fn lazy_expiry_hides_and_replaces() {
        let mut txn = Recording {
            ts: 50,
            ..Recording::default()
        };
        // A hash that expired at 100 ms.
        let hash = Meta {
            kind: Kind::Hash,
            ver: 7,
            expire_at_ms: 100,
            trailer: vec![],
        };
        txn.put(&layout::meta_key(0, b"h"), hash.encode())
            .await
            .expect("a put");
        // Before its expiry it is a hash; after, it reads as absent.
        assert_eq!(
            get(&mut txn, 0, b"h", 99).await.expect("get"),
            Err(WrongType)
        );
        assert_eq!(get(&mut txn, 0, b"h", 100).await.expect("get"), Ok(None));
        // APPEND over it creates a fresh string with no TTL, and queues the
        // dead hash's version for GC.
        assert_eq!(
            append(&mut txn, 0, b"h", b"v", 200).await.expect("append"),
            Ok(1)
        );
        let m = super::super::read_meta(&mut txn, 0, b"h")
            .await
            .expect("a read")
            .expect("the meta");
        assert_eq!(m.kind, Kind::String);
        assert_eq!(m.expire_at_ms, 0);
        assert!(txn.data.contains_key(&layout::gc_key(0, b"h", 7)));
    }
}
