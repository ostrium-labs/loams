//! List commands (plan KV2 Task 8): `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`,
//! `LPOP`, `RPOP` (with count), `LLEN`, `LINDEX`, `LRANGE`, `LSET`,
//! `LTRIM`, `LINSERT`, `LREM`, `LPOS`, `LMOVE`, `RPOPLPUSH`. The blocking
//! forms are out of v1 (design §77 §8.5).

use std::sync::Arc;

use bytes::Bytes;

use super::{Spec, int_arg, syntax_error, txn, upper, wrongtype};
use crate::frame::Value;
use crate::server::{Conn, Shared};
use crate::types::list::{self, ListError, PosOptions};
use crate::types::now_ms;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let key = args[1].clone();
    let max_shift = shared.config.list_max_shift;
    let result = match spec.name {
        "lpush" | "rpush" | "lpushx" | "rpushx" => {
            let left = spec.name.starts_with('l');
            let only_existing = spec.name.ends_with('x');
            let values: Vec<Vec<u8>> = args[2..].iter().map(|a| a.to_vec()).collect();
            read(
                txn(shared, "resp.push", move |t| {
                    let (key, values) = (key.clone(), values.clone());
                    Box::pin(async move {
                        list::push(t, db, &key, &values, left, only_existing, now_ms()).await
                    })
                })
                .await,
            )
            .map(Value::Integer)
        }
        "lpop" | "rpop" => pop(shared, db, key, args, spec.name == "lpop").await,
        "llen" => read(
            txn(shared, "resp.llen", move |t| {
                let key = key.clone();
                Box::pin(async move { list::len(t, db, &key, now_ms()).await })
            })
            .await,
        )
        .map(Value::Integer),
        "lindex" => match int_arg(&args[2]) {
            Ok(i) => read(
                txn(shared, "resp.lindex", move |t| {
                    let key = key.clone();
                    Box::pin(async move { list::index(t, db, &key, i, now_ms()).await })
                })
                .await,
            )
            .map(|v| v.map_or(Value::Null, Value::bulk)),
            Err(e) => Err(e),
        },
        "lrange" => match (int_arg(&args[2]), int_arg(&args[3])) {
            (Ok(s), Ok(e)) => read(
                txn(shared, "resp.lrange", move |t| {
                    let key = key.clone();
                    Box::pin(async move { list::range(t, db, &key, s, e, now_ms()).await })
                })
                .await,
            )
            .map(|items| Value::Array(items.into_iter().map(Value::bulk).collect())),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "lset" => match int_arg(&args[2]) {
            Ok(i) => {
                let value = args[3].clone();
                list_result(
                    txn(shared, "resp.lset", move |t| {
                        let (key, value) = (key.clone(), value.clone());
                        Box::pin(async move { list::set(t, db, &key, i, &value, now_ms()).await })
                    })
                    .await,
                    "lset",
                    max_shift,
                )
                .map(|()| Value::ok())
            }
            Err(e) => Err(e),
        },
        "ltrim" => match (int_arg(&args[2]), int_arg(&args[3])) {
            (Ok(s), Ok(e)) => read(
                txn(shared, "resp.ltrim", move |t| {
                    let key = key.clone();
                    Box::pin(async move { list::trim(t, db, &key, s, e, now_ms()).await })
                })
                .await,
            )
            .map(|()| Value::ok()),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "linsert" => {
            let before = match upper(&args[2]).as_str() {
                "BEFORE" => Some(true),
                "AFTER" => Some(false),
                _ => None,
            };
            match before {
                None => Err(syntax_error()),
                Some(before) => {
                    let (pivot, value) = (args[3].clone(), args[4].clone());
                    list_result(
                        txn(shared, "resp.linsert", move |t| {
                            let (key, pivot, value) = (key.clone(), pivot.clone(), value.clone());
                            Box::pin(async move {
                                list::insert(
                                    t,
                                    db,
                                    &key,
                                    before,
                                    &pivot,
                                    &value,
                                    max_shift,
                                    now_ms(),
                                )
                                .await
                            })
                        })
                        .await,
                        "linsert",
                        max_shift,
                    )
                    .map(Value::Integer)
                }
            }
        }
        "lrem" => match int_arg(&args[2]) {
            Ok(count) => {
                let value = args[3].clone();
                list_result(
                    txn(shared, "resp.lrem", move |t| {
                        let (key, value) = (key.clone(), value.clone());
                        Box::pin(async move {
                            list::remove(t, db, &key, count, &value, max_shift, now_ms()).await
                        })
                    })
                    .await,
                    "lrem",
                    max_shift,
                )
                .map(Value::Integer)
            }
            Err(e) => Err(e),
        },
        "lpos" => lpos(shared, db, key, args).await,
        "lmove" | "rpoplpush" => {
            let dirs = if spec.name == "rpoplpush" {
                Some((false, true))
            } else {
                match (direction(&args[3]), direction(&args[4])) {
                    (Some(a), Some(b)) => Some((a, b)),
                    _ => None,
                }
            };
            match dirs {
                None => Err(syntax_error()),
                Some((from_left, to_left)) => {
                    let dst = args[2].clone();
                    read(
                        txn(shared, "resp.lmove", move |t| {
                            let (src, dst) = (key.clone(), dst.clone());
                            Box::pin(async move {
                                list::move_one(t, db, &src, &dst, from_left, to_left, now_ms())
                                    .await
                            })
                        })
                        .await,
                    )
                    .map(|v| v.map_or(Value::Null, Value::bulk))
                }
            }
        }
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

/// `LEFT` is true, `RIGHT` false.
fn direction(arg: &[u8]) -> Option<bool> {
    match upper(arg).as_str() {
        "LEFT" => Some(true),
        "RIGHT" => Some(false),
        _ => None,
    }
}

/// A list read's outcome: a store error or `WRONGTYPE` becomes its reply.
fn read<T>(r: Result<list::Read<T>, Value>) -> Result<T, Value> {
    r?.map_err(|_| wrongtype())
}

/// A list command's outcome, with Valkey's errors (and Loams' for a list
/// over `list_max_shift`).
fn list_result<T>(
    r: Result<Result<T, ListError>, Value>,
    command: &str,
    max_shift: usize,
) -> Result<T, Value> {
    match r? {
        Ok(v) => Ok(v),
        Err(ListError::WrongType) => Err(wrongtype()),
        Err(ListError::NoSuchKey) => Err(Value::err("ERR no such key")),
        Err(ListError::OutOfRange) => Err(Value::err("ERR index out of range")),
        Err(ListError::TooLong) => Err(Value::err(format!(
            "ERR {} on a list longer than list-max-shift ({max_shift}) is not supported by Loams",
            command.to_ascii_uppercase()
        ))),
    }
}

/// `LPOP`/`RPOP key [count]`.
async fn pop(
    shared: &Shared,
    db: u8,
    key: Bytes,
    args: &[Bytes],
    left: bool,
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
            Some(u64::try_from(n).unwrap_or(0))
        }
        None => None,
    };
    let n = count.unwrap_or(1);
    let popped = read(
        txn(shared, "resp.pop", move |t| {
            let key = key.clone();
            Box::pin(async move { list::pop(t, db, &key, left, n, now_ms()).await })
        })
        .await,
    )?;
    Ok(match (popped, count) {
        (None, None) => Value::Null,
        (None, Some(_)) => Value::NullArray,
        (Some(mut items), None) => items.pop().map_or(Value::Null, Value::bulk),
        (Some(items), Some(_)) => Value::Array(items.into_iter().map(Value::bulk).collect()),
    })
}

/// `LPOS key element [RANK rank] [COUNT num-matches] [MAXLEN len]`.
async fn lpos(shared: &Shared, db: u8, key: Bytes, args: &[Bytes]) -> Result<Value, Value> {
    let mut o = PosOptions {
        rank: 1,
        count: None,
        maxlen: 0,
    };
    let mut i = 3;
    while i < args.len() {
        let Some(arg) = args.get(i + 1) else {
            return Err(syntax_error());
        };
        match upper(&args[i]).as_str() {
            "RANK" => {
                let r = int_arg(arg)?;
                if r == 0 {
                    return Err(Value::err(
                        "ERR RANK can't be zero: use 1 to start from the first match, 2 from the second ... or use negative to start from the last match",
                    ));
                }
                if r == i64::MIN {
                    return Err(Value::err("ERR value is out of range"));
                }
                o.rank = r;
            }
            "COUNT" => {
                let c = int_arg(arg)?;
                if c < 0 {
                    return Err(Value::err("ERR COUNT can't be negative"));
                }
                o.count = Some(u64::try_from(c).unwrap_or(0));
            }
            "MAXLEN" => {
                let m = int_arg(arg)?;
                if m < 0 {
                    return Err(Value::err("ERR MAXLEN can't be negative"));
                }
                o.maxlen = u64::try_from(m).unwrap_or(0);
            }
            _ => return Err(syntax_error()),
        }
        i += 2;
    }
    let value = args[2].clone();
    let found = read(
        txn(shared, "resp.lpos", move |t| {
            let (key, value) = (key.clone(), value.clone());
            Box::pin(async move { list::pos(t, db, &key, &value, o, now_ms()).await })
        })
        .await,
    )?;
    Ok(match o.count {
        None => found.first().map_or(Value::Null, |i| Value::Integer(*i)),
        Some(_) => Value::Array(found.into_iter().map(Value::Integer).collect()),
    })
}
