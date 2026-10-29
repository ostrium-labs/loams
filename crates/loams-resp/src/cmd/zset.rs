//! Sorted-set commands (plan KV2 Task 7): `ZADD`, `ZINCRBY`, `ZREM`,
//! `ZCARD`, `ZSCORE`, `ZMSCORE`, `ZRANK`/`ZREVRANK`, `ZRANGE` (`BYSCORE`,
//! `BYLEX`, `REV`, `LIMIT`, `WITHSCORES`), `ZRANGESTORE`, `ZREVRANGE`,
//! `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZRANGEBYLEX`, `ZREVRANGEBYLEX`,
//! `ZCOUNT`, `ZLEXCOUNT`, `ZREMRANGEBYRANK`/`BYSCORE`/`BYLEX`, `ZPOPMIN`,
//! `ZPOPMAX`, `ZRANDMEMBER`, `ZUNION`/`ZINTER`/`ZDIFF` and their `STORE`
//! forms, `ZINTERCARD`, `ZSCAN`.

use std::sync::Arc;

use bytes::Bytes;
use loams_kv::{Txn, TxnError};

use super::hash::{Rng, ScanArgs, random_picks, scan_reply, scan_scope};
use super::{Spec, int_arg, syntax_error, txn, upper, wrongtype};
use crate::cursor::Resume;
use crate::frame::{Proto, Value, format_double};
use crate::layout::Meta;
use crate::server::{Conn, Shared};
use crate::types::counter::parse_float;
use crate::types::now_ms;
use crate::types::zset::{
    self, AddFlags, Added, Aggregate, Entry, LexBound, Limit, ScoreBound, ZOp,
};

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let proto = conn.proto;
    let result = match spec.name {
        "zadd" => zadd(shared, db, args).await,
        "zincrby" => zincrby(shared, db, args).await,
        "zrem" => {
            let members: Vec<Vec<u8>> = args[2..].iter().map(|a| a.to_vec()).collect();
            let key = args[1].clone();
            txn(shared, "resp.zrem", move |t| {
                let (key, members) = (key.clone(), members.clone());
                Box::pin(async move { zset::rem(t, db, &key, &members, now_ms()).await })
            })
            .await
            .and_then(read)
            .map(Value::Integer)
        }
        "zcard" => {
            with_zset(shared, db, args[1].clone(), |t, db, key, meta| {
                Box::pin(async move {
                    let n = match meta {
                        Some(m) => zset::card(t, db, &key, &m).await?,
                        None => 0,
                    };
                    Ok(Value::Integer(n))
                })
            })
            .await
        }
        "zscore" | "zmscore" => {
            let members: Vec<Vec<u8>> = args[2..].iter().map(|a| a.to_vec()).collect();
            let single = spec.name == "zscore";
            with_zset(shared, db, args[1].clone(), move |t, db, key, meta| {
                let members = members.clone();
                Box::pin(async move {
                    let mut out = Vec::with_capacity(members.len());
                    for m in &members {
                        let s = match &meta {
                            Some(meta) => zset::score(t, db, &key, meta, m).await?,
                            None => None,
                        };
                        out.push(s.map_or(Value::Null, Value::Double));
                    }
                    Ok(if single {
                        out.pop().unwrap_or(Value::Null)
                    } else {
                        Value::Array(out)
                    })
                })
            })
            .await
        }
        "zrank" | "zrevrank" => zrank(shared, db, args, spec.name == "zrevrank").await,
        "zrange" | "zrangestore" => {
            let store = spec.name == "zrangestore";
            let (src, rest) = if store {
                (&args[2], &args[3..])
            } else {
                (&args[1], &args[2..])
            };
            match parse_opts(&rest[2..], true) {
                Ok(o) => match build(
                    o.by.unwrap_or(ByKind::Rank),
                    o.rev,
                    &rest[0],
                    &rest[1],
                    o.limit,
                    o.withscores && !store,
                ) {
                    Ok(_) if o.withscores && store => Err(syntax_error()),
                    Ok(q) if store => {
                        range_store(shared, db, args[1].clone(), src.clone(), q).await
                    }
                    Ok(q) => range(shared, db, src.clone(), q, proto).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        }
        "zrevrange" | "zrangebyscore" | "zrevrangebyscore" | "zrangebylex" | "zrevrangebylex" => {
            let (by, rev) = match spec.name {
                "zrevrange" => (ByKind::Rank, true),
                "zrangebyscore" => (ByKind::Score, false),
                "zrevrangebyscore" => (ByKind::Score, true),
                "zrangebylex" => (ByKind::Lex, false),
                _ => (ByKind::Lex, true),
            };
            match parse_opts(&args[4..], false) {
                Ok(o) => match build(by, rev, &args[2], &args[3], o.limit, o.withscores) {
                    Ok(q) => range(shared, db, args[1].clone(), q, proto).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        }
        "zcount" | "zlexcount" => {
            let by = if spec.name == "zcount" {
                ByKind::Score
            } else {
                ByKind::Lex
            };
            match build(by, false, &args[2], &args[3], None, false) {
                Ok(q) => count(shared, db, args[1].clone(), q).await,
                Err(e) => Err(e),
            }
        }
        "zremrangebyrank" | "zremrangebyscore" | "zremrangebylex" => {
            let by = match spec.name {
                "zremrangebyrank" => ByKind::Rank,
                "zremrangebyscore" => ByKind::Score,
                _ => ByKind::Lex,
            };
            match build(by, false, &args[2], &args[3], None, false) {
                Ok(q) => remove_range(shared, db, args[1].clone(), q).await,
                Err(e) => Err(e),
            }
        }
        "zpopmin" | "zpopmax" => zpop(shared, db, args, spec.name == "zpopmax", proto).await,
        "zrandmember" => zrandmember(shared, conn, args).await,
        "zunion" | "zinter" | "zdiff" => match Algebra::parse(&args[1..], spec.name, false) {
            Ok(a) => algebra(shared, db, a, None, proto).await,
            Err(e) => Err(e),
        },
        "zunionstore" | "zinterstore" | "zdiffstore" => {
            match Algebra::parse(&args[2..], spec.name, true) {
                Ok(a) => algebra(shared, db, a, Some(args[1].clone()), proto).await,
                Err(e) => Err(e),
            }
        }
        "zintercard" => zintercard(shared, db, args).await,
        "zscan" => zscan(shared, conn, args).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

fn read<T>(r: zset::Read<T>) -> Result<T, Value> {
    r.map_err(|_| wrongtype())
}

/// The body run on a live sorted set (`None`: absent) in one transaction.
type ZBody<'t> = super::Body<'t, Value>;

/// Runs `body` over the sorted set at `key`; `WRONGTYPE` for another type.
async fn with_zset<F>(shared: &Shared, db: u8, key: Bytes, body: F) -> Result<Value, Value>
where
    F: for<'t> Fn(&'t mut Txn, u8, Bytes, Option<Meta>) -> ZBody<'t> + Clone + Send + 'static,
{
    let out = txn(shared, "resp.zread", move |t| {
        let (key, body) = (key.clone(), body.clone());
        Box::pin(async move {
            match zset::live(t, db, &key, now_ms()).await? {
                Err(_) => Ok(Err(())),
                Ok(meta) => Ok(Ok(body(t, db, key, meta).await?)),
            }
        })
    })
    .await?;
    out.map_err(|()| wrongtype())
}

/// Scores and members, as `WITHSCORES` replies them: pairs in RESP3, a
/// flat array in RESP2.
fn entries_reply(entries: Vec<Entry>, withscores: bool, proto: Proto) -> Value {
    if !withscores {
        return Value::Array(entries.into_iter().map(|e| Value::bulk(e.member)).collect());
    }
    if proto == Proto::Resp3 {
        return Value::Array(
            entries
                .into_iter()
                .map(|e| Value::Array(vec![Value::bulk(e.member), Value::Double(e.score)]))
                .collect(),
        );
    }
    Value::Array(
        entries
            .into_iter()
            .flat_map(|e| [Value::bulk(e.member), Value::Double(e.score)])
            .collect(),
    )
}

fn not_float() -> Value {
    Value::err("ERR value is not a valid float")
}

async fn zadd(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let mut flags = AddFlags::default();
    let mut i = 2;
    while i < args.len() {
        match upper(&args[i]).as_str() {
            "NX" => flags.nx = true,
            "XX" => flags.xx = true,
            "GT" => flags.gt = true,
            "LT" => flags.lt = true,
            "CH" => flags.ch = true,
            "INCR" => flags.incr = true,
            _ => break,
        }
        i += 1;
    }
    let rest = &args[i..];
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return Err(syntax_error());
    }
    if flags.incr && rest.len() > 2 {
        return Err(Value::err(
            "ERR INCR option supports a single increment-element pair",
        ));
    }
    if flags.nx && flags.xx {
        return Err(Value::err(
            "ERR XX and NX options at the same time are not compatible",
        ));
    }
    if (flags.gt && flags.nx) || (flags.lt && flags.nx) || (flags.gt && flags.lt) {
        return Err(Value::err(
            "ERR GT, LT, and/or NX options at the same time are not compatible",
        ));
    }
    let mut items = Vec::with_capacity(rest.len() / 2);
    for pair in rest.chunks(2) {
        let score = parse_float(&pair[0]).ok_or_else(not_float)?;
        items.push((score, pair[1].to_vec()));
    }
    add(shared, db, args[1].clone(), items, flags).await
}

async fn add(
    shared: &Shared,
    db: u8,
    key: Bytes,
    items: Vec<(f64, Vec<u8>)>,
    flags: AddFlags,
) -> Result<Value, Value> {
    let added = read(
        txn(shared, "resp.zadd", move |t| {
            let (key, items) = (key.clone(), items.clone());
            Box::pin(async move { zset::add(t, db, &key, &items, flags, now_ms()).await })
        })
        .await?,
    )?;
    match added {
        Added::Count(n) => Ok(Value::Integer(n)),
        Added::Incr(s) => Ok(s.map_or(Value::Null, Value::Double)),
        Added::NaN => Err(Value::err("ERR resulting score is not a number (NaN)")),
    }
}

async fn zincrby(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let by = parse_float(&args[2]).ok_or_else(not_float)?;
    let flags = AddFlags {
        incr: true,
        ..AddFlags::default()
    };
    add(
        shared,
        db,
        args[1].clone(),
        vec![(by, args[3].to_vec())],
        flags,
    )
    .await
}

async fn zrank(shared: &Shared, db: u8, args: &[Bytes], rev: bool) -> Result<Value, Value> {
    let withscore = match args.get(3) {
        None => false,
        Some(w) if args.len() == 4 && upper(w) == "WITHSCORE" => true,
        Some(_) => return Err(syntax_error()),
    };
    let member = args[2].to_vec();
    with_zset(shared, db, args[1].clone(), move |t, db, key, meta| {
        let member = member.clone();
        Box::pin(async move {
            let found = match meta {
                Some(m) => zset::rank(t, db, &key, &m, &member, rev).await?,
                None => None,
            };
            Ok(match (found, withscore) {
                (None, false) => Value::Null,
                (None, true) => Value::NullArray,
                (Some((r, _)), false) => Value::Integer(r),
                (Some((r, s)), true) => Value::Array(vec![Value::Integer(r), Value::Double(s)]),
            })
        })
    })
    .await
}

/// How a range selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByKind {
    Rank,
    Score,
    Lex,
}

/// A parsed range.
#[derive(Debug, Clone, PartialEq)]
enum By {
    Rank(i64, i64),
    Score(ScoreBound, ScoreBound),
    Lex(LexBound, LexBound),
}

#[derive(Debug, Clone, PartialEq)]
struct Query {
    by: By,
    rev: bool,
    limit: Limit,
    withscores: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Opts {
    by: Option<ByKind>,
    rev: bool,
    limit: Option<Limit>,
    withscores: bool,
}

/// The options after a range's bounds. `allow_by` admits `BYSCORE`,
/// `BYLEX` and `REV` (`ZRANGE`'s form).
fn parse_opts(args: &[Bytes], allow_by: bool) -> Result<Opts, Value> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        match upper(&args[i]).as_str() {
            "BYSCORE" if allow_by => o.by = Some(ByKind::Score),
            "BYLEX" if allow_by => o.by = Some(ByKind::Lex),
            "REV" if allow_by => o.rev = true,
            "WITHSCORES" => o.withscores = true,
            "LIMIT" if i + 2 < args.len() => {
                o.limit = Some(Limit {
                    offset: int_arg(&args[i + 1])?,
                    count: int_arg(&args[i + 2])?,
                });
                i += 2;
            }
            _ => return Err(syntax_error()),
        }
        i += 1;
    }
    Ok(o)
}

/// A query from its bounds as given: for a reversed score or lex range
/// they come as `max min`.
fn build(
    by: ByKind,
    rev: bool,
    a: &[u8],
    b: &[u8],
    limit: Option<Limit>,
    withscores: bool,
) -> Result<Query, Value> {
    if limit.is_some() && by == ByKind::Rank {
        return Err(Value::err(
            "ERR syntax error, LIMIT is only supported in combination with either BYSCORE or BYLEX",
        ));
    }
    if withscores && by == ByKind::Lex {
        return Err(Value::err(
            "ERR syntax error, WITHSCORES not supported in combination with BYLEX",
        ));
    }
    let (lo, hi) = if rev && by != ByKind::Rank {
        (b, a)
    } else {
        (a, b)
    };
    let by = match by {
        ByKind::Rank => By::Rank(int_arg(a)?, int_arg(b)?),
        ByKind::Score => {
            let bad = || Value::err("ERR min or max is not a float");
            By::Score(
                ScoreBound::parse(lo).ok_or_else(bad)?,
                ScoreBound::parse(hi).ok_or_else(bad)?,
            )
        }
        ByKind::Lex => {
            let bad = || Value::err("ERR min or max not valid string range item");
            By::Lex(
                LexBound::parse(lo).ok_or_else(bad)?,
                LexBound::parse(hi).ok_or_else(bad)?,
            )
        }
    };
    Ok(Query {
        by,
        rev,
        limit: limit.unwrap_or(Limit::NONE),
        withscores,
    })
}

async fn select(
    t: &mut Txn,
    db: u8,
    key: &[u8],
    meta: &Meta,
    q: &Query,
) -> Result<Vec<Entry>, TxnError> {
    match &q.by {
        By::Rank(s, e) => zset::range_by_rank(t, db, key, meta, *s, *e, q.rev).await,
        By::Score(min, max) => {
            zset::range_by_score(t, db, key, meta, *min, *max, q.rev, q.limit).await
        }
        By::Lex(min, max) => zset::range_by_lex(t, db, key, meta, min, max, q.rev, q.limit).await,
    }
}

/// The entries a query selects on `key`, in one transaction.
async fn query(shared: &Shared, db: u8, key: Bytes, q: Query) -> Result<Vec<Entry>, Value> {
    read(
        txn(shared, "resp.zrange", move |t| {
            let (key, q) = (key.clone(), q.clone());
            Box::pin(async move {
                match zset::live(t, db, &key, now_ms()).await? {
                    Err(w) => Ok(Err(w)),
                    Ok(None) => Ok(Ok(Vec::new())),
                    Ok(Some(meta)) => Ok(Ok(select(t, db, &key, &meta, &q).await?)),
                }
            })
        })
        .await?,
    )
}

async fn range(
    shared: &Shared,
    db: u8,
    key: Bytes,
    q: Query,
    proto: Proto,
) -> Result<Value, Value> {
    let withscores = q.withscores;
    let entries = query(shared, db, key, q).await?;
    Ok(entries_reply(entries, withscores, proto))
}

async fn count(shared: &Shared, db: u8, key: Bytes, q: Query) -> Result<Value, Value> {
    let entries = query(shared, db, key, q).await?;
    Ok(Value::Integer(
        i64::try_from(entries.len()).unwrap_or(i64::MAX),
    ))
}

/// `ZRANGESTORE`: the selected entries stored at `dst` (replacing any
/// type; an empty result deletes it).
async fn range_store(
    shared: &Shared,
    db: u8,
    dst: Bytes,
    src: Bytes,
    q: Query,
) -> Result<Value, Value> {
    read(
        txn(shared, "resp.zrangestore", move |t| {
            let (dst, src, q) = (dst.clone(), src.clone(), q.clone());
            Box::pin(async move {
                let now = now_ms();
                let entries = match zset::live(t, db, &src, now).await? {
                    Err(w) => return Ok(Err(w)),
                    Ok(None) => Vec::new(),
                    Ok(Some(meta)) => select(t, db, &src, &meta, &q).await?,
                };
                Ok(Ok(zset::store(t, db, &dst, &entries, now).await?))
            })
        })
        .await?,
    )
    .map(Value::Integer)
}

/// `ZREMRANGEBY*`: removes the selected entries.
async fn remove_range(shared: &Shared, db: u8, key: Bytes, q: Query) -> Result<Value, Value> {
    read(
        txn(shared, "resp.zremrange", move |t| {
            let (key, q) = (key.clone(), q.clone());
            Box::pin(async move {
                match zset::live(t, db, &key, now_ms()).await? {
                    Err(w) => Ok(Err(w)),
                    Ok(None) => Ok(Ok(0)),
                    Ok(Some(meta)) => {
                        let entries = select(t, db, &key, &meta, &q).await?;
                        Ok(Ok(zset::remove(t, db, &key, &meta, &entries).await?))
                    }
                }
            })
        })
        .await?,
    )
    .map(Value::Integer)
}

/// `ZPOPMIN`/`ZPOPMAX key [count]`.
async fn zpop(
    shared: &Shared,
    db: u8,
    args: &[Bytes],
    max: bool,
    proto: Proto,
) -> Result<Value, Value> {
    if args.len() > 3 {
        return Err(syntax_error());
    }
    let count = match args.get(2) {
        Some(c) => {
            let n = int_arg(c)?;
            if n < 0 {
                return Err(Value::err("ERR value is out of range, must be positive"));
            }
            Some(n)
        }
        None => None,
    };
    let n = count.unwrap_or(1);
    let key = args[1].clone();
    let popped = read(
        txn(shared, "resp.zpop", move |t| {
            let key = key.clone();
            Box::pin(async move {
                if n == 0 {
                    return Ok(Ok(Vec::new()));
                }
                match zset::live(t, db, &key, now_ms()).await? {
                    Err(w) => Ok(Err(w)),
                    Ok(None) => Ok(Ok(Vec::new())),
                    Ok(Some(meta)) => {
                        let entries =
                            zset::range_by_rank(t, db, &key, &meta, 0, n - 1, max).await?;
                        zset::remove(t, db, &key, &meta, &entries).await?;
                        Ok(Ok(entries))
                    }
                }
            })
        })
        .await?,
    )?;
    Ok(match count {
        // One pair, flat in either protocol.
        None => Value::Array(
            popped
                .into_iter()
                .flat_map(|e| [Value::bulk(e.member), Value::Double(e.score)])
                .collect(),
        ),
        Some(_) => entries_reply(popped, true, proto),
    })
}

/// `ZRANDMEMBER key [count [WITHSCORES]]` (reads the whole set).
async fn zrandmember(shared: &Shared, conn: &Conn, args: &[Bytes]) -> Result<Value, Value> {
    if args.len() > 4 {
        return Err(syntax_error());
    }
    let count = match args.get(2) {
        Some(c) => Some(int_arg(c)?),
        None => None,
    };
    let withscores = match args.get(3) {
        Some(w) if upper(w) == "WITHSCORES" => true,
        Some(_) => return Err(syntax_error()),
        None => false,
    };
    if let Some(c) = count
        && (c == i64::MIN
            || (withscores && c.unsigned_abs() > u64::try_from(i64::MAX / 2).unwrap_or(0)))
    {
        return Err(Value::err("ERR value is out of range"));
    }
    let q = Query {
        by: By::Rank(0, -1),
        rev: false,
        limit: Limit::NONE,
        withscores,
    };
    let all = query(shared, conn.db, args[1].clone(), q).await?;
    let mut rng = Rng::seeded(conn.id);
    let Some(count) = count else {
        if all.is_empty() {
            return Ok(Value::Null);
        }
        let i = rng.below(all.len());
        return Ok(Value::bulk(all[i].member.clone()));
    };
    let picks: Vec<Entry> = random_picks(&mut rng, all.len(), count)
        .into_iter()
        .map(|i| all[i].clone())
        .collect();
    Ok(entries_reply(picks, withscores, conn.proto))
}

/// The arguments of `ZUNION`, `ZINTER`, `ZDIFF` and their `STORE` forms,
/// from `numkeys`.
#[derive(Debug, Clone, PartialEq)]
struct Algebra {
    op: ZOp,
    keys: Vec<Vec<u8>>,
    weights: Vec<f64>,
    how: Aggregate,
    withscores: bool,
}

impl Algebra {
    fn parse(args: &[Bytes], command: &str, store: bool) -> Result<Algebra, Value> {
        let op = if command.starts_with("zunion") {
            ZOp::Union
        } else if command.starts_with("zinter") {
            ZOp::Inter
        } else {
            ZOp::Diff
        };
        let numkeys = int_arg(&args[0])?;
        if numkeys <= 0 {
            return Err(Value::err(format!(
                "ERR at least 1 input key is needed for '{command}' command"
            )));
        }
        let n = usize::try_from(numkeys).unwrap_or(usize::MAX);
        if n > args.len() - 1 {
            return Err(syntax_error());
        }
        let keys = args[1..=n].iter().map(|a| a.to_vec()).collect();
        let mut out = Algebra {
            op,
            keys,
            weights: Vec::new(),
            how: Aggregate::Sum,
            withscores: false,
        };
        let mut i = n + 1;
        while i < args.len() {
            match upper(&args[i]).as_str() {
                "WEIGHTS" if op != ZOp::Diff && i + n < args.len() => {
                    for w in &args[i + 1..=i + n] {
                        out.weights.push(
                            parse_float(w)
                                .ok_or_else(|| Value::err("ERR weight value is not a float"))?,
                        );
                    }
                    i += n;
                }
                "AGGREGATE" if op != ZOp::Diff && i + 1 < args.len() => {
                    out.how = match upper(&args[i + 1]).as_str() {
                        "SUM" => Aggregate::Sum,
                        "MIN" => Aggregate::Min,
                        "MAX" => Aggregate::Max,
                        _ => return Err(syntax_error()),
                    };
                    i += 1;
                }
                "WITHSCORES" if !store => out.withscores = true,
                _ => return Err(syntax_error()),
            }
            i += 1;
        }
        Ok(out)
    }
}

/// Reads every input (sorted sets, or sets with score 1).
async fn inputs(
    t: &mut Txn,
    db: u8,
    keys: &[Vec<u8>],
    now: u64,
) -> Result<zset::Read<Vec<Vec<Entry>>>, TxnError> {
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        match zset::read_input(t, db, k, now).await? {
            Ok(e) => out.push(e),
            Err(w) => return Ok(Err(w)),
        }
    }
    Ok(Ok(out))
}

