//! Keyspace operations (plan KV2 Task 9): pages of a database's live keys
//! for `KEYS` and `SCAN`, a random key, and moving or copying a key with
//! its data for `RENAME`, `RENAMENX` and `COPY`.
//!
//! Every read pages through the database's meta range only (D1653), never
//! the data. A moved or copied collection gets a fresh version under the
//! new name and its data keys are rewritten under it; the old version goes
//! to the GC queue. A key with more than [`MOVE_MAX_KEYS`] data keys is
//! refused rather than moved in one transaction (documented).

use loams_kv::{TxnError, key};

use super::{Kv, collection, corrupt, delete_key, drop_version, read_live};
use crate::layout::{self, Meta};

/// The most data keys `RENAME`/`COPY` move in one transaction.
pub const MOVE_MAX_KEYS: usize = 10_000;

/// One page of a database's keys from `from` (a user key; `None`: the
/// first), up to `count` stored keys looked at: the live ones with their
/// meta records, and the user key to resume from if more remain.
pub(crate) async fn page<K: Kv>(
    txn: &mut K,
    db: u8,
    from: Option<&[u8]>,
    count: usize,
    now_ms: u64,
) -> Result<(Vec<(Vec<u8>, Meta)>, Option<Vec<u8>>), TxnError> {
    let (range_start, end) = layout::meta_range(db);
    let start = match from {
        Some(ukey) => layout::meta_key(db, ukey),
        None => range_start,
    };
    let count = count.max(1);
    let mut pairs = txn.scan(&start, Some(&end), count + 1).await?;
    let next = if pairs.len() > count {
        pairs
            .pop()
            .and_then(|(k, _)| layout::user_key_of_meta(db, &k))
    } else {
        None
    };
    let mut out = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        let ukey = layout::user_key_of_meta(db, &k)
            .ok_or_else(|| TxnError::Fatal("resp: a meta key does not decode".into()))?;
        let meta = Meta::decode(&v).map_err(|e| corrupt(&e))?;
        if !meta.expired(now_ms) {
            out.push((ukey, meta));
        }
    }
    Ok((out, next))
}

/// A live key at or after `start` (a user key), wrapping to the first key
/// of the database; `None` when no key is live among the first `tries`
/// looked at from each point.
pub(crate) async fn key_from<K: Kv>(
    txn: &mut K,
    db: u8,
    start: &[u8],
    tries: usize,
    now_ms: u64,
) -> Result<Option<Vec<u8>>, TxnError> {
    for from in [Some(start), None] {
        let (live, _) = page(txn, db, from, tries, now_ms).await?;
        if let Some((ukey, _)) = live.into_iter().next() {
            return Ok(Some(ukey));
        }
    }
    Ok(None)
}

/// What a move or copy did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Moved {
    Done,
    /// The source is absent.
    NoSource,
    /// The destination exists and was not to be replaced.
    DestinationExists,
    /// The source has more than [`MOVE_MAX_KEYS`] data keys.
    TooLarge,
}

