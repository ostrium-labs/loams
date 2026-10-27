//! String commands (plan KV2 Task 3): `GET`, `SET` with every option,
//! `SETNX`, `SETEX`, `PSETEX`, `GETSET`, `GETDEL`, `GETEX`, `MGET`, `MSET`,
//! `MSETNX`, `APPEND`, `STRLEN`, `GETRANGE`/`SUBSTR`, `SETRANGE`. Replies
//! and error texts follow Valkey's documented behaviour; `LCS` is refused
//! (in the command table's not-supported list).

use std::sync::Arc;

use bytes::Bytes;

use super::keys::expire_when;
use super::{Spec, int_arg, syntax_error, txn, upper, wrong_arity, wrongtype};
use crate::frame::Value;
use crate::server::{Conn, Shared};
use crate::types::{self, delete_key, put_meta, read_live, string};

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let max = shared.config.proto_max_bulk_len;
    let result = match spec.name {
        "get" => get(shared, db, args[1].clone()).await,
        "set" => match SetOptions::parse(&args[3..]) {
            Ok(opts) => set(shared, db, args[1].clone(), args[2].clone(), opts, "set").await,
            Err(e) => Err(e),
        },
        "setnx" => {
            let opts = SetOptions {
                nx: true,
                ..SetOptions::default()
            };
            set(shared, db, args[1].clone(), args[2].clone(), opts, "setnx")
                .await
                .map(|v| Value::Integer(i64::from(v == Value::ok())))
        }
        "setex" | "psetex" => {
            let unit = if spec.name == "setex" {
                Unit::Ex
            } else {
                Unit::Px
            };
            match int_arg(&args[2]) {
                Ok(n) => {
                    let opts = SetOptions {
                        expire: Some((unit, n)),
                        ..SetOptions::default()
                    };
                    set(
                        shared,
                        db,
                        args[1].clone(),
                        args[3].clone(),
                        opts,
                        spec.name,
                    )
                    .await
                }
                Err(e) => Err(e),
            }
        }
        "getset" => {
            let opts = SetOptions {
                get: true,
                ..SetOptions::default()
            };
            set(shared, db, args[1].clone(), args[2].clone(), opts, "getset").await
        }
        "getdel" => getdel(shared, db, args[1].clone()).await,
        "getex" => getex(shared, db, args).await,
        "mget" => mget(shared, db, args).await,
        "mset" | "msetnx" => mset(shared, db, args, spec.name).await,
        "append" => append(shared, db, args[1].clone(), args[2].clone(), max).await,
        "strlen" => strlen(shared, db, args[1].clone()).await,
        "getrange" | "substr" => getrange(shared, db, args).await,
        "setrange" => setrange(shared, db, args, max).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    result.unwrap_or_else(|e| e)
}

/// The unit and base of an expiry option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    /// `EX`: seconds from now.
    Ex,
    /// `PX`: milliseconds from now.
    Px,
    /// `EXAT`: Unix seconds.
    ExAt,
    /// `PXAT`: Unix milliseconds.
    PxAt,
}

impl Unit {
    fn of(word: &str) -> Option<Unit> {
        Some(match word {
            "EX" => Unit::Ex,
            "PX" => Unit::Px,
            "EXAT" => Unit::ExAt,
            "PXAT" => Unit::PxAt,
            _ => return None,
        })
    }

    fn unit_ms(self) -> i64 {
        match self {
            Unit::Ex | Unit::ExAt => 1000,
            Unit::Px | Unit::PxAt => 1,
        }
    }

    fn absolute(self) -> bool {
        matches!(self, Unit::ExAt | Unit::PxAt)
    }
}

/// `SET`'s options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SetOptions {
    pub nx: bool,
    pub xx: bool,
    pub get: bool,
    pub keepttl: bool,
    pub expire: Option<(Unit, i64)>,
}

impl SetOptions {
    /// Parses the options after `SET key value`, as Valkey does: `NX` and
    /// `XX` exclude each other, `KEEPTTL` and the expiry forms exclude each
    /// other, and an expiry form needs its argument.
    pub(crate) fn parse(args: &[Bytes]) -> Result<SetOptions, Value> {
        let mut o = SetOptions::default();
        let mut i = 0;
        while i < args.len() {
            let word = upper(&args[i]);
            let next = args.get(i + 1);
            match word.as_str() {
                "NX" if !o.xx => o.nx = true,
                "XX" if !o.nx => o.xx = true,
                "GET" => o.get = true,
                "KEEPTTL" if o.expire.is_none() => o.keepttl = true,
                w => match (Unit::of(w), next) {
                    (Some(unit), Some(n))
                        if !o.keepttl && o.expire.is_none_or(|(u, _)| u == unit) =>
                    {
                        o.expire = Some((unit, int_arg(n)?));
                        i += 1;
                    }
                    _ => return Err(syntax_error()),
                },
            }
            i += 1;
        }
        Ok(o)
    }

