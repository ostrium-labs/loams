//! Sorted sets (plan KV2 Task 7; design §77 §8.3–§8.4): a member is
//! `data_prefix ‖ 'm' ‖ enc(member)` → its score (`f64` bits, big-endian),
//! and the score index is `data_prefix ‖ 'z' ‖ key::f64(score) ‖
//! enc(member)` → empty, so the index sorts by score, then member, as
//! Redis orders a sorted set. The cardinality is in the 16 length shards.
//!
//! Ranges by score read only their part of the index; ranges by rank read
//! the index from the near end up to the last rank asked for, and `ZRANK`
//! counts through the index (both O(rank), documented). Ranges by lex read
//! the member keys, whose order is the members' byte order: Redis defines
//! them only for members of equal score, where that is the index order
//! too. A score of `-0` is stored as `0` (the two compare equal; Valkey
//! may print `-0`, a documented deviation). Written from Valkey's
//! documented semantics, with Tidis's `src/tikv/zset.rs` read as a
//! reference, not ported.

use loams_kv::{TxnError, key};

use super::collection::{self, open};
use super::{Kv, WrongType, delete_key, read_live};
use crate::layout::{self, Kind, Meta, SET_MEMBER, ZSET_MEMBER, ZSET_SCORE};
use crate::types::counter::parse_float;

/// A sorted-set read: the value, or `WrongType`.
pub(crate) type Read<T> = Result<T, WrongType>;

/// A member and its score.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub member: Vec<u8>,
    pub score: f64,
}

/// `-0` as `0`.
fn norm(s: f64) -> f64 {
    if s == 0.0 { 0.0 } else { s }
}

fn member_key(db: u8, ukey: &[u8], ver: u64, member: &[u8]) -> Vec<u8> {
    layout::item_key(db, ukey, ver, ZSET_MEMBER, member)
}

fn tag_base(db: u8, ukey: &[u8], ver: u64, tag: u8) -> Vec<u8> {
    let mut k = layout::data_prefix(db, ukey, ver);
    k.push(tag);
    k
}

fn index_key(db: u8, ukey: &[u8], ver: u64, score: f64, member: &[u8]) -> Vec<u8> {
    let mut k = tag_base(db, ukey, ver, ZSET_SCORE);
    key::put_f64(&mut k, score);
    key::put_bytes(&mut k, member);
    k
}

fn decode_index(k: &[u8], base_len: usize) -> Option<Entry> {
    let rest = k.get(base_len..)?;
    let (score, n) = key::get_f64(rest).ok()?;
    let (member, m) = key::get_bytes(&rest[n..]).ok()?;
    (n + m == rest.len()).then_some(Entry { member, score })
}

fn score_value(v: &[u8]) -> f64 {
    f64::from_bits(u64::from_be_bytes(v.try_into().unwrap_or([0; 8])))
}

fn bad_key() -> TxnError {
    TxnError::Fatal("resp: a sorted-set key does not decode".into())
}

/// A `ZRANGEBYSCORE`-style bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScoreBound {
    Inclusive(f64),
    Exclusive(f64),
}

impl ScoreBound {
    /// `1.5`, `(1.5`, `-inf`, `+inf`; `None` for anything else (Valkey's
    /// "min or max is not a float").
    pub fn parse(arg: &[u8]) -> Option<ScoreBound> {
        match arg.split_first() {
            Some((b'(', rest)) => parse_float(rest).map(ScoreBound::Exclusive),
            _ => parse_float(arg).map(ScoreBound::Inclusive),
        }
    }

    fn holds_min(self, s: f64) -> bool {
        match self {
            ScoreBound::Inclusive(b) => s >= b,
            ScoreBound::Exclusive(b) => s > b,
        }
    }

    fn holds_max(self, s: f64) -> bool {
        match self {
            ScoreBound::Inclusive(b) => s <= b,
            ScoreBound::Exclusive(b) => s < b,
        }
    }
}

/// A `ZRANGEBYLEX`-style bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexBound {
    /// `-`
    Min,
    /// `+`
    Max,
    Inclusive(Vec<u8>),
    Exclusive(Vec<u8>),
}

impl LexBound {
    /// `-`, `+`, `[a`, `(a`; `None` for anything else.
    pub fn parse(arg: &[u8]) -> Option<LexBound> {
        match arg.split_first() {
            Some((b'-', [])) => Some(LexBound::Min),
            Some((b'+', [])) => Some(LexBound::Max),
            Some((b'[', rest)) => Some(LexBound::Inclusive(rest.to_vec())),
            Some((b'(', rest)) => Some(LexBound::Exclusive(rest.to_vec())),
            _ => None,
        }
    }
}

