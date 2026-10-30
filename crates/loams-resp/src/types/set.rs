//! Sets (plan KV2 Task 6; design §77 §8.3–§8.4): a member is
//! `data_prefix ‖ 's' ‖ enc(member)` → empty, the cardinality is kept in
//! the 16 length shards (Q1226). Written from Valkey's documented
//! semantics, with Tidis's `src/tikv/set.rs` read as a reference, not
//! ported.

use std::collections::BTreeSet;

use loams_kv::TxnError;

use super::collection::{self, open, open_or_create};
use super::{Kv, WrongType, delete_key, read_live};
use crate::layout::{self, Kind, Meta, SET_MEMBER};

/// A set read: the value, or `WrongType`.
pub(crate) type Read<T> = Result<T, WrongType>;

fn member_key(db: u8, ukey: &[u8], meta: &Meta, member: &[u8]) -> Vec<u8> {
    layout::item_key(db, ukey, meta.ver, SET_MEMBER, member)
}

/// The live set's meta record, or `None`.
pub(crate) async fn live<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Option<Meta>>, TxnError> {
    Ok(open(txn, db, ukey, Kind::Set, now_ms).await?.1)
}

/// `SADD`: adds the members and returns how many were new.
pub(crate) async fn add<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    members: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<i64>, TxnError> {
    let meta = match open_or_create(txn, db, ukey, Kind::Set, now_ms).await? {
        Ok(m) => m,
        Err(w) => return Ok(Err(w)),
    };
    add_to(txn, db, ukey, &meta, members).await.map(Ok)
}

async fn add_to<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    members: &[Vec<u8>],
) -> Result<i64, TxnError> {
    let mut added = 0;
    for m in members {
        let key = member_key(db, ukey, meta, m);
        if txn.get(&key).await?.is_some() {
            continue;
        }
        txn.put(&key, Vec::new()).await?;
        collection::len_add(txn, db, ukey, meta.ver, m, 1).await?;
        added += 1;
    }
    Ok(added)
}

/// `SREM`: removes the members that are present and returns how many.
/// Removing the last members deletes the set (meta key first).
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
    rem_from(txn, db, ukey, &meta, members).await.map(Ok)
}

async fn rem_from<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    meta: &Meta,
    members: &[Vec<u8>],
) -> Result<i64, TxnError> {
    let mut present: Vec<&Vec<u8>> = Vec::new();
    for m in members {
        if present.contains(&m) {
            continue;
        }
        if txn.get(&member_key(db, ukey, meta, m)).await?.is_some() {
            present.push(m);
        }
    }
    let removed = i64::try_from(present.len()).unwrap_or(i64::MAX);
    if removed == 0 {
        return Ok(0);
    }
    if collection::len(txn, db, ukey, meta.ver).await? <= removed {
        collection::remove(txn, db, ukey, meta).await?;
        return Ok(removed);
    }
    for m in present {
        txn.delete(&member_key(db, ukey, meta, m)).await?;
        collection::len_add(txn, db, ukey, meta.ver, m, -1).await?;
    }
    Ok(removed)
}

/// `SMEMBERS`: every member, in byte order; empty for a missing key.
pub(crate) async fn members<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    now_ms: u64,
) -> Result<Read<Vec<Vec<u8>>>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok(Vec::new())),
        Ok(Some(m)) => Ok(Ok(collection::items(txn, db, ukey, m.ver, SET_MEMBER)
            .await?
            .into_iter()
            .map(|(member, _)| member)
            .collect())),
    }
}

/// `SMISMEMBER` (and `SISMEMBER`): whether each member is present.
pub(crate) async fn contains<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    members: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<Vec<bool>>, TxnError> {
    let meta = match live(txn, db, ukey, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(None) => return Ok(Ok(vec![false; members.len()])),
        Ok(Some(m)) => m,
    };
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        out.push(txn.get(&member_key(db, ukey, &meta, m)).await?.is_some());
    }
    Ok(Ok(out))
}