    /// The expiry in Unix ms (0: none) at `now`, or Valkey's error for a
    /// non-positive or overflowing time.
    fn expire_at(self, now: u64, command: &str) -> Result<Option<i64>, Value> {
        let Some((unit, n)) = self.expire else {
            return Ok(None);
        };
        if n <= 0 {
            return Err(Value::err(format!(
                "ERR invalid expire time in '{command}' command"
            )));
        }
        expire_when(n, unit.unit_ms(), unit.absolute(), now, command).map(Some)
    }
}

async fn get(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    let read = txn(shared, "resp.get", move |t| {
        let key = key.clone();
        Box::pin(async move { string::get(t, db, &key, types::now_ms()).await })
    })
    .await?;
    match read {
        Err(_) => Err(wrongtype()),
        Ok(v) => Ok(v.map_or(Value::Null, Value::bulk)),
    }
}

/// `SET` and its single-purpose forms. Replies `OK`, or the null when `NX`
/// or `XX` stops it; with `GET`, the old value (or the null) either way.
async fn set(
    shared: &Shared,
    db: u8,
    key: Bytes,
    value: Bytes,
    opts: SetOptions,
    command: &'static str,
) -> Result<Value, Value> {
    // Refuse a bad expiry before touching the key.
    opts.expire_at(types::now_ms(), command)?;
    let reply = txn(shared, "resp.set", move |t| {
        let (key, value) = (key.clone(), value.clone());
        Box::pin(async move {
            let now = types::now_ms();
            let found = read_live(t, db, &key, now).await?;
            let old = if opts.get {
                match &found.live {
                    Some(m) if m.kind != crate::layout::Kind::String => {
                        return Ok(Err(()));
                    }
                    Some(m) => Some(string::body(t, db, &key, m).await?),
                    None => None,
                }
            } else {
                None
            };
            let exists = found.live.is_some();
            let applies = !((opts.nx && exists) || (opts.xx && !exists));
            if applies {
                let at = match opts.expire_at(now, command) {
                    Ok(at) => at,
                    Err(_) => return Ok(Err(())),
                };
                let expire = if opts.keepttl {
                    found.live.as_ref().map_or(0, |m| m.expire_at_ms)
                } else {
                    at.map_or(0, |at| u64::try_from(at).unwrap_or(0))
                };
                if at.is_some() && expire <= now {
                    // An absolute time already past: set, then expire at once.
                    delete_key(t, db, &key, now).await?;
                } else {
                    string::write(t, db, &key, &value, expire, found.stored.as_ref()).await?;
                }
            }
            Ok(Ok((applies, old)))
        })
    })
    .await?;
    let Ok((applied, old)) = reply else {
        return Err(wrongtype());
    };
    Ok(if opts.get {
        old.map_or(Value::Null, Value::bulk)
    } else if applied {
        Value::ok()
    } else {
        Value::Null
    })
}

async fn getdel(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    let read = txn(shared, "resp.getdel", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let v = string::get(t, db, &key, now).await?;
            if let Ok(Some(_)) = v {
                delete_key(t, db, &key, now).await?;
            }
            Ok(v)
        })
    })
    .await?;
    match read {
        Err(_) => Err(wrongtype()),
        Ok(v) => Ok(v.map_or(Value::Null, Value::bulk)),
    }
}

