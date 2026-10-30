//! Lists (plan KV2 Task 8; design §77 §8.3, Tidis's integer indexes): an
//! element is `data_prefix ‖ 'l' ‖ key::u64(index)` → its bytes, and the
//! meta record's trailer holds `head` and `tail` (big-endian `u64`s), so
//! the elements are the indexes `head..tail`. A new list starts at `2^63`
//! and grows both ways. Every write changes `head` or `tail`, so it writes
//! the meta key first (ruling K2-4), and the length needs no shards.
//!
//! `LINSERT` and `LREM` move every element after the change; they refuse
//! lists longer than `list_max_shift` (default 10,000). Written from
//! Valkey's documented semantics, with Tidis's `src/tikv/list.rs` read as
//! a reference, not ported.

use loams_kv::{TxnError, key};

use super::collection::{self, open};
use super::{Kv, WrongType, drop_version, put_meta};
use crate::layout::{self, Kind, LIST_ELEM, Meta};

/// A list read: the value, or `WrongType`.
pub(crate) type Read<T> = Result<T, WrongType>;

/// Where a new list's indexes start.
pub const MID: u64 = 1 << 63;

/// A list's element indexes, `head..tail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub head: u64,
    pub tail: u64,
}

impl Bounds {
    pub fn len(self) -> u64 {
        self.tail - self.head
    }

    pub fn is_empty(self) -> bool {
        self.tail == self.head
    }

    fn encode(self) -> Vec<u8> {
        let mut t = self.head.to_be_bytes().to_vec();
        t.extend_from_slice(&self.tail.to_be_bytes());
        t
    }

    fn decode(trailer: &[u8]) -> Option<Bounds> {
        let head = u64::from_be_bytes(trailer.get(..8)?.try_into().ok()?);
        let tail = u64::from_be_bytes(trailer.get(8..16)?.try_into().ok()?);
        (head <= tail).then_some(Bounds { head, tail })
    }
}

/// Why a list command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListError {
    WrongType,
    NoSuchKey,
    OutOfRange,
    /// Longer than `list_max_shift` for `LINSERT`/`LREM`.
    TooLong,
}

impl From<WrongType> for ListError {
    fn from(_: WrongType) -> Self {
        ListError::WrongType
    }
}

fn elem_key(db: u8, ukey: &[u8], ver: u64, idx: u64) -> Vec<u8> {
    let mut k = layout::data_prefix(db, ukey, ver);
    k.push(LIST_ELEM);
    key::put_u64(&mut k, idx);
    k
}

fn bad_meta() -> TxnError {
    TxnError::Fatal("resp: a list meta record without bounds".into())
}

/// A live list: its meta record and bounds.
#[derive(Debug, Clone)]
pub(crate) struct List {
    pub meta: Meta,
    pub b: Bounds,
}

impl List {
    fn len_i64(&self) -> i64 {
        i64::try_from(self.b.len()).unwrap_or(i64::MAX)
    }

    /// Writes the meta record with bounds `b` (the first write of every
    /// list change).
    async fn set_bounds<K: Kv>(
        &mut self,
        txn: &mut K,
        db: u8,
        ukey: &[u8],
        b: Bounds,
    ) -> Result<(), TxnError> {
        self.b = b;
        self.meta.trailer = b.encode();
        put_meta(txn, db, ukey, &self.meta).await
    }

    async fn read<K: Kv>(
        &self,
        txn: &mut K,
        db: u8,
        ukey: &[u8],
        from: u64,
        to: u64,
    ) -> Result<Vec<Vec<u8>>, TxnError> {
        let lo = elem_key(db, ukey, self.meta.ver, from);
        let hi = elem_key(db, ukey, self.meta.ver, to);
        Ok(collection::scan_range(txn, &lo, &hi, false, None)
            .await?
            .into_iter()
            .map(|(_, v)| v)
            .collect())
    }

    /// Every element, head to tail.
    async fn all<K: Kv>(&self, txn: &mut K, db: u8, ukey: &[u8]) -> Result<Vec<Vec<u8>>, TxnError> {
        self.read(txn, db, ukey, self.b.head, self.b.tail).await
    }
}