/// `SCARD`.
pub(crate) async fn card<K: Kv>(
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

/// `SSCAN`'s page: up to `count` members from `from`, and where to resume.
pub(crate) async fn page<K: Kv>(
    txn: &mut K,
    db: u8,
    ukey: &[u8],
    from: Option<&[u8]>,
    count: usize,
    now_ms: u64,
) -> Result<Read<(Vec<Vec<u8>>, Option<Vec<u8>>)>, TxnError> {
    match live(txn, db, ukey, now_ms).await? {
        Err(w) => Ok(Err(w)),
        Ok(None) => Ok(Ok((Vec::new(), None))),
        Ok(Some(m)) => {
            let (items, next) =
                collection::items_page(txn, db, ukey, m.ver, SET_MEMBER, from, count).await?;
            Ok(Ok((items.into_iter().map(|(k, _)| k).collect(), next)))
        }
    }
}

/// `SMOVE`: moves `member` from `src` to `dst`. Both keys are type-checked
/// first; returns whether `member` was in `src`.
pub(crate) async fn move_member<K: Kv>(
    txn: &mut K,
    db: u8,
    src: &[u8],
    dst: &[u8],
    member: &[u8],
    now_ms: u64,
) -> Result<Read<bool>, TxnError> {
    let src_meta = match live(txn, db, src, now_ms).await? {
        Err(w) => return Ok(Err(w)),
        Ok(m) => m,
    };
    if let Err(w) = live(txn, db, dst, now_ms).await? {
        return Ok(Err(w));
    }
    let Some(src_meta) = src_meta else {
        return Ok(Ok(false));
    };
    if txn
        .get(&member_key(db, src, &src_meta, member))
        .await?
        .is_none()
    {
        return Ok(Ok(false));
    }
    if src == dst {
        return Ok(Ok(true));
    }
    let member = vec![member.to_vec()];
    rem_from(txn, db, src, &src_meta, &member).await?;
    match add(txn, db, dst, &member, now_ms).await? {
        Ok(_) => Ok(Ok(true)),
        Err(w) => Ok(Err(w)),
    }
}

/// Replaces whatever is at `dst` with a set of `members` (the `*STORE`
/// commands): the old key is deleted (any type), and an empty result
/// leaves no key. Returns the new cardinality.
pub(crate) async fn store<K: Kv>(
    txn: &mut K,
    db: u8,
    dst: &[u8],
    members: &BTreeSet<Vec<u8>>,
    now_ms: u64,
) -> Result<i64, TxnError> {
    let found = read_live(txn, db, dst, now_ms).await?;
    if members.is_empty() {
        delete_key(txn, db, dst, now_ms).await?;
        return Ok(0);
    }
    let meta = collection::create(txn, db, dst, Kind::Set, found.stored.as_ref()).await?;
    let members: Vec<Vec<u8>> = members.iter().cloned().collect();
    add_to(txn, db, dst, &meta, &members).await
}

/// The set operations `SINTER`, `SUNION` and `SDIFF` compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Inter,
    Union,
    Diff,
}

/// `op` over `sets`, in order (for `Diff`: the first minus the others).
/// A missing key is the empty set.
pub fn combine(op: Op, sets: &[BTreeSet<Vec<u8>>]) -> BTreeSet<Vec<u8>> {
    let Some((first, rest)) = sets.split_first() else {
        return BTreeSet::new();
    };
    let mut out = first.clone();
    for s in rest {
        match op {
            Op::Inter => out.retain(|m| s.contains(m)),
            Op::Union => out.extend(s.iter().cloned()),
            Op::Diff => out.retain(|m| !s.contains(m)),
        }
        if out.is_empty() && op != Op::Union {
            break;
        }
    }
    out
}

