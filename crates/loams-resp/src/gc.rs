//! The GC worker (plan KV2 Task 10; design §77 §8.3 "GC of old versions",
//! D1656): it pages the GC queue `'g' ‖ enc(db ‖ ukey) ‖ ver` and deletes
//! each queued version's data keys in transactions of at most `batch`
//! keys (`distributedio/titan`'s batched deletes; Tidis deleted a whole
//! version in one transaction and failed on big collections, its #97).
//! The queue entry goes in the transaction that deletes the last keys.

use loams_kv::{Store, TxnError, TxnOptions, key};

use crate::layout::{self, Meta};
use crate::types::{Kv, corrupt};

/// The queue entries one call looks at, at most.
const QUEUE_PAGE: usize = 64;

/// A queue entry's database, user key and version.
fn decode(k: &[u8]) -> Option<(u8, Vec<u8>, u64)> {
    let rest = k.strip_prefix(&[layout::GC])?;
    let (scoped, a) = key::get_bytes(rest).ok()?;
    let (ver, b) = key::get_u64(&rest[a..]).ok()?;
    let (&db, ukey) = scoped.split_first()?;
    (a + b == rest.len()).then(|| (db, ukey.to_vec(), ver))
}

/// Deletes the data of every queued version, `batch` keys per transaction;
/// returns how many data keys it deleted.
pub async fn collect(store: &Store, batch: usize) -> Result<u64, TxnError> {
    let batch = batch.max(1);
    let mut deleted = 0;
    loop {
        let (start, end) = layout::gc_range();
        let queue = store
            .run(TxnOptions::new("resp.gc"), move |t| {
                let (start, end) = (start.clone(), end.clone());
                Box::pin(async move { t.scan(&start, Some(&end), QUEUE_PAGE).await })
            })
            .await?
            .value;
        let full = queue.len() == QUEUE_PAGE;
        for (entry, _) in queue {
            deleted += collect_one(store, &entry, batch).await?;
        }
        if !full {
            return Ok(deleted);
        }
    }
}

/// Deletes one queued version's data and then its entry.
async fn collect_one(store: &Store, entry: &[u8], batch: usize) -> Result<u64, TxnError> {
    let mut deleted = 0;
    loop {
        let entry = entry.to_vec();
        let (n, done) = store
            .run(TxnOptions::new("resp.gc"), move |t| {
                let entry = entry.clone();
                Box::pin(async move { collect_step(t, &entry, batch).await })
            })
            .await?
            .value;
        deleted += n;
        if done {
            return Ok(deleted);
        }
    }
}

/// One transaction of GC: up to `batch` data keys of the entry's version,
/// and the entry itself once nothing is left. Returns the keys deleted and
/// whether the entry is gone.
pub(crate) async fn collect_step<K: Kv>(
    t: &mut K,
    entry: &[u8],
    batch: usize,
) -> Result<(u64, bool), TxnError> {
    let Some((db, ukey, ver)) = decode(entry) else {
        // Not ours to understand: drop it rather than loop on it.
        t.delete(entry).await?;
        return Ok((0, true));
    };
    // A version still current is not garbage (a version is never reused,
    // `layout::next_version`; this is the belt to that brace).
    if let Some(v) = t.get(&layout::meta_key(db, &ukey)).await?
        && Meta::decode(&v).map_err(|e| corrupt(&e))?.ver == ver
    {
        t.delete(entry).await?;
        return Ok((0, true));
    }
    let prefix = layout::data_prefix(db, &ukey, ver);
    let keys = t
        .scan(&prefix, Some(&key::prefix_end(&prefix)), batch)
        .await?;
    let n = u64::try_from(keys.len()).unwrap_or(u64::MAX);
    for (k, _) in &keys {
        t.delete(k).await?;
    }
    if keys.len() < batch {
        t.delete(entry).await?;
        return Ok((n, true));
    }
    Ok((n, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::hash;
    use crate::types::recording::Recording;

    #[test]
    fn entries_decode() {
        let k = layout::gc_key(4, b"u\x00k", 99);
        assert_eq!(decode(&k), Some((4, b"u\x00k".to_vec(), 99)));
        assert_eq!(decode(b"x"), None);
    }

    /// A 10,000-field hash is deleted in transactions of at most 256 keys.
    #[tokio::test]
    async fn gc_deletes_in_bounded_batches() {
        let mut t = Recording {
            ts: 7,
            ..Recording::default()
        };
        let fields: Vec<(Vec<u8>, Vec<u8>)> = (0..10_000)
            .map(|i| (format!("f{i:05}").into_bytes(), b"v".to_vec()))
            .collect();
        hash::set(&mut t, 0, b"big", &fields, false, 0)
            .await
            .expect("hset")
            .expect("a hash");
        let meta = crate::types::read_meta(&mut t, 0, b"big")
            .await
            .expect("a read")
            .expect("meta");
        crate::types::delete_key(&mut t, 0, b"big", 0)
            .await
            .expect("del");
        let entry = layout::gc_key(0, b"big", meta.ver);
        assert!(t.data.contains_key(&entry));
        let data_keys = t.data.len() - 1;
        let mut steps = 0;
        let mut total = 0;
        loop {
            t.writes.clear();
            let (n, done) = collect_step(&mut t, &entry, 256).await.expect("a step");
            assert!(
                t.writes.len() <= 257,
                "at most 256 keys and the entry per step"
            );
            total += n;
            steps += 1;
            if done {
                break;
            }
        }
        assert_eq!(total, u64::try_from(data_keys).unwrap_or(0));
        assert!(steps >= 10_000 / 256, "{steps} steps");
        assert!(t.data.is_empty(), "nothing is left: {} keys", t.data.len());
    }

    #[tokio::test]
    async fn a_current_version_is_kept() {
        let mut t = Recording {
            ts: 7,
            ..Recording::default()
        };
        let fields = vec![(b"f".to_vec(), b"v".to_vec())];
        hash::set(&mut t, 0, b"h", &fields, false, 0)
            .await
            .expect("hset")
            .expect("a hash");
        let meta = crate::types::read_meta(&mut t, 0, b"h")
            .await
            .expect("a read")
            .expect("meta");
        let entry = layout::gc_key(0, b"h", meta.ver);
        t.put(&entry, vec![1]).await.expect("a put");
        assert_eq!(
            collect_step(&mut t, &entry, 256).await.expect("a step"),
            (0, true)
        );
        assert_eq!(hash::len(&mut t, 0, b"h", 0).await.expect("hlen"), Ok(1));
        assert!(!t.data.contains_key(&entry));
    }
}