fn score_lo(base: &[u8], b: ScoreBound) -> Vec<u8> {
    let mut k = base.to_vec();
    match b {
        ScoreBound::Inclusive(s) => {
            key::put_f64(&mut k, norm(s));
            k
        }
        ScoreBound::Exclusive(s) => {
            key::put_f64(&mut k, norm(s));
            key::prefix_end(&k)
        }
    }
}

fn score_hi(base: &[u8], b: ScoreBound) -> Vec<u8> {
    let mut k = base.to_vec();
    match b {
        ScoreBound::Inclusive(s) => {
            key::put_f64(&mut k, norm(s));
            key::prefix_end(&k)
        }
        ScoreBound::Exclusive(s) => {
            key::put_f64(&mut k, norm(s));
            k
        }
    }
}

fn lex_key(base: &[u8], b: &LexBound, lo: bool) -> Vec<u8> {
    let end = key::prefix_end(base);
    match b {
        LexBound::Min => base.to_vec(),
        LexBound::Max => end,
        LexBound::Inclusive(m) | LexBound::Exclusive(m) => {
            let mut k = base.to_vec();
            key::put_bytes(&mut k, m);
            // Encodings are prefix-free, so `k ‖ 0` is the first key after
            // `k`.
            let after = matches!(b, LexBound::Inclusive(_)) != lo;
            if after {
                k.push(0);
            }
            k
        }
    }
}

/// The live sorted set's meta record, or `None`.
pub(crate) async fn live<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Option<Meta>>, TxnError> {
    Ok(open(txn, db, ukey, Kind::Zset, now_ms).await?.1)
}

/// The score of `member`, if present.
pub(crate) async fn score<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    member: &[u8],
) -> Result<Option<f64>, TxnError> {
    Ok(txn
        .get(&member_key(db, ukey, meta.ver, member))
        .await?
        .map(|v| score_value(&v)))
}

/// Sets `member`'s score (its member key and index key), counting a new
/// member in its length shard.
async fn put_score<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    member: &[u8],
    old: Option<f64>,
    new: f64,
) -> Result<(), TxnError> {
    let new = norm(new);
    if let Some(old) = old {
        if old == new {
            return Ok(());
        }
        txn.delete(&index_key(db, ukey, meta.ver, old, member))
            .await?;
    }
    txn.put(
        &member_key(db, ukey, meta.ver, member),
        new.to_bits().to_be_bytes().to_vec(),
    )
    .await?;
    txn.put(&index_key(db, ukey, meta.ver, new, member), Vec::new())
        .await?;
    if old.is_none() {
        collection::len_add(txn, db, ukey, meta.ver, member, 1).await?;
    }
    Ok(())
}

/// Removes `entries` (members known present, with their scores). Removing
/// them all deletes the sorted set, meta key first.
async fn remove_entries<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    entries: &[Entry],
) -> Result<i64, TxnError> {
    let removed = i64::try_from(entries.len()).unwrap_or(i64::MAX);
    if removed == 0 {
        return Ok(0);
    }
    if collection::len(txn, db, ukey, meta.ver).await? <= removed {
        collection::remove(txn, db, ukey, meta).await?;
        return Ok(removed);
    }
    for e in entries {
        txn.delete(&member_key(db, ukey, meta.ver, &e.member))
            .await?;
        txn.delete(&index_key(db, ukey, meta.ver, e.score, &e.member))
            .await?;
        collection::len_add(txn, db, ukey, meta.ver, &e.member, -1).await?;
    }
    Ok(removed)
}

/// `ZADD`'s flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AddFlags {
    pub nx: bool,
    pub xx: bool,
    pub gt: bool,
    pub lt: bool,
    pub ch: bool,
    pub incr: bool,
}

/// What `ZADD` did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Added {
    /// Members added (with `CH`: added or changed).
    Count(i64),
    /// `INCR`: the new score, or `None` when a flag stopped it.
    Incr(Option<f64>),
    /// `INCR` would make the score NaN.
    NaN,
}