async fn algebra(
    shared: &Shared,
    db: u8,
    a: Algebra,
    dst: Option<Bytes>,
    proto: Proto,
) -> Result<Value, Value> {
    let withscores = a.withscores;
    let storing = dst.is_some();
    let result = read(
        txn(shared, "resp.zunion", move |t| {
            let (a, dst) = (a.clone(), dst.clone());
            Box::pin(async move {
                let now = now_ms();
                let sets = match inputs(t, db, &a.keys, now).await? {
                    Ok(s) => s,
                    Err(w) => return Ok(Err(w)),
                };
                let entries = zset::combine(a.op, &sets, &a.weights, a.how);
                if let Some(dst) = dst {
                    let n = zset::store(t, db, &dst, &entries, now).await?;
                    return Ok(Ok((Vec::new(), n)));
                }
                Ok(Ok((entries, 0)))
            })
        })
        .await?,
    )?;
    Ok(if storing {
        Value::Integer(result.1)
    } else {
        entries_reply(result.0, withscores, proto)
    })
}

/// `ZINTERCARD numkeys key [key …] [LIMIT limit]`.
async fn zintercard(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let numkeys = int_arg(&args[1])?;
    if numkeys <= 0 {
        return Err(Value::err("ERR numkeys should be greater than 0"));
    }
    let n = usize::try_from(numkeys).unwrap_or(usize::MAX);
    if n > args.len() - 2 {
        return Err(Value::err(
            "ERR Number of keys can't be greater than number of args",
        ));
    }
    let mut limit = 0usize;
    let mut i = 2 + n;
    while i < args.len() {
        match (upper(&args[i]).as_str(), args.get(i + 1)) {
            ("LIMIT", Some(l)) => {
                let l = int_arg(l)?;
                if l < 0 {
                    return Err(Value::err("ERR LIMIT can't be negative"));
                }
                limit = usize::try_from(l).unwrap_or(usize::MAX);
                i += 2;
            }
            _ => return Err(syntax_error()),
        }
    }
    let a = Algebra {
        op: ZOp::Inter,
        keys: args[2..2 + n].iter().map(|k| k.to_vec()).collect(),
        weights: Vec::new(),
        how: Aggregate::Sum,
        withscores: false,
    };
    let Value::Array(items) = algebra(shared, db, a, None, Proto::Resp2).await? else {
        return Ok(Value::Integer(0));
    };
    let n = if limit == 0 {
        items.len()
    } else {
        items.len().min(limit)
    };
    Ok(Value::Integer(i64::try_from(n).unwrap_or(i64::MAX)))
}

