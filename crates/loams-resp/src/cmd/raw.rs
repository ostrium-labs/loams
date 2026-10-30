//! Raw databases (coordinator ruling 2026-10-30; design note
//! `docs/design/kv-rawkv.md`): a database listed in
//! `raw_databases` keeps plain strings on TiKV's RawKV API through
//! [`loams_kv::RawStore`], so its commands never take a timestamp from PD.
//!
//! A raw database serves strings only:
//!
//! | Command | How |
//! |---|---|
//! | `GET`, `MGET`, `STRLEN`, `EXISTS`, `TYPE`, `TTL`, `PTTL` | raw reads |
//! | `SET` (plain, `EX`/`PX`/`EXAT`/`PXAT`), `SETEX`, `PSETEX`, `MSET` | raw puts; `MSET` is not atomic |
//! | `SET NX`, `SETNX` | compare-and-swap from absent |
//! | `SET XX`, `SET … GET`, `GETSET` | a compare-and-swap loop |
//! | `INCR`, `DECR`, `INCRBY`, `DECRBY` | a compare-and-swap loop |
//! | `DEL`, `UNLINK` | raw deletes (the count is read first, not atomic) |
//!
//! Everything else (hashes, sets, sorted sets, lists, `EXPIRE` and its
//! family, `KEYS`/`SCAN`/`DBSIZE`/`FLUSHDB`, and anything multi-key atomic)
//! answers an error naming the database. Three limits come from TiKV's raw
//! API: a compare-and-swap writes without a TTL, and there is no way to
//! change a key's TTL in place. So `SET NX`/`XX`/`GET` with an expiry,
//! `KEEPTTL`, `EXPIRE` and `PERSIST` are refused, and `INCR` on a key that
//! has a TTL is refused (it would lose the TTL). TTLs are whole seconds,
//! rounded up.
//!
//! A key is `db ‖ user key` under the Redis product root. Raw and
//! transactional data never meet: TiKV keeps them apart by key mode.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use loams_kv::{MAX_VALUE_BYTES, RawStore, RawTtl};

use super::string::SetOptions;
use super::{Spec, int_arg, not_integer};
use crate::frame::Value;
use crate::server::{Conn, Shared};
use crate::types;

/// The reply for a command a raw database does not serve.
fn not_raw(name: &str, db: u8) -> Value {
    Value::err(format!(
        "ERR '{name}' is not supported in raw database {db}: raw databases serve strings only \
         (GET, SET, MGET, MSET, DEL, EXISTS, TTL, INCR and their forms)"
    ))
}

fn raw_err(e: loams_kv::RawError) -> Value {
    Value::err(format!("ERR raw store: {e}"))
}

fn key(db: u8, user: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(1 + user.len());
    k.push(db);
    k.extend_from_slice(user);
    k
}

fn too_big(value: &[u8]) -> Option<Value> {
    (value.len() > MAX_VALUE_BYTES).then(|| {
        Value::err(format!(
            "ERR value of {} bytes is over a raw database's limit of {MAX_VALUE_BYTES}",
            value.len()
        ))
    })
}