/// Copies the live key `src` of database `db` to `dst` of database
/// `dst_db` (replacing whatever is there when `replace`), then deletes
/// `src` when `delete_source` (`RENAME`). The copy's TTL is the source's.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn copy<K: Kv>(
    txn: &mut K,
    db: u8,
    src: &[u8],
    dst_db: u8,
    dst: &[u8],
    replace: bool,
    delete_source: bool,
    now_ms: u64,
) -> Result<Moved, TxnError> {
    let Some(meta) = read_live(txn, db, src, now_ms).await?.live else {
        return Ok(Moved::NoSource);
    };
    if db == dst_db && src == dst {
        return Ok(Moved::Done);
    }
    let found = read_live(txn, dst_db, dst, now_ms).await?;
    if found.live.is_some() && !replace {
        return Ok(Moved::DestinationExists);
    }
    let prefix = layout::data_prefix(db, src, meta.ver);
    let data = collection::scan_range(
        txn,
        &prefix,
        &key::prefix_end(&prefix),
        false,
        Some(MOVE_MAX_KEYS + 1),
    )
    .await?;
    if data.len() > MOVE_MAX_KEYS {
        return Ok(Moved::TooLarge);
    }
    // The destination: its old key (any type) gives way, then the new meta
    // record first, then the data under the new version.
    if found.live.is_some() {
        delete_key(txn, dst_db, dst, now_ms).await?;
    }
    let stored = read_live(txn, dst_db, dst, now_ms).await?.stored;
    let copy = Meta {
        ver: layout::next_version(txn.start_ts(), stored.as_ref().map(|m| m.ver)),
        ..meta.clone()
    };
    txn.put(&layout::meta_key(dst_db, dst), copy.encode())
        .await?;
    crate::expire::note(txn, dst_db, dst, &copy).await?;
    if let Some(old) = &stored {
        drop_version(txn, dst_db, dst, old).await?;
    }
    let new_prefix = layout::data_prefix(dst_db, dst, copy.ver);
    for (k, v) in data {
        let mut moved = new_prefix.clone();
        moved.extend_from_slice(&k[prefix.len()..]);
        txn.put(&moved, v).await?;
    }
    if delete_source {
        txn.delete(&layout::meta_key(db, src)).await?;
        drop_version(txn, db, src, &meta).await?;
    }
    Ok(Moved::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Kind;
    use crate::types::recording::Recording;
    use crate::types::{hash, string};

    #[tokio::test]
    async fn rename_moves_data_and_ttl() {
        let mut txn = Recording {
            ts: 5,
            ..Recording::default()
        };
        let fields: Vec<(Vec<u8>, Vec<u8>)> = (0..5)
            .map(|i| (format!("f{i}").into_bytes(), b"v".to_vec()))
            .collect();
        hash::set(&mut txn, 0, b"h", &fields, false, 0)
            .await
            .expect("hset")
            .expect("a hash");
        let mut meta = crate::types::read_meta(&mut txn, 0, b"h")
            .await
            .expect("a read")
            .expect("meta");
        meta.expire_at_ms = 9_000;
        crate::types::put_meta(&mut txn, 0, b"h", &meta)
            .await
            .expect("a put");
        txn.ts = 6;
        assert_eq!(
            copy(&mut txn, 0, b"h", 0, b"g", true, true, 0)
                .await
                .expect("rename"),
            Moved::Done
        );
        assert_eq!(
            crate::types::read_meta(&mut txn, 0, b"h")
                .await
                .expect("a read"),
            None
        );
        let moved = crate::types::read_meta(&mut txn, 0, b"g")
            .await
            .expect("a read")
            .expect("meta");
        assert_eq!(moved.kind, Kind::Hash);
        assert_eq!(moved.expire_at_ms, 9_000, "the TTL moves with the key");
        assert_ne!(moved.ver, meta.ver);
        assert_eq!(hash::len(&mut txn, 0, b"g", 0).await.expect("hlen"), Ok(5));
        assert!(
            txn.data.contains_key(&layout::gc_key(0, b"h", meta.ver)),
            "the old version is queued"
        );
    }

    #[tokio::test]
    async fn copy_respects_replace_and_databases() {
        let mut txn = Recording {
            ts: 5,
            ..Recording::default()
        };
        string::write(&mut txn, 0, b"a", b"1", 0, None)
            .await
            .expect("a write");
        string::write(&mut txn, 0, b"b", b"2", 0, None)
            .await
            .expect("a write");
        assert_eq!(
            copy(&mut txn, 0, b"a", 0, b"b", false, false, 0)
                .await
                .expect("copy"),
            Moved::DestinationExists
        );
        assert_eq!(
            copy(&mut txn, 0, b"a", 0, b"b", true, false, 0)
                .await
                .expect("copy"),
            Moved::Done
        );
        assert_eq!(
            string::get(&mut txn, 0, b"b", 0).await.expect("get"),
            Ok(Some(b"1".to_vec()))
        );
        assert_eq!(
            copy(&mut txn, 0, b"a", 3, b"a", false, false, 0)
                .await
                .expect("copy"),
            Moved::Done
        );
        assert_eq!(
            string::get(&mut txn, 3, b"a", 0).await.expect("get"),
            Ok(Some(b"1".to_vec()))
        );
        assert_eq!(
            string::get(&mut txn, 0, b"a", 0).await.expect("get"),
            Ok(Some(b"1".to_vec()))
        );
        assert_eq!(
            copy(&mut txn, 0, b"none", 0, b"x", true, true, 0)
                .await
                .expect("copy"),
            Moved::NoSource
        );
    }

    #[tokio::test]
    async fn pages_skip_expired_and_resume() {
        let mut txn = Recording {
            ts: 5,
            ..Recording::default()
        };
        for i in 0..10 {
            let expire = if i % 3 == 0 { 50 } else { 0 };
            string::write(&mut txn, 0, format!("k{i}").as_bytes(), b"v", expire, None)
                .await
                .expect("a write");
        }
        string::write(&mut txn, 1, b"other-db", b"v", 0, None)
            .await
            .expect("a write");
        let mut seen = Vec::new();
        let mut from: Option<Vec<u8>> = None;
        loop {
            let (live, next) = page(&mut txn, 0, from.as_deref(), 3, 100)
                .await
                .expect("a page");
            seen.extend(
                live.into_iter()
                    .map(|(k, _)| String::from_utf8_lossy(&k).into_owned()),
            );
            match next {
                Some(n) => from = Some(n),
                None => break,
            }
        }
        assert_eq!(seen, ["k1", "k2", "k4", "k5", "k7", "k8"]);
        assert_eq!(
            key_from(&mut txn, 0, b"k5", 4, 100).await.expect("a key"),
            Some(b"k5".to_vec())
        );
        assert_eq!(
            key_from(&mut txn, 0, b"zzz", 4, 100).await.expect("a key"),
            Some(b"k1".to_vec()),
            "wraps"
        );
        assert_eq!(
            key_from(&mut txn, 2, b"", 4, 100).await.expect("a key"),
            None
        );
    }
}
