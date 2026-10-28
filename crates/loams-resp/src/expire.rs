//! The bucketed expiry index and its worker (plan KV2 Task 10; design §77
//! §8.3 "Expiry", D1655; `distributedio/titan`'s design, bucketed).
//!
//! Every write that sets a TTL also writes
//! `db ‖ 'x' ‖ bucket ‖ key::u64(expire_at_ms) ‖ enc(ukey) ‖ ver` ([`note`]).
//! Entries are never updated in place: a later TTL change, `PERSIST` or
//! overwrite leaves the old entry behind, and the worker drops it when its
//! time comes, because it deletes a key only when the key's meta record
//! still has that version and that expiry. The 16 buckets (by a hash of
//! the user key) spread TTL writes over 16 index ranges instead of one
//! tail (Titan issue #160).

use loams_kv::{Store, TxnError, TxnOptions, key};

use crate::layout::{self, EXPIRY, EXPIRY_BUCKETS, Meta};
use crate::types::{Kv, corrupt, delete_key};

/// Records `meta`'s expiry in the index (nothing for a key without one).
pub(crate) async fn note<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<(), TxnError> {
    if meta.expire_at_ms == 0 {
        return Ok(());
    }
    txn.put(
        &layout::expiry_key(db, ukey, meta.ver, meta.expire_at_ms),
        Vec::new(),
    )
    .await
}

/// An index entry's parts: expiry, user key, version.
fn decode(k: &[u8]) -> Option<(u64, Vec<u8>, u64)> {
    let rest = k.get(3..)?;
    let (at, a) = key::get_u64(rest).ok()?;
    let (ukey, b) = key::get_bytes(&rest[a..]).ok()?;
    let (ver, c) = key::get_u64(&rest[a + b..]).ok()?;
    (a + b + c == rest.len()).then_some((at, ukey, ver))
}

/// The index range of bucket `bucket` of database `db` up to `now_ms`.
fn due_range(db: u8, bucket: u8, now_ms: u64) -> (Vec<u8>, Vec<u8>) {
    let start = vec![db, EXPIRY, bucket];
    let mut end = start.clone();
    key::put_u64(&mut end, now_ms.saturating_add(1));
    (start, end)
}

/// Reclaims the keys of databases `0..databases` that expired by
/// `now_ms`, in transactions of at most `batch` index entries; returns how
/// many keys it deleted. Each entry is removed; its key only when its meta
/// record still has the entry's version and expiry (and so has expired).
pub async fn sweep(
    store: &Store,
    databases: u16,
    now_ms: u64,
    batch: usize,
) -> Result<u64, TxnError> {
    let batch = batch.max(1);
    let mut deleted = 0;
    for db in 0..databases {
        let db = u8::try_from(db).unwrap_or(u8::MAX);
        for bucket in 0..EXPIRY_BUCKETS {
            loop {
                let (start, end) = due_range(db, bucket, now_ms);
                let (n, full) = store
                    .run(TxnOptions::new("resp.expire"), move |t| {
                        let (start, end) = (start.clone(), end.clone());
                        Box::pin(async move { sweep_page(t, db, &start, &end, batch, now_ms).await })
                    })
                    .await?
                    .value;
                deleted += n;
                if !full {
                    break;
                }
            }
        }
    }
    Ok(deleted)
}

