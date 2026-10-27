//! The Redis types on `loams-kv` transactions (plan KV2 Tasks 3–8).
//!
//! Every function takes a [`Kv`]: a `loams_kv::Txn` in the server, a
//! recording wrapper in tests (so `meta_key_is_written_first` can see the
//! order of writes, ruling K2-4). Functions read the meta key first, check
//! its type, and when they write, write (or delete) the meta key before any
//! data key, so on TiKV the meta key is the transaction's primary (T0-7).
//!
//! Expiry is lazy here: a meta record past its `expire_at_ms` reads as
//! absent ([`read_live`]), and a write over it replaces it, removing the
//! dead version's data as [`drop_version`] does. The expiry index and its
//! worker are plan Task 10.

use loams_kv::{Pair, Txn, TxnError};

use crate::layout::{self, Kind, Meta, StringBody};

pub mod collection;
pub mod counter;
pub mod hash;
pub mod list;
pub mod set;
pub mod string;
pub mod zset;

/// The transaction operations the types use.
#[allow(async_fn_in_trait)]
pub(crate) trait Kv {
    /// The transaction's start timestamp.
    fn start_ts(&self) -> u64;
    async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError>;
    async fn batch_get(&mut self, keys: Vec<Vec<u8>>) -> Result<Vec<Pair>, TxnError>;
    async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError>;
    /// Like [`scan`](Kv::scan), in descending key order.
    async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError>;
    async fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), TxnError>;
    async fn delete(&mut self, key: &[u8]) -> Result<(), TxnError>;
}

impl Kv for Txn {
    fn start_ts(&self) -> u64 {
        Txn::start_ts(self).0
    }
    async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        Txn::get(self, key).await
    }
    async fn batch_get(&mut self, keys: Vec<Vec<u8>>) -> Result<Vec<Pair>, TxnError> {
        Txn::batch_get(self, keys).await
    }
    async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        Txn::scan(self, start, end, limit).await
    }
    async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        Txn::scan_reverse(self, start, end, limit).await
    }
    async fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), TxnError> {
        Txn::put(self, key, value).await
    }
    async fn delete(&mut self, key: &[u8]) -> Result<(), TxnError> {
        Txn::delete(self, key).await
    }
}

/// `WRONGTYPE`: the key holds another type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrongType;

/// Valkey's `WRONGTYPE` text.
pub const WRONGTYPE: &str = "WRONGTYPE Operation against a key holding the wrong kind of value";

/// A store error for a meta record that does not decode: the store holds
/// something this gateway did not write.
pub(crate) fn corrupt(e: &layout::LayoutError) -> TxnError {
    TxnError::Fatal(format!("resp: {e}"))
}

/// The meta record of `ukey`, expired or not.
pub(crate) async fn read_meta<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
) -> Result<Option<Meta>, TxnError> {
    match txn.get(&layout::meta_key(db, ukey)).await? {
        Some(bytes) => Meta::decode(&bytes).map(Some).map_err(|e| corrupt(&e)),
        None => Ok(None),
    }
}

/// What a read of `ukey` finds at `now_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// The live record, if any.
    pub live: Option<Meta>,
    /// The record, live or expired (a write over it must clean it up).
    pub stored: Option<Meta>,
}

/// Reads `ukey`'s meta, treating an expired record as absent.
pub(crate) async fn read_live<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Found, TxnError> {
    let stored = read_meta(txn, db, ukey).await?;
    let live = stored.clone().filter(|m| !m.expired(now_ms));
    Ok(Found { live, stored })
}

/// Removes the data of a version that is no longer reachable: a chunked
/// string's chunks are deleted inline; a collection's version goes to the
/// GC queue (D1656), unless it is the version `keep` that this transaction
/// is about to reuse. Inline strings have no data.
pub(crate) async fn drop_version<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    old: &Meta,
) -> Result<(), TxnError> {
    if old.kind.is_collection() {
        return txn
            .put(&layout::gc_key(db, ukey, old.ver), vec![old.kind as u8])
            .await;
    }
    if let Some(StringBody::Chunked(len)) = old.string_body() {
        for i in 0..layout::chunk_count(len) {
            txn.delete(&layout::chunk_key(db, ukey, old.ver, i)).await?;
        }
    }
    Ok(())
}

/// Deletes `ukey`: the meta key first, then its version's data (by
/// [`drop_version`]). Returns whether a live key was deleted.
pub(crate) async fn delete_key<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<bool, TxnError> {
    let found = read_live(txn, db, ukey, now_ms).await?;
    let Some(stored) = found.stored else {
        return Ok(false);
    };
    txn.delete(&layout::meta_key(db, ukey)).await?;
    drop_version(txn, db, ukey, &stored).await?;
    Ok(found.live.is_some())
}

/// Rewrites `meta` under `ukey` (a TTL or trailer change; same version).
pub(crate) async fn put_meta<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<(), TxnError> {
    txn.put(&layout::meta_key(db, ukey), meta.encode()).await
}

/// The live key's type, or `WRONGTYPE` when it is not `kind`.
pub(crate) fn expect_kind(found: &Found, kind: Kind) -> Result<Option<&Meta>, WrongType> {
    match &found.live {
        Some(m) if m.kind != kind => Err(WrongType),
        other => Ok(other.as_ref()),
    }
}

/// Unix time in milliseconds.
pub fn now_ms() -> u64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) mod recording {
    //! A [`Kv`] that records the keys it writes, in order.

    use super::{Kv, Pair, TxnError};
    use std::collections::BTreeMap;

    /// An in-memory transaction: a sorted map and the log of writes.
    #[derive(Debug, Default)]
    pub struct Recording {
        pub data: BTreeMap<Vec<u8>, Vec<u8>>,
        pub writes: Vec<Vec<u8>>,
        pub ts: u64,
    }

    impl Kv for Recording {
        fn start_ts(&self) -> u64 {
            self.ts
        }
        async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
            Ok(self.data.get(key).cloned())
        }
        async fn batch_get(&mut self, keys: Vec<Vec<u8>>) -> Result<Vec<Pair>, TxnError> {
            let mut out: Vec<Pair> = keys
                .into_iter()
                .filter_map(|k| self.data.get(&k).map(|v| (k, v.clone())))
                .collect();
            out.sort();
            out.dedup_by(|a, b| a.0 == b.0);
            Ok(out)
        }
        async fn scan(
            &mut self,
            start: &[u8],
            end: Option<&[u8]>,
            limit: usize,
        ) -> Result<Vec<Pair>, TxnError> {
            Ok(self
                .data
                .range(start.to_vec()..)
                .take_while(|(k, _)| end.is_none_or(|e| k.as_slice() < e))
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
        async fn scan_reverse(
            &mut self,
            start: &[u8],
            end: Option<&[u8]>,
            limit: usize,
        ) -> Result<Vec<Pair>, TxnError> {
            Ok(self
                .data
                .range(start.to_vec()..)
                .take_while(|(k, _)| end.is_none_or(|e| k.as_slice() < e))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
        async fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), TxnError> {
            self.writes.push(key.to_vec());
            self.data.insert(key.to_vec(), value);
            Ok(())
        }
        async fn delete(&mut self, key: &[u8]) -> Result<(), TxnError> {
            self.writes.push(key.to_vec());
            self.data.remove(key);
            Ok(())
        }
    }
}