/// The live list at `ukey`, or `None`.
pub(crate) async fn live<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Option<List>>, TxnError> {
    match open(txn, db, ukey, Kind::List, now_ms).await?.1 {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok(None)),
        Ok(Some(meta)) => {
            let b = Bounds::decode(&meta.trailer).ok_or_else(bad_meta)?;
            Ok(Ok(Some(List { meta, b })))
        }
    }
}

/// `LPUSH`/`RPUSH` (and `*PUSHX` with `only_existing`): returns the new
/// length, or 0 when `only_existing` and the key is absent.
pub(crate) async fn push<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    values: &[Vec<u8>],
    left: bool,
    only_existing: bool,
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    let (found, read) = open(txn, db, ukey, Kind::List, now_ms).await?;
    let (mut list, replaced) = match read {
        Err(w) => return Ok(Err(w)),
        Ok(Some(meta)) => {
            let b = Bounds::decode(&meta.trailer).ok_or_else(bad_meta)?;
            (List { meta, b }, None)
        }
        Ok(None) if only_existing => return Ok(Ok(0)),
        Ok(None) => {
            let b = Bounds {
                head: MID,
                tail: MID,
            };
            let meta = Meta {
                kind: Kind::List,
                ver: layout::next_version(txn.start_ts(), found.stored.as_ref().map(|m| m.ver)),
                expire_at_ms: 0,
                trailer: b.encode(),
            };
            (List { meta, b }, found.stored)
        }
    };
    let n = u64::try_from(values.len()).unwrap_or(u64::MAX);
    let old = list.b;
    let new = if left {
        Bounds {
            head: old.head - n,
            tail: old.tail,
        }
    } else {
        Bounds {
            head: old.head,
            tail: old.tail + n,
        }
    };
    list.set_bounds(txn, db, ukey, new).await?;
    if let Some(stored) = replaced {
        drop_version(txn, db, ukey, &stored).await?;
    }
    for (i, v) in values.iter().enumerate() {
        let i = u64::try_from(i).unwrap_or(u64::MAX);
        let idx = if left { old.head - 1 - i } else { old.tail + i };
        txn.put(&elem_key(db, ukey, list.meta.ver, idx), v.clone())
            .await?;
    }
    Ok(Ok(list.len_i64()))
}

/// `LPOP`/`RPOP`: removes up to `count` elements from one end and returns
/// them in pop order; `None` when the key is absent.
pub(crate) async fn pop<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    left: bool,
    count: u64,
    now_ms: u64,
) -> Result<Read<Option<Vec<Vec<u8>>>>, TxnError> {
    let mut list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(None)),
        Ok(Some(l)) => l,
    };
    let n = count.min(list.b.len());
    if n == 0 || list.b.is_empty() {
        return Ok(Ok(Some(Vec::new())));
    }
    let b = list.b;
    let (from, to) = if left {
        (b.head, b.head + n)
    } else {
        (b.tail - n, b.tail)
    };
    let mut out = list.read(txn, db, ukey, from, to).await?;
    if !left {
        out.reverse();
    }
    if n == b.len() {
        collection::remove(txn, db, ukey, &list.meta).await?;
        return Ok(Ok(Some(out)));
    }
    let new = if left {
        Bounds {
            head: to,
            tail: b.tail,
        }
    } else {
        Bounds {
            head: b.head,
            tail: from,
        }
    };
    list.set_bounds(txn, db, ukey, new).await?;
    for idx in from..to {
        txn.delete(&elem_key(db, ukey, list.meta.ver, idx)).await?;
    }
    Ok(Ok(Some(out)))
}

/// `LLEN`.
pub(crate) async fn len<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    Ok(live(txn, db, ukey, now_ms)
        .await?
        .map(|l| l.map_or(0, |l| l.len_i64())))
}

/// An index of a list of `len` (negative from the tail), or `None` out of
/// range.
fn at(i: i64, len: i64) -> Option<u64> {
    let i = if i < 0 { i + len } else { i };
    if i < 0 || i >= len {
        return None;
    }
    u64::try_from(i).ok()
}