/// `GETEX key [EX s | PX ms | EXAT ts | PXAT ms | PERSIST]`.
async fn getex(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let mut expire: Option<(Unit, i64)> = None;
    let mut persist = false;
    let mut i = 2;
    while i < args.len() {
        let word = upper(&args[i]);
        match (word.as_str(), Unit::of(&word), args.get(i + 1)) {
            ("PERSIST", _, _) if expire.is_none() => persist = true,
            (_, Some(unit), Some(n)) if !persist && expire.is_none_or(|(u, _)| u == unit) => {
                expire = Some((unit, int_arg(n)?));
                i += 1;
            }
            _ => return Err(syntax_error()),
        }
        i += 1;
    }
    let opts = SetOptions {
        expire,
        ..SetOptions::default()
    };
    opts.expire_at(types::now_ms(), "getex")?;
    let key = args[1].clone();
    let read = txn(shared, "resp.getex", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let (_, read) = string::meta(t, db, &key, now).await?;
            let mut meta = match read {
                Ok(Some(m)) => m,
                Ok(None) => return Ok(Ok(None)),
                Err(w) => return Ok(Err(w)),
            };
            let value = string::body(t, db, &key, &meta).await?;
            let at = opts.expire_at(now, "getex").ok().flatten();
            match at {
                Some(at) if at <= i64::try_from(now).unwrap_or(i64::MAX) => {
                    delete_key(t, db, &key, now).await?;
                }
                Some(at) => {
                    meta.expire_at_ms = u64::try_from(at).unwrap_or(0);
                    put_meta(t, db, &key, &meta).await?;
                }
                None if persist && meta.expire_at_ms != 0 => {
                    meta.expire_at_ms = 0;
                    put_meta(t, db, &key, &meta).await?;
                }
                None => {}
            }
            Ok(Ok(Some(value)))
        })
    })
    .await?;
    match read {
        Err(_) => Err(wrongtype()),
        Ok(v) => Ok(v.map_or(Value::Null, Value::bulk)),
    }
}

/// `MGET`: each key's string, or the null for a missing key or another
/// type.
async fn mget(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let keys: Vec<Bytes> = args[1..].to_vec();
    let values = txn(shared, "resp.mget", move |t| {
        let keys = keys.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let mut out = Vec::with_capacity(keys.len());
            for k in &keys {
                out.push(string::get(t, db, k, now).await?.ok().flatten());
            }
            Ok(out)
        })
    })
    .await?;
    Ok(Value::Array(
        values
            .into_iter()
            .map(|v| v.map_or(Value::Null, Value::bulk))
            .collect(),
    ))
}

/// `MSET` (always `OK`) and `MSETNX` (1 when no key existed and all were
/// set, else 0 and nothing is set). Every key loses its TTL.
async fn mset(
    shared: &Shared,
    db: u8,
    args: &[Bytes],
    command: &'static str,
) -> Result<Value, Value> {
    if !(args.len() - 1).is_multiple_of(2) {
        return Err(wrong_arity(command));
    }
    let nx = command == "msetnx";
    let pairs: Vec<(Bytes, Bytes)> = args[1..]
        .chunks(2)
        .map(|p| (p[0].clone(), p[1].clone()))
        .collect();
    let set = txn(shared, "resp.mset", move |t| {
        let pairs = pairs.clone();
        Box::pin(async move {
            let now = types::now_ms();
            if nx {
                for (k, _) in &pairs {
                    if read_live(t, db, k, now).await?.live.is_some() {
                        return Ok(false);
                    }
                }
            }
            for (k, v) in &pairs {
                let found = read_live(t, db, k, now).await?;
                string::write(t, db, k, v, 0, found.stored.as_ref()).await?;
            }
            Ok(true)
        })
    })
    .await?;
    Ok(if nx {
        Value::Integer(i64::from(set))
    } else {
        Value::ok()
    })
}

/// The error for a string that would outgrow `proto-max-bulk-len`.
fn too_big() -> Value {
    Value::err("ERR string exceeds maximum allowed size (proto-max-bulk-len)")
}

async fn append(
    shared: &Shared,
    db: u8,
    key: Bytes,
    value: Bytes,
    max: usize,
) -> Result<Value, Value> {
    let grown = txn(shared, "resp.append", move |t| {
        let (key, value) = (key.clone(), value.clone());
        Box::pin(async move {
            let now = types::now_ms();
            let (_, read) = string::meta(t, db, &key, now).await?;
            match read {
                Err(w) => return Ok(Err(Some(w))),
                Ok(Some(m)) if string::len(&m).saturating_add(value.len() as u64) > max as u64 => {
                    return Ok(Err(None));
                }
                Ok(_) => {}
            }
            Ok(string::append(t, db, &key, &value, now)
                .await?
                .map_err(Some))
        })
    })
    .await?;
    match grown {
        Ok(n) => Ok(Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))),
        Err(Some(_)) => Err(wrongtype()),
        Err(None) => Err(too_big()),
    }
}

async fn strlen(shared: &Shared, db: u8, key: Bytes) -> Result<Value, Value> {
    let read = txn(shared, "resp.strlen", move |t| {
        let key = key.clone();
        Box::pin(async move {
            let (_, read) = string::meta(t, db, &key, types::now_ms()).await?;
            Ok(read.map(|m| m.map_or(0, |m| string::len(&m))))
        })
    })
    .await?;
    match read {
        Err(_) => Err(wrongtype()),
        Ok(n) => Ok(Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))),
    }
}

