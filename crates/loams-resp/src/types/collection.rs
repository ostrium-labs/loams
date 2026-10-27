//! What every collection type shares (plan KV2 Tasks 5–8): opening a key
//! of one kind, creating it (meta first, ruling K2-4), the sharded length
//! (Q1226) and paged scans of its items.

use loams_kv::{Pair, TxnError};

use super::{Found, Kv, WrongType, drop_version, expect_kind, read_live};
use crate::layout::{self, Kind, Meta};

/// The items one scan transaction step reads at most.
pub const PAGE: usize = 1024;

/// A collection's live meta record at `ukey`, and what is stored there.
pub(crate) async fn open<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    kind: Kind,
    now_ms: u64,
) -> Result<(Found, Result<Option<Meta>, WrongType>), TxnError> {
    let found = read_live(txn, db, ukey, now_ms).await?;
    let read = expect_kind(&found, kind).map(Option::<&Meta>::cloned);
    Ok((found, read))
}

/// Creates an empty collection of `kind` at `ukey` over whatever `stored`
/// is (nothing, or an expired key): its meta record first, then the old
/// version's data dropped. Returns the new meta record.
pub(crate) async fn create<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    kind: Kind,
    stored: Option<&Meta>,
) -> Result<Meta, TxnError> {
    let meta = Meta {
        kind,
        ver: layout::next_version(txn.start_ts(), stored.map(|m| m.ver)),
        expire_at_ms: 0,
        trailer: Vec::new(),
    };
    txn.put(&layout::meta_key(db, ukey), meta.encode()).await?;
    if let Some(old) = stored {
        drop_version(txn, db, ukey, old).await?;
    }
    Ok(meta)
}

/// The live collection, created empty when absent: `Ok(meta)` or
/// `WrongType`.
pub(crate) async fn open_or_create<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    kind: Kind,
    now_ms: u64,
) -> Result<Result<Meta, WrongType>, TxnError> {
    let (found, read) = open(txn, db, ukey, kind, now_ms).await?;
    match read {
        Err(w) => Ok(Err(w)),
        Ok(Some(m)) => Ok(Ok(m)),
        Ok(None) => Ok(Ok(create(txn, db, ukey, kind, found.stored.as_ref()).await?)),
    }
}

fn shard_value(bytes: &[u8]) -> i64 {
    bytes.try_into().map(i64::from_be_bytes).unwrap_or_default()
}

/// Adds `delta` to the length shard `item` counts in.
pub(crate) async fn len_add<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    ver: u64,
    item: &[u8],
    delta: i64,
) -> Result<(), TxnError> {
    if delta == 0 {
        return Ok(());
    }
    let key = layout::len_shard_key(db, ukey, ver, layout::len_shard(item));
    let current = txn.get(&key).await?.map_or(0, |v| shard_value(&v));
    let next = current + delta;
    if next == 0 {
        txn.delete(&key).await
    } else {
        txn.put(&key, next.to_be_bytes().to_vec()).await
    }
}

/// The collection's length: the sum of its shards.
pub(crate) async fn len<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    ver: u64,
) -> Result<i64, TxnError> {
    let (start, end) = layout::tag_range(db, ukey, ver, layout::LEN_SHARD);
    let shards = txn
        .scan(&start, Some(&end), usize::from(layout::LEN_SHARDS))
        .await?;
    Ok(shards.iter().map(|(_, v)| shard_value(v)).sum())
}

/// Every item under `tag` of the collection, with its value, in key order:
/// paged scans of [`PAGE`] in this transaction.
pub(crate) async fn items<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    ver: u64,
    tag: u8,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>, TxnError> {
    let (mut start, end) = layout::tag_range(db, ukey, ver, tag);
    let prefix_len = start.len();
    let mut out = Vec::new();
    loop {
        let page: Vec<Pair> = txn.scan(&start, Some(&end), PAGE).await?;
        let full = page.len() == PAGE;
        if let Some((last, _)) = page.last() {
            start = last.clone();
            start.push(0);
        }
        for (k, v) in page {
            let item = layout::item_of(&k, prefix_len)
                .ok_or_else(|| TxnError::Fatal("resp: an item key does not decode".into()))?;
            out.push((item, v));
        }
        if !full {
            return Ok(out);
        }
    }
}

/// One page of items under `tag` from `from` (an item; `None`: the
/// first), at most `count`; and the item to resume from, if any remain.
pub(crate) async fn items_page<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    ver: u64,
    tag: u8,
    from: Option<&[u8]>,
    count: usize,
) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, Option<Vec<u8>>), TxnError> {
    let (range_start, end) = layout::tag_range(db, ukey, ver, tag);
    let prefix_len = range_start.len();
    let start = match from {
        Some(item) => layout::item_key(db, ukey, ver, tag, item),
        None => range_start,
    };
    let count = count.max(1);
    let mut page = txn.scan(&start, Some(&end), count + 1).await?;
    let next = if page.len() > count {
        page.pop()
            .and_then(|(k, _)| layout::item_of(&k, prefix_len))
    } else {
        None
    };
    let mut out = Vec::with_capacity(page.len());
    for (k, v) in page {
        let item = layout::item_of(&k, prefix_len)
            .ok_or_else(|| TxnError::Fatal("resp: an item key does not decode".into()))?;
        out.push((item, v));
    }
    Ok((out, next))
}

/// Deletes the whole collection: its meta key first (ruling K2-4), then its
/// version goes to the GC queue. Handlers that remove items call this
/// instead of removing them one by one when the removal would empty the
/// collection (Redis deletes an emptied collection), so the meta key is
/// still the first key written.
pub(crate) async fn remove<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<(), TxnError> {
    txn.delete(&layout::meta_key(db, ukey)).await?;
    drop_version(txn, db, ukey, meta).await
}