/// Runs one command against a raw database.
pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let Some(raw) = shared.raw.as_ref() else {
        return Value::err(format!("ERR database {db} is raw but no raw store is open"));
    };
    let result = match spec.name {
        "get" => raw
            .get(&key(db, &args[1]))
            .await
            .map(|v| v.map_or(Value::Null, |v| Value::bulk(Bytes::from(v))))
            .map_err(raw_err),
        "strlen" => raw
            .get(&key(db, &args[1]))
            .await
            .map(|v| Value::Integer(v.map_or(0, |v| v.len() as i64)))
            .map_err(raw_err),
        "mget" => mget(raw, db, &args[1..]).await,
        "exists" => exists(raw, db, &args[1..]).await,
        "type" => raw
            .get(&key(db, &args[1]))
            .await
            .map(|v| Value::Simple(if v.is_some() { "string" } else { "none" }.to_owned()))
            .map_err(raw_err),
        "ttl" | "pttl" => ttl(raw, db, &args[1], spec.name == "pttl").await,
        "set" => match SetOptions::parse(&args[3..]) {
            Ok(opts) => set(raw, db, &args[1], &args[2], opts, "set").await,
            Err(e) => Err(e),
        },
        "setnx" => {
            let opts = SetOptions {
                nx: true,
                ..SetOptions::default()
            };
            set(raw, db, &args[1], &args[2], opts, "setnx")
                .await
                .map(|v| Value::Integer(i64::from(v == Value::ok())))
        }
        "setex" | "psetex" => match int_arg(&args[2]) {
            Ok(n) => {
                let unit = if spec.name == "setex" { "EX" } else { "PX" };
                let opts = SetOptions::parse(&[Bytes::from(unit), Bytes::from(n.to_string())]);
                match opts {
                    Ok(opts) => set(raw, db, &args[1], &args[3], opts, spec.name).await,
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        },
        "getset" => {
            let opts = SetOptions {
                get: true,
                ..SetOptions::default()
            };
            set(raw, db, &args[1], &args[2], opts, "getset").await
        }
        "mset" => mset(raw, db, &args[1..]).await,
        "del" | "unlink" => del(raw, db, &args[1..]).await,
        "incr" => incr_by(raw, db, &args[1], 1).await,
        "decr" => incr_by(raw, db, &args[1], -1).await,
        "incrby" => match int_arg(&args[2]) {
            Ok(n) => incr_by(raw, db, &args[1], n).await,
            Err(e) => Err(e),
        },
        "decrby" => match int_arg(&args[2]) {
            Ok(n) if n != i64::MIN => incr_by(raw, db, &args[1], -n).await,
            Ok(_) => Err(Value::err("ERR decrement would overflow")),
            Err(e) => Err(e),
        },
        other => Err(not_raw(other, db)),
    };
    result.unwrap_or_else(|e| e)
}

async fn mget(raw: &RawStore, db: u8, keys: &[Bytes]) -> Result<Value, Value> {
    let full: Vec<Vec<u8>> = keys.iter().map(|k| key(db, k)).collect();
    let found: std::collections::HashMap<Vec<u8>, Vec<u8>> = raw
        .batch_get(&full)
        .await
        .map_err(raw_err)?
        .into_iter()
        .collect();
    Ok(Value::Array(
        full.iter()
            .map(|k| {
                found
                    .get(k)
                    .map_or(Value::Null, |v| Value::bulk(Bytes::from(v.clone())))
            })
            .collect(),
    ))
}

async fn exists(raw: &RawStore, db: u8, keys: &[Bytes]) -> Result<Value, Value> {
    let full: Vec<Vec<u8>> = keys.iter().map(|k| key(db, k)).collect();
    let found: std::collections::HashSet<Vec<u8>> = raw
        .batch_get(&full)
        .await
        .map_err(raw_err)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    // Valkey counts a key named twice twice.
    Ok(Value::Integer(
        full.iter().filter(|k| found.contains(*k)).count() as i64,
    ))
}

async fn ttl(raw: &RawStore, db: u8, user: &[u8], ms: bool) -> Result<Value, Value> {
    Ok(Value::Integer(
        match raw.ttl(&key(db, user)).await.map_err(raw_err)? {
            RawTtl::Absent => -2,
            RawTtl::Persistent => -1,
            RawTtl::Expires(left) if ms => left.as_millis() as i64,
            RawTtl::Expires(left) => left.as_secs_f64().ceil() as i64,
        },
    ))
}

async fn mset(raw: &RawStore, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Err(super::wrong_arity("mset"));
    }
    let mut pairs = Vec::with_capacity(args.len() / 2);
    for pair in args.chunks(2) {
        if let Some(e) = too_big(&pair[1]) {
            return Err(e);
        }
        pairs.push((key(db, &pair[0]), pair[1].to_vec()));
    }
    raw.batch_put(pairs, None).await.map_err(raw_err)?;
    Ok(Value::ok())
}