async fn getrange(shared: &Shared, db: u8, args: &[Bytes]) -> Result<Value, Value> {
    let start = int_arg(&args[2])?;
    let end = int_arg(&args[3])?;
    let key = args[1].clone();
    let read = txn(shared, "resp.getrange", move |t| {
        let key = key.clone();
        Box::pin(async move { string::getrange(t, db, &key, start, end, types::now_ms()).await })
    })
    .await?;
    match read {
        Err(_) => Err(wrongtype()),
        Ok(bytes) => Ok(Value::bulk(bytes)),
    }
}

async fn setrange(shared: &Shared, db: u8, args: &[Bytes], max: usize) -> Result<Value, Value> {
    let offset = int_arg(&args[2])?;
    let Ok(offset) = usize::try_from(offset) else {
        return Err(Value::err("ERR offset is out of range"));
    };
    let (key, value) = (args[1].clone(), args[3].clone());
    // Valkey checks the type, then an empty value, then the size.
    let too_large = offset.saturating_add(value.len()) > max;
    let grown = txn(shared, "resp.setrange", move |t| {
        let (key, value) = (key.clone(), value.clone());
        Box::pin(async move {
            let now = types::now_ms();
            let (_, read) = string::meta(t, db, &key, now).await?;
            match read {
                Err(w) => return Ok(Err(Some(w))),
                Ok(live) if value.is_empty() => {
                    return Ok(Ok(live.map_or(0, |m| string::len(&m))));
                }
                Ok(_) if too_large => return Ok(Err(None)),
                Ok(_) => {}
            }
            Ok(string::setrange(t, db, &key, offset, &value, now)
                .await?
                .map_err(Some))
        })
    })
    .await?;
    match grown {
        Ok(n) => Ok(Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))),
        Err(Some(_)) => Err(wrongtype()),
        Err(None) => Err(too_big()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(words: &[&str]) -> Result<SetOptions, Value> {
        let args: Vec<Bytes> = words.iter().map(|w| Bytes::from(w.to_string())).collect();
        SetOptions::parse(&args)
    }

    /// Every pair of `SET` options: which combine and which are a syntax
    /// error, as Valkey parses them.
    #[test]
    fn set_options_matrix() {
        let ok = [
            vec!["NX"],
            vec!["XX"],
            vec!["GET"],
            vec!["NX", "GET"],
            vec!["XX", "GET"],
            vec!["KEEPTTL"],
            vec!["XX", "KEEPTTL", "GET"],
            vec!["EX", "10"],
            vec!["PX", "10"],
            vec!["EXAT", "10"],
            vec!["PXAT", "10"],
            vec!["NX", "EX", "10", "GET"],
            vec!["ex", "10"],
            vec!["EX", "5", "EX", "10"],
        ];
        for words in ok {
            assert!(opts(&words).is_ok(), "{words:?}");
        }
        let syntax = [
            vec!["NX", "XX"],
            vec!["XX", "NX"],
            vec!["KEEPTTL", "EX", "10"],
            vec!["EX", "10", "KEEPTTL"],
            vec!["EX", "10", "PX", "10"],
            vec!["EXAT", "10", "PXAT", "10"],
            vec!["EX"],
            vec!["FOO"],
        ];
        for words in syntax {
            assert_eq!(opts(&words), Err(syntax_error()), "{words:?}");
        }
        assert_eq!(
            opts(&["EX", "ten"]),
            Err(Value::err("ERR value is not an integer or out of range"))
        );
        let o = opts(&["XX", "PX", "250", "GET"]).expect("valid");
        assert!(o.xx && o.get && !o.nx && !o.keepttl);
        assert_eq!(o.expire, Some((Unit::Px, 250)));
    }

    #[test]
    fn set_expiry_errors() {
        let ex = |n| SetOptions {
            expire: Some((Unit::Ex, n)),
            ..SetOptions::default()
        };
        let invalid = Err(Value::err("ERR invalid expire time in 'set' command"));
        assert_eq!(ex(0).expire_at(1_000, "set"), invalid);
        assert_eq!(ex(-5).expire_at(1_000, "set"), invalid);
        assert_eq!(ex(i64::MAX).expire_at(1_000, "set"), invalid);
        assert_eq!(ex(2).expire_at(1_000, "set"), Ok(Some(3_000)));
        let pxat = SetOptions {
            expire: Some((Unit::PxAt, 500)),
            ..SetOptions::default()
        };
        assert_eq!(pxat.expire_at(1_000, "set"), Ok(Some(500)));
        assert_eq!(SetOptions::default().expire_at(1_000, "set"), Ok(None));
    }
}