async fn sweep_page<K: Kv>(
    t: &mut K,
    db: u8,
    start: &[u8],
    end: &[u8],
    batch: usize,
    now_ms: u64,
) -> Result<(u64, bool), TxnError> {
    let entries = t.scan(start, Some(end), batch).await?;
    let full = entries.len() == batch;
    let mut n = 0;
    for (k, _) in entries {
        if let Some((at, ukey, ver)) = decode(&k) {
            let meta = match t.get(&layout::meta_key(db, &ukey)).await? {
                Some(v) => Some(Meta::decode(&v).map_err(|e| corrupt(&e))?),
                None => None,
            };
            if let Some(meta) = meta
                && meta.ver == ver
                && meta.expire_at_ms == at
                && meta.expired(now_ms)
            {
                delete_key(t, db, &ukey, now_ms).await?;
                n += 1;
            }
        }
        t.delete(&k).await?;
    }
    Ok((n, full))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;
    use crate::types::{hash, read_meta, string};

    #[test]
    fn entries_decode() {
        let k = layout::expiry_key(3, b"a\x00b", 77, 1_234);
        assert_eq!(decode(&k), Some((1_234, b"a\x00b".to_vec(), 77)));
        let (start, end) = due_range(3, layout::expiry_bucket(b"a\x00b"), 1_234);
        assert!(k >= start && k < end, "due at its own time");
        let (_, before) = due_range(3, layout::expiry_bucket(b"a\x00b"), 1_233);
        assert!(k >= before, "not due a millisecond before");
    }

    #[tokio::test]
    async fn expired_keys_are_reclaimed_without_reads() {
        let mut t = Recording { ts: 5, ..Recording::default() };
        string::write(&mut t, 0, b"s", b"v", 100, None).await.expect("a write");
        let fields = vec![(b"f".to_vec(), b"v".to_vec())];
        hash::set(&mut t, 0, b"h", &fields, false, 0).await.expect("hset").expect("a hash");
        let mut m = read_meta(&mut t, 0, b"h").await.expect("a read").expect("meta");
        m.expire_at_ms = 100;
        crate::types::put_meta(&mut t, 0, b"h", &m).await.expect("a put");
        note(&mut t, 0, b"h", &m).await.expect("a note");
        string::write(&mut t, 0, b"keep", b"v", 0, None).await.expect("a write");
        let mut total = 0;
        for bucket in 0..EXPIRY_BUCKETS {
            let (start, end) = due_range(0, bucket, 200);
            total += sweep_page(&mut t, 0, &start, &end, 256, 200).await.expect("a sweep").0;
        }
        assert_eq!(total, 2);
        assert_eq!(read_meta(&mut t, 0, b"s").await.expect("a read"), None);
        assert_eq!(read_meta(&mut t, 0, b"h").await.expect("a read"), None);
        assert!(t.data.contains_key(&layout::gc_key(0, b"h", m.ver)), "the hash goes to GC");
        assert!(read_meta(&mut t, 0, b"keep").await.expect("a read").is_some());
        assert!(!t.data.keys().any(|k| k.get(1) == Some(&EXPIRY)), "the index is empty");
    }

    #[tokio::test]
    async fn overwritten_key_is_not_expired_by_stale_index() {
        let mut t = Recording { ts: 5, ..Recording::default() };
        string::write(&mut t, 0, b"k", b"old", 100, None).await.expect("a write");
        let old = read_meta(&mut t, 0, b"k").await.expect("a read").expect("meta");
        // Overwritten without a TTL, and a second key whose TTL was later
        // extended: both stale entries must leave the keys alone.
        t.ts = 6;
        string::write(&mut t, 0, b"k", b"new", 0, Some(&old)).await.expect("a write");
        string::write(&mut t, 0, b"x", b"v", 100, None).await.expect("a write");
        let mut x = read_meta(&mut t, 0, b"x").await.expect("a read").expect("meta");
        x.expire_at_ms = 10_000;
        crate::types::put_meta(&mut t, 0, b"x", &x).await.expect("a put");
        note(&mut t, 0, b"x", &x).await.expect("a note");
        let mut total = 0;
        for bucket in 0..EXPIRY_BUCKETS {
            let (start, end) = due_range(0, bucket, 200);
            total += sweep_page(&mut t, 0, &start, &end, 256, 200).await.expect("a sweep").0;
        }
        assert_eq!(total, 0);
        assert_eq!(string::get(&mut t, 0, b"k", 200).await.expect("get"), Ok(Some(b"new".to_vec())));
        assert_eq!(string::get(&mut t, 0, b"x", 200).await.expect("get"), Ok(Some(b"v".to_vec())));
        // The extended TTL's own entry is still there for later.
        assert!(t.data.contains_key(&layout::expiry_key(0, b"x", x.ver, 10_000)));
    }
}