async fn del(raw: &RawStore, db: u8, keys: &[Bytes]) -> Result<Value, Value> {
    let full: Vec<Vec<u8>> = keys.iter().map(|k| key(db, k)).collect();
    let mut present: Vec<Vec<u8>> = raw
        .batch_get(&full)
        .await
        .map_err(raw_err)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    present.sort();
    present.dedup();
    raw.batch_delete(&present).await.map_err(raw_err)?;
    Ok(Value::Integer(present.len() as i64))
}

/// `SET` and its forms on a raw database. Replies `OK`, or the null when `NX`
/// or `XX` stops it; with `GET`, the old value (or the null).
async fn set(
    raw: &RawStore,
    db: u8,
    user: &[u8],
    value: &[u8],
    opts: SetOptions,
    command: &'static str,
) -> Result<Value, Value> {
    if let Some(e) = too_big(value) {
        return Err(e);
    }
    let k = key(db, user);
    let now = types::now_ms();
    let expire_at = opts.expire_at(now, command)?;
    if opts.keepttl {
        return Err(Value::err(
            "ERR SET KEEPTTL is not supported in a raw database (TiKV cannot keep a TTL across a write)",
        ));
    }
    let conditional = opts.nx || opts.xx || opts.get;
    if conditional && expire_at.is_some() {
        return Err(Value::err(
            "ERR SET with NX, XX or GET and an expiry is not supported in a raw database \
             (a conditional raw write cannot carry a TTL)",
        ));
    }
    if !conditional {
        match expire_at {
            None => raw.put(&k, value.to_vec(), None).await.map_err(raw_err)?,
            Some(at) => {
                let left = at - now as i64;
                if left <= 0 {
                    // Already past: Valkey keeps nothing.
                    raw.delete(&k).await.map_err(raw_err)?;
                } else {
                    let ttl = Duration::from_millis(left as u64);
                    raw.put(&k, value.to_vec(), Some(ttl))
                        .await
                        .map_err(raw_err)?;
                }
            }
        }
        return Ok(Value::ok());
    }
    // NX, XX or GET: a compare-and-swap loop. A plain SET drops a TTL in
    // Valkey too, so the swap's lack of one matches.
    loop {
        let current = raw.get(&k).await.map_err(raw_err)?;
        let stop = (opts.nx && current.is_some()) || (opts.xx && current.is_none());
        let old = || {
            current
                .clone()
                .map_or(Value::Null, |v| Value::bulk(Bytes::from(v)))
        };
        if stop {
            return Ok(if opts.get { old() } else { Value::Null });
        }
        let (_, swapped) = raw
            .compare_and_swap(&k, current.as_deref(), value.to_vec())
            .await
            .map_err(raw_err)?;
        if swapped {
            return Ok(if opts.get { old() } else { Value::ok() });
        }
    }
}

async fn incr_by(raw: &RawStore, db: u8, user: &[u8], by: i64) -> Result<Value, Value> {
    let k = key(db, user);
    loop {
        let current = raw.get(&k).await.map_err(raw_err)?;
        let n: i64 = match &current {
            None => 0,
            Some(v) => std::str::from_utf8(v)
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(not_integer)?,
        };
        if current.is_some() && matches!(raw.ttl(&k).await.map_err(raw_err)?, RawTtl::Expires(_)) {
            return Err(Value::err(
                "ERR INCR on a key with a TTL is not supported in a raw database \
                 (the swap would drop the TTL)",
            ));
        }
        let Some(next) = n.checked_add(by) else {
            return Err(Value::err("ERR increment or decrement would overflow"));
        };
        let (_, swapped) = raw
            .compare_and_swap(&k, current.as_deref(), next.to_string().into_bytes())
            .await
            .map_err(raw_err)?;
        if swapped {
            return Ok(Value::Integer(next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::syntax_error;
    use super::*;

    #[test]
    fn keys_carry_the_database() {
        assert_eq!(key(3, b"k"), vec![3, b'k']);
    }

    #[test]
    fn syntax_error_is_reachable() {
        // SET's parser is shared with the transactional path.
        let bad = SetOptions::parse(&[Bytes::from("NX"), Bytes::from("XX")]);
        assert_eq!(bad, Err(syntax_error()));
    }
}