/// `LINDEX`.
pub(crate) async fn index<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    i: i64,
    now_ms: u64,
) -> Result<Read<Option<Vec<u8>>>, TxnError> {
    let list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(None)),
        Ok(Some(l)) => l,
    };
    let Some(i) = at(i, list.len_i64()) else {
        return Ok(Ok(None));
    };
    Ok(Ok(txn
        .get(&elem_key(db, ukey, list.meta.ver, list.b.head + i))
        .await?))
}

/// Valkey's `LRANGE`/`LTRIM` bounds: the kept range `start..=stop`, or
/// `None` when it is empty.
pub fn range_bounds(start: i64, stop: i64, len: i64) -> Option<(u64, u64)> {
    let mut start = if start < 0 { start + len } else { start };
    let mut stop = if stop < 0 { stop + len } else { stop };
    if start < 0 {
        start = 0;
    }
    if start > stop || start >= len {
        return None;
    }
    if stop >= len {
        stop = len - 1;
    }
    Some((u64::try_from(start).ok()?, u64::try_from(stop).ok()?))
}

/// `LRANGE`.
pub(crate) async fn range<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    start: i64,
    stop: i64,
    now_ms: u64,
) -> Result<Read<Vec<Vec<u8>>>, TxnError> {
    let list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(Vec::new())),
        Ok(Some(l)) => l,
    };
    let Some((s, e)) = range_bounds(start, stop, list.len_i64()) else {
        return Ok(Ok(Vec::new()));
    };
    let h = list.b.head;
    Ok(Ok(list.read(txn, db, ukey, h + s, h + e + 1).await?))
}

/// `LSET`.
pub(crate) async fn set<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    i: i64,
    value: &[u8],
    now_ms: u64,
) -> Result<Result<(), ListError>, TxnError> {
    let list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w.into())),
        Ok(None) => return Ok(Err(ListError::NoSuchKey)),
        Ok(Some(l)) => l,
    };
    let Some(i) = at(i, list.len_i64()) else {
        return Ok(Err(ListError::OutOfRange));
    };
    txn.put(
        &elem_key(db, ukey, list.meta.ver, list.b.head + i),
        value.to_vec(),
    )
    .await?;
    Ok(Ok(()))
}

/// `LTRIM`: keeps `start..=stop`; an empty result deletes the key.
pub(crate) async fn trim<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    start: i64,
    stop: i64,
    now_ms: u64,
) -> Result<Read<()>, TxnError> {
    let mut list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(())),
        Ok(Some(l)) => l,
    };
    let b = list.b;
    let Some((s, e)) = range_bounds(start, stop, list.len_i64()) else {
        collection::remove(txn, db, ukey, &list.meta).await?;
        return Ok(Ok(()));
    };
    let new = Bounds {
        head: b.head + s,
        tail: b.head + e + 1,
    };
    if new == b {
        return Ok(Ok(()));
    }
    list.set_bounds(txn, db, ukey, new).await?;
    for idx in (b.head..new.head).chain(new.tail..b.tail) {
        txn.delete(&elem_key(db, ukey, list.meta.ver, idx)).await?;
    }
    Ok(Ok(()))
}

/// Rewrites the list as `elems` from position `from` on (positions before
/// `from` are unchanged), deleting what falls off the end: the meta key
/// first.
async fn rewrite_from<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    list: &mut List,
    elems: &[Vec<u8>],
    from: usize,
) -> Result<(), TxnError> {
    let old = list.b;
    let n = u64::try_from(elems.len()).unwrap_or(u64::MAX);
    let new = Bounds {
        head: old.head,
        tail: old.head + n,
    };
    list.set_bounds(txn, db, ukey, new).await?;
    for (j, v) in elems.iter().enumerate().skip(from) {
        let j = u64::try_from(j).unwrap_or(u64::MAX);
        txn.put(&elem_key(db, ukey, list.meta.ver, old.head + j), v.clone())
            .await?;
    }
    for idx in new.tail..old.tail {
        txn.delete(&elem_key(db, ukey, list.meta.ver, idx)).await?;
    }
    Ok(())
}

