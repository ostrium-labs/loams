//! Set commands (plan KV2 Task 6): `SADD`, `SREM`, `SMEMBERS`,
//! `SISMEMBER`, `SMISMEMBER`, `SCARD`, `SPOP`, `SRANDMEMBER`, `SMOVE`,
//! `SINTER`, `SINTERCARD`, `SINTERSTORE`, `SUNION`, `SUNIONSTORE`, `SDIFF`,
//! `SDIFFSTORE`, `SSCAN`.

use std::sync::Arc;

use bytes::Bytes;

use super::hash::{Rng, ScanArgs, random_picks, scan_reply, scan_scope};
use super::{Spec, int_arg, syntax_error, txn, wrongtype};
use crate::cursor::Resume;
use crate::frame::Value;
use crate::server::{Conn, Shared};
use crate::types::now_ms;
use crate::types::set::{self, Op};

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let key = args[1].clone();
    let rest: Vec<Vec<u8>> = args[2..].iter().map(|a| a.to_vec()).collect();
    let result = match spec.name {
        "sadd" => sadd(shared, db, key, rest).await,
        "srem" => srem(shared, db, key, rest).await,
        "smembers" => members(shared, db, key)
            .await
            .map(|m| Value::Set(m.into_iter().map(Value::bulk).collect())),
        "sismember" => contains(shared, db, key, rest)
            .await
            .map(|v| Value::Integer(i64::from(v.first().copied().unwrap_or(false)))),
        "smismember" => contains(shared, db, key, rest).await.map(|v| {
            Value::Array(
                v.into_iter()
                    .map(|b| Value::Integer(i64::from(b)))
                    .collect(),
            )
        }),
        "scard" => scard(shared, db, key).await,
        "spop" => spop(shared, conn, key, args).await,
        "srandmember" => srandmember(shared, conn, key, args).await,
        "smove" => smove(shared, db, args).await,
        "sinter" | "sunion" | "sdiff" => {
            let keys = args[1..].iter().map(|a| a.to_vec()).collect();
            algebra(shared, db, op_of(spec.name), keys)
                .await
                .map(|m| Value::Set(m.into_iter().map(Value::bulk).collect()))
        }
        "sinterstore" | "sunionstore" | "sdiffstore" => {
            let keys = args[2..].iter().map(|a| a.to_vec()).collect();
            store(shared, db, op_of(spec.name), key, keys).await
        }
        "sintercard" => sintercard(shared, db, args).await,
        "sscan" => sscan(shared, conn, key, args).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

fn op_of(name: &str) -> Op {
    if name.starts_with("sinter") {
        Op::Inter
    } else if name.starts_with("sunion") {
        Op::Union
    } else {
        Op::Diff
    }
}

fn read<T>(r: set::Read<T>) -> Result<T, Value> {
    r.map_err(|_| wrongtype())
}

async fn sadd(shared: &Shared, db: u8, key: Bytes, members: Vec<Vec<u8>>) -> Result<Value, Value> {
    read(
        txn(shared, "resp.sadd", move |t| {
            let (key, members) = (key.clone(), members.clone());
            Box::pin(async move { set::add(t, db, &key, &members, now_ms()).await })
        })
        .await?,
    )
    .map(Value::Integer)
}

async fn srem(shared: &Shared, db: u8, key: Bytes, members: Vec<Vec<u8>>) -> Result<Value, Value> {
    read(
        txn(shared, "resp.srem", move |t| {
            let (key, members) = (key.clone(), members.clone());
            Box::pin(async move { set::rem(t, db, &key, &members, now_ms()).await })
        })
        .await?,
    )
    .map(Value::Integer)
}

async fn members(shared: &Shared, db: u8, key: Bytes) -> Result<Vec<Vec<u8>>, Value> {
    read(
        txn(shared, "resp.smembers", move |t| {
            let key = key.clone();
            Box::pin(async move { set::members(t, db, &key, now_ms()).await })
        })
        .await?,
    )
}

async fn contains(
    shared: &Shared,
    db: u8,
    key: Bytes,
    members: Vec<Vec<u8>>,
) -> Result<Vec<bool>, Value> {
    read(
        txn(shared, "resp.sismember", move |t| {
            let (key, members) = (key.clone(), members.clone());
            Box::pin(async move { set::contains(t, db, &key, &members, now_ms()).await })
        })
        .await?,
    )
}

async fn scard(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    read(
        txn(shared, "resp.scard", move |t| {
            let key = key.clone();
            Box::pin(async move { set::card(t, db, &key, now_ms()).await })
        })
        .await?,
    )
    .map(Value::Integer)
}

/// `SPOP key [count]`: removes and returns random members.
async fn spop(shared: &Shared, conn: &Conn, key: Bytes, args: &[Bytes]) -> Result<Value, Value> {
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
    let db = conn.db;
    let mut rng = Rng::seeded(conn.id);
    let popped = read(
        txn(shared, "resp.spop", move |t| {
            let key = key.clone();
            let seed = rng.next_u64();
            Box::pin(async move {
                let now = now_ms();
                let all = match set::members(t, db, &key, now).await? {
                    Ok(m) => m,
                    Err(w) => return Ok(Err(w)),
                };
                let mut rng = Rng::seeded(seed);
                let picks = random_picks(&mut rng, all.len(), count.unwrap_or(1));
                let chosen: Vec<Vec<u8>> = picks.into_iter().map(|i| all[i].clone()).collect();
                match set::rem(t, db, &key, &chosen, now).await? {
                    Ok(_) => Ok(Ok(chosen)),
                    Err(w) => Ok(Err(w)),
                }
            })
        })
        .await?,
    )?;
    Ok(match count {
        None => popped.into_iter().next().map_or(Value::Null, Value::bulk),
        Some(_) => Value::Set(popped.into_iter().map(Value::bulk).collect()),
    })
}

/// `SRANDMEMBER key [count]`: random members, distinct for a positive
/// count, with repetition for a negative one.
async fn srandmember(
    shared: &Shared,
    conn: &Conn,
    key: Bytes,
    args: &[Bytes],
) -> Result<Value, Value> {
    if args.len() > 3 {
        return Err(syntax_error());
    }
    let count = match args.get(2) {
        Some(c) => {
            let n = int_arg(c)?;
            if n == i64::MIN {
                return Err(Value::err("ERR value is out of range"));
            }
            Some(n)
        }
        None => None,
    };
    let all = members(shared, conn.db, key).await?;
    let mut rng = Rng::seeded(conn.id);
    let Some(count) = count else {
        if all.is_empty() {
            return Ok(Value::Null);
        }
        let i = rng.below(all.len());
        return Ok(Value::bulk(all[i].clone()));
    };
    let picks = random_picks(&mut rng, all.len(), count);
    Ok(Value::Array(
        picks
            .into_iter()
            .map(|i| Value::bulk(all[i].clone()))
            .collect(),
    ))
}

async fn smove(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let (src, dst, member) = (args[1].clone(), args[2].clone(), args[3].clone());
    read(
        txn(shared, "resp.smove", move |t| {
            let (src, dst, member) = (src.clone(), dst.clone(), member.clone());
            Box::pin(async move { set::move_member(t, db, &src, &dst, &member, now_ms()).await })
        })
        .await?,
    )
    .map(|moved| Value::Integer(i64::from(moved)))
}

async fn algebra(
    shared: &Shared,
    db: u8,
    op: Op,
    keys: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, Value> {
    let sets = read(
        txn(shared, "resp.sinter", move |t| {
            let keys = keys.clone();
            Box::pin(async move { set::read_all(t, db, &keys, now_ms()).await })
        })
        .await?,
    )?;
    Ok(set::combine(op, &sets).into_iter().collect())
}

/// The `*STORE` forms: computed and stored in one transaction.
async fn store(
    shared: &Shared,
    db: u8,
    op: Op,
    dst: Bytes,
    keys: Vec<Vec<u8>>,
) -> Result<Value, Value> {
    read(
        txn(shared, "resp.sinterstore", move |t| {
            let (dst, keys) = (dst.clone(), keys.clone());
            Box::pin(async move {
                let now = now_ms();
                let sets = match set::read_all(t, db, &keys, now).await? {
                    Ok(s) => s,
                    Err(w) => return Ok(Err(w)),
                };
                let result = set::combine(op, &sets);
                Ok(Ok(set::store(t, db, &dst, &result, now).await?))
            })
        })
        .await?,
    )
    .map(Value::Integer)
}

/// `SINTERCARD numkeys key [key …] [LIMIT limit]`.
async fn sintercard(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
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
    let keys: Vec<Vec<u8>> = args[2..2 + n].iter().map(|a| a.to_vec()).collect();
    let mut limit = 0usize;
    let mut i = 2 + n;
    while i < args.len() {
        match (super::upper(&args[i]).as_str(), args.get(i + 1)) {
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
    let inter = algebra(shared, db, Op::Inter, keys).await?;
    let n = if limit == 0 {
        inter.len()
    } else {
        inter.len().min(limit)
    };
    Ok(Value::Integer(i64::try_from(n).unwrap_or(i64::MAX)))
}

async fn sscan(
    shared: &Shared,
    conn: &mut Conn,
    key: Bytes,
    args: &[Bytes],
) -> Result<Value, Value> {
    let scan = ScanArgs::parse(&args[2..], false)?;
    let db = conn.db;
    let scope = scan_scope("sscan", db, &key);
    let from = match conn.cursors.take(scan.cursor, &scope) {
        Resume::Start => None,
        Resume::From(item) => Some(item),
        Resume::Unknown => return Ok(scan_reply(0, Vec::new())),
    };
    let count = scan.count;
    let (page, next) = read(
        txn(shared, "resp.sscan", move |t| {
            let (key, from) = (key.clone(), from.clone());
            Box::pin(async move { set::page(t, db, &key, from.as_deref(), count, now_ms()).await })
        })
        .await?,
    )?;
    let items = page
        .into_iter()
        .filter(|m| scan.matches(m))
        .map(Value::bulk)
        .collect();
    let cursor = next.map_or(0, |item| conn.cursors.save(&scope, item));
    Ok(scan_reply(cursor, items))
}