/// `ZADD` (and `ZINCRBY`, as `INCR`).
pub(crate) async fn add<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    items: &[(f64, Vec<u8>)],
    flags: AddFlags,
    now_ms: u64,
) -> Result<Read<Added>, TxnError> {
    let (found, read) = open(txn, db, ukey, Kind::Zset, now_ms).await?;
    let meta = match read {
        Err(w) => return Ok(Err(w)),
        Ok(Some(m)) => m,
        Ok(None) if flags.xx => {
            return Ok(Ok(if flags.incr {
                Added::Incr(None)
            } else {
                Added::Count(0)
            }));
        }
        Ok(None) => collection::create(txn, db, ukey, Kind::Zset, found.stored.as_ref()).await?,
    };
    let (mut added, mut changed) = (0i64, 0i64);
    let mut last = None;
    for (s, member) in items {
        let old = score(txn, db, ukey, &meta, member).await?;
        let new = if flags.incr {
            old.unwrap_or(0.0) + s
        } else {
            *s
        };
        if new.is_nan() {
            return Ok(Ok(Added::NaN));
        }
        let applies = match old {
            Some(o) => !flags.nx && (!flags.gt || new > o) && (!flags.lt || new < o),
            None => !flags.xx,
        };
        if !applies {
            continue;
        }
        if old.is_none() {
            added += 1;
        } else if old != Some(norm(new)) {
            changed += 1;
        }
        put_score(txn, db, ukey, &meta, member, old, new).await?;
        last = Some(norm(new));
    }
    Ok(Ok(if flags.incr {
        Added::Incr(last)
    } else if flags.ch {
        Added::Count(added + changed)
    } else {
        Added::Count(added)
    }))
}

/// `ZREM`: removes the members present; returns how many.
pub(crate) async fn rem<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    members: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    let meta = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(0)),
        Ok(Some(m)) => m,
    };
    let mut present: Vec<Entry> = Vec::new();
    for m in members {
        if present.iter().any(|e| &e.member == m) {
            continue;
        }
        if let Some(s) = score(txn, db, ukey, &meta, m).await? {
            present.push(Entry {
                member: m.clone(),
                score: s,
            });
        }
    }
    Ok(Ok(remove_entries(txn, db, ukey, &meta, &present).await?))
}

/// `ZCARD`.
pub(crate) async fn card<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<i64, TxnError> {
    collection::len(txn, db, ukey, meta.ver).await
}

/// Valkey's rank normalisation over `len` members: the ranks `start..=stop`
/// as a range, or `None` when it selects nothing.
pub fn rank_bounds(start: i64, stop: i64, len: i64) -> Option<(usize, usize)> {
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
    Some((usize::try_from(start).ok()?, usize::try_from(stop).ok()?))
}

/// The members of ranks `start..=stop` (Valkey's negative indexes), from
/// the lowest score, or the highest when `rev`.
pub(crate) async fn range_by_rank<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    start: i64,
    stop: i64,
    rev: bool,
) -> Result<Vec<Entry>, TxnError> {
    let len = card(txn, db, ukey, meta).await?;
    let Some((start, stop)) = rank_bounds(start, stop, len) else {
        return Ok(Vec::new());
    };
    let base = tag_base(db, ukey, meta.ver, ZSET_SCORE);
    let end = key::prefix_end(&base);
    let pairs = collection::scan_range(txn, &base, &end, rev, Some(stop + 1)).await?;
    pairs
        .into_iter()
        .skip(start)
        .map(|(k, _)| decode_index(&k, base.len()).ok_or_else(bad_key))
        .collect()
}

/// `LIMIT offset count`: a negative offset selects nothing, a negative
/// count everything after the offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub offset: i64,
    pub count: i64,
}

impl Limit {
    /// No `LIMIT`.
    pub const NONE: Limit = Limit {
        offset: 0,
        count: -1,
    };

    fn scan_limit(self) -> Option<usize> {
        if self.count < 0 {
            return None;
        }
        Some(usize::try_from(self.offset.saturating_add(self.count)).unwrap_or(usize::MAX))
    }

    fn apply<T>(self, items: Vec<T>) -> Vec<T> {
        if self.offset < 0 {
            return Vec::new();
        }
        let skip = usize::try_from(self.offset).unwrap_or(usize::MAX);
        let take = usize::try_from(self.count).unwrap_or(usize::MAX);
        items.into_iter().skip(skip).take(take).collect()
    }
}

