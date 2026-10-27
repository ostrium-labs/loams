//! Hash commands (plan KV2 Task 5): `HSET`, `HSETNX`, `HMSET`, `HGET`,
//! `HMGET`, `HGETALL`, `HKEYS`, `HVALS`, `HLEN`, `HEXISTS`, `HDEL`,
//! `HSTRLEN`, `HINCRBY`, `HINCRBYFLOAT`, `HRANDFIELD`, `HSCAN`.

use std::sync::Arc;

use bytes::Bytes;
use loams_kv::TxnOptions;

use super::{
    Spec, glob, int_arg, not_integer, syntax_error, txn, txn_with, upper, wrong_arity, wrongtype,
};
use crate::cursor::{self, Resume};
use crate::frame::{Proto, Value};
use crate::server::{Conn, Shared};
use crate::types::counter::{IncrError, parse_float};
use crate::types::hash;
use crate::types::now_ms;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let key = args[1].clone();
    let result = match spec.name {
        "hset" | "hmset" => hset(shared, db, args, spec.name).await,
        "hsetnx" => {
            let pair = vec![(args[2].to_vec(), args[3].to_vec())];
            set_pairs(shared, db, key, pair, true)
                .await
                .map(Value::Integer)
        }
        "hget" => get(shared, db, key, vec![args[2].to_vec()])
            .await
            .map(|mut v| v.pop().flatten().map_or(Value::Null, Value::bulk)),
        "hmget" => {
            let fields = args[2..].iter().map(|f| f.to_vec()).collect();
            get(shared, db, key, fields).await.map(|v| {
                Value::Array(
                    v.into_iter()
                        .map(|f| f.map_or(Value::Null, Value::bulk))
                        .collect(),
                )
            })
        }
        "hexists" => get(shared, db, key, vec![args[2].to_vec()])
            .await
            .map(|v| Value::Integer(i64::from(v.first().is_some_and(Option::is_some)))),
        "hstrlen" => get(shared, db, key, vec![args[2].to_vec()]).await.map(|v| {
            let n = v.first().and_then(Option::as_ref).map_or(0, Vec::len);
            Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))
        }),
        "hgetall" | "hkeys" | "hvals" => all(shared, db, key).await.map(|pairs| match spec.name {
            "hgetall" => Value::Map(
                pairs
                    .into_iter()
                    .map(|(f, v)| (Value::bulk(f), Value::bulk(v)))
                    .collect(),
            ),
            "hkeys" => Value::Array(pairs.into_iter().map(|(f, _)| Value::bulk(f)).collect()),
            _ => Value::Array(pairs.into_iter().map(|(_, v)| Value::bulk(v)).collect()),
        }),
        "hlen" => hlen(shared, db, key).await,
        "hdel" => hdel(shared, db, key, args).await,
        "hincrby" => hincrby(shared, db, key, args).await,
        "hincrbyfloat" => hincrbyfloat(shared, db, key, args).await,
        "hrandfield" => hrandfield(shared, conn, key, args).await,
        "hscan" => hscan(shared, conn, key, args).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

/// A hash read's outcome: `WRONGTYPE` becomes its reply.
fn read<T>(r: hash::Read<T>) -> Result<T, Value> {
    r.map_err(|_| wrongtype())
}

async fn hset(
    shared: &Shared,
    db: u8,
    args: &[Bytes],
    command: &'static str,
) -> Result<Value, Value> {
    if !(args.len() - 2).is_multiple_of(2) {
        return Err(wrong_arity(command));
    }
    let pairs = args[2..]
        .chunks(2)
        .map(|p| (p[0].to_vec(), p[1].to_vec()))
        .collect();
    let added = set_pairs(shared, db, args[1].clone(), pairs, false).await?;
    Ok(if command == "hmset" {
        Value::ok()
    } else {
        Value::Integer(added)
    })
}

