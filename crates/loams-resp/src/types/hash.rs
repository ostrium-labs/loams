//! Hashes (plan KV2 Task 5; design §77 §8.3–§8.4, Q1226): a field is
//! `data_prefix ‖ 'h' ‖ enc(field)` → value, the length is kept in 16
//! shards beside the fields. Replacing or deleting a hash moves its version
//! to the GC queue (D1656; the worker is Task 10). Written from Valkey's
//! documented semantics, with Tidis's `src/tikv/hash.rs` read as a
//! reference, not ported.

use loams_kv::TxnError;

use super::collection::{self, open, open_or_create};
use super::counter::{IncrError, parse_float};
use super::{Kv, WrongType};
use crate::frame::{format_float_human, parse_int};
use crate::layout::{self, HASH_FIELD, Kind, Meta};

/// A hash read: the value, or `WrongType`.
pub(crate) type Read<T> = Result<T, WrongType>;

fn field_key(db: u8, ukey: &[u8], meta: &Meta, field: &[u8]) -> Vec<u8> {
    layout::item_key(db, ukey, meta.ver, HASH_FIELD, field)
}

/// `HSET`/`HMSET`/`HSETNX`: sets each field (only absent ones with `nx`)
/// and returns how many were new.
pub(crate) async fn set<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    pairs: &[(Vec<u8>, Vec<u8>)],
    nx: bool,
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    let meta = match open_or_create(txn, db, ukey, Kind::Hash, now_ms).await? {
        Ok(m) => m,
        Err(w) => return Ok(Err(w)),
    };
    let mut added = 0;
    for (field, value) in pairs {
        let key = field_key(db, ukey, &meta, field);
        let exists = txn.get(&key).await?.is_some();
        if exists && nx {
            continue;
        }
        txn.put(&key, value.clone()).await?;
        if !exists {
            added += 1;
            collection::len_add(txn, db, ukey, meta.ver, field, 1).await?;
        }
    }
    Ok(Ok(added))
}

/// The live hash's meta record, or `None`.
async fn live<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Option<Meta>>, TxnError> {
    Ok(open(txn, db, ukey, Kind::Hash, now_ms).await?.1)
}

/// `HMGET` (and `HGET` with one field): each field's value or `None`.
pub(crate) async fn get<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    fields: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<Vec<Option<Vec<u8>>>>, TxnError> {
    let meta = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(vec![None; fields.len()])),
        Ok(Some(m)) => m,
    };
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        out.push(txn.get(&field_key(db, ukey, &meta, f)).await?);
    }
    Ok(Ok(out))
}

/// `HGETALL`, `HKEYS`, `HVALS`: every field and value, in field order.
pub(crate) async fn all<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Vec<(Vec<u8>, Vec<u8>)>>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok(Vec::new())),
        Ok(Some(m)) => Ok(Ok(
            collection::items(txn, db, ukey, m.ver, HASH_FIELD).await?
        )),
    }
}

/// `HSCAN`'s page: up to `count` fields from `from`, and where to resume.
pub(crate) async fn page<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    from: Option<&[u8]>,
    count: usize,
    now_ms: u64,
) -> Result<Read<(Vec<(Vec<u8>, Vec<u8>)>, Option<Vec<u8>>)>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok((Vec::new(), None))),
        Ok(Some(m)) => Ok(Ok(collection::items_page(
            txn, db, ukey, m.ver, HASH_FIELD, from, count,
        )
        .await?)),
    }
}

/// `HLEN`: the sum of the length shards.
pub(crate) async fn len<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok(0)),
        Ok(Some(m)) => Ok(Ok(collection::len(txn, db, ukey, m.ver).await?)),
    }
}

/// `HDEL`: removes the fields that exist and returns how many. Removing
/// the last fields deletes the hash (meta key first, then its version to
/// GC) instead of deleting them one by one.
pub(crate) async fn del<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    fields: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    let meta = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(0)),
        Ok(Some(m)) => m,
    };
    let mut present: Vec<&Vec<u8>> = Vec::new();
    for f in fields {
        if present.contains(&f) {
            continue;
        }
        if txn.get(&field_key(db, ukey, &meta, f)).await?.is_some() {
            present.push(f);
        }
    }
    let removed = i64::try_from(present.len()).unwrap_or(i64::MAX);
    if removed == 0 {
        return Ok(Ok(0));
    }
    if collection::len(txn, db, ukey, meta.ver).await? <= removed {
        collection::remove(txn, db, ukey, &meta).await?;
        return Ok(Ok(removed));
    }
    for f in present {
        txn.delete(&field_key(db, ukey, &meta, f)).await?;
        collection::len_add(txn, db, ukey, meta.ver, f, -1).await?;
    }
    Ok(Ok(removed))
}