/// `LINSERT key BEFORE|AFTER pivot value`: the new length, -1 when the
/// pivot is absent, 0 when the key is.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    before: bool,
    pivot: &[u8],
    value: &[u8],
    max_shift: usize,
    now_ms: u64,
) -> Result<Result<i64, ListError>, TxnError> {
    let mut list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w.into())),
        Ok(None) => return Ok(Ok(0)),
        Ok(Some(l)) => l,
    };
    if list.b.len() > u64::try_from(max_shift).unwrap_or(u64::MAX) {
        return Ok(Err(ListError::TooLong));
    }
    let mut elems = list.all(txn, db, ukey).await?;
    let Some(p) = elems.iter().position(|e| e == pivot) else {
        return Ok(Ok(-1));
    };
    let q = if before { p } else { p + 1 };
    elems.insert(q, value.to_vec());
    rewrite_from(txn, db, ukey, &mut list, &elems, q).await?;
    Ok(Ok(list.len_i64()))
}

/// `LREM key count value`: removes up to `|count|` matches (all for 0),
/// from the head for a positive count, from the tail for a negative one;
/// returns how many.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn remove<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    count: i64,
    value: &[u8],
    max_shift: usize,
    now_ms: u64,
) -> Result<Result<i64, ListError>, TxnError> {
    let mut list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w.into())),
        Ok(None) => return Ok(Ok(0)),
        Ok(Some(l)) => l,
    };
    if list.b.len() > u64::try_from(max_shift).unwrap_or(u64::MAX) {
        return Ok(Err(ListError::TooLong));
    }
    let elems = list.all(txn, db, ukey).await?;
    let limit = if count == 0 {
        usize::MAX
    } else {
        usize::try_from(count.unsigned_abs()).unwrap_or(usize::MAX)
    };
    let mut drop = vec![false; elems.len()];
    let mut removed = 0usize;
    let order: Box<dyn Iterator<Item = usize>> = if count < 0 {
        Box::new((0..elems.len()).rev())
    } else {
        Box::new(0..elems.len())
    };
    for i in order {
        if removed == limit {
            break;
        }
        if elems[i] == value {
            drop[i] = true;
            removed += 1;
        }
    }
    if removed == 0 {
        return Ok(Ok(0));
    }
    if removed == elems.len() {
        collection::remove(txn, db, ukey, &list.meta).await?;
        return Ok(Ok(i64::try_from(removed).unwrap_or(i64::MAX)));
    }
    let first = drop.iter().position(|d| *d).unwrap_or(0);
    let kept: Vec<Vec<u8>> = elems
        .into_iter()
        .zip(drop)
        .filter(|(_, d)| !d)
        .map(|(e, _)| e)
        .collect();
    rewrite_from(txn, db, ukey, &mut list, &kept, first).await?;
    Ok(Ok(i64::try_from(removed).unwrap_or(i64::MAX)))
}

/// `LPOS`'s options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PosOptions {
    /// Which match to start from: 1 the first, -1 the last.
    pub rank: i64,
    /// How many matches (0: all); `None` replies one index, not an array.
    pub count: Option<u64>,
    /// How many elements to look at (0: all).
    pub maxlen: u64,
}

/// The indexes `LPOS` finds in `elems`.
pub fn positions(elems: &[Vec<u8>], value: &[u8], o: PosOptions) -> Vec<i64> {
    let len = elems.len();
    let want = match o.count {
        None => 1,
        Some(0) => usize::MAX,
        Some(n) => usize::try_from(n).unwrap_or(usize::MAX),
    };
    let look = if o.maxlen == 0 {
        len
    } else {
        usize::try_from(o.maxlen).unwrap_or(usize::MAX).min(len)
    };
    let mut skip = usize::try_from(o.rank.unsigned_abs().saturating_sub(1)).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for step in 0..look {
        let i = if o.rank < 0 { len - 1 - step } else { step };
        if elems[i] != value {
            continue;
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        out.push(i64::try_from(i).unwrap_or(i64::MAX));
        if out.len() == want {
            break;
        }
    }
    out
}

/// `LPOS`: every element read, then [`positions`].
pub(crate) async fn pos<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    value: &[u8],
    o: PosOptions,
    now_ms: u64,
) -> Result<Read<Vec<i64>>, TxnError> {
    let list = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(Vec::new())),
        Ok(Some(l)) => l,
    };
    let elems = list.all(txn, db, ukey).await?;
    Ok(Ok(positions(&elems, value, o)))
}