async fn set_pairs(
    shared: &Shared,
    db: u8,
    key: Bytes,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    nx: bool,
) -> Result<i64, Value> {
    read(
        txn(shared, "resp.hset", move |t| {
            let (key, pairs) = (key.clone(), pairs.clone());
            Box::pin(async move { hash::set(t, db, &key, &pairs, nx, now_ms()).await })
        })
        .await?,
    )
}

async fn get(
    shared: &Shared,
    db: u8,
    key: Bytes,
    fields: Vec<Vec<u8>>,
) -> Result<Vec<Option<Vec<u8>>>, Value> {
    read(
        txn(shared, "resp.hget", move |t| {
            let (key, fields) = (key.clone(), fields.clone());
            Box::pin(async move { hash::get(t, db, &key, &fields, now_ms()).await })
        })
        .await?,
    )
}

async fn all(shared: &Shared, db: u8, key: Bytes) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Value> {
    read(
        txn(shared, "resp.hgetall", move |t| {
            let key = key.clone();
            Box::pin(async move { hash::all(t, db, &key, now_ms()).await })
        })
        .await?,
    )
}

async fn hlen(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    read(
        txn(shared, "resp.hlen", move |t| {
            let key = key.clone();
            Box::pin(async move { hash::len(t, db, &key, now_ms()).await })
        })
        .await?,
    )
    .map(Value::Integer)
}

async fn hdel(shared: &Shared, db: u8, key: Bytes, args: &[Bytes]) -> Result<Value, Value> {
    let fields: Vec<Vec<u8>> = args[2..].iter().map(|f| f.to_vec()).collect();
    read(
        txn(shared, "resp.hdel", move |t| {
            let (key, fields) = (key.clone(), fields.clone());
            Box::pin(async move { hash::del(t, db, &key, &fields, now_ms()).await })
        })
        .await?,
    )
    .map(Value::Integer)
}

/// One hash field is as hot as a counter (ruling of Task 4).
fn hot(op: &'static str) -> TxnOptions {
    TxnOptions {
        max_attempts: 64,
        ..TxnOptions::new(op)
    }
}

async fn hincrby(shared: &Shared, db: u8, key: Bytes, args: &[Bytes]) -> Result<Value, Value> {
    let field = args[2].clone();
    let by = int_arg(&args[3])?;
    let result = txn_with(shared, hot("resp.hincrby"), move |t| {
        let (key, field) = (key.clone(), field.clone());
        Box::pin(async move { hash::incr_by(t, db, &key, &field, by, now_ms()).await })
    })
    .await?;
    match result {
        Ok(n) => Ok(Value::Integer(n)),
        Err(IncrError::NotANumber) => Err(Value::err("ERR hash value is not an integer")),
        Err(e) => Err(incr_error(e)),
    }
}

async fn hincrbyfloat(shared: &Shared, db: u8, key: Bytes, args: &[Bytes]) -> Result<Value, Value> {
    let field = args[2].clone();
    let Some(by) = parse_float(&args[3]) else {
        return Err(Value::err("ERR value is not a valid float"));
    };
    let result = txn_with(shared, hot("resp.hincrbyfloat"), move |t| {
        let (key, field) = (key.clone(), field.clone());
        Box::pin(async move { hash::incr_by_float(t, db, &key, &field, by, now_ms()).await })
    })
    .await?;
    match result {
        Ok(text) => Ok(Value::bulk(text)),
        Err(IncrError::NotANumber) => Err(Value::err("ERR hash value is not a float")),
        Err(e) => Err(incr_error(e)),
    }
}

fn incr_error(e: IncrError) -> Value {
    match e {
        IncrError::WrongType => wrongtype(),
        IncrError::NotANumber => not_integer(),
        IncrError::Overflow => Value::err("ERR increment or decrement would overflow"),
        IncrError::NotFinite => Value::err("ERR increment would produce NaN or Infinity"),
    }
}