/// Reads every key's members (a missing key: the empty set); `WrongType`
/// when any key holds another type.
pub(crate) async fn read_all<K: Kv>(
    txn: &mut K,
    db: u8,
    keys: &[Vec<u8>],
    now_ms: u64,
) -> Result<Read<Vec<BTreeSet<Vec<u8>>>>, TxnError> {
    let mut sets = Vec::with_capacity(keys.len());
    for k in keys {
        match members(txn, db, k, now_ms).await? {
            Ok(m) => sets.push(m.into_iter().collect()),
            Err(w) => return Ok(Err(w)),
        }
    }
    Ok(Ok(sets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::recording::Recording;
    use crate::types::{read_meta, string};
    use proptest::prelude::*;

    fn v(items: &[&str]) -> Vec<Vec<u8>> {
        items.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[tokio::test]
    async fn add_rem_and_card() {
        let mut txn = Recording {
            ts: 2,
            ..Recording::default()
        };
        assert_eq!(
            add(&mut txn, 0, b"s", &v(&["a", "b", "a"]), 0)
                .await
                .expect("sadd"),
            Ok(2)
        );
        assert_eq!(
            add(&mut txn, 0, b"s", &v(&["b", "c"]), 0)
                .await
                .expect("sadd"),
            Ok(1)
        );
        assert_eq!(card(&mut txn, 0, b"s", 0).await.expect("scard"), Ok(3));
        assert_eq!(
            contains(&mut txn, 0, b"s", &v(&["a", "z"]), 0)
                .await
                .expect("smismember"),
            Ok(vec![true, false])
        );
        assert_eq!(
            rem(&mut txn, 0, b"s", &v(&["a", "a", "z"]), 0)
                .await
                .expect("srem"),
            Ok(1)
        );
        assert_eq!(
            members(&mut txn, 0, b"s", 0).await.expect("smembers"),
            Ok(v(&["b", "c"]))
        );
        txn.writes.clear();
        assert_eq!(
            rem(&mut txn, 0, b"s", &v(&["b", "c"]), 0)
                .await
                .expect("srem"),
            Ok(2)
        );
        assert_eq!(
            txn.writes.first(),
            Some(&layout::meta_key(0, b"s")),
            "meta first"
        );
        assert_eq!(read_meta(&mut txn, 0, b"s").await.expect("a read"), None);
    }

    #[tokio::test]
    async fn smove_and_store() {
        let mut txn = Recording {
            ts: 2,
            ..Recording::default()
        };
        add(&mut txn, 0, b"a", &v(&["x", "y"]), 0)
            .await
            .expect("sadd")
            .expect("a set");
        string::write(&mut txn, 0, b"str", b"v", 0, None)
            .await
            .expect("a write");
        assert_eq!(
            move_member(&mut txn, 0, b"a", b"str", b"x", 0)
                .await
                .expect("smove"),
            Err(WrongType),
            "the destination is type-checked"
        );
        assert_eq!(
            move_member(&mut txn, 0, b"a", b"b", b"zz", 0)
                .await
                .expect("smove"),
            Ok(false)
        );
        assert_eq!(
            move_member(&mut txn, 0, b"a", b"b", b"x", 0)
                .await
                .expect("smove"),
            Ok(true)
        );
        assert_eq!(
            move_member(&mut txn, 0, b"a", b"a", b"y", 0)
                .await
                .expect("smove"),
            Ok(true)
        );
        assert_eq!(
            members(&mut txn, 0, b"a", 0).await.expect("smembers"),
            Ok(v(&["y"]))
        );
        assert_eq!(
            members(&mut txn, 0, b"b", 0).await.expect("smembers"),
            Ok(v(&["x"]))
        );
        // A store over a string replaces it; an empty result deletes it.
        let result: BTreeSet<Vec<u8>> = v(&["p", "q"]).into_iter().collect();
        assert_eq!(
            store(&mut txn, 0, b"str", &result, 0).await.expect("store"),
            2
        );
        assert_eq!(card(&mut txn, 0, b"str", 0).await.expect("scard"), Ok(2));
        assert_eq!(
            store(&mut txn, 0, b"str", &BTreeSet::new(), 0)
                .await
                .expect("store"),
            0
        );
        assert_eq!(read_meta(&mut txn, 0, b"str").await.expect("a read"), None);
    }

    /// Emptying a set deletes it and queues its version for GC; creating it
    /// again in the same transaction (one start timestamp) must take a new
    /// version, or the new set would share data keys and length shards with
    /// the dead one and lose them to its GC (the model test's minimal case).
    #[tokio::test]
    async fn recreated_in_one_transaction_gets_a_new_version() {
        let mut txn = Recording {
            ts: 1,
            ..Recording::default()
        };
        assert_eq!(
            add(&mut txn, 0, b"k", &v(&["9"]), 0).await.expect("sadd"),
            Ok(1)
        );
        assert_eq!(
            rem(&mut txn, 0, b"k", &v(&["9"]), 0).await.expect("srem"),
            Ok(1)
        );
        assert_eq!(
            add(&mut txn, 0, b"k", &v(&["0"]), 0).await.expect("sadd"),
            Ok(1)
        );
        assert_eq!(card(&mut txn, 0, b"k", 0).await.expect("scard"), Ok(1));
        assert_eq!(
            members(&mut txn, 0, b"k", 0).await.expect("smembers"),
            Ok(v(&["0"]))
        );
        let meta = read_meta(&mut txn, 0, b"k")
            .await
            .expect("a read")
            .expect("a set");
        assert_eq!(meta.ver, 2, "version 1 waits for GC");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// SADD and SREM through the store, and the set algebra, agree
        /// with a `BTreeSet` model.
        #[test]
        fn set_algebra_matches_model(
            ops in proptest::collection::vec(
                (0usize..3, any::<bool>(), proptest::collection::vec(0u8..12, 1..4)),
                1..40,
            ),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a runtime");
            rt.block_on(async {
                let mut txn = Recording { ts: 1, ..Recording::default() };
                let mut model: Vec<BTreeSet<Vec<u8>>> = vec![BTreeSet::new(); 3];
                let keys = [b"k0".to_vec(), b"k1".to_vec(), b"k2".to_vec()];
                for (which, adding, items) in ops {
                    let items: Vec<Vec<u8>> = items.into_iter().map(|b| vec![b]).collect();
                    let key = &keys[which];
                    if adding {
                        let want = items
                            .iter()
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .filter(|m| !model[which].contains(*m))
                            .count();
                        let got = add(&mut txn, 0, key, &items, 0).await.expect("sadd");
                        prop_assert_eq!(got, Ok(i64::try_from(want).unwrap_or(0)));
                        model[which].extend(items);
                    } else {
                        let want = items
                            .iter()
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .filter(|m| model[which].contains(*m))
                            .count();
                        let got = rem(&mut txn, 0, key, &items, 0).await.expect("srem");
                        prop_assert_eq!(got, Ok(i64::try_from(want).unwrap_or(0)));
                        for m in &items {
                            model[which].remove(m);
                        }
                    }
                    let stored = read_all(&mut txn, 0, &keys, 0).await.expect("read").expect("sets");
                    prop_assert_eq!(&stored, &model);
                    let card_got = card(&mut txn, 0, key, 0).await.expect("scard");
                    prop_assert_eq!(card_got, Ok(i64::try_from(model[which].len()).unwrap_or(0)));
                }
                for op in [Op::Inter, Op::Union, Op::Diff] {
                    let want: BTreeSet<Vec<u8>> = match op {
                        Op::Inter => model[0].iter().filter(|m| model[1].contains(*m) && model[2].contains(*m)).cloned().collect(),
                        Op::Union => model.iter().flatten().cloned().collect(),
                        Op::Diff => model[0].iter().filter(|m| !model[1].contains(*m) && !model[2].contains(*m)).cloned().collect(),
                    };
                    prop_assert_eq!(combine(op, &model), want);
                }
                Ok(())
            })?;
        }
    }
}