/// The members with `min ≤ score ≤ max` (by the bounds' kinds), ascending,
/// or descending when `rev`, after `limit`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn range_by_score<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    min: ScoreBound,
    max: ScoreBound,
    rev: bool,
    limit: Limit,
) -> Result<Vec<Entry>, TxnError> {
    if limit.offset < 0 {
        return Ok(Vec::new());
    }
    let base = tag_base(db, ukey, meta.ver, ZSET_SCORE);
    let (lo, hi) = (score_lo(&base, min), score_hi(&base, max));
    let pairs = collection::scan_range(txn, &lo, &hi, rev, limit.scan_limit()).await?;
    let entries = pairs
        .into_iter()
        .map(|(k, _)| decode_index(&k, base.len()).ok_or_else(bad_key))
        .collect::<Result<Vec<_>, _>>()?;
    // The key bounds are exact for finite scores; the filter keeps
    // infinite bounds exact too.
    let entries = entries
        .into_iter()
        .filter(|e| min.holds_min(e.score) && max.holds_max(e.score))
        .collect();
    Ok(limit.apply(entries))
}

/// The members between the lex bounds, in byte order (or reversed), after
/// `limit`, with their scores.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn range_by_lex<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    min: &LexBound,
    max: &LexBound,
    rev: bool,
    limit: Limit,
) -> Result<Vec<Entry>, TxnError> {
    if limit.offset < 0 {
        return Ok(Vec::new());
    }
    let base = tag_base(db, ukey, meta.ver, ZSET_MEMBER);
    let (lo, hi) = (lex_key(&base, min, true), lex_key(&base, max, false));
    let pairs = collection::scan_range(txn, &lo, &hi, rev, limit.scan_limit()).await?;
    let entries = pairs
        .into_iter()
        .map(|(k, v)| {
            layout::item_of(&k, base.len())
                .map(|member| Entry {
                    member,
                    score: score_value(&v),
                })
                .ok_or_else(bad_key)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(limit.apply(entries))
}

/// Every member, by score.
pub(crate) async fn all<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
) -> Result<Vec<Entry>, TxnError> {
    range_by_rank(txn, db, ukey, meta, 0, -1, false).await
}

/// `ZRANK`/`ZREVRANK`: the member's rank and score, counting through the
/// index (O(rank)).
pub(crate) async fn rank<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    member: &[u8],
    rev: bool,
) -> Result<Option<(i64, f64)>, TxnError> {
    let Some(s) = score(txn, db, ukey, meta, member).await? else {
        return Ok(None);
    };
    let base = tag_base(db, ukey, meta.ver, ZSET_SCORE);
    let at = index_key(db, ukey, meta.ver, s, member);
    let (lo, hi) = if rev {
        let mut after = at;
        after.push(0);
        (after, key::prefix_end(&base))
    } else {
        (base, at)
    };
    let before = collection::scan_range(txn, &lo, &hi, false, None)
        .await?
        .len();
    Ok(Some((i64::try_from(before).unwrap_or(i64::MAX), s)))
}

/// Removes the given members (known present) and returns how many.
pub(crate) async fn remove<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    entries: &[Entry],
) -> Result<i64, TxnError> {
    remove_entries(txn, db, ukey, meta, entries).await
}

/// `ZSCAN`'s page: members and their scores from `from`, and where to
/// resume.
pub(crate) async fn page<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    from: Option<&[u8]>,
    count: usize,
    now_ms: u64,
) -> Result<Read<(Vec<Entry>, Option<Vec<u8>>)>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok((Vec::new(), None))),
        Ok(Some(m)) => {
            let (items, next) =
                collection::items_page(txn, db, ukey, m.ver, ZSET_MEMBER, from, count).await?;
            Ok(Ok((
                items
                    .into_iter()
                    .map(|(member, v)| Entry {
                        member,
                        score: score_value(&v),
                    })
                    .collect(),
                next,
            )))
        }
    }
}

/// An input of `ZUNION`/`ZINTER`/`ZDIFF`: a sorted set's members, a set's
/// members with score 1, or nothing for a missing key.
pub(crate) async fn read_input<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Vec<Entry>>, TxnError> {
    let found = read_live(txn, db, ukey, now_ms).await?;
    let Some(meta) = found.live else {
        return Ok(Ok(Vec::new()));
    };
    match meta.kind {
        Kind::Zset => Ok(Ok(all(txn, db, ukey, &meta).await?)),
        Kind::Set => Ok(Ok(collection::items(txn, db, ukey, meta.ver, SET_MEMBER)
            .await?
            .into_iter()
            .map(|(member, _)| Entry { member, score: 1.0 })
            .collect())),
        _ => Ok(Err(WrongType)),
    }
}