/// `LMOVE src dst LEFT|RIGHT LEFT|RIGHT` (and `RPOPLPUSH`): pops one
/// element from `src` and pushes it onto `dst`; `None` when `src` is
/// absent. `src` is type-checked, then (when it exists) `dst`.
pub(crate) async fn move_one<K: Kv>(
    txn: &mut K,
    db: u8,
    src: &[u8],
    dst: &[u8],
    from_left: bool,
    to_left: bool,
    now_ms: u64,
) -> Result<Read<Option<Vec<u8>>>, TxnError> {
    match live(txn, db, src, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(None)),
        Ok(Some(_)) => {}
    }
    if let Err(w) = live(txn, db, dst, now_ms).await? {
        return Ok(Err(w));
    }
    let popped = match pop(txn, db, src, from_left, 1, now_ms).await? {
        Ok(Some(mut v)) if !v.is_empty() => v.remove(0),
        Ok(_) => return Ok(Ok(None)),
        Err(w) => return Ok(Err(w)),
    };
    match push(
        txn,
        db,
        dst,
        std::slice::from_ref(&popped),
        to_left,
        false,
        now_ms,
    )
    .await?
    {
        Ok(_) => Ok(Ok(Some(popped))),
        Err(w) => Ok(Err(w)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;
    use proptest::prelude::*;
    use std::collections::VecDeque;

    fn v(items: &[&str]) -> Vec<Vec<u8>> {
        items.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    async fn whole(txn: &mut Recording, k: &[u8]) -> Vec<Vec<u8>> {
        range(txn, 0, k, 0, -1, 0)
            .await
            .expect("lrange")
            .expect("a list")
    }

    #[tokio::test]
    async fn push_pop_and_ranges() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        assert_eq!(
            push(&mut txn, 0, b"l", &v(&["a", "b", "c"]), true, false, 0)
                .await
                .expect("lpush"),
            Ok(3)
        );
        assert_eq!(
            push(&mut txn, 0, b"l", &v(&["d"]), false, false, 0)
                .await
                .expect("rpush"),
            Ok(4)
        );
        assert_eq!(whole(&mut txn, b"l").await, v(&["c", "b", "a", "d"]));
        assert_eq!(
            push(&mut txn, 0, b"none", &v(&["x"]), true, true, 0)
                .await
                .expect("lpushx"),
            Ok(0)
        );
        assert_eq!(
            index(&mut txn, 0, b"l", -1, 0).await.expect("lindex"),
            Ok(Some(b"d".to_vec()))
        );
        assert_eq!(
            index(&mut txn, 0, b"l", 9, 0).await.expect("lindex"),
            Ok(None)
        );
        assert_eq!(
            pop(&mut txn, 0, b"l", false, 2, 0).await.expect("rpop"),
            Ok(Some(v(&["d", "a"])))
        );
        assert_eq!(
            pop(&mut txn, 0, b"l", true, 1, 0).await.expect("lpop"),
            Ok(Some(v(&["c"])))
        );
        assert_eq!(len(&mut txn, 0, b"l", 0).await.expect("llen"), Ok(1));
        txn.writes.clear();
        assert_eq!(
            pop(&mut txn, 0, b"l", true, 5, 0).await.expect("lpop"),
            Ok(Some(v(&["b"])))
        );
        assert_eq!(
            txn.writes.first(),
            Some(&layout::meta_key(0, b"l")),
            "meta first"
        );
        assert_eq!(
            crate::types::read_meta(&mut txn, 0, b"l")
                .await
                .expect("a read"),
            None
        );
        assert_eq!(
            pop(&mut txn, 0, b"l", true, 1, 0).await.expect("lpop"),
            Ok(None)
        );
    }

    #[tokio::test]
    async fn set_trim_insert_remove() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        push(
            &mut txn,
            0,
            b"l",
            &v(&["a", "b", "c", "b", "e"]),
            false,
            false,
            0,
        )
        .await
        .expect("rpush")
        .expect("a list");
        assert_eq!(
            set(&mut txn, 0, b"l", 1, b"B", 0).await.expect("lset"),
            Ok(())
        );
        assert_eq!(
            set(&mut txn, 0, b"l", 9, b"x", 0).await.expect("lset"),
            Err(ListError::OutOfRange)
        );
        assert_eq!(
            set(&mut txn, 0, b"none", 0, b"x", 0).await.expect("lset"),
            Err(ListError::NoSuchKey)
        );
        assert_eq!(
            insert(&mut txn, 0, b"l", true, b"c", b"X", 100, 0)
                .await
                .expect("linsert"),
            Ok(6)
        );
        assert_eq!(
            whole(&mut txn, b"l").await,
            v(&["a", "B", "X", "c", "b", "e"])
        );
        assert_eq!(
            insert(&mut txn, 0, b"l", false, b"zz", b"Y", 100, 0)
                .await
                .expect("linsert"),
            Ok(-1)
        );
        assert_eq!(
            remove(&mut txn, 0, b"l", -1, b"b", 100, 0)
                .await
                .expect("lrem"),
            Ok(1)
        );
        assert_eq!(whole(&mut txn, b"l").await, v(&["a", "B", "X", "c", "e"]));
        assert_eq!(
            trim(&mut txn, 0, b"l", 1, -2, 0).await.expect("ltrim"),
            Ok(())
        );
        assert_eq!(whole(&mut txn, b"l").await, v(&["B", "X", "c"]));
        assert_eq!(
            trim(&mut txn, 0, b"l", 5, 10, 0).await.expect("ltrim"),
            Ok(())
        );
        assert_eq!(
            crate::types::read_meta(&mut txn, 0, b"l")
                .await
                .expect("a read"),
            None
        );
    }

    #[tokio::test]
    async fn linsert_shift_limit() {
        let mut txn = Recording {
            ts: 3,
            ..Recording::default()
        };
        let items: Vec<Vec<u8>> = (0..20).map(|i| format!("e{i}").into_bytes()).collect();
        push(&mut txn, 0, b"l", &items, false, false, 0)
            .await
            .expect("rpush")
            .expect("a list");
        assert_eq!(
            insert(&mut txn, 0, b"l", true, b"e5", b"x", 10, 0)
                .await
                .expect("linsert"),
            Err(ListError::TooLong)
        );
        assert_eq!(
            remove(&mut txn, 0, b"l", 0, b"e5", 10, 0)
                .await
                .expect("lrem"),
            Err(ListError::TooLong)
        );
        assert_eq!(
            insert(&mut txn, 0, b"l", true, b"e5", b"x", 20, 0)
                .await
                .expect("linsert"),
            Ok(21)
        );
    }

    #[test]
    fn lpos_positions() {
        let elems = v(&["a", "b", "c", "b", "b", "d"]);
        let o = |rank, count, maxlen| PosOptions {
            rank,
            count,
            maxlen,
        };
        assert_eq!(positions(&elems, b"b", o(1, None, 0)), vec![1]);
        assert_eq!(positions(&elems, b"b", o(2, None, 0)), vec![3]);
        assert_eq!(positions(&elems, b"b", o(-1, None, 0)), vec![4]);
        assert_eq!(positions(&elems, b"b", o(1, Some(0), 0)), vec![1, 3, 4]);
        assert_eq!(positions(&elems, b"b", o(-2, Some(2), 0)), vec![3, 1]);
        assert_eq!(positions(&elems, b"b", o(1, Some(0), 2)), vec![1]);
        assert_eq!(positions(&elems, b"z", o(1, None, 0)), Vec::<i64>::new());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Pushes, pops, sets, trims, inserts and removes agree with a
        /// `VecDeque` model.
        #[test]
        fn list_ops_match_model(
            ops in proptest::collection::vec((0u8..7, 0u8..5, -6i64..6, -6i64..6), 1..40),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
            rt.block_on(async {
                let mut txn = Recording { ts: 1, ..Recording::default() };
                let mut model: VecDeque<Vec<u8>> = VecDeque::new();
                for (op, val, a, b) in ops {
                    let value = vec![b'a' + val];
                    let n = i64::try_from(model.len()).unwrap_or(0);
                    match op {
                        0 => {
                            push(&mut txn, 0, b"l", std::slice::from_ref(&value), true, false, 0).await.expect("lpush").expect("a list");
                            model.push_front(value);
                        }
                        1 => {
                            push(&mut txn, 0, b"l", std::slice::from_ref(&value), false, false, 0).await.expect("rpush").expect("a list");
                            model.push_back(value);
                        }
                        2 => {
                            let k = a.unsigned_abs() % 3 + 1;
                            let got = pop(&mut txn, 0, b"l", b >= 0, k, 0).await.expect("pop").expect("a list");
                            let mut want = Vec::new();
                            for _ in 0..k {
                                let x = if b >= 0 { model.pop_front() } else { model.pop_back() };
                                if let Some(x) = x { want.push(x); }
                            }
                            if n == 0 {
                                prop_assert_eq!(got, None);
                            } else {
                                prop_assert_eq!(got, Some(want));
                            }
                        }
                        3 => {
                            let got = set(&mut txn, 0, b"l", a, &value, 0).await.expect("lset");
                            match at(a, n) {
                                Some(i) if n > 0 => {
                                    prop_assert_eq!(got, Ok(()));
                                    model[usize::try_from(i).unwrap_or(0)] = value;
                                }
                                _ if n == 0 => prop_assert_eq!(got, Err(ListError::NoSuchKey)),
                                _ => prop_assert_eq!(got, Err(ListError::OutOfRange)),
                            }
                        }
                        4 => {
                            trim(&mut txn, 0, b"l", a, b, 0).await.expect("ltrim").expect("a list");
                            model = match range_bounds(a, b, n) {
                                Some((s, e)) => model
                                    .iter()
                                    .skip(usize::try_from(s).unwrap_or(0))
                                    .take(usize::try_from(e - s + 1).unwrap_or(0))
                                    .cloned()
                                    .collect(),
                                None => VecDeque::new(),
                            };
                        }
                        5 => {
                            let pivot = vec![b'a' + u8::try_from(a.unsigned_abs() % 5).unwrap_or(0)];
                            let got = insert(&mut txn, 0, b"l", b >= 0, &pivot, &value, 1000, 0).await.expect("linsert");
                            let want = if n == 0 {
                                0
                            } else if let Some(p) = model.iter().position(|e| *e == pivot) {
                                model.insert(if b >= 0 { p } else { p + 1 }, value);
                                n + 1
                            } else {
                                -1
                            };
                            prop_assert_eq!(got, Ok(want));
                        }
                        _ => {
                            let got = remove(&mut txn, 0, b"l", a, &value, 1000, 0).await.expect("lrem");
                            let limit = if a == 0 { usize::MAX } else { usize::try_from(a.unsigned_abs()).unwrap_or(0) };
                            let idxs: Vec<usize> = if a < 0 {
                                (0..model.len()).rev().filter(|i| model[*i] == value).take(limit).collect()
                            } else {
                                (0..model.len()).filter(|i| model[*i] == value).take(limit).collect()
                            };
                            let mut kept = VecDeque::new();
                            for (i, e) in model.iter().enumerate() {
                                if !idxs.contains(&i) { kept.push_back(e.clone()); }
                            }
                            model = kept;
                            prop_assert_eq!(got, Ok(i64::try_from(idxs.len()).unwrap_or(0)));
                        }
                    }
                    let stored = range(&mut txn, 0, b"l", 0, -1, 0).await.expect("lrange").expect("a list");
                    prop_assert_eq!(stored, model.iter().cloned().collect::<Vec<_>>());
                    let l = len(&mut txn, 0, b"l", 0).await.expect("llen").expect("a list");
                    prop_assert_eq!(l, i64::try_from(model.len()).unwrap_or(0));
                }
                Ok(())
            })?;
        }
    }
}