/// A small xorshift64* generator; `HRANDFIELD` and its kin need no
/// cryptographic randomness.
#[derive(Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn seeded(salt: u64) -> Rng {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::from(d.subsec_nanos()) ^ d.as_secs());
        Rng((nanos ^ salt.rotate_left(17)) | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number in `0..n` (`n > 0`).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next_u64() % n64).unwrap_or(0)
    }
}

/// The indexes `HRANDFIELD`-style commands reply with, over `len` items:
/// `count ≥ 0` picks that many distinct ones (all when `count ≥ len`);
/// `count < 0` picks `-count` with repetition.
pub(crate) fn random_picks(rng: &mut Rng, len: usize, count: i64) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    if count < 0 {
        let n = usize::try_from(count.unsigned_abs()).unwrap_or(usize::MAX);
        return (0..n).map(|_| rng.below(len)).collect();
    }
    let n = usize::try_from(count).unwrap_or(usize::MAX);
    let mut all: Vec<usize> = (0..len).collect();
    if n >= len {
        return all;
    }
    // A partial Fisher–Yates shuffle.
    for i in 0..n {
        let j = i + rng.below(len - i);
        all.swap(i, j);
    }
    all.truncate(n);
    all
}

async fn hrandfield(
    shared: &Shared,
    conn: &Conn,
    key: Bytes,
    args: &[Bytes],
) -> Result<Value, Value> {
    if args.len() > 4 {
        return Err(syntax_error());
    }
    let count = match args.get(2) {
        Some(c) => Some(int_arg(c)?),
        None => None,
    };
    let withvalues = match args.get(3) {
        Some(w) if upper(w) == "WITHVALUES" => true,
        Some(_) => return Err(syntax_error()),
        None => false,
    };
    if let Some(c) = count
        && (c == i64::MIN
            || (withvalues && c.unsigned_abs() > u64::try_from(i64::MAX / 2).unwrap_or(0)))
    {
        return Err(Value::err("ERR value is out of range"));
    }
    let pairs = all(shared, conn.db, key).await?;
    let mut rng = Rng::seeded(conn.id);
    let Some(count) = count else {
        if pairs.is_empty() {
            return Ok(Value::Null);
        }
        let i = rng.below(pairs.len());
        return Ok(Value::bulk(pairs[i].0.clone()));
    };
    let picks = random_picks(&mut rng, pairs.len(), count);
    let items = picks.into_iter().map(|i| &pairs[i]);
    Ok(if !withvalues {
        Value::Array(items.map(|(f, _)| Value::bulk(f.clone())).collect())
    } else if conn.proto == Proto::Resp3 {
        Value::Array(
            items
                .map(|(f, v)| Value::Array(vec![Value::bulk(f.clone()), Value::bulk(v.clone())]))
                .collect(),
        )
    } else {
        Value::Array(
            items
                .flat_map(|(f, v)| [Value::bulk(f.clone()), Value::bulk(v.clone())])
                .collect(),
        )
    })
}

/// The options of `HSCAN`, `SSCAN` and `ZSCAN` after the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanArgs {
    pub cursor: u64,
    pub pattern: Option<Bytes>,
    pub count: usize,
    pub novalues: bool,
}

impl ScanArgs {
    /// Parses `cursor [MATCH pattern] [COUNT count]` and, when allowed,
    /// `NOVALUES`.
    pub(crate) fn parse(args: &[Bytes], allow_novalues: bool) -> Result<ScanArgs, Value> {
        let mut out = ScanArgs {
            cursor: cursor::parse(&args[0])?,
            pattern: None,
            count: 10,
            novalues: false,
        };
        let mut i = 1;
        while i < args.len() {
            match (upper(&args[i]).as_str(), args.get(i + 1)) {
                ("MATCH", Some(p)) => {
                    out.pattern = Some(p.clone());
                    i += 2;
                }
                ("COUNT", Some(n)) => {
                    let n = int_arg(n)?;
                    if n < 1 {
                        return Err(syntax_error());
                    }
                    out.count = usize::try_from(n).unwrap_or(usize::MAX).min(100_000);
                    i += 2;
                }
                ("NOVALUES", _) if allow_novalues => {
                    out.novalues = true;
                    i += 1;
                }
                _ => return Err(syntax_error()),
            }
        }
        Ok(out)
    }