/// Replaces whatever is at `dst` with a sorted set of `entries` (the
/// `*STORE` forms); an empty result leaves no key. Returns the count.
pub(crate) async fn store<K: Kv>(
    txn: &mut K,
    db: u8,
    dst: &[u8],
    entries: &[Entry],
    now_ms: u64,
) -> Result<i64, TxnError> {
    let found = read_live(txn, db, dst, now_ms).await?;
    if entries.is_empty() {
        delete_key(txn, db, dst, now_ms).await?;
        return Ok(0);
    }
    let meta = collection::create(txn, db, dst, Kind::Zset, found.stored.as_ref()).await?;
    let mut n = 0;
    for e in entries {
        let old = score(txn, db, dst, &meta, &e.member).await?;
        put_score(txn, db, dst, &meta, &e.member, old, e.score).await?;
        if old.is_none() {
            n += 1;
        }
    }
    Ok(n)
}

/// How `ZUNION`/`ZINTER` combine a member's scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Sum,
    Min,
    Max,
}

/// The set operations on sorted sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZOp {
    Union,
    Inter,
    Diff,
}

fn aggregate(how: Aggregate, a: f64, b: f64) -> f64 {
    let r = match how {
        Aggregate::Sum => a + b,
        Aggregate::Min => a.min(b),
        Aggregate::Max => a.max(b),
    };
    // inf + -inf: Redis takes 0.
    if r.is_nan() { 0.0 } else { r }
}