async fn zscan(shared: &Shared, conn: &mut Conn, args: &[Bytes]) -> Result<Value, Value> {
    let scan = ScanArgs::parse(&args[2..], true)?;
    let db = conn.db;
    let key = args[1].clone();
    let scope = scan_scope("zscan", db, &key);
    let from = match conn.cursors.take(scan.cursor, &scope) {
        Resume::Start => None,
        Resume::From(item) => Some(item),
        Resume::Unknown => return Ok(scan_reply(0, Vec::new())),
    };
    let count = scan.count;
    let (page, next) = read(
        txn(shared, "resp.zscan", move |t| {
            let (key, from) = (key.clone(), from.clone());
            Box::pin(async move { zset::page(t, db, &key, from.as_deref(), count, now_ms()).await })
        })
        .await?,
    )?;
    let mut items = Vec::new();
    for e in page {
        if !scan.matches(&e.member) {
            continue;
        }
        items.push(Value::bulk(e.member));
        if !scan.novalues {
            items.push(Value::bulk(format_double(e.score)));
        }
    }
    let cursor = next.map_or(0, |item| conn.cursors.save(&scope, item));
    Ok(scan_reply(cursor, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(words: &[&str]) -> Vec<Bytes> {
        words.iter().map(|w| Bytes::from(w.to_string())).collect()
    }

    #[test]
    fn range_options() {
        let o = parse_opts(
            &b(&["BYSCORE", "REV", "LIMIT", "1", "2", "WITHSCORES"]),
            true,
        )
        .expect("valid");
        assert_eq!(o.by, Some(ByKind::Score));
        assert!(o.rev && o.withscores);
        assert_eq!(
            o.limit,
            Some(Limit {
                offset: 1,
                count: 2
            })
        );
        assert!(parse_opts(&b(&["BYSCORE"]), false).is_err());
        assert!(parse_opts(&b(&["LIMIT", "1"]), true).is_err());
        // A reversed score range takes its bounds as max, min.
        let q = build(ByKind::Score, true, b"5", b"(1", None, false).expect("valid");
        assert_eq!(
            q.by,
            By::Score(ScoreBound::Exclusive(1.0), ScoreBound::Inclusive(5.0))
        );
        assert!(build(ByKind::Rank, false, b"0", b"1", Some(Limit::NONE), false).is_err());
        assert!(build(ByKind::Lex, false, b"-", b"+", None, true).is_err());
        assert_eq!(
            build(ByKind::Score, false, b"x", b"1", None, false),
            Err(Value::err("ERR min or max is not a float"))
        );
        assert_eq!(
            build(ByKind::Lex, false, b"a", b"+", None, false),
            Err(Value::err("ERR min or max not valid string range item"))
        );
    }

    #[test]
    fn algebra_options() {
        let a = Algebra::parse(
            &b(&["2", "a", "b", "WEIGHTS", "1", "2.5", "AGGREGATE", "max"]),
            "zunionstore",
            true,
        )
        .expect("valid");
        assert_eq!(a.keys.len(), 2);
        assert_eq!(a.weights, vec![1.0, 2.5]);
        assert_eq!(a.how, Aggregate::Max);
        assert_eq!(
            Algebra::parse(&b(&["0", "a"]), "zunion", false),
            Err(Value::err(
                "ERR at least 1 input key is needed for 'zunion' command"
            ))
        );
        assert!(Algebra::parse(&b(&["3", "a", "b"]), "zinter", false).is_err());
        assert!(Algebra::parse(&b(&["1", "a", "WEIGHTS", "1"]), "zdiff", false).is_err());
        assert!(Algebra::parse(&b(&["1", "a", "WITHSCORES"]), "zinterstore", true).is_err());
        assert_eq!(
            Algebra::parse(&b(&["1", "a", "WEIGHTS", "x"]), "zinter", false),
            Err(Value::err("ERR weight value is not a float"))
        );
    }

    #[test]
    fn withscores_replies_per_protocol() {
        let e = vec![Entry {
            member: b"m".to_vec(),
            score: 1.5,
        }];
        assert_eq!(
            entries_reply(e.clone(), true, Proto::Resp2),
            Value::Array(vec![Value::bulk(&b"m"[..]), Value::Double(1.5)])
        );
        assert_eq!(
            entries_reply(e.clone(), true, Proto::Resp3),
            Value::Array(vec![Value::Array(vec![
                Value::bulk(&b"m"[..]),
                Value::Double(1.5)
            ])])
        );
        assert_eq!(
            entries_reply(e, false, Proto::Resp3),
            Value::Array(vec![Value::bulk(&b"m"[..])])
        );
    }
}