/// `HINCRBY`: adds `by` to the field's integer (0 when absent).
pub(crate) async fn incr_by<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    field: &[u8],
    by: i64,
    now_ms: u64,
) -> Result<Result<i64, IncrError>, TxnError> {
    let meta = match open_or_create(txn, db, ukey, Kind::Hash, now_ms).await? {
        Ok(m) => m,
        Err(_) => return Ok(Err(IncrError::WrongType)),
    };
    let key = field_key(db, ukey, &meta, field);
    let old = txn.get(&key).await?;
    let current = match &old {
        Some(v) => match parse_int(v) {
            Some(n) => n,
            None => return Ok(Err(IncrError::NotANumber)),
        },
        None => 0,
    };
    let Some(next) = current.checked_add(by) else {
        return Ok(Err(IncrError::Overflow));
    };
    txn.put(&key, next.to_string().into_bytes()).await?;
    if old.is_none() {
        collection::len_add(txn, db, ukey, meta.ver, field, 1).await?;
    }
    Ok(Ok(next))
}

/// `HINCRBYFLOAT`: adds `by` to the field's float (0 when absent); returns
/// the stored text.
pub(crate) async fn incr_by_float<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    field: &[u8],
    by: f64,
    now_ms: u64,
) -> Result<Result<String, IncrError>, TxnError> {
    let meta = match open_or_create(txn, db, ukey, Kind::Hash, now_ms).await? {
        Ok(m) => m,
        Err(_) => return Ok(Err(IncrError::WrongType)),
    };
    let key = field_key(db, ukey, &meta, field);
    let old = txn.get(&key).await?;
    let current = match &old {
        Some(v) => match parse_float(v) {
            Some(n) => n,
            None => return Ok(Err(IncrError::NotANumber)),
        },
        None => 0.0,
    };
    let next = current + by;
    if !next.is_finite() {
        return Ok(Err(IncrError::NotFinite));
    }
    let text = format_float_human(next);
    txn.put(&key, text.clone().into_bytes()).await?;
    if old.is_none() {
        collection::len_add(txn, db, ukey, meta.ver, field, 1).await?;
    }
    Ok(Ok(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;
    use crate::types::{read_meta, string};

    fn pairs(n: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
        (0..n)
            .map(|i| (format!("f{i}").into_bytes(), format!("v{i}").into_bytes()))
            .collect()
    }

    #[tokio::test]
    async fn hash_length_shards_sum() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        assert_eq!(
            set(&mut txn, 0, b"h", &pairs(100), false, 0)
                .await
                .expect("hset"),
            Ok(100)
        );
        // Overwriting existing fields adds nothing; a repeated field counts once.
        let again = vec![
            (b"f1".to_vec(), b"x".to_vec()),
            (b"new".to_vec(), b"y".to_vec()),
            (b"new".to_vec(), b"z".to_vec()),
        ];
        assert_eq!(
            set(&mut txn, 0, b"h", &again, false, 0)
                .await
                .expect("hset"),
            Ok(1)
        );
        assert_eq!(len(&mut txn, 0, b"h", 0).await.expect("hlen"), Ok(101));
        let meta = read_meta(&mut txn, 0, b"h")
            .await
            .expect("a read")
            .expect("the meta");
        let (start, end) = layout::tag_range(0, b"h", meta.ver, layout::LEN_SHARD);
        let shards = txn.data.range(start..end).count();
        assert!(
            shards > 1 && shards <= usize::from(layout::LEN_SHARDS),
            "{shards}"
        );
        // HSETNX leaves an existing field alone.
        assert_eq!(
            set(
                &mut txn,
                0,
                b"h",
                &[(b"f1".to_vec(), b"no".to_vec())],
                true,
                0
            )
            .await
            .expect("hsetnx"),
            Ok(0)
        );
        assert_eq!(
            get(&mut txn, 0, b"h", &[b"f1".to_vec(), b"zz".to_vec()], 0)
                .await
                .expect("hmget"),
            Ok(vec![Some(b"x".to_vec()), None])
        );
        assert_eq!(
            del(
                &mut txn,
                0,
                b"h",
                &[b"f1".to_vec(), b"f1".to_vec(), b"zz".to_vec()],
                0
            )
            .await
            .expect("hdel"),
            Ok(1)
        );
        assert_eq!(len(&mut txn, 0, b"h", 0).await.expect("hlen"), Ok(100));
        let all_fields = all(&mut txn, 0, b"h", 0)
            .await
            .expect("hgetall")
            .expect("a hash");
        assert_eq!(all_fields.len(), 100);
    }

    #[tokio::test]
    async fn deleting_the_last_fields_deletes_the_hash() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        set(&mut txn, 0, b"h", &pairs(2), false, 0)
            .await
            .expect("hset")
            .expect("a hash");
        txn.writes.clear();
        assert_eq!(
            del(&mut txn, 0, b"h", &[b"f0".to_vec(), b"f1".to_vec()], 0)
                .await
                .expect("hdel"),
            Ok(2)
        );
        assert_eq!(txn.writes.first(), Some(&layout::meta_key(0, b"h")));
        assert_eq!(read_meta(&mut txn, 0, b"h").await.expect("a read"), None);
        assert!(txn.data.contains_key(&layout::gc_key(0, b"h", 3)));
    }

    #[tokio::test]
    async fn hash_overwrite_by_set_queues_gc() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        set(&mut txn, 0, b"h", &pairs(10), false, 0)
            .await
            .expect("hset")
            .expect("a hash");
        let old = read_meta(&mut txn, 0, b"h")
            .await
            .expect("a read")
            .expect("the meta");
        txn.ts = 4;
        string::write(&mut txn, 0, b"h", b"now a string", 0, Some(&old))
            .await
            .expect("a write");
        assert!(txn.data.contains_key(&layout::gc_key(0, b"h", old.ver)));
        // A new hash under the name does not see the old fields.
        txn.ts = 5;
        crate::types::delete_key(&mut txn, 0, b"h", 0)
            .await
            .expect("del");
        set(
            &mut txn,
            0,
            b"h",
            &[(b"a".to_vec(), b"1".to_vec())],
            false,
            0,
        )
        .await
        .expect("hset")
        .expect("a hash");
        assert_eq!(len(&mut txn, 0, b"h", 0).await.expect("hlen"), Ok(1));
        assert_eq!(
            get(&mut txn, 0, b"h", &[b"f3".to_vec()], 0)
                .await
                .expect("hget"),
            Ok(vec![None])
        );
    }

    #[tokio::test]
    async fn hset_on_string_is_wrongtype() {
        let mut txn = Recording::default();
        string::write(&mut txn, 0, b"s", b"v", 0, None)
            .await
            .expect("a write");
        let field = vec![(b"f".to_vec(), b"v".to_vec())];
        assert_eq!(
            set(&mut txn, 0, b"s", &field, false, 0)
                .await
                .expect("hset"),
            Err(WrongType)
        );
        assert_eq!(
            len(&mut txn, 0, b"s", 0).await.expect("hlen"),
            Err(WrongType)
        );
        assert_eq!(
            del(&mut txn, 0, b"s", &[b"f".to_vec()], 0)
                .await
                .expect("hdel"),
            Err(WrongType)
        );
        assert_eq!(
            incr_by(&mut txn, 0, b"s", b"f", 1, 0)
                .await
                .expect("hincrby"),
            Err(IncrError::WrongType)
        );
        assert_eq!(
            string::get(&mut txn, 0, b"s", 0).await.expect("get"),
            Ok(Some(b"v".to_vec()))
        );
    }

    #[tokio::test]
    async fn hincr_errors_and_counts() {
        let mut txn = Recording::default();
        assert_eq!(
            incr_by(&mut txn, 0, b"h", b"n", 5, 0)
                .await
                .expect("hincrby"),
            Ok(5)
        );
        assert_eq!(len(&mut txn, 0, b"h", 0).await.expect("hlen"), Ok(1));
        assert_eq!(
            incr_by(&mut txn, 0, b"h", b"n", i64::MAX, 0)
                .await
                .expect("hincrby"),
            Err(IncrError::Overflow)
        );
        set(
            &mut txn,
            0,
            b"h",
            &[(b"s".to_vec(), b"abc".to_vec())],
            false,
            0,
        )
        .await
        .expect("hset")
        .expect("a hash");
        assert_eq!(
            incr_by(&mut txn, 0, b"h", b"s", 1, 0)
                .await
                .expect("hincrby"),
            Err(IncrError::NotANumber)
        );
        assert_eq!(
            incr_by_float(&mut txn, 0, b"h", b"fl", 10.5, 0)
                .await
                .expect("hincrbyfloat"),
            Ok("10.5".to_owned())
        );
        assert_eq!(
            incr_by_float(&mut txn, 0, b"h", b"fl", 0.1, 0)
                .await
                .expect("hincrbyfloat"),
            Ok("10.6".to_owned())
        );
        assert_eq!(len(&mut txn, 0, b"h", 0).await.expect("hlen"), Ok(3));
    }
}