/// `op` over `inputs` with `weights` (one per input) and `how`; the result
/// is ordered by score, then member.
pub fn combine(op: ZOp, inputs: &[Vec<Entry>], weights: &[f64], how: Aggregate) -> Vec<Entry> {
    use std::collections::BTreeMap;
    let Some((first, rest)) = inputs.split_first() else {
        return Vec::new();
    };
    let weight = |i: usize, s: f64| {
        let r = s * weights.get(i).copied().unwrap_or(1.0);
        if r.is_nan() { 0.0 } else { r }
    };
    let mut acc: BTreeMap<Vec<u8>, f64> = first
        .iter()
        .map(|e| {
            (
                e.member.clone(),
                if op == ZOp::Diff {
                    e.score
                } else {
                    weight(0, e.score)
                },
            )
        })
        .collect();
    for (i, input) in rest.iter().enumerate() {
        let i = i + 1;
        match op {
            ZOp::Union => {
                for e in input {
                    let s = weight(i, e.score);
                    acc.entry(e.member.clone())
                        .and_modify(|v| *v = aggregate(how, *v, s))
                        .or_insert(s);
                }
            }
            ZOp::Inter => {
                let other: BTreeMap<&[u8], f64> = input
                    .iter()
                    .map(|e| (e.member.as_slice(), e.score))
                    .collect();
                acc.retain(|m, _| other.contains_key(m.as_slice()));
                for (m, v) in acc.iter_mut() {
                    if let Some(s) = other.get(m.as_slice()) {
                        *v = aggregate(how, *v, weight(i, *s));
                    }
                }
            }
            ZOp::Diff => {
                for e in input {
                    acc.remove(&e.member);
                }
            }
        }
    }
    let mut out: Vec<Entry> = acc
        .into_iter()
        .map(|(member, score)| Entry {
            member,
            score: norm(score),
        })
        .collect();
    out.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.member.cmp(&b.member))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn items(pairs: &[(f64, &str)]) -> Vec<(f64, Vec<u8>)> {
        pairs
            .iter()
            .map(|(s, m)| (*s, m.as_bytes().to_vec()))
            .collect()
    }

    fn members(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| String::from_utf8_lossy(&e.member).into_owned())
            .collect()
    }

    async fn meta(txn: &mut Recording, k: &[u8]) -> Meta {
        live(txn, 0, k, 0)
            .await
            .expect("a read")
            .expect("a zset")
            .expect("present")
    }

    #[tokio::test]
    async fn zadd_flags_and_ranges() {
        let mut txn = Recording {
            ts: 4,
            ..Recording::default()
        };
        let plain = AddFlags::default();
        assert_eq!(
            add(
                &mut txn,
                0,
                b"z",
                &items(&[(1.0, "a"), (2.0, "b"), (3.0, "c")]),
                plain,
                0
            )
            .await
            .expect("zadd"),
            Ok(Added::Count(3))
        );
        let ch = AddFlags { ch: true, ..plain };
        assert_eq!(
            add(
                &mut txn,
                0,
                b"z",
                &items(&[(5.0, "a"), (2.0, "b"), (0.5, "d")]),
                ch,
                0
            )
            .await
            .expect("zadd"),
            Ok(Added::Count(2)),
            "one changed, one added"
        );
        let gt = AddFlags {
            gt: true,
            ch: true,
            ..plain
        };
        assert_eq!(
            add(&mut txn, 0, b"z", &items(&[(1.0, "a"), (9.0, "c")]), gt, 0)
                .await
                .expect("zadd"),
            Ok(Added::Count(1))
        );
        let m = meta(&mut txn, b"z").await;
        let all_entries = all(&mut txn, 0, b"z", &m).await.expect("all");
        assert_eq!(members(&all_entries), ["d", "b", "a", "c"]);
        assert_eq!(card(&mut txn, 0, b"z", &m).await.expect("zcard"), 4);
        assert_eq!(
            rank(&mut txn, 0, b"z", &m, b"a", false)
                .await
                .expect("zrank"),
            Some((2, 5.0))
        );
        assert_eq!(
            rank(&mut txn, 0, b"z", &m, b"a", true)
                .await
                .expect("zrevrank"),
            Some((1, 5.0))
        );
        assert_eq!(
            rank(&mut txn, 0, b"z", &m, b"x", false)
                .await
                .expect("zrank"),
            None
        );
        let top2 = range_by_rank(&mut txn, 0, b"z", &m, 0, 1, true)
            .await
            .expect("zrevrange");
        assert_eq!(members(&top2), ["c", "a"]);
        let mid = range_by_score(
            &mut txn,
            0,
            b"z",
            &m,
            ScoreBound::Exclusive(0.5),
            ScoreBound::Inclusive(5.0),
            false,
            Limit::NONE,
        )
        .await
        .expect("zrangebyscore");
        assert_eq!(members(&mid), ["b", "a"]);
        let inf = range_by_score(
            &mut txn,
            0,
            b"z",
            &m,
            ScoreBound::Inclusive(f64::NEG_INFINITY),
            ScoreBound::Inclusive(f64::INFINITY),
            true,
            Limit {
                offset: 1,
                count: 2,
            },
        )
        .await
        .expect("zrevrangebyscore");
        assert_eq!(members(&inf), ["a", "b"]);
        // INCR, NX on an existing member, NaN.
        let incr = AddFlags {
            incr: true,
            ..plain
        };
        assert_eq!(
            add(&mut txn, 0, b"z", &items(&[(2.5, "b")]), incr, 0)
                .await
                .expect("zincrby"),
            Ok(Added::Incr(Some(4.5)))
        );
        let incr_nx = AddFlags {
            incr: true,
            nx: true,
            ..plain
        };
        assert_eq!(
            add(&mut txn, 0, b"z", &items(&[(1.0, "b")]), incr_nx, 0)
                .await
                .expect("zadd"),
            Ok(Added::Incr(None))
        );
        add(&mut txn, 0, b"z", &items(&[(f64::INFINITY, "i")]), plain, 0)
            .await
            .expect("zadd")
            .expect("a zset");
        assert_eq!(
            add(
                &mut txn,
                0,
                b"z",
                &items(&[(f64::NEG_INFINITY, "i")]),
                incr,
                0
            )
            .await
            .expect("zadd"),
            Ok(Added::NaN)
        );
        // XX on a missing key creates nothing.
        let xx = AddFlags { xx: true, ..plain };
        assert_eq!(
            add(&mut txn, 0, b"none", &items(&[(1.0, "a")]), xx, 0)
                .await
                .expect("zadd"),
            Ok(Added::Count(0))
        );
        assert_eq!(
            crate::types::read_meta(&mut txn, 0, b"none")
                .await
                .expect("a read"),
            None
        );
    }

    #[tokio::test]
    async fn zadd_nan_refused_and_negative_zero() {
        let mut txn = Recording {
            ts: 4,
            ..Recording::default()
        };
        add(
            &mut txn,
            0,
            b"z",
            &items(&[(-0.0, "n"), (0.0, "p")]),
            AddFlags::default(),
            0,
        )
        .await
        .expect("zadd")
        .expect("a zset");
        let m = meta(&mut txn, b"z").await;
        let s = score(&mut txn, 0, b"z", &m, b"n")
            .await
            .expect("zscore")
            .expect("present");
        assert!(s == 0.0 && s.is_sign_positive(), "-0 is stored as 0");
        // Equal scores order by member.
        assert_eq!(
            members(&all(&mut txn, 0, b"z", &m).await.expect("all")),
            ["n", "p"]
        );
        assert!(ScoreBound::parse(b"nan").is_none());
        assert!(ScoreBound::parse(b"(nan").is_none());
        assert_eq!(
            ScoreBound::parse(b"-inf"),
            Some(ScoreBound::Inclusive(f64::NEG_INFINITY))
        );
        assert_eq!(ScoreBound::parse(b"(2"), Some(ScoreBound::Exclusive(2.0)));
    }

    #[tokio::test]
    async fn zrange_bylex_bounds() {
        let mut txn = Recording {
            ts: 4,
            ..Recording::default()
        };
        let all_zero: Vec<(f64, Vec<u8>)> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|m| (0.0, m.as_bytes().to_vec()))
            .collect();
        add(&mut txn, 0, b"z", &all_zero, AddFlags::default(), 0)
            .await
            .expect("zadd")
            .expect("a zset");
        let m = meta(&mut txn, b"z").await;
        let lex = |s: &str| LexBound::parse(s.as_bytes()).expect("a bound");
        for (min, max, rev, want) in [
            ("-", "+", false, vec!["a", "b", "c", "d", "e"]),
            ("[b", "[d", false, vec!["b", "c", "d"]),
            ("(b", "(d", false, vec!["c"]),
            ("[d", "[b", false, vec![]),
            ("+", "-", false, vec![]),
            ("[b", "+", true, vec!["e", "d", "c", "b"]),
            ("-", "(c", true, vec!["b", "a"]),
            ("[aa", "[c", false, vec!["b", "c"]),
        ] {
            let got = range_by_lex(
                &mut txn,
                0,
                b"z",
                &m,
                &lex(min),
                &lex(max),
                rev,
                Limit::NONE,
            )
            .await
            .expect("zrangebylex");
            assert_eq!(members(&got), want, "{min} {max} rev={rev}");
        }
        assert!(LexBound::parse(b"b").is_none());
        assert!(LexBound::parse(b"").is_none());
        let limited = range_by_lex(
            &mut txn,
            0,
            b"z",
            &m,
            &lex("-"),
            &lex("+"),
            false,
            Limit {
                offset: 1,
                count: 2,
            },
        )
        .await
        .expect("zrangebylex");
        assert_eq!(members(&limited), ["b", "c"]);
    }

    #[tokio::test]
    async fn removing_everything_deletes_the_key() {
        let mut txn = Recording {
            ts: 4,
            ..Recording::default()
        };
        add(
            &mut txn,
            0,
            b"z",
            &items(&[(1.0, "a"), (2.0, "b")]),
            AddFlags::default(),
            0,
        )
        .await
        .expect("zadd")
        .expect("a zset");
        assert_eq!(
            rem(&mut txn, 0, b"z", &[b"a".to_vec(), b"x".to_vec()], 0)
                .await
                .expect("zrem"),
            Ok(1)
        );
        txn.writes.clear();
        assert_eq!(
            rem(&mut txn, 0, b"z", &[b"b".to_vec()], 0)
                .await
                .expect("zrem"),
            Ok(1)
        );
        assert_eq!(txn.writes.first(), Some(&layout::meta_key(0, b"z")));
        assert_eq!(
            crate::types::read_meta(&mut txn, 0, b"z")
                .await
                .expect("a read"),
            None
        );
    }

    #[test]
    fn rank_bounds_follow_valkey() {
        assert_eq!(rank_bounds(0, -1, 5), Some((0, 4)));
        assert_eq!(rank_bounds(-2, -1, 5), Some((3, 4)));
        assert_eq!(rank_bounds(3, 100, 5), Some((3, 4)));
        assert_eq!(rank_bounds(5, 10, 5), None);
        assert_eq!(rank_bounds(3, 1, 5), None);
        assert_eq!(rank_bounds(-100, 0, 5), Some((0, 0)));
        assert_eq!(rank_bounds(0, -1, 0), None);
    }

    #[test]
    fn combine_weights_and_aggregates() {
        let a = vec![
            Entry {
                member: b"x".to_vec(),
                score: 1.0,
            },
            Entry {
                member: b"y".to_vec(),
                score: 2.0,
            },
        ];
        let b = vec![
            Entry {
                member: b"y".to_vec(),
                score: 10.0,
            },
            Entry {
                member: b"z".to_vec(),
                score: 3.0,
            },
        ];
        let inputs = [a, b];
        let union = combine(ZOp::Union, &inputs, &[1.0, 2.0], Aggregate::Sum);
        assert_eq!(members(&union), ["x", "z", "y"]);
        assert_eq!(union[2].score, 22.0);
        let inter_max = combine(ZOp::Inter, &inputs, &[], Aggregate::Max);
        assert_eq!(
            inter_max,
            vec![Entry {
                member: b"y".to_vec(),
                score: 10.0
            }]
        );
        let diff = combine(ZOp::Diff, &inputs, &[], Aggregate::Sum);
        assert_eq!(members(&diff), ["x"]);
        let infs = [
            vec![Entry {
                member: b"m".to_vec(),
                score: f64::INFINITY,
            }],
            vec![Entry {
                member: b"m".to_vec(),
                score: f64::NEG_INFINITY,
            }],
        ];
        assert_eq!(
            combine(ZOp::Union, &infs, &[], Aggregate::Sum)[0].score,
            0.0
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// ZADD, ZREM and the range reads agree with a model: a map of
        /// member to score, ordered by (score, member).
        #[test]
        fn zset_ranges_match_model(
            ops in proptest::collection::vec((any::<bool>(), 0u8..10, -5i8..5), 1..40),
            lo in -6i8..6,
            width in 0i8..8,
            start in -12i64..12,
            stop in -12i64..12,
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
            rt.block_on(async {
                let mut txn = Recording { ts: 1, ..Recording::default() };
                let mut model: BTreeMap<Vec<u8>, f64> = BTreeMap::new();
                for (adding, m, s) in ops {
                    let member = vec![b'a' + m];
                    if adding {
                        add(&mut txn, 0, b"z", &[(f64::from(s), member.clone())], AddFlags::default(), 0)
                            .await
                            .expect("zadd")
                            .expect("a zset");
                        model.insert(member, f64::from(s));
                    } else {
                        rem(&mut txn, 0, b"z", std::slice::from_ref(&member), 0).await.expect("zrem").expect("a zset");
                        model.remove(&member);
                    }
                }
                let mut ordered: Vec<(f64, Vec<u8>)> =
                    model.iter().map(|(m, s)| (*s, m.clone())).collect();
                ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                let Some(m) = live(&mut txn, 0, b"z", 0).await.expect("a read").expect("a zset") else {
                    prop_assert!(model.is_empty());
                    return Ok(());
                };
                let len = i64::try_from(ordered.len()).unwrap_or(0);
                prop_assert_eq!(card(&mut txn, 0, b"z", &m).await.expect("zcard"), len);
                // By rank, both directions.
                for rev in [false, true] {
                    let mut seq = ordered.clone();
                    if rev {
                        seq.reverse();
                    }
                    let want: Vec<Vec<u8>> = match rank_bounds(start, stop, len) {
                        Some((s, e)) => seq[s..=e].iter().map(|(_, m)| m.clone()).collect(),
                        None => vec![],
                    };
                    let got: Vec<Vec<u8>> = range_by_rank(&mut txn, 0, b"z", &m, start, stop, rev)
                        .await
                        .expect("a range")
                        .into_iter()
                        .map(|e| e.member)
                        .collect();
                    prop_assert_eq!(got, want);
                }
                // By score, inclusive bounds.
                let (min, max) = (f64::from(lo), f64::from(lo) + f64::from(width));
                let want: Vec<Vec<u8>> = ordered
                    .iter()
                    .filter(|(s, _)| *s >= min && *s <= max)
                    .map(|(_, m)| m.clone())
                    .collect();
                let got: Vec<Vec<u8>> = range_by_score(
                    &mut txn, 0, b"z", &m,
                    ScoreBound::Inclusive(min), ScoreBound::Inclusive(max), false, Limit::NONE,
                )
                .await
                .expect("a range")
                .into_iter()
                .map(|e| e.member)
                .collect();
                prop_assert_eq!(got, want);
                // Ranks.
                for (i, (_, member)) in ordered.iter().enumerate() {
                    let r = rank(&mut txn, 0, b"z", &m, member, false).await.expect("zrank");
                    prop_assert_eq!(r.map(|(r, _)| r), Some(i64::try_from(i).unwrap_or(0)));
                }
                Ok(())
            })?;
        }
    }
}