    /// Whether `item` passes the `MATCH` pattern.
    pub(crate) fn matches(&self, item: &[u8]) -> bool {
        self.pattern.as_ref().is_none_or(|p| glob(p, item))
    }
}

/// The scope a `*SCAN` cursor belongs to: the command and its key.
pub(crate) fn scan_scope(command: &str, db: u8, key: &[u8]) -> Vec<u8> {
    let mut s = command.as_bytes().to_vec();
    s.push(0);
    s.push(db);
    s.extend_from_slice(key);
    s
}

async fn hscan(
    shared: &Shared,
    conn: &mut Conn,
    key: Bytes,
    args: &[Bytes],
) -> Result<Value, Value> {
    let scan = ScanArgs::parse(&args[2..], true)?;
    let db = conn.db;
    let scope = scan_scope("hscan", db, &key);
    let from = match conn.cursors.take(scan.cursor, &scope) {
        Resume::Start => None,
        Resume::From(item) => Some(item),
        Resume::Unknown => return Ok(scan_reply(0, Vec::new())),
    };
    let count = scan.count;
    let (page, next) = read(
        txn(shared, "resp.hscan", move |t| {
            let (key, from) = (key.clone(), from.clone());
            Box::pin(async move { hash::page(t, db, &key, from.as_deref(), count, now_ms()).await })
        })
        .await?,
    )?;
    let mut items = Vec::new();
    for (f, v) in page {
        if !scan.matches(&f) {
            continue;
        }
        items.push(Value::bulk(f));
        if !scan.novalues {
            items.push(Value::bulk(v));
        }
    }
    let cursor = next.map_or(0, |item| conn.cursors.save(&scope, item));
    Ok(scan_reply(cursor, items))
}

/// A `*SCAN` reply: the next cursor (as a bulk string) and the items.
pub(crate) fn scan_reply(cursor: u64, items: Vec<Value>) -> Value {
    Value::Array(vec![Value::bulk(cursor.to_string()), Value::Array(items)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrandfield_counts() {
        let mut rng = Rng::seeded(7);
        assert!(random_picks(&mut rng, 0, 5).is_empty());
        assert!(random_picks(&mut rng, 5, 0).is_empty());
        // More than the hash holds: every field once.
        let mut all = random_picks(&mut rng, 5, 10);
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);
        // Fewer: distinct, in range.
        for _ in 0..100 {
            let mut some = random_picks(&mut rng, 50, 7);
            assert_eq!(some.len(), 7);
            assert!(some.iter().all(|i| *i < 50));
            some.sort_unstable();
            some.dedup();
            assert_eq!(some.len(), 7, "distinct");
        }
        // Negative: exactly that many, repeats allowed.
        let reps = random_picks(&mut rng, 2, -100);
        assert_eq!(reps.len(), 100);
        assert!(reps.iter().all(|i| *i < 2));
        assert!(reps.contains(&0) && reps.contains(&1));
    }

    #[test]
    fn scan_args() {
        let a =
            |s: &[&str]| -> Vec<Bytes> { s.iter().map(|x| Bytes::from(x.to_string())).collect() };
        let p = ScanArgs::parse(&a(&["0", "MATCH", "f*", "COUNT", "5", "NOVALUES"]), true)
            .expect("valid");
        assert_eq!(p.count, 5);
        assert!(p.novalues);
        assert!(p.matches(b"field") && !p.matches(b"x"));
        assert!(ScanArgs::parse(&a(&["0", "NOVALUES"]), false).is_err());
        assert!(ScanArgs::parse(&a(&["0", "COUNT", "0"]), true).is_err());
        assert!(ScanArgs::parse(&a(&["x"]), true).is_err());
        assert!(ScanArgs::parse(&a(&["0", "MATCH"]), true).is_err());
    }
}
